//! Guided first-run experience (`autophagy setup`).
//!
//! Detects each local coding agent, imports the ones the user chooses under the
//! same redaction and projection gates as `import`, runs the deterministic
//! digest so there is something to see immediately, and optionally installs
//! background monitoring through the existing daemon lifecycle. Nothing here is
//! destructive: setup only ever adds, and it routes an already-imported but
//! unindexed database through `reindex` instead of a useless reimport.
//!
//! The flow is interactive when attached to a terminal. With no terminal (or
//! with `--yes`) it runs from flags alone, which is what the end-to-end tests
//! drive.

use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
};

use autophagy_adapter_claude_code::{
    ClaudeImportOptions, DiscoveryOptions as ClaudeDiscoveryOptions, default_projects_root,
    discover as claude_discover, import_claude_code,
};
use autophagy_adapter_codex::{
    CodexImportOptions, default_sessions_root as codex_sessions_root, discover as codex_discover,
    import_codex,
};
use autophagy_adapter_opencode::{
    OpenCodeImportOptions, default_storage_root, discover as opencode_discover, import_opencode,
};
use autophagy_adapter_pi::{
    PiImportOptions, default_sessions_root as pi_sessions_root, discover as pi_discover, import_pi,
};
use autophagy_core::{ReindexOptions, ReindexSummary, reindex};
use autophagy_patterns::{DetectorConfig, detect_with_report};
use autophagy_store::EventStore;
use serde::Serialize;

use crate::{
    CliError, CommandReport, OutputFormat, daemon, derive_instance_key, digest_report, open_store,
    resolve_database_path, watch::NativeAdapter, write_report,
};

/// Parsed `setup` command inputs.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug)]
pub struct SetupPlan {
    /// Adapters to consider; empty means every native adapter.
    pub adapters: Vec<NativeAdapter>,
    /// Make commands searchable (import indexing gate) when non-interactive.
    pub index_tool_input: bool,
    /// Persist prompt/response/tool-result text when non-interactive.
    pub include_content: bool,
    /// Already-redacted metadata keys to index as searchable text.
    pub index_metadata: Vec<String>,
    /// Install background monitoring when non-interactive.
    pub monitor: bool,
    /// Seconds between monitoring discovery cycles.
    pub interval: u64,
    /// Run without prompting.
    pub yes: bool,
}

/// Per-adapter outcome recorded in the structured report.
#[derive(Clone, Debug, Serialize)]
pub struct SetupAdapterReport {
    /// Stable adapter identifier.
    pub adapter: String,
    /// Whether a local history root was found.
    pub present: bool,
    /// Sessions/transcripts discovered under the default root.
    pub sessions_found: usize,
    /// Whether the user chose to import it.
    pub selected: bool,
    /// Events inserted by this adapter's import.
    pub inserted: u64,
}

/// Structured result of a guided setup run.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Serialize)]
pub struct SetupReport {
    /// Whether the flow prompted interactively.
    pub interactive: bool,
    /// Resolved decision to index tool input.
    pub index_tool_input: bool,
    /// Resolved decision to persist content.
    pub include_content: bool,
    /// Per-adapter detection and import outcomes.
    pub adapters: Vec<SetupAdapterReport>,
    /// Rebuild summary when an already-imported database was healed in place.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reindex: Option<ReindexSummary>,
    /// Events the digest scanned.
    pub digest_events_scanned: usize,
    /// Deterministic findings surfaced by the digest.
    pub digest_findings: usize,
    /// Near-threshold observations the digest surfaced when nothing qualified.
    pub digest_observations: usize,
    /// Whether background monitoring was installed.
    pub monitor_installed: bool,
}

