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
use autophagy_patterns::DetectorConfig;
use autophagy_store::EventStore;
use clap::ValueEnum;
use serde::Serialize;

use crate::{
    CliError, CommandReport, OutputFormat, config::Config, daemon, derive_instance_key,
    digest_report, open_store, resolve_database_path, watch::NativeAdapter, write_report,
};

/// Parsed `setup` command inputs plus which flags were passed explicitly.
///
/// The `*_explicit` markers let setup apply precedence (default < config < flag)
/// on the non-interactive path and prefill prompt defaults from config on the
/// interactive path.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug)]
pub struct SetupPlan {
    /// Adapters to consider; empty means every native adapter.
    pub adapters: Vec<NativeAdapter>,
    /// Whether `--adapter` was passed explicitly.
    pub adapters_explicit: bool,
    /// Make commands searchable (import indexing gate) when non-interactive.
    pub index_tool_input: bool,
    /// Whether `--index-tool-input` was passed explicitly.
    pub index_tool_input_explicit: bool,
    /// Persist prompt/response/tool-result text when non-interactive.
    pub include_content: bool,
    /// Whether `--include-content` was passed explicitly.
    pub include_content_explicit: bool,
    /// Already-redacted metadata keys to index as searchable text.
    pub index_metadata: Vec<String>,
    /// Whether `--index-metadata` was passed explicitly.
    pub index_metadata_explicit: bool,
    /// Install background monitoring when non-interactive.
    pub monitor: bool,
    /// Seconds between monitoring discovery cycles.
    pub interval: u64,
    /// Whether `--interval` was passed explicitly.
    pub interval_explicit: bool,
    /// Run without prompting.
    pub yes: bool,
    /// Model backend for richer suggestions when non-interactive; `None`
    /// leaves any existing choice untouched.
    pub model_backend: Option<SetupModelBackend>,
}

/// A synthesis model backend `setup` can configure. Autophagy is fully
/// functional without one; a backend only enriches mutation candidates.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum SetupModelBackend {
    /// No model: the deterministic engine only (the default).
    None,
    /// The authenticated Claude Code CLI already on this machine.
    ClaudeCli,
    /// The authenticated Codex CLI already on this machine.
    CodexCli,
    /// A local Ollama server at localhost.
    Ollama,
}

