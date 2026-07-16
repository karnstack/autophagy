//! Foreground continuous ingestion (`autophagy watch`).
//!
//! Wires the native adapters into the model-free [`run_watch`](autophagy_core::run_watch)
//! loop hosted by `autophagy-core`. The command only ingests: it discovers and
//! incrementally imports transcripts under the same redaction, privacy, and
//! projection gates as one-shot `import`, and never executes or installs
//! anything.

use std::{
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use autophagy_adapter_claude_code::{
    ClaudeImportOptions, default_projects_root, import_claude_code,
};
use autophagy_adapter_codex::{CodexImportOptions, default_sessions_root, import_codex};
use autophagy_adapter_opencode::{
    OpenCodeImportOptions, default_storage_root, import_opencode,
};
use autophagy_adapter_pi::{
    PiImportOptions, default_sessions_root as default_pi_sessions_root, import_pi,
};
use autophagy_core::{
    AdapterOutcome, CycleOutcome, CycleReport, SourceError, WatchConfig, WatchSource, WatchSummary,
    run_watch,
};
use autophagy_store::EventStore;
use clap::ValueEnum;
use serde::Serialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{CliError, OutputFormat, derive_instance_key, open_store, resolve_database_path};

/// Native adapters the watch loop and daemon can drive.
///
/// This is the single extensible seam for continuous ingestion: a new native
/// adapter (for example Pi or `OpenCode`) plugs in by adding one variant here,
/// one arm in [`NativeAdapter::build_source`], and one entry in
/// [`NativeAdapter::ALL`]. The default adapter set, the watch loop, and the
/// daemon unit's `--adapter` arguments are all derived from this type, so no
/// adapter list is hard-coded anywhere else.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum NativeAdapter {
    /// Claude Code transcripts under the projects root.
    ClaudeCode,
    /// Codex rollout transcripts under the sessions root.
    Codex,
    /// Pi coding-agent sessions under the sessions root.
    Pi,
    /// `OpenCode` session storage under the storage root.
    OpenCode,
}

impl NativeAdapter {
    /// Every native adapter, used as the default watch set. Mirrors the native
    /// adapters `import` supports, so watch and daemon defaults stay in step.
    pub const ALL: &'static [Self] = &[Self::ClaudeCode, Self::Codex, Self::Pi, Self::OpenCode];

    /// Stable adapter identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
        }
    }

    /// Build an incremental watch source for this adapter over its default
    /// history root, honouring the same privacy gates as one-shot import.
    fn build_source(
        self,
        include_content: bool,
        projects: &[String],
        exclude_paths: &[String],
    ) -> Result<Box<dyn WatchSource>, CliError> {
        match self {
            Self::ClaudeCode => {
                let root = default_projects_root()?;
                Ok(Box::new(ClaudeWatchSource::new(
                    root,
                    include_content,
                    projects,
                    exclude_paths,
                )?))
            }
            Self::Codex => {
                let root = default_sessions_root()?;
                Ok(Box::new(CodexWatchSource::new(
                    root,
                    include_content,
                    projects,
                    exclude_paths,
                )?))
            }
            Self::Pi => {
                let root = default_pi_sessions_root()?;
                Ok(Box::new(PiWatchSource::new(
                    root,
                    include_content,
                    projects,
                    exclude_paths,
                )?))
            }
            Self::OpenCode => {
                let root = default_storage_root()?;
                Ok(Box::new(OpenCodeWatchSource::new(
                    root,
                    include_content,
                    projects,
                    exclude_paths,
                )?))
            }
        }
    }
}

struct ClaudeWatchSource {
    options: Option<ClaudeImportOptions>,
}

impl ClaudeWatchSource {
    fn new(
        root: PathBuf,
        include_content: bool,
        projects: &[String],
        exclude_paths: &[String],
    ) -> Result<Self, CliError> {
        if !root.exists() {
            return Ok(Self { options: None });
        }
        let instance_key = derive_instance_key(&root)?;
        let mut options = ClaudeImportOptions::new(root, instance_key);
        options.include_content = include_content;
        options.projects = projects.to_vec();
        options.exclude_paths = exclude_paths.to_vec();
        Ok(Self {
            options: Some(options),
        })
    }
}

impl WatchSource for ClaudeWatchSource {
    fn adapter(&self) -> &str {
        NativeAdapter::ClaudeCode.as_str()
    }

    fn import_cycle(&mut self, store: &mut EventStore) -> Result<CycleOutcome, SourceError> {
        let Some(options) = &self.options else {
            return Ok(CycleOutcome::new(NativeAdapter::ClaudeCode.as_str()));
        };
        let summary = import_claude_code(Some(store), options)?;
        Ok(CycleOutcome {
            adapter: NativeAdapter::ClaudeCode.as_str().to_owned(),
            inserted: summary.inserted,
            duplicates: summary.duplicates,
            skipped: summary.skipped,
            conflicts: summary.conflicts,
            rejected: summary.rejected,
            diagnostics: count(summary.diagnostics.len()),
        })
    }
}