/// Run the guided setup flow.
///
/// # Errors
///
/// Returns [`CliError`] for storage, discovery, or import failures, or
/// [`CliError::SetupNonInteractive`] when there is no terminal and no explicit
/// non-interactive intent.
#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
pub fn run(
    database: Option<PathBuf>,
    output: OutputFormat,
    plan: SetupPlan,
) -> Result<SetupReport, CliError> {
    let json = matches!(output, OutputFormat::Json);
    let tty = io::stdin().is_terminal();
    let interactive = !plan.yes && tty && !json;
    if !interactive && !plan.yes {
        // No terminal to prompt at, and no explicit go-ahead.
        return Err(CliError::SetupNonInteractive);
    }
    // Prose is streamed only in human (text) mode; JSON callers get the report.
    let verbose = !json;
    let mut ui = Ui {
        interactive,
        verbose,
    };

    let database = resolve_database_path(database)?;
    let mut store = open_store(&database)?;

    ui.say("Autophagy setup — local, offline, nothing leaves your machine.");
    ui.say("");

    // 1. Detect adapters and decide which to import.
    let candidates = if plan.adapters.is_empty() {
        NativeAdapter::ALL.to_vec()
    } else {
        dedup(&plan.adapters)
    };
    let mut adapter_reports = Vec::new();
    let mut selected = Vec::new();
    for adapter in candidates {
        let detection = detect_adapter(adapter)?;
        if !detection.present {
            ui.say(&format!("{}: not found — skipping.", label(adapter)));
            adapter_reports.push(SetupAdapterReport {
                adapter: adapter.as_str().to_owned(),
                present: false,
                sessions_found: 0,
                selected: false,
                inserted: 0,
            });
            continue;
        }
        let choose = ui.prompt_yes_no(
            &format!(
                "{}: {} session(s) found — import?",
                label(adapter),
                detection.count
            ),
            true,
        );
        adapter_reports.push(SetupAdapterReport {
            adapter: adapter.as_str().to_owned(),
            present: true,
            sessions_found: detection.count,
            selected: choose,
            inserted: 0,
        });
        if choose {
            selected.push(adapter);
        }
    }

    // 2. Privacy questions (at most two): indexing and content persistence.
    ui.say("");
    let index_tool_input = if interactive {
        ui.prompt_yes_no(
            "Make the commands your agents ran searchable? They are stored locally either way; \
             indexing enables exact recall, and secrets are filtered by redaction rules \
             (recommended)",
            true,
        )
    } else {
        plan.index_tool_input
    };
    let include_content = if interactive {
        ui.prompt_yes_no(
            "Also store prompt, response, and tool-result text? Enables richer search; still \
             local and redacted",
            false,
        )
    } else {
        plan.include_content
    };

    // A database imported before signature indexing existed keeps its events
    // but has no signatures; reimport is a no-op, so route through reindex.
    let needs_heal = store.stats()?.events > 0 && store.signature_count()? == 0;

    // 3. Run the selected imports.
    ui.say("");
    for report in &mut adapter_reports {
        let Some(adapter) = selected
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == report.adapter)
        else {
            continue;
        };
        ui.say(&format!("Importing {}…", label(adapter)));
        let inserted = import_adapter(
            adapter,
            &mut store,
            index_tool_input,
            include_content,
            &plan.index_metadata,
        )?;
        report.inserted = inserted;
        ui.say(&format!("  {inserted} event(s) inserted."));
    }

    // Heal an already-imported but unindexed database in place.
    let reindex_summary = if index_tool_input && needs_heal {
        let summary = reindex(
            &mut store,
            &ReindexOptions {
                index_tool_input: true,
                index_metadata: plan.index_metadata.clone(),
                exclude_paths: Vec::new(),
            },
        )?;
        // Only claim a heal when the rebuild actually produced signatures a
        // reimport could not; a corpus with no indexable tool events genuinely
        // has none, and saying otherwise would mislead.
        if summary.signatures_written > 0 {
            ui.say(&format!(
                "Rebuilt the search index for {} existing event(s) — {} command signature(s) that reimport could not restore.",
                summary.events_scanned, summary.signatures_written
            ));
        }
        Some(summary)
    } else {
        None
    };

    // 4. Run the digest immediately and surface it through the shared renderer.
    // Using the same detect-with-report pass as `digest`, a zero-finding scan
    // still shows the scan stats and near-threshold observations rather than a
    // silent nothing.
    ui.say("");
    let events = store.list_events_for_detection(None)?;
    let digest = digest_report(detect_with_report(&events, DetectorConfig::default()))?;
    let digest_events_scanned = digest.events_scanned;
    let digest_findings = digest.findings.len();
    let digest_observations = digest.observations.len();
    if verbose {
        write_report(
            io::stdout().lock(),
            OutputFormat::Text,
            &CommandReport::Digest(digest),
        )?;
    }

    // 5. Offer monitoring.
    ui.say("");
    let want_monitor = if interactive {
        ui.prompt_yes_no(
            "Keep watching automatically? Installs a background service (launchd/systemd)",
            false,
        )
    } else {
        plan.monitor
    };
    let mut monitor_installed = false;
    if want_monitor {
        if selected.is_empty() {
            ui.say("No adapters selected to watch — skipping monitoring.");
        } else {
            let report = daemon::install(
                Some(database.clone()),
                plan.interval,
                adapter_names(&selected),
            )?;
            monitor_installed = report.supported && report.unit_present;
            if monitor_installed {
                ui.say("Monitoring installed. Undo any time with `autophagy daemon uninstall`.");
            } else if let Some(message) = &report.message {
                ui.say(message);
            }
        }
    }

    // 6. What next.
    if verbose {
        ui.say("");
        ui.say("What next:");
        ui.say("  autophagy patterns            list repeated problems with exact evidence");
        ui.say("  autophagy search \"<query>\"    recall commands and context");
        ui.say("  autophagy mutations propose    turn findings into reviewable lessons");
        ui.say("  the macOS app                  a window over your local database");
    }

    Ok(SetupReport {
        interactive,
        index_tool_input,
        include_content,
        adapters: adapter_reports,
        reindex: reindex_summary,
        digest_events_scanned,
        digest_findings,
        digest_observations,
        monitor_installed,
    })
}