impl SetupModelBackend {
    /// The `--provider` name `mutations synthesize` expects.
    const fn provider_name(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::ClaudeCli => Some("claude-cli"),
            Self::CodexCli => Some("codex-cli"),
            Self::Ollama => Some("ollama"),
        }
    }

    /// Whether this backend sends the structured prompt to a vendor cloud.
    const fn reaches_cloud(self) -> bool {
        matches!(self, Self::ClaudeCli | Self::CodexCli)
    }
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
    /// Resolved config file path.
    pub config_path: String,
    /// Whether the config file was (re)written this run.
    pub config_written: bool,
    /// Human-readable summary of settings that changed from the prior config.
    pub changed: Vec<String>,
    /// Whether an installed daemon was reinstalled to apply config changes.
    pub daemon_reinstalled: bool,
    /// Synthesis provider chosen this run, when one was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_backend: Option<String>,
    /// Manifest file written for the chosen backend, when one was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_path: Option<String>,
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
    config: &Config,
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

    // Whether the user pointed the database somewhere explicit. Setup's config
    // file, by contrast, never moves with `--database`, so an explicit database
    // is the signal that the two locations have diverged (see the global-config
    // notice below).
    let database_explicit = database.is_some();
    let database = resolve_database_path(database)?;
    let mut store = open_store(&database)?;

    // Prior config values drive prompt prefills and change detection.
    let prior_index_tool_input = config.import_index_tool_input.unwrap_or(false);
    let prior_include_content = config.import_include_content.unwrap_or(false);
    let prior_adapters: Vec<String> = config.import_adapters.clone().unwrap_or_default();
    let prior_interval = config.watch_interval_seconds;

    ui.say("Autophagy setup — local, offline, nothing leaves your machine.");
    // Only claim this is a re-run when a config file is actually on disk; a
    // first run against an empty (or freshly relocated) config directory has no
    // prior settings to show as defaults.
    if crate::config::config_path()?.exists() {
        ui.say("Re-running setup — current settings are shown as defaults.");
    }
    ui.say("");

    // 1. Detect adapters and decide which to import. Candidates come from an
    // explicit `--adapter`, else the configured set, else every adapter.
    let candidates = if plan.adapters_explicit && !plan.adapters.is_empty() {
        dedup(&plan.adapters)
    } else if let Some(configured) = config.import_adapters_parsed()? {
        if configured.is_empty() {
            NativeAdapter::ALL.to_vec()
        } else {
            configured
        }
    } else {
        NativeAdapter::ALL.to_vec()
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
    // Interactive prompts prefill with the current config value (or the
    // recommended default on first run); non-interactive follows precedence.
    ui.say("");
    let index_tool_input = if interactive {
        ui.prompt_yes_no(
            "Make the commands your agents ran searchable? They are stored locally either way; \
             indexing enables exact recall, and secrets are filtered by redaction rules \
             (recommended)",
            config.import_index_tool_input.unwrap_or(true),
        )
    } else if plan.index_tool_input_explicit {
        plan.index_tool_input
    } else {
        prior_index_tool_input
    };
    let include_content = if interactive {
        ui.prompt_yes_no(
            "Also store prompt, response, and tool-result text? Enables richer search; still \
             local and redacted",
            config.import_include_content.unwrap_or(false),
        )
    } else if plan.include_content_explicit {
        plan.include_content
    } else {
        prior_include_content
    };
    let index_metadata = if plan.index_metadata_explicit {
        plan.index_metadata.clone()
    } else {
        config.import_index_metadata.clone().unwrap_or_default()
    };
    let interval = if plan.interval_explicit {
        plan.interval
    } else {
        config.interval_or_default()
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
            &index_metadata,
        )?;
        report.inserted = inserted;
        ui.say(&format!("  {inserted} event(s) inserted."));
    }

    // Rebuild the search index when indexing is on and the store either has no
    // signatures yet or indexing was just newly enabled — a reimport alone
    // cannot make already-stored events searchable.
    let newly_enabled_index = index_tool_input && !prior_index_tool_input;
    let reindex_summary = if index_tool_input && (needs_heal || newly_enabled_index) {
        let summary = reindex(
            &mut store,
            &ReindexOptions {
                index_tool_input: true,
                index_metadata: index_metadata.clone(),
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
    let digest = digest_report(
        crate::detection::detect_cached(&store, None, DetectorConfig::default(), false)?,
        DetectorConfig::default(),
    )?;
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
            let report =
                daemon::install(Some(database.clone()), interval, adapter_names(&selected))?;
            monitor_installed = report.supported && report.unit_present;
            if monitor_installed {
                ui.say("Monitoring installed. Undo any time with `autophagy daemon uninstall`.");
            } else if let Some(message) = &report.message {
                ui.say(message);
            }
        }
    }

    // 6. Offer a synthesis model backend. Fully optional: the deterministic
    // engine needs no model; a backend only enriches candidates. Interactive
    // choices are limited to what is actually available on this machine.
    ui.say("");
    let backend = if interactive {
        prompt_model_backend(&mut ui, config.synthesis_provider.as_deref())
    } else {
        plan.model_backend.unwrap_or(SetupModelBackend::None)
    };
    let mut chosen_backend = None;
    let mut manifest_path_written = None;
    if let Some(provider) = backend.provider_name() {
        if backend.reaches_cloud() {
            ui.say(&format!(
                "Note: with {provider}, the small structured prompt (template fields and evidence \
                 IDs, never raw session text) is sent to the vendor's cloud through your existing \
                 CLI login. Synthesis will also require --allow-remote-endpoint."
            ));
        }
        let manifest = write_model_manifest(backend)?;
        ui.say(&format!("Wrote model manifest to {}.", manifest.display()));
        ui.say(&format!(
            "Try it: autophagy mutations synthesize --provider {provider}{}",
            if backend.reaches_cloud() {
                " --allow-remote-endpoint"
            } else {
                ""
            }
        ));
        chosen_backend = Some(provider.to_owned());
        manifest_path_written = Some(manifest.display().to_string());
    }

    // 7. Persist the chosen settings so later runs and commands inherit them.
    // Settings live in a single global config file that does NOT move with
    // `--database`. When the user redirected the database to an explicit path
    // but the config still lands in the shared application-support directory,
    // say so plainly before writing — otherwise a throwaway `--database /tmp/…`
    // run silently rewrites their real global config. AUTOPHAGY_CONFIG_DIR is
    // the escape hatch that keeps a run fully isolated; surface it here.
    let config_is_global = std::env::var_os(crate::config::CONFIG_DIR_ENV).is_none();
    if config_is_global && database_explicit {
        let notice = format!(
            "note: settings are saved globally to {} (set {} to keep this run isolated)",
            crate::config::config_path()?.display(),
            crate::config::CONFIG_DIR_ENV,
        );
        if interactive {
            ui.say(&notice);
        } else {
            // Non-interactive (`--yes`, including JSON): stdout carries the
            // report, so the advisory goes to stderr where it can't corrupt it.
            eprintln!("{notice}");
        }
    }
    let selected_names = adapter_names(&selected);
    let config_path = crate::config::write_setup(&crate::config::SetupValues {
        adapters: selected_names.clone(),
        index_tool_input,
        include_content,
        index_metadata: index_metadata.clone(),
        interval_seconds: interval,
        synthesis_provider: chosen_backend.clone(),
        synthesis_manifest_path: manifest_path_written.clone(),
    })?;
    let mut changed = Vec::new();
    if index_tool_input != prior_index_tool_input {
        changed.push(format!("index_tool_input → {index_tool_input}"));
    }
    if include_content != prior_include_content {
        changed.push(format!("include_content → {include_content}"));
    }
    if selected_names != prior_adapters {
        changed.push(format!("adapters → [{}]", selected_names.join(", ")));
    }
    if prior_interval.is_some_and(|prior| prior != interval) {
        changed.push(format!("interval_seconds → {interval}"));
    }
    ui.say("");
    ui.say(&format!("Saved settings to {}.", config_path.display()));
    if config.present && !changed.is_empty() {
        ui.say(&format!("Changed: {}.", changed.join(", ")));
    }

    // 8. If an installed daemon's inputs changed and we did not just install
    // one fresh, offer to reinstall so the change takes effect (the unit bakes
    // its arguments in at install time).
    let adapters_changed = selected_names != prior_adapters;
    let interval_changed = prior_interval.is_some_and(|prior| prior != interval);
    let mut daemon_reinstalled = false;
    if !monitor_installed && (adapters_changed || interval_changed) && !selected.is_empty() {
        let installed = daemon::status(Some(database.clone()))?.unit_present;
        if installed {
            let reinstall = if interactive {
                ui.prompt_yes_no(
                    "Monitoring is installed and its settings changed. Reinstall it now to apply?",
                    true,
                )
            } else {
                plan.yes
            };
            if reinstall {
                let report =
                    daemon::install(Some(database.clone()), interval, selected_names.clone())?;
                daemon_reinstalled = report.supported && report.unit_present;
                if daemon_reinstalled {
                    ui.say("Monitoring reinstalled with the new settings.");
                }
            } else {
                ui.say("Run `autophagy daemon install` to apply the change when ready.");
            }
        }
    }

    // 9. What next.
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
        config_path: config_path.display().to_string(),
        config_written: true,
        changed,
        daemon_reinstalled,
        model_backend: chosen_backend,
        manifest_path: manifest_path_written,
    })
}