struct CodexWatchSource {
    options: Option<CodexImportOptions>,
}

impl CodexWatchSource {
    fn new(
        root: PathBuf,
        include_content: bool,
        projects: &[String],
        exclude_paths: &[String],
    ) -> Result<Self, CliError> {
        if !root.exists() {
            return Ok(Self { options: None });
        }
        let instance_key = derive_instance_key(&root)?;
        let mut options = CodexImportOptions::new(root, instance_key);
        options.include_content = include_content;
        options.projects = projects.to_vec();
        options.exclude_paths = exclude_paths.to_vec();
        Ok(Self {
            options: Some(options),
        })
    }
}

impl WatchSource for CodexWatchSource {
    fn adapter(&self) -> &str {
        NativeAdapter::Codex.as_str()
    }

    fn import_cycle(&mut self, store: &mut EventStore) -> Result<CycleOutcome, SourceError> {
        let Some(options) = &self.options else {
            return Ok(CycleOutcome::new(NativeAdapter::Codex.as_str()));
        };
        let summary = import_codex(Some(store), options)?;
        Ok(CycleOutcome {
            adapter: NativeAdapter::Codex.as_str().to_owned(),
            inserted: summary.inserted,
            duplicates: summary.duplicates,
            skipped: summary.skipped,
            conflicts: summary.conflicts,
            rejected: summary.rejected,
            diagnostics: count(summary.diagnostics.len()),
        })
    }
}

struct PiWatchSource {
    options: Option<PiImportOptions>,
}

impl PiWatchSource {
    fn new(
        root: PathBuf,
        include_content: bool,
        projects: &[String],
        exclude_paths: &[String],
    ) -> Result<Self, CliError> {
        if !root.exists() {
            return Ok(Self { options: None });
        }
        let instance_key = derive_instance_key(&root)?;
        let mut options = PiImportOptions::new(root, instance_key);
        options.include_content = include_content;
        options.projects = projects.to_vec();
        options.exclude_paths = exclude_paths.to_vec();
        Ok(Self {
            options: Some(options),
        })
    }
}

impl WatchSource for PiWatchSource {
    fn adapter(&self) -> &str {
        NativeAdapter::Pi.as_str()
    }

    fn import_cycle(&mut self, store: &mut EventStore) -> Result<CycleOutcome, SourceError> {
        let Some(options) = &self.options else {
            return Ok(CycleOutcome::new(NativeAdapter::Pi.as_str()));
        };
        let summary = import_pi(Some(store), options)?;
        Ok(CycleOutcome {
            adapter: NativeAdapter::Pi.as_str().to_owned(),
            inserted: summary.inserted,
            duplicates: summary.duplicates,
            skipped: summary.skipped,
            conflicts: summary.conflicts,
            rejected: summary.rejected,
            diagnostics: count(summary.diagnostics.len()),
        })
    }
}

struct OpenCodeWatchSource {
    options: Option<OpenCodeImportOptions>,
}

impl OpenCodeWatchSource {
    fn new(
        root: PathBuf,
        include_content: bool,
        projects: &[String],
        exclude_paths: &[String],
    ) -> Result<Self, CliError> {
        if !root.exists() {
            return Ok(Self { options: None });
        }
        let instance_key = derive_instance_key(&root)?;
        let mut options = OpenCodeImportOptions::new(root, instance_key);
        options.include_content = include_content;
        options.projects = projects.to_vec();
        options.exclude_paths = exclude_paths.to_vec();
        Ok(Self {
            options: Some(options),
        })
    }
}

impl WatchSource for OpenCodeWatchSource {
    fn adapter(&self) -> &str {
        NativeAdapter::OpenCode.as_str()
    }

    fn import_cycle(&mut self, store: &mut EventStore) -> Result<CycleOutcome, SourceError> {
        let Some(options) = &self.options else {
            return Ok(CycleOutcome::new(NativeAdapter::OpenCode.as_str()));
        };
        let summary = import_opencode(Some(store), options)?;
        Ok(CycleOutcome {
            adapter: NativeAdapter::OpenCode.as_str().to_owned(),
            inserted: summary.inserted,
            duplicates: summary.duplicates,
            skipped: summary.skipped,
            conflicts: summary.conflicts,
            rejected: summary.rejected,
            diagnostics: count(summary.diagnostics.len()),
        })
    }
}

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Final aggregate returned when the loop exits (for exit-code and JSON).
#[derive(Debug, Serialize)]
pub struct WatchRunReport {
    /// Cycle interval in seconds.
    pub interval_secs: u64,
    /// Whether this was a single-cycle run.
    pub once: bool,
    /// Adapters that were driven.
    pub adapters: Vec<String>,
    /// Cycles executed.
    pub cycles: u64,
    /// Total events inserted.
    pub inserted: u64,
    /// Total per-adapter failures observed.
    pub failures: u64,
}