/// Minimal, testable console helper.
struct Ui {
    interactive: bool,
    verbose: bool,
}

impl Ui {
    fn say(&self, line: &str) {
        if self.verbose {
            println!("{line}");
        }
    }

    /// Ask a yes/no question. Returns `default` without reading when not
    /// interactive (non-interactive runs are driven entirely by flags).
    fn prompt_yes_no(&mut self, question: &str, default: bool) -> bool {
        if !self.interactive {
            return default;
        }
        let hint = if default { "[Y/n]" } else { "[y/N]" };
        loop {
            print!("{question} {hint} ");
            let _ = io::stdout().flush();
            let mut line = String::new();
            if io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
                return default; // EOF: accept the default.
            }
            match line.trim().to_ascii_lowercase().as_str() {
                "" => return default,
                "y" | "yes" => return true,
                "n" | "no" => return false,
                _ => println!("Please answer y or n."),
            }
        }
    }
}

/// Local detection of one adapter's default history root.
struct Detection {
    present: bool,
    count: usize,
}

impl Detection {
    const fn absent() -> Self {
        Self {
            present: false,
            count: 0,
        }
    }
}

fn detect_adapter(adapter: NativeAdapter) -> Result<Detection, CliError> {
    match adapter {
        NativeAdapter::ClaudeCode => {
            let root = default_projects_root()?;
            if !root.exists() {
                return Ok(Detection::absent());
            }
            let plan = claude_discover(&ClaudeDiscoveryOptions {
                input: root,
                include_subagents: false,
            })?;
            Ok(Detection {
                present: true,
                count: plan.files.len(),
            })
        }
        NativeAdapter::Codex => {
            let root = codex_sessions_root()?;
            if !root.exists() {
                return Ok(Detection::absent());
            }
            let plan = codex_discover(&root)?;
            Ok(Detection {
                present: true,
                count: plan.files.len(),
            })
        }
        NativeAdapter::Pi => {
            let root = pi_sessions_root()?;
            if !root.exists() {
                return Ok(Detection::absent());
            }
            let plan = pi_discover(&root)?;
            Ok(Detection {
                present: true,
                count: plan.files.len(),
            })
        }
        NativeAdapter::OpenCode => {
            let root = default_storage_root()?;
            if !root.exists() {
                return Ok(Detection::absent());
            }
            let plan = opencode_discover(&root)?;
            Ok(Detection {
                present: true,
                count: plan.sessions.len(),
            })
        }
    }
}