/// Ask which synthesis model backend to use, offering only what is available.
fn prompt_model_backend(ui: &mut Ui, current: Option<&str>) -> SetupModelBackend {
    let claude = binary_on_path("claude");
    let codex = binary_on_path("codex");
    let mut options = vec!["none (deterministic only — Autophagy is fully functional without)"];
    if claude {
        options.push("claude (your existing Claude Code CLI login)");
    }
    if codex {
        options.push("codex (your existing Codex CLI login)");
    }
    options.push("ollama (a local Ollama server, zero marginal cost)");
    ui.say("Want richer suggestions from a model? Options:");
    for option in &options {
        ui.say(&format!("  - {option}"));
    }
    if let Some(current) = current {
        ui.say(&format!("  (currently configured: {current})"));
    }
    loop {
        let answer = ui.prompt_line("Model backend [none]:");
        match answer.trim().to_ascii_lowercase().as_str() {
            "" | "none" => return SetupModelBackend::None,
            "claude" | "claude-cli" if claude => return SetupModelBackend::ClaudeCli,
            "codex" | "codex-cli" if codex => return SetupModelBackend::CodexCli,
            "ollama" => return SetupModelBackend::Ollama,
            other => ui.say(&format!("`{other}` is not one of the offered options.")),
        }
    }
}

/// Whether an executable with this name is reachable through `PATH`.
fn binary_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file()
    })
}

/// Write a ready-to-use synthesis manifest for the chosen backend into the
/// config directory, returning its path. Contains no secrets: it names a
/// binary or a loopback endpoint and declared capabilities only.
fn write_model_manifest(backend: SetupModelBackend) -> Result<PathBuf, CliError> {
    let (name, format, path, revision, hints) = match backend {
        SetupModelBackend::None => unreachable!("no manifest for the deterministic engine"),
        SetupModelBackend::ClaudeCli => (
            "claude-code-login",
            "claude_cli",
            "claude",
            "cli",
            serde_json::json!({ "min_memory_mb": 512 }),
        ),
        SetupModelBackend::CodexCli => (
            "codex-login",
            "codex_cli",
            "codex",
            "cli",
            serde_json::json!({ "min_memory_mb": 512 }),
        ),
        SetupModelBackend::Ollama => (
            "qwen3-coder:30b",
            "ollama",
            "http://localhost:11434",
            "local",
            serde_json::json!({ "min_memory_mb": 8192 }),
        ),
    };
    let manifest = serde_json::json!({
        "spec_version": "synthesis-manifest/0.3",
        "name": name,
        "format": format,
        "path": path,
        "revision": revision,
        "capabilities": ["mutation_synthesis"],
        "resource_hints": hints,
    });
    let target = crate::config::config_path()?
        .parent()
        .map(|dir| dir.join("synthesis-manifest.json"))
        .ok_or_else(|| CliError::Config("config path has no parent directory".to_owned()))?;
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&manifest).map_err(CliError::from)?
    );
    std::fs::write(&target, rendered).map_err(|error| {
        CliError::Config(format!(
            "could not write model manifest {}: {error}",
            target.display()
        ))
    })?;
    Ok(target)
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
    /// Read one free-form answer line; empty string when non-interactive/EOF.
    fn prompt_line(&mut self, question: &str) -> String {
        if !self.interactive {
            return String::new();
        }
        print!("{question} ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            return String::new();
        }
        line.trim().to_owned()
    }

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