impl WatchRunReport {
    /// Whether operator attention is warranted (a failure occurred).
    #[must_use]
    pub const fn has_issues(&self) -> bool {
        self.failures > 0
    }
}

/// Resolve the requested adapter selection, defaulting to every native adapter.
fn selected_adapters(requested: &[NativeAdapter]) -> Vec<NativeAdapter> {
    if requested.is_empty() {
        return NativeAdapter::ALL.to_vec();
    }
    let mut chosen = Vec::new();
    for adapter in requested {
        if !chosen.contains(adapter) {
            chosen.push(*adapter);
        }
    }
    chosen
}

/// Resolve the requested adapters (defaulting to all) to their stable names.
/// Used by `daemon install` to render the unit's `--adapter` arguments from the
/// same seam that drives the foreground loop.
#[must_use]
pub fn adapter_names(requested: &[NativeAdapter]) -> Vec<String> {
    selected_adapters(requested)
        .iter()
        .map(|adapter| adapter.as_str().to_owned())
        .collect()
}

/// Register a shared shutdown flag flipped by SIGINT and SIGTERM.
fn install_signal_handlers() -> Result<Arc<AtomicBool>, CliError> {
    let flag = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&flag))?;
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&flag))?;
    Ok(flag)
}

/// Run the watch command. Prints per-cycle summary lines itself (text or NDJSON)
/// and a final summary, so the caller must not re-render the returned report.
#[allow(clippy::too_many_arguments)]
pub fn run(
    database: Option<PathBuf>,
    requested: &[NativeAdapter],
    interval_secs: u64,
    once: bool,
    include_content: bool,
    projects: &[String],
    exclude_paths: &[String],
    output: OutputFormat,
) -> Result<WatchRunReport, CliError> {
    let adapters = selected_adapters(requested);
    let mut sources = Vec::with_capacity(adapters.len());
    for adapter in &adapters {
        sources.push(adapter.build_source(include_content, projects, exclude_paths)?);
    }

    let database = resolve_database_path(database)?;
    let mut store = open_store(&database)?;
    let shutdown = install_signal_handlers()?;
    let config = WatchConfig {
        interval: Duration::from_secs(interval_secs),
        once,
    };

    let mut stdout = io::stdout().lock();
    let mut observer = |report: &CycleReport| {
        if report.is_quiet() {
            return;
        }
        // Best-effort progress output; a broken pipe should not abort the loop.
        let _ = report_cycle(&mut stdout, output, report);
    };

    let summary: WatchSummary = run_watch(
        &mut store,
        &mut sources,
        &config,
        &shutdown,
        &mut observer,
    );

    let report = WatchRunReport {
        interval_secs,
        once,
        adapters: adapters.iter().map(|a| a.as_str().to_owned()).collect(),
        cycles: summary.cycles,
        inserted: summary.inserted,
        failures: summary.failures,
    };
    report_summary(&mut stdout, output, &report)?;
    Ok(report)
}

fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown-time".to_owned())
}

fn report_cycle(
    writer: &mut impl Write,
    output: OutputFormat,
    report: &CycleReport,
) -> io::Result<()> {
    match output {
        OutputFormat::Json => {
            serde_json::to_writer(&mut *writer, report).map_err(io::Error::other)?;
            writeln!(writer)?;
        }
        OutputFormat::Text => {
            let timestamp = now();
            for outcome in &report.outcomes {
                match outcome {
                    AdapterOutcome::Imported(cycle) if !cycle.is_quiet() => writeln!(
                        writer,
                        "{timestamp} cycle {} {}: {} inserted · {} duplicates · {} skipped · {} conflicts · {} rejected · {} diagnostics",
                        report.cycle,
                        cycle.adapter,
                        cycle.inserted,
                        cycle.duplicates,
                        cycle.skipped,
                        cycle.conflicts,
                        cycle.rejected,
                        cycle.diagnostics
                    )?,
                    AdapterOutcome::Failed(failure) if !failure.suppressed => writeln!(
                        writer,
                        "{timestamp} cycle {} {}: import failed: {}",
                        report.cycle, failure.adapter, failure.message
                    )?,
                    AdapterOutcome::Imported(_) | AdapterOutcome::Failed(_) => {}
                }
            }
        }
    }
    writer.flush()
}

fn report_summary(
    writer: &mut impl Write,
    output: OutputFormat,
    report: &WatchRunReport,
) -> Result<(), CliError> {
    match output {
        OutputFormat::Json => {
            serde_json::to_writer(&mut *writer, report)?;
            writeln!(writer)?;
        }
        OutputFormat::Text => writeln!(
            writer,
            "{} watched {} cycle(s) · {} events inserted · {} failure(s) · adapters: {}",
            now(),
            report.cycles,
            report.inserted,
            report.failures,
            report.adapters.join(", ")
        )?,
    }
    writer.flush()?;
    Ok(())
}