fn import_adapter(
    adapter: NativeAdapter,
    store: &mut EventStore,
    index_tool_input: bool,
    include_content: bool,
    index_metadata: &[String],
) -> Result<u64, CliError> {
    match adapter {
        NativeAdapter::ClaudeCode => {
            let root = default_projects_root()?;
            let instance_key = derive_instance_key(&root)?;
            let mut options = ClaudeImportOptions::new(root, instance_key);
            options.include_content = include_content;
            options.index_tool_input = index_tool_input;
            options.index_metadata = index_metadata.to_vec();
            Ok(import_claude_code(Some(store), &options)?.inserted)
        }
        NativeAdapter::Codex => {
            let root = codex_sessions_root()?;
            let instance_key = derive_instance_key(&root)?;
            let mut options = CodexImportOptions::new(root, instance_key);
            options.include_content = include_content;
            options.index_tool_input = index_tool_input;
            options.index_metadata = index_metadata.to_vec();
            Ok(import_codex(Some(store), &options)?.inserted)
        }
        NativeAdapter::Pi => {
            let root = pi_sessions_root()?;
            let instance_key = derive_instance_key(&root)?;
            let mut options = PiImportOptions::new(root, instance_key);
            options.include_content = include_content;
            options.index_tool_input = index_tool_input;
            options.index_metadata = index_metadata.to_vec();
            Ok(import_pi(Some(store), &options)?.inserted)
        }
        NativeAdapter::OpenCode => {
            let root = default_storage_root()?;
            let instance_key = derive_instance_key(&root)?;
            let mut options = OpenCodeImportOptions::new(root, instance_key);
            options.include_content = include_content;
            options.index_tool_input = index_tool_input;
            options.index_metadata = index_metadata.to_vec();
            Ok(import_opencode(Some(store), &options)?.inserted)
        }
    }
}

const fn label(adapter: NativeAdapter) -> &'static str {
    match adapter {
        NativeAdapter::ClaudeCode => "Claude Code",
        NativeAdapter::Codex => "Codex",
        NativeAdapter::Pi => "Pi",
        NativeAdapter::OpenCode => "OpenCode",
    }
}

fn adapter_names(adapters: &[NativeAdapter]) -> Vec<String> {
    adapters
        .iter()
        .map(|adapter| adapter.as_str().to_owned())
        .collect()
}

fn dedup(adapters: &[NativeAdapter]) -> Vec<NativeAdapter> {
    let mut chosen = Vec::new();
    for adapter in adapters {
        if !chosen.contains(adapter) {
            chosen.push(*adapter);
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, path::PathBuf};

    use autophagy_install::{SupervisorKind, SupervisorPlan};

    use crate::{
        CliError,
        daemon::{self, DaemonEnv, Supervisor},
    };

    /// Shimmed supervisor: records the loaded state without a real init system,
    /// mirroring the daemon module's own lifecycle tests.
    struct FakeSupervisor {
        loaded: RefCell<bool>,
    }

    impl Supervisor for FakeSupervisor {
        fn load(&self, _plan: &SupervisorPlan) -> Result<(), CliError> {
            *self.loaded.borrow_mut() = true;
            Ok(())
        }

        fn unload(&self, _plan: &SupervisorPlan) -> Result<(), CliError> {
            *self.loaded.borrow_mut() = false;
            Ok(())
        }

        fn is_loaded(&self, _plan: &SupervisorPlan) -> Result<bool, CliError> {
            Ok(*self.loaded.borrow())
        }
    }

    /// Setup's monitoring step installs through the same reversible daemon seam
    /// (`daemon::install_with`) the `daemon install` command uses. With the
    /// shimmed supervisor, choosing to monitor writes and loads the unit.
    #[test]
    fn monitor_step_writes_and_loads_a_unit_through_the_shared_seam() {
        let dir = tempfile::tempdir().expect("temp dir");
        let env = DaemonEnv {
            kind: SupervisorKind::Launchd,
            unit_directory: dir.path().join("units"),
            binary_path: PathBuf::from("/opt/autophagy/bin/autophagy"),
            interval_secs: 60,
            adapters: vec!["claude-code".to_owned()],
            database: Some(dir.path().join("autophagy.db")),
            log_path: dir.path().join("data").join("watch.log"),
            error_log_path: dir.path().join("data").join("watch.err.log"),
        };
        let supervisor = FakeSupervisor {
            loaded: RefCell::new(false),
        };

        let report = daemon::install_with(&env, &supervisor).expect("install");
        assert!(report.unit_present);
        assert_eq!(report.job_loaded, Some(true));
        assert!(
            report
                .program_arguments
                .iter()
                .any(|argument| argument == "watch")
        );
        let unit = PathBuf::from(report.unit_path.expect("unit path"));
        assert!(unit.exists(), "the supervisor unit file must be written");
    }
}
