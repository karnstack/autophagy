//! Command-line entry point for importing and querying local agent activity.

mod config;
mod daemon;
mod setup;
mod status;
mod watch;

use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs::{self, File},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use autophagy_adapter_claude_code::{
    ClaudeImportOptions, ClaudeImportSummary, default_projects_root, import_claude_code,
};
use autophagy_adapter_codex::{
    CodexImportOptions, CodexImportSummary, default_sessions_root, import_codex,
};
use autophagy_adapter_opencode::{
    OpenCodeImportOptions, OpenCodeImportSummary, default_storage_root, import_opencode,
};
use autophagy_adapter_pi::{
    PiImportOptions, PiImportSummary, default_sessions_root as default_pi_sessions_root, import_pi,
};
use autophagy_core::{
    ImportOptions, ImportSummary, ReindexError, ReindexOptions, ReindexSummary, import_jsonl,
    reindex,
};
use autophagy_events::Event;
use autophagy_install::{
    InstallError, InstallTarget, InstalledArtifact, SkillPlan, SupervisorError, materialize,
    plan_skill, unmaterialize,
};
use autophagy_mutations::{GenerationOutcome, equivalence_key, generate_candidates};
use autophagy_patterns::{
    DetectionDiagnostics, DetectionReport, DetectorConfig, EvidencePacket, Observation, detect,
    detect_with_report,
};
use autophagy_replay::{
    CounterfactualOutcome, ExpectedAction, ReplayDraftError, ReplayEvaluationError, ReplayReport,
    ReplaySuite, evaluate, extract_review_draft,
};
use autophagy_shadow::{
    ShadowEvaluationError, ShadowReport, ShadowSuite, evaluate as evaluate_shadow,
};
use autophagy_store::{
    DeleteAllSummary, DeleteSummary, EventStore, InstallationRegistration,
    InstallationTransitionOutcome, MutationDetails, MutationRecord, MutationRegisterOutcome,
    MutationRegistration, MutationTransitionOutcome, PruneSummary, ReplayRegisterOutcome,
    ReplayRegistration, RetrievalHit, RetrievalOutcome, RetrievalQuery, SessionSummary,
    ShadowRegisterOutcome, ShadowRegistration, StoreError,
};
use autophagy_synthesis::{
    DeterministicReferenceProvider, EndpointLocality, ManifestError, ModelFormat, ModelManifest,
    OllamaProvider, OpenAiCompatibleProvider, SynthesisOutcome, SynthesisProvider, TokenUsage,
    classify_endpoint, synthesize_candidates,
};
use clap::{ArgMatches, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use directories::ProjectDirs;
use serde::Serialize;
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Parser)]
#[command(
    name = "autophagy",
    version,
    about = "The self-improvement layer for local coding agents",
    arg_required_else_help = true
)]
struct Cli {
    /// Local database path. Defaults to the platform-local application data directory.
    #[arg(long, global = true, env = "AUTOPHAGY_DB", value_name = "PATH")]
    database: Option<PathBuf>,

    /// Output format for command results.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    output: OutputFormat,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ImportAdapter {
    GenericJsonl,
    ClaudeCode,
    Codex,
    Pi,
    Opencode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum OutcomeArg {
    Success,
    Failure,
}

impl From<OutcomeArg> for RetrievalOutcome {
    fn from(value: OutcomeArg) -> Self {
        match value {
            OutcomeArg::Success => Self::Success,
            OutcomeArg::Failure => Self::Failure,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Import normalized AEP JSONL or native agent history.
    Import {
        /// Input file/root. `-` means stdin for generic JSONL or the adapter's default history root.
        #[arg(default_value = "-", value_name = "PATH")]
        input: PathBuf,

        /// Source format to discover and normalize.
        #[arg(long, value_enum, default_value_t = ImportAdapter::GenericJsonl)]
        adapter: ImportAdapter,

        /// Stable source identity. Defaults to the canonical input path or `stdin`.
        #[arg(long, value_name = "KEY")]
        instance_key: Option<String>,

        /// Optional user-facing label for this source.
        #[arg(long, value_name = "NAME")]
        display_name: Option<String>,

        /// Include only events with this exact project path. Repeatable.
        #[arg(long = "project", value_name = "PATH")]
        projects: Vec<String>,

        /// Exclude project or artifact paths matching this glob. Repeatable.
        #[arg(long = "exclude-path", value_name = "GLOB")]
        exclude_paths: Vec<String>,

        /// Include Claude Code `agent-*.jsonl` subagent transcripts.
        #[arg(long)]
        include_subagents: bool,

        /// Persist native-adapter prompt, response, and tool-result text in event metadata.
        #[arg(long)]
        include_content: bool,

        /// Index tool input after confirming the source has already been redacted.
        #[arg(long)]
        index_tool_input: bool,

        /// Index an already-redacted event metadata key. Repeatable.
        #[arg(long = "index-metadata", value_name = "KEY")]
        index_metadata: Vec<String>,

        /// Parse and filter without creating or changing the database.
        #[arg(long)]
        dry_run: bool,

        /// Maximum line diagnostics retained in the result.
        #[arg(long, default_value_t = 100, value_name = "COUNT")]
        max_diagnostics: usize,
    },

    /// List recently active imported sessions.
    Sessions {
        /// Maximum number of sessions to return.
        #[arg(long, default_value_t = 50, value_name = "COUNT")]
        limit: u32,
    },

    /// Recall evidence by exact signature and/or full text with ranked explanations.
    Search {
        /// FTS5 query expression. Required unless `--signature` is supplied.
        #[arg(required_unless_present = "signature")]
        query: Option<String>,

        /// Exact normalized operation signature, such as
        /// `operation/v1|shell|cargo test`.
        #[arg(long, value_name = "SIGNATURE")]
        signature: Option<String>,

        /// Restrict to one exact project path (repository filter).
        #[arg(long, value_name = "PATH")]
        project: Option<String>,

        /// Restrict to events within the last N days (recency filter).
        #[arg(long, value_name = "DAYS")]
        since_days: Option<u32>,

        /// Restrict to an exact AEP event type. Repeatable (event-kind filter).
        #[arg(long = "event-kind", value_name = "TYPE")]
        event_kinds: Vec<String>,

        /// Restrict to a success or failure outcome polarity.
        #[arg(long, value_enum, value_name = "OUTCOME")]
        outcome: Option<OutcomeArg>,

        /// Maximum number of matches to return.
        #[arg(long, default_value_t = 20, value_name = "COUNT")]
        limit: u32,
    },

    /// Run every deterministic detector and emit a digestion report.
    Digest {
        /// Limit digestion to one exact project path.
        #[arg(long, value_name = "PATH")]
        project: Option<String>,

        #[command(flatten)]
        thresholds: ThresholdArgs,
    },

    /// List deterministic Evidence Packet v0.1 findings.
    Patterns {
        /// Limit detection to one exact project path.
        #[arg(long, value_name = "PATH")]
        project: Option<String>,

        #[command(flatten)]
        thresholds: ThresholdArgs,
    },

    /// Manage review-only, zero-permission mutation candidates.
    Mutations {
        #[command(subcommand)]
        action: MutationAction,
    },

    /// Export redacted canonical AEP events as JSONL to standard output.
    Export {
        /// Limit export to one exact project path.
        #[arg(long, value_name = "PATH")]
        project: Option<String>,
    },

    /// Apply an age-based session retention policy.
    Prune {
        /// Delete sessions whose last event is older than this many days.
        #[arg(long, value_name = "DAYS")]
        older_than_days: u32,

        /// Limit pruning to one exact project path.
        #[arg(long, value_name = "PATH")]
        project: Option<String>,

        /// Report the exact deletion effect and roll the transaction back.
        #[arg(long)]
        dry_run: bool,
    },

    /// Delete a session or all local Autophagy data.
    Delete {
        #[command(subcommand)]
        target: DeleteTarget,
    },

    /// Continuously ingest native agent history until interrupted.
    ///
    /// Ingest-only: applies the same redaction, privacy, and projection gates as
    /// `import` and never executes or installs anything.
    Watch {
        /// Native adapter to watch. Repeatable; defaults to all native adapters.
        #[arg(long = "adapter", value_enum, value_name = "ADAPTER")]
        adapters: Vec<watch::NativeAdapter>,

        /// Seconds to wait between discovery cycles.
        #[arg(long, default_value_t = config::DEFAULT_INTERVAL_SECONDS, value_name = "SECONDS")]
        interval: u64,

        /// Run one cycle and exit (useful for launchd/systemd and tests).
        #[arg(long)]
        once: bool,

        /// Persist native-adapter prompt, response, and tool-result text.
        #[arg(long)]
        include_content: bool,

        /// Include only events with this exact project path. Repeatable.
        #[arg(long = "project", value_name = "PATH")]
        projects: Vec<String>,

        /// Exclude project or artifact paths matching this glob. Repeatable.
        #[arg(long = "exclude-path", value_name = "GLOB")]
        exclude_paths: Vec<String>,
    },

    /// Manage the background watch daemon (launchd on macOS, systemd on Linux).
    Daemon {
        #[command(subcommand)]
        action: DaemonCommand,
    },

    /// Rebuild the derived search index from already-stored events.
    ///
    /// Heals a database imported before signature indexing existed, or without
    /// `--index-tool-input`: reimport is an idempotent no-op, so this rebuilds
    /// the free-text and exact-signature projections in place from the stored
    /// events. It never alters events, cursors, or evidence.
    Reindex {
        /// Rebuild the exact-signature index and make redacted tool input
        /// searchable. Mirrors `import --index-tool-input`; without it, only
        /// project paths and tool names stay searchable.
        #[arg(long)]
        index_tool_input: bool,

        /// Index an already-redacted event metadata key as searchable text.
        /// Repeatable. Mirrors `import --index-metadata`.
        #[arg(long = "index-metadata", value_name = "KEY")]
        index_metadata: Vec<String>,

        /// Exclude project or artifact paths matching this glob from the rebuilt
        /// index under current policy. Repeatable.
        #[arg(long = "exclude-path", value_name = "GLOB")]
        exclude_paths: Vec<String>,
    },

    /// Guided first-run: pick what to import and monitor, then see results.
    ///
    /// Detects each local coding agent, imports the ones you choose, runs the
    /// deterministic digest, and optionally installs background monitoring.
    /// With no terminal, pass `--adapter`, `--index-tool-input`, `--monitor`,
    /// and `--yes` to run the same flow non-interactively.
    Setup {
        /// Native adapter to set up. Repeatable. Restricts detection to these;
        /// defaults to every native adapter.
        #[arg(long = "adapter", value_enum, value_name = "ADAPTER")]
        adapters: Vec<watch::NativeAdapter>,

        /// Make the commands your agents ran searchable (exact recall). Secrets
        /// are filtered by redaction rules. Selects the import indexing gate.
        #[arg(long)]
        index_tool_input: bool,

        /// Also persist prompt, response, and tool-result text in event
        /// metadata. Still local and redacted.
        #[arg(long)]
        include_content: bool,

        /// Index an already-redacted event metadata key as searchable text.
        /// Repeatable.
        #[arg(long = "index-metadata", value_name = "KEY")]
        index_metadata: Vec<String>,

        /// Install background monitoring (a launchd/systemd user service).
        #[arg(long)]
        monitor: bool,

        /// Seconds between discovery cycles for installed monitoring.
        #[arg(long, default_value_t = config::DEFAULT_INTERVAL_SECONDS, value_name = "SECONDS")]
        interval: u64,

        /// Assume yes and run non-interactively without prompting.
        #[arg(long)]
        yes: bool,
    },

    /// Show local state: database, imports, index, daemon, and thresholds.
    ///
    /// A fast, read-only snapshot that works against an empty database and with
    /// no config file. Honours `--output json`.
    Status,

    /// Read and write the persistent configuration file.
    ///
    /// Config sets your defaults so you do not repeat flags. Precedence is
    /// built-in defaults, then this file, then any explicit flag on a command.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

/// Subcommands of `autophagy config`.
#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Show every effective value and whether it comes from config or default.
    List,
    /// Print one effective value.
    Get {
        /// Dotted key, e.g. `detect.min_occurrences`.
        key: String,
    },
    /// Set one value (validated and typed) in the config file.
    Set {
        /// Dotted key, e.g. `watch.interval_seconds`.
        key: String,
        /// New value. Lists are comma-separated, e.g. `claude-code,codex`.
        value: String,
    },
    /// Remove one value, reverting it to the built-in default.
    Unset {
        /// Dotted key to remove.
        key: String,
    },
    /// Print the config file path.
    Path,
    /// Open the config file in `$EDITOR`, then validate the result.
    Edit,
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    /// Generate and load a user-level supervisor unit running `autophagy watch`.
    Install {
        /// Native adapter to watch. Repeatable; defaults to all native adapters.
        #[arg(long = "adapter", value_enum, value_name = "ADAPTER")]
        adapters: Vec<watch::NativeAdapter>,

        /// Seconds to wait between discovery cycles.
        #[arg(long, default_value_t = config::DEFAULT_INTERVAL_SECONDS, value_name = "SECONDS")]
        interval: u64,
    },
    /// Unload and remove the supervisor unit, leaving nothing behind.
    Uninstall,
    /// Report whether the unit is present and the job loaded.
    Status,
}

/// Detector threshold flags shared by every detection-bearing command.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Copy, Debug, clap::Args)]
pub struct ThresholdArgs {
    /// Minimum supporting events.
    #[arg(long, default_value_t = config::DEFAULT_MIN_OCCURRENCES, value_name = "COUNT")]
    min_occurrences: u32,

    /// Minimum distinct supporting sessions.
    #[arg(long, default_value_t = config::DEFAULT_MIN_SESSIONS, value_name = "COUNT")]
    min_sessions: u32,

    /// Optional anti-noise floor on support share in basis points (0-10000).
    ///
    /// Qualification is decided by recurrence, not failure share; this floor
    /// defaults to 0 (disabled) and only suppresses candidates whose failure
    /// share is vanishingly small.
    #[arg(long, default_value_t = config::DEFAULT_MIN_SUPPORT_RATIO_BPS, value_parser = clap::value_parser!(u16).range(0..=10_000), value_name = "BPS")]
    min_support_ratio_bps: u16,
}

impl From<ThresholdArgs> for DetectorConfig {
    fn from(value: ThresholdArgs) -> Self {
        Self {
            min_occurrences: value.min_occurrences,
            min_sessions: value.min_sessions,
            min_support_ratio_bps: value.min_support_ratio_bps,
        }
    }
}

#[derive(Debug, Subcommand)]
enum DeleteTarget {
    /// Delete one session and its evidence.
    Session {
        /// AEP session identifier.
        session_id: String,
    },
    /// Delete every local source, cursor, session, event, and artifact.
    All {
        /// Required destructive confirmation phrase: `delete-all`.
        #[arg(long, value_name = "PHRASE")]
        confirm: String,
    },
}

#[derive(Debug, Subcommand)]
enum MutationAction {
    /// Detect findings and register deterministic review candidates.
    Propose {
        /// Limit candidate generation to one exact project path.
        #[arg(long, value_name = "PATH")]
        project: Option<String>,

        #[command(flatten)]
        thresholds: ThresholdArgs,

        /// Generate packages without changing the registry.
        #[arg(long)]
        dry_run: bool,
    },
    /// Synthesize candidates through a provider-neutral, contract-bound boundary.
    Synthesize {
        /// Synthesis provider to consult. Must match the manifest `format`.
        /// Defaults to `synthesis.provider` in config, else `deterministic`.
        #[arg(long, value_enum, default_value_t = SynthesisProviderChoice::Deterministic)]
        provider: SynthesisProviderChoice,

        /// Local model manifest (synthesis-manifest/0.1 or 0.2) JSON file.
        /// Defaults to `synthesis.manifest_path` in config when omitted.
        #[arg(long, value_name = "PATH")]
        manifest: Option<PathBuf>,

        /// Allow an HTTP provider endpoint whose host is not loopback. Evidence
        /// then leaves this machine; a warning is emitted. Off by default.
        #[arg(long)]
        allow_remote_endpoint: bool,

        /// Limit synthesis to one exact project path.
        #[arg(long, value_name = "PATH")]
        project: Option<String>,

        #[command(flatten)]
        thresholds: ThresholdArgs,

        /// Synthesize packages without changing the registry.
        #[arg(long)]
        dry_run: bool,
    },
    /// List all registered candidates and their current state.
    List,
    /// Show one immutable package and its complete lifecycle audit.
    Show {
        /// Stable mutation identity.
        mutation_id: String,
    },
    /// Record completion of every adversarial review check.
    Challenge {
        /// Stable mutation identity.
        mutation_id: String,

        /// Completed challenge check. Repeat until all required checks are present.
        #[arg(long = "check", value_enum, value_name = "CHECK")]
        checks: Vec<ChallengeCheck>,

        /// Optional reviewer context retained in the audit record.
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
    },
    /// Reject a candidate with an auditable reason.
    Reject {
        /// Stable mutation identity.
        mutation_id: String,

        /// Human-readable rejection reason.
        #[arg(long, value_name = "TEXT")]
        reason: String,
    },
    /// Evaluate annotated decision points without executing the mutation.
    Replay {
        /// Stable mutation identity.
        mutation_id: String,

        /// Replay Suite v0.1 JSON file.
        #[arg(long, value_name = "PATH")]
        scenarios: PathBuf,
    },
    /// Export an evidence-linked Replay Suite draft for human annotation.
    ReplayDraft {
        /// Stable mutation identity.
        mutation_id: String,

        /// Destination for the Replay Suite v0.1 JSON draft.
        #[arg(long, value_name = "PATH")]
        suite: PathBuf,

        /// Nearby events retained on either side of each exact evidence event.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(0..=20), value_name = "COUNT")]
        context_events: u8,

        /// Replace an existing destination file.
        #[arg(long)]
        force: bool,
    },
    /// Measure would-be trigger precision without applying the mutation.
    Shadow {
        /// Stable mutation identity.
        mutation_id: String,

        /// Shadow Suite v0.1 JSON file.
        #[arg(long, value_name = "PATH")]
        observations: PathBuf,
    },
    /// Install one shadow-passed mutation as a repo-scoped coding-agent skill.
    Install {
        /// Stable mutation identity.
        mutation_id: String,

        /// Existing target repository root.
        #[arg(long, value_name = "PATH")]
        repository: PathBuf,

        /// Coding agent to materialize the skill for.
        #[arg(long, value_enum, default_value_t = InstallTargetChoice::Codex)]
        target: InstallTargetChoice,

        /// Required phrase acknowledging the scoped filesystem write: `repo-skill-write`.
        #[arg(long, value_name = "PHRASE")]
        confirm_permissions: String,

        /// Preview the exact path and content hash without writing or activating.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove an audited repo-scoped skill and retire its mutation.
    Uninstall {
        /// Stable mutation identity.
        mutation_id: String,
    },
}

/// Coding-agent installation target selectable on the command line.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum InstallTargetChoice {
    /// Codex repo-scoped skill under `.agents/skills`.
    Codex,
    /// Claude Code repo-scoped skill under `.claude/skills`.
    ClaudeCode,
}

impl From<InstallTargetChoice> for InstallTarget {
    fn from(choice: InstallTargetChoice) -> Self {
        match choice {
            InstallTargetChoice::Codex => Self::Codex,
            InstallTargetChoice::ClaudeCode => Self::ClaudeCode,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum SynthesisProviderChoice {
    /// Built-in pure, model-free, offline reference provider.
    Deterministic,
    /// Local Ollama server (`/api/chat` with JSON Schema structured output).
    Ollama,
    /// Local OpenAI-compatible server (`/v1/chat/completions` with `json_schema`).
    OpenaiCompatible,
}

impl SynthesisProviderChoice {
    /// The manifest format this provider choice requires.
    const fn required_format(self) -> ModelFormat {
        match self {
            Self::Deterministic => ModelFormat::Deterministic,
            Self::Ollama => ModelFormat::Ollama,
            Self::OpenaiCompatible => ModelFormat::OpenAiCompatible,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic",
            Self::Ollama => "ollama",
            Self::OpenaiCompatible => "openai-compatible",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum ChallengeCheck {
    CoincidenceConsidered,
    SessionsComparable,
    TriggerObservable,
    LegitimateUsesBounded,
    EquivalentSearched,
    CounterexamplesReviewed,
}

impl ChallengeCheck {
    const ALL: [Self; 6] = [
        Self::CoincidenceConsidered,
        Self::SessionsComparable,
        Self::TriggerObservable,
        Self::LegitimateUsesBounded,
        Self::EquivalentSearched,
        Self::CounterexamplesReviewed,
    ];
}

#[derive(Debug, Serialize)]
#[serde(tag = "command", content = "result", rename_all = "snake_case")]
enum CommandReport {
    Import(ImportReport),
    Sessions(Vec<SessionSummary>),
    Search(Vec<RetrievalHit>),
    Digest(DigestReport),
    Patterns(PatternsReport),
    #[serde(rename = "mutations_propose")]
    MutationProposal(MutationProposalReport),
    #[serde(rename = "mutations_synthesize")]
    MutationSynthesis(MutationSynthesisReport),
    #[serde(rename = "mutations_list")]
    MutationList(Vec<MutationRecord>),
    #[serde(rename = "mutations_show")]
    MutationShow(MutationDetails),
    #[serde(rename = "mutations_transition")]
    MutationTransition(MutationTransitionOutcome),
    #[serde(rename = "mutations_replay")]
    MutationReplay(MutationReplayReport),
    #[serde(rename = "mutations_replay_draft")]
    MutationReplayDraft(MutationReplayDraftReport),
    #[serde(rename = "mutations_shadow")]
    MutationShadow(MutationShadowReport),
    #[serde(rename = "mutations_install")]
    MutationInstall(MutationInstallReport),
    #[serde(rename = "mutations_uninstall")]
    MutationUninstall(InstallationTransitionOutcome),
    Export(Vec<Event>),
    Prune(PruneSummary),
    DeleteSession(DeleteSummary),
    DeleteAll(DeleteAllSummary),
    Watch(watch::WatchRunReport),
    Daemon(daemon::DaemonReport),
    Reindex(ReindexReport),
    Setup(setup::SetupReport),
    Status(Box<status::StatusReport>),
    Config(config::ConfigReport),
}

#[derive(Debug, Serialize)]
struct ReindexReport {
    index_tool_input: bool,
    #[serde(flatten)]
    summary: ReindexSummary,
}

#[derive(Debug, Serialize)]
struct DigestReport {
    spec_version: &'static str,
    generated_at: String,
    events_scanned: usize,
    sessions_scanned: usize,
    candidate_signatures: usize,
    model_used: bool,
    network_used: bool,
    findings: Vec<EvidencePacket>,
    observations: Vec<Observation>,
}

/// Build a digest report from one deterministic detection pass. Shared by the
/// `digest` command and `setup`'s immediate digest so both render the same
/// deterministic, model-free report through the same path.
fn digest_report(report: DetectionReport) -> Result<DigestReport, CliError> {
    let DetectionDiagnostics {
        events_scanned,
        sessions_scanned,
        candidate_signatures,
        observations,
    } = report.diagnostics;
    Ok(DigestReport {
        spec_version: "digest/0.1",
        generated_at: OffsetDateTime::now_utc().format(&Rfc3339)?,
        events_scanned,
        sessions_scanned,
        candidate_signatures,
        model_used: false,
        network_used: false,
        findings: report.findings,
        observations,
    })
}

#[derive(Debug, Serialize)]
struct PatternsReport {
    events_scanned: usize,
    sessions_scanned: usize,
    candidate_signatures: usize,
    findings: Vec<EvidencePacket>,
    observations: Vec<Observation>,
}

#[derive(Debug, Serialize)]
struct MutationProposalReport {
    dry_run: bool,
    generated: Vec<GenerationOutcome>,
    registrations: Vec<MutationRegisterOutcome>,
}

#[derive(Debug, Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct MutationSynthesisReport {
    dry_run: bool,
    provider: String,
    model: String,
    model_used: bool,
    network_used: bool,
    remote_endpoint_allowed: bool,
    manifest_path: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_completion_tokens: Option<u64>,
    synthesized: Vec<SynthesisOutcome>,
    registrations: Vec<MutationRegisterOutcome>,
}

#[derive(Debug, Serialize)]
struct ChallengeAssessment {
    spec_version: &'static str,
    checks: Vec<ChallengeCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Debug, Serialize)]
struct MutationReplayReport {
    evaluation: ReplayReport,
    registration: ReplayRegisterOutcome,
}

#[derive(Debug, Serialize)]
struct MutationReplayDraftReport {
    path: String,
    context_events: u8,
    scenarios: usize,
    intervention_scenarios: usize,
    no_op_scenarios: usize,
    unreviewed_scenarios: usize,
    draft: ReplaySuite,
}

#[derive(Debug, Serialize)]
struct MutationShadowReport {
    evaluation: ShadowReport,
    registration: ShadowRegisterOutcome,
}

#[derive(Debug, Serialize)]
struct MutationInstallReport {
    installation_id: String,
    target: &'static str,
    repository_root: String,
    relative_path: String,
    content_hash: String,
    required_permission: &'static str,
    dry_run: bool,
    materialized: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    transition: Option<InstallationTransitionOutcome>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ImportReport {
    Generic(ImportSummary),
    ClaudeCode(ClaudeImportSummary),
    Codex(CodexImportSummary),
    Pi(PiImportSummary),
    Opencode(OpenCodeImportSummary),
}

impl ImportReport {
    const fn has_issues(&self) -> bool {
        match self {
            Self::Generic(summary) => summary.has_issues(),
            Self::ClaudeCode(summary) => summary.has_issues(),
            Self::Codex(summary) => summary.has_issues(),
            Self::Pi(summary) => summary.has_issues(),
            Self::Opencode(summary) => summary.has_issues(),
        }
    }
}

impl CommandReport {
    const fn has_issues(&self) -> bool {
        match self {
            Self::Import(summary) => summary.has_issues(),
            Self::MutationReplay(report) => !report.evaluation.passed,
            Self::MutationShadow(report) => !report.evaluation.passed,
            Self::Watch(report) => report.has_issues(),
            Self::Sessions(_)
            | Self::Search(_)
            | Self::Digest(_)
            | Self::Patterns(_)
            | Self::MutationProposal(_)
            | Self::MutationSynthesis(_)
            | Self::MutationList(_)
            | Self::MutationShow(_)
            | Self::MutationTransition(_)
            | Self::MutationReplayDraft(_)
            | Self::MutationInstall(_)
            | Self::MutationUninstall(_)
            | Self::Export(_)
            | Self::Prune(_)
            | Self::DeleteSession(_)
            | Self::DeleteAll(_)
            | Self::Daemon(_)
            | Self::Reindex(_)
            | Self::Setup(_)
            | Self::Status(_)
            | Self::Config(_) => false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("I/O operation failed: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Import(#[from] autophagy_core::ImportError),
    #[error(transparent)]
    ClaudeImport(#[from] autophagy_adapter_claude_code::ClaudeImportError),
    #[error(transparent)]
    ClaudeDiscovery(#[from] autophagy_adapter_claude_code::DiscoveryError),
    #[error(transparent)]
    CodexImport(#[from] autophagy_adapter_codex::CodexImportError),
    #[error(transparent)]
    CodexDiscovery(#[from] autophagy_adapter_codex::CodexDiscoveryError),
    #[error(transparent)]
    PiImport(#[from] autophagy_adapter_pi::PiImportError),
    #[error(transparent)]
    PiDiscovery(#[from] autophagy_adapter_pi::PiDiscoveryError),
    #[error(transparent)]
    OpenCodeImport(#[from] autophagy_adapter_opencode::OpenCodeImportError),
    #[error(transparent)]
    OpenCodeDiscovery(#[from] autophagy_adapter_opencode::OpenCodeDiscoveryError),
    #[error("could not serialize command output: {0}")]
    Json(#[from] serde_json::Error),
    #[error("could not determine the platform-local application data directory")]
    DataDirectoryUnavailable,
    #[error("could not format report timestamp: {0}")]
    TimeFormat(#[from] time::error::Format),
    #[error("deleting all data requires --confirm delete-all")]
    DeleteAllConfirmation,
    #[error("challenge is incomplete; missing checks: {0}")]
    IncompleteChallenge(String),
    #[error(transparent)]
    Replay(#[from] ReplayEvaluationError),
    #[error(transparent)]
    ReplayDraft(#[from] ReplayDraftError),
    #[error(transparent)]
    Shadow(#[from] ShadowEvaluationError),
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error(transparent)]
    Supervisor(#[from] SupervisorError),
    #[error("could not determine the home directory; set HOME")]
    HomeDirectoryUnavailable,
    #[error("supervisor command failed ({command}): {detail}")]
    SupervisorCommand {
        /// The command that failed.
        command: String,
        /// Failure detail.
        detail: String,
    },
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(
        "provider '{provider}' requires a manifest with format '{expected}', but the manifest declares '{actual}'"
    )]
    ProviderFormatMismatch {
        provider: &'static str,
        expected: &'static str,
        actual: &'static str,
    },
    #[error(
        "synthesis endpoint host '{host}' is not loopback; pass --allow-remote-endpoint to send evidence off this machine"
    )]
    RemoteEndpointRefused { host: String },
    #[error("replay scenario cites event '{0}', which is not in the local evidence store")]
    MissingReplayEvidence(String),
    #[error("shadow observation cites event '{0}', which is not in the local evidence store")]
    MissingShadowEvidence(String),
    #[error("installation requires --confirm-permissions repo-skill-write")]
    InstallPermissionConfirmation,
    #[error("installation audit does not match the deterministic materialization plan")]
    InstallationAuditMismatch,
    #[error("audit update failed ({primary}); filesystem rollback also failed ({rollback})")]
    FilesystemAuditRollback { primary: String, rollback: String },
    #[error(transparent)]
    Reindex(#[from] ReindexError),
    #[error(
        "setup needs an interactive terminal; re-run with explicit flags to go non-interactive, \
         e.g. `autophagy setup --adapter claude-code --index-tool-input --monitor --yes`"
    )]
    SetupNonInteractive,
    #[error("configuration error: {0}")]
    Config(String),
    #[error(
        "mutations synthesize needs a manifest; pass --manifest <PATH> or set synthesis.manifest_path in config"
    )]
    MissingManifest,
}

fn main() -> ExitCode {
    // Parse into `ArgMatches` as well as the typed `Cli`, so precedence can ask
    // clap which flags were explicitly passed (via `ValueSource`).
    let matches = Cli::command().get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    let output = cli.output;
    match dispatch(cli, &matches).and_then(|report| {
        let has_issues = report.has_issues();
        write_report(io::stdout().lock(), output, &report)?;
        Ok(has_issues)
    }) {
        Ok(true) => ExitCode::from(2),
        Ok(false) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Load config (except for the `config` command, which manages its own loading
/// so it keeps working against a malformed file), then execute.
fn dispatch(cli: Cli, matches: &ArgMatches) -> Result<CommandReport, CliError> {
    if matches!(cli.command, Commands::Config { .. }) {
        let Commands::Config { action } = cli.command else {
            unreachable!("checked above")
        };
        return Ok(CommandReport::Config(config::run(action)?));
    }
    let mut warnings = Vec::new();
    let config = config::Config::load(&mut warnings)?;
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    execute(cli, matches, &config)
}

/// Descend to the deepest active subcommand's matches, where the leaf flags
/// (e.g. `mutations propose --min-occurrences`) actually live.
fn leaf_matches(matches: &ArgMatches) -> &ArgMatches {
    let mut current = matches;
    while let Some((_, sub)) = current.subcommand() {
        current = sub;
    }
    current
}

#[allow(clippy::too_many_lines)]
fn execute(
    cli: Cli,
    matches: &ArgMatches,
    config: &config::Config,
) -> Result<CommandReport, CliError> {
    let leaf = leaf_matches(matches);
    match cli.command {
        Commands::Import {
            input,
            adapter,
            instance_key,
            display_name,
            projects,
            exclude_paths,
            include_subagents,
            include_content,
            index_tool_input,
            index_metadata,
            dry_run,
            max_diagnostics,
        } => {
            // default < config < explicit flag, resolved once for every adapter.
            let index_tool_input = config::resolve_bool(
                leaf,
                "index_tool_input",
                index_tool_input,
                config.import_index_tool_input,
            );
            let include_content = config::resolve_bool(
                leaf,
                "include_content",
                include_content,
                config.import_include_content,
            );
            let index_metadata = config::resolve_list(
                leaf,
                "index_metadata",
                &index_metadata,
                config.import_index_metadata.as_deref(),
            );
            let exclude_paths = config::resolve_list(
                leaf,
                "exclude_paths",
                &exclude_paths,
                config.import_exclude_paths.as_deref(),
            );
            match adapter {
                ImportAdapter::GenericJsonl => {
                    let instance_key = instance_key.unwrap_or(derive_instance_key(&input)?);
                    let mut options = ImportOptions::new(instance_key);
                    options.display_name = display_name;
                    options.projects = projects;
                    options.exclude_paths = exclude_paths;
                    options.index_tool_input = index_tool_input;
                    options.index_metadata = index_metadata;
                    options.dry_run = dry_run;
                    options.max_diagnostics = max_diagnostics;
                    let reader = open_input(&input)?;
                    let summary = if dry_run {
                        import_jsonl(reader, None, &options)?
                    } else {
                        let database = resolve_database_path(cli.database)?;
                        let mut store = open_store(&database)?;
                        import_jsonl(reader, Some(&mut store), &options)?
                    };
                    Ok(CommandReport::Import(ImportReport::Generic(summary)))
                }
                ImportAdapter::ClaudeCode => {
                    let input = if input == Path::new("-") {
                        default_projects_root()?
                    } else {
                        input
                    };
                    let instance_key = instance_key.unwrap_or(derive_instance_key(&input)?);
                    let mut options = ClaudeImportOptions::new(input, instance_key);
                    options.display_name = display_name;
                    options.projects = projects;
                    options.exclude_paths = exclude_paths;
                    options.include_subagents = include_subagents;
                    options.include_content = include_content;
                    options.index_tool_input = index_tool_input;
                    options.index_metadata = index_metadata;
                    options.dry_run = dry_run;
                    options.max_diagnostics = max_diagnostics;
                    let summary = if dry_run {
                        import_claude_code(None, &options)?
                    } else {
                        let database = resolve_database_path(cli.database)?;
                        let mut store = open_store(&database)?;
                        import_claude_code(Some(&mut store), &options)?
                    };
                    Ok(CommandReport::Import(ImportReport::ClaudeCode(summary)))
                }
                ImportAdapter::Codex => {
                    let input = if input == Path::new("-") {
                        default_sessions_root()?
                    } else {
                        input
                    };
                    let instance_key = instance_key.unwrap_or(derive_instance_key(&input)?);
                    let mut options = CodexImportOptions::new(input, instance_key);
                    options.display_name = display_name;
                    options.projects = projects;
                    options.exclude_paths = exclude_paths;
                    options.include_content = include_content;
                    options.index_tool_input = index_tool_input;
                    options.index_metadata = index_metadata;
                    options.dry_run = dry_run;
                    options.max_diagnostics = max_diagnostics;
                    let summary = if dry_run {
                        import_codex(None, &options)?
                    } else {
                        let database = resolve_database_path(cli.database)?;
                        let mut store = open_store(&database)?;
                        import_codex(Some(&mut store), &options)?
                    };
                    Ok(CommandReport::Import(ImportReport::Codex(summary)))
                }
                ImportAdapter::Pi => {
                    let input = if input == Path::new("-") {
                        default_pi_sessions_root()?
                    } else {
                        input
                    };
                    let instance_key = instance_key.unwrap_or(derive_instance_key(&input)?);
                    let mut options = PiImportOptions::new(input, instance_key);
                    options.display_name = display_name;
                    options.projects = projects;
                    options.exclude_paths = exclude_paths;
                    options.include_content = include_content;
                    options.index_tool_input = index_tool_input;
                    options.index_metadata = index_metadata;
                    options.dry_run = dry_run;
                    options.max_diagnostics = max_diagnostics;
                    let summary = if dry_run {
                        import_pi(None, &options)?
                    } else {
                        let database = resolve_database_path(cli.database)?;
                        let mut store = open_store(&database)?;
                        import_pi(Some(&mut store), &options)?
                    };
                    Ok(CommandReport::Import(ImportReport::Pi(summary)))
                }
                ImportAdapter::Opencode => {
                    let input = if input == Path::new("-") {
                        default_storage_root()?
                    } else {
                        input
                    };
                    let instance_key = instance_key.unwrap_or(derive_instance_key(&input)?);
                    let mut options = OpenCodeImportOptions::new(input, instance_key);
                    options.display_name = display_name;
                    options.projects = projects;
                    options.exclude_paths = exclude_paths;
                    options.include_content = include_content;
                    options.index_tool_input = index_tool_input;
                    options.index_metadata = index_metadata;
                    options.dry_run = dry_run;
                    options.max_diagnostics = max_diagnostics;
                    let summary = if dry_run {
                        import_opencode(None, &options)?
                    } else {
                        let database = resolve_database_path(cli.database)?;
                        let mut store = open_store(&database)?;
                        import_opencode(Some(&mut store), &options)?
                    };
                    Ok(CommandReport::Import(ImportReport::Opencode(summary)))
                }
            }
        }
        Commands::Sessions { limit } => {
            let database = resolve_database_path(cli.database)?;
            let store = open_store(&database)?;
            Ok(CommandReport::Sessions(store.list_sessions(limit)?))
        }
        Commands::Search {
            query,
            signature,
            project,
            since_days,
            event_kinds,
            outcome,
            limit,
        } => {
            let database = resolve_database_path(cli.database)?;
            let store = open_store(&database)?;
            let since =
                since_days.map(|days| OffsetDateTime::now_utc() - Duration::days(i64::from(days)));
            let retrieval = RetrievalQuery {
                text: query,
                signature,
                project,
                since,
                event_kinds,
                outcome: outcome.map(RetrievalOutcome::from),
                limit,
            };
            Ok(CommandReport::Search(store.retrieve(&retrieval)?))
        }
        Commands::Digest {
            project,
            thresholds,
        } => {
            let database = resolve_database_path(cli.database)?;
            let store = open_store(&database)?;
            let events = store.list_events_for_detection(project.as_deref())?;
            let report = detect_with_report(
                &events,
                config::resolve_thresholds(leaf, thresholds, config),
            );
            Ok(CommandReport::Digest(digest_report(report)?))
        }
        Commands::Patterns {
            project,
            thresholds,
        } => {
            let database = resolve_database_path(cli.database)?;
            let store = open_store(&database)?;
            let events = store.list_events_for_detection(project.as_deref())?;
            let report = detect_with_report(
                &events,
                config::resolve_thresholds(leaf, thresholds, config),
            );
            let DetectionDiagnostics {
                events_scanned,
                sessions_scanned,
                candidate_signatures,
                observations,
            } = report.diagnostics;
            Ok(CommandReport::Patterns(PatternsReport {
                events_scanned,
                sessions_scanned,
                candidate_signatures,
                findings: report.findings,
                observations,
            }))
        }
        Commands::Mutations { action } => {
            execute_mutation_action(cli.database, action, leaf, config)
        }
        Commands::Export { project } => {
            let database = resolve_database_path(cli.database)?;
            let store = open_store(&database)?;
            Ok(CommandReport::Export(
                store.list_events_for_detection(project.as_deref())?,
            ))
        }
        Commands::Prune {
            older_than_days,
            project,
            dry_run,
        } => {
            let database = resolve_database_path(cli.database)?;
            let mut store = open_store(&database)?;
            let cutoff = OffsetDateTime::now_utc() - Duration::days(i64::from(older_than_days));
            Ok(CommandReport::Prune(store.prune_before(
                cutoff,
                project.as_deref(),
                dry_run,
            )?))
        }
        Commands::Delete { target } => {
            let database = resolve_database_path(cli.database)?;
            let mut store = open_store(&database)?;
            match target {
                DeleteTarget::Session { session_id } => Ok(CommandReport::DeleteSession(
                    store.delete_session(&session_id)?,
                )),
                DeleteTarget::All { confirm } => {
                    if confirm != "delete-all" {
                        return Err(CliError::DeleteAllConfirmation);
                    }
                    Ok(CommandReport::DeleteAll(store.delete_all()?))
                }
            }
        }
        Commands::Watch {
            adapters,
            interval,
            once,
            include_content,
            projects,
            exclude_paths,
        } => {
            let adapters = config::resolve_adapters(leaf, &adapters, config)?;
            let interval = config::resolve_interval(leaf, interval, config);
            let include_content = config::resolve_bool(
                leaf,
                "include_content",
                include_content,
                config.import_include_content,
            );
            let exclude_paths = config::resolve_list(
                leaf,
                "exclude_paths",
                &exclude_paths,
                config.import_exclude_paths.as_deref(),
            );
            let report = watch::run(
                cli.database,
                &adapters,
                interval,
                once,
                include_content,
                &projects,
                &exclude_paths,
                cli.output,
            )?;
            Ok(CommandReport::Watch(report))
        }
        Commands::Daemon { action } => {
            let report = match action {
                DaemonCommand::Install { adapters, interval } => {
                    // The unit bakes in explicit args, but they are derived from
                    // config at install time (config changes need a reinstall).
                    let daemon_leaf = leaf_matches(matches);
                    let adapters = config::resolve_adapters(daemon_leaf, &adapters, config)?;
                    let interval = config::resolve_interval(daemon_leaf, interval, config);
                    let names = watch::adapter_names(&adapters);
                    daemon::install(cli.database, interval, names)?
                }
                DaemonCommand::Uninstall => daemon::uninstall(cli.database)?,
                DaemonCommand::Status => daemon::status(cli.database)?,
            };
            Ok(CommandReport::Daemon(report))
        }
        Commands::Reindex {
            index_tool_input,
            index_metadata,
            exclude_paths,
        } => {
            let index_tool_input = config::resolve_bool(
                leaf,
                "index_tool_input",
                index_tool_input,
                config.import_index_tool_input,
            );
            let index_metadata = config::resolve_list(
                leaf,
                "index_metadata",
                &index_metadata,
                config.import_index_metadata.as_deref(),
            );
            let exclude_paths = config::resolve_list(
                leaf,
                "exclude_paths",
                &exclude_paths,
                config.import_exclude_paths.as_deref(),
            );
            let database = resolve_database_path(cli.database)?;
            let mut store = open_store(&database)?;
            let options = ReindexOptions {
                index_tool_input,
                index_metadata,
                exclude_paths,
            };
            let summary = reindex(&mut store, &options)?;
            Ok(CommandReport::Reindex(ReindexReport {
                index_tool_input,
                summary,
            }))
        }
        Commands::Setup {
            adapters,
            index_tool_input,
            include_content,
            index_metadata,
            monitor,
            interval,
            yes,
        } => {
            let report = setup::run(
                cli.database,
                cli.output,
                config,
                setup::SetupPlan {
                    adapters,
                    adapters_explicit: config::flag_set(leaf, "adapters"),
                    index_tool_input,
                    index_tool_input_explicit: config::flag_set(leaf, "index_tool_input"),
                    include_content,
                    include_content_explicit: config::flag_set(leaf, "include_content"),
                    index_metadata,
                    index_metadata_explicit: config::flag_set(leaf, "index_metadata"),
                    monitor,
                    interval,
                    interval_explicit: config::flag_set(leaf, "interval"),
                    yes,
                },
            )?;
            Ok(CommandReport::Setup(report))
        }
        Commands::Status => Ok(CommandReport::Status(Box::new(status::run(
            cli.database,
            config,
        )?))),
        // Handled in `dispatch` before config is loaded.
        Commands::Config { .. } => unreachable!("config handled in dispatch"),
    }
}

#[allow(clippy::too_many_lines)]
fn execute_mutation_action(
    database: Option<PathBuf>,
    action: MutationAction,
    matches: &ArgMatches,
    config: &config::Config,
) -> Result<CommandReport, CliError> {
    let database = resolve_database_path(database)?;
    let mut store = open_store(&database)?;
    match action {
        MutationAction::Propose {
            project,
            thresholds,
            dry_run,
        } => {
            let events = store.list_events_for_detection(project.as_deref())?;
            let findings = detect(
                &events,
                config::resolve_thresholds(matches, thresholds, config),
            );
            let generated = generate_candidates(&findings);
            let mut registrations = Vec::new();
            if !dry_run {
                for outcome in &generated {
                    let GenerationOutcome::Candidate { package } = outcome else {
                        continue;
                    };
                    let registration = MutationRegistration {
                        mutation_id: package.mutation_id.clone(),
                        source_finding_id: package.source_finding_id.clone(),
                        source_detector: package.source_detector.as_str().to_owned(),
                        equivalence_key: equivalence_key(package),
                        spec_version: package.spec_version.as_str().to_owned(),
                        semantic_version: package.version.clone(),
                        package: serde_json::to_value(package)?,
                        supporting_event_ids: package.hypothesis.supporting_event_ids.clone(),
                        counterexample_event_ids: package
                            .hypothesis
                            .counterexample_event_ids
                            .clone(),
                    };
                    registrations.push(store.register_mutation(&registration)?);
                }
            }
            Ok(CommandReport::MutationProposal(MutationProposalReport {
                dry_run,
                generated,
                registrations,
            }))
        }
        MutationAction::Synthesize {
            provider,
            manifest,
            allow_remote_endpoint,
            project,
            thresholds,
            dry_run,
        } => {
            // Resolve provider and manifest under default < config < flag.
            let provider = if config::flag_set(matches, "provider") {
                provider
            } else {
                match config.synthesis_provider.as_deref() {
                    Some("ollama") => SynthesisProviderChoice::Ollama,
                    Some("openai-compatible") => SynthesisProviderChoice::OpenaiCompatible,
                    _ => provider,
                }
            };
            let manifest = match manifest {
                Some(path) => path,
                None => config
                    .synthesis_manifest_path
                    .as_ref()
                    .map(PathBuf::from)
                    .ok_or(CliError::MissingManifest)?,
            };
            let manifest_path = manifest.display().to_string();
            let model_manifest = ModelManifest::from_path(&manifest)?;
            // The provider choice must match what the manifest declares.
            if model_manifest.format != provider.required_format() {
                return Err(CliError::ProviderFormatMismatch {
                    provider: provider.as_str(),
                    expected: provider.required_format().as_str(),
                    actual: model_manifest.format.as_str(),
                });
            }
            let synthesis_provider: Box<dyn SynthesisProvider> = match provider {
                SynthesisProviderChoice::Deterministic => Box::new(DeterministicReferenceProvider),
                SynthesisProviderChoice::Ollama => Box::new(OllamaProvider::from_manifest(
                    &model_manifest,
                    allow_remote_endpoint,
                )),
                SynthesisProviderChoice::OpenaiCompatible => Box::new(
                    OpenAiCompatibleProvider::from_manifest(&model_manifest, allow_remote_endpoint),
                ),
            };
            // Enforce the loopback default up front for HTTP providers: refuse a
            // non-loopback endpoint unless the operator opted in, and warn
            // clearly when they did.
            let mut warnings = Vec::new();
            if synthesis_provider.uses_network() {
                if let Ok(EndpointLocality::Remote { host }) =
                    classify_endpoint(&model_manifest.path)
                {
                    if allow_remote_endpoint {
                        warnings.push(format!(
                            "evidence will be sent to NON-LOOPBACK endpoint host '{host}' because --allow-remote-endpoint is set; the structured request (baseline text, constraints, and cited event IDs) leaves this machine"
                        ));
                    } else {
                        return Err(CliError::RemoteEndpointRefused { host });
                    }
                }
            }
            let events = store.list_events_for_detection(project.as_deref())?;
            let findings = detect(
                &events,
                config::resolve_thresholds(matches, thresholds, config),
            );
            let synthesized =
                synthesize_candidates(&findings, &model_manifest, synthesis_provider.as_ref());
            let (total_prompt_tokens, total_completion_tokens) = aggregate_usage(&synthesized);
            let mut registrations = Vec::new();
            if !dry_run {
                for outcome in &synthesized {
                    let SynthesisOutcome::Candidate { package, .. } = outcome else {
                        continue;
                    };
                    let registration = MutationRegistration {
                        mutation_id: package.mutation_id.clone(),
                        source_finding_id: package.source_finding_id.clone(),
                        source_detector: package.source_detector.as_str().to_owned(),
                        equivalence_key: equivalence_key(package),
                        spec_version: package.spec_version.as_str().to_owned(),
                        semantic_version: package.version.clone(),
                        package: serde_json::to_value(package)?,
                        supporting_event_ids: package.hypothesis.supporting_event_ids.clone(),
                        counterexample_event_ids: package
                            .hypothesis
                            .counterexample_event_ids
                            .clone(),
                    };
                    registrations.push(store.register_mutation(&registration)?);
                }
            }
            Ok(CommandReport::MutationSynthesis(MutationSynthesisReport {
                dry_run,
                provider: synthesis_provider.name().to_owned(),
                model: model_manifest.name.clone(),
                model_used: synthesis_provider.uses_model(),
                network_used: synthesis_provider.uses_network(),
                remote_endpoint_allowed: allow_remote_endpoint,
                manifest_path,
                warnings,
                total_prompt_tokens,
                total_completion_tokens,
                synthesized,
                registrations,
            }))
        }
        MutationAction::List => Ok(CommandReport::MutationList(store.list_mutations()?)),
        MutationAction::Show { mutation_id } => Ok(CommandReport::MutationShow(
            store.get_mutation(&mutation_id)?,
        )),
        MutationAction::Challenge {
            mutation_id,
            checks,
            note,
        } => {
            let completed = checks.into_iter().collect::<BTreeSet<_>>();
            let missing = ChallengeCheck::ALL
                .into_iter()
                .filter(|check| !completed.contains(check))
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                let missing = missing
                    .iter()
                    .map(|check| {
                        check
                            .to_possible_value()
                            .expect("value enum")
                            .get_name()
                            .to_owned()
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(CliError::IncompleteChallenge(missing));
            }
            let assessment = ChallengeAssessment {
                spec_version: "challenge/0.1",
                checks: completed.into_iter().collect(),
                note,
            };
            Ok(CommandReport::MutationTransition(
                store.challenge_mutation(&mutation_id, &serde_json::to_value(assessment)?)?,
            ))
        }
        MutationAction::Reject {
            mutation_id,
            reason,
        } => Ok(CommandReport::MutationTransition(
            store.reject_mutation(&mutation_id, &reason)?,
        )),
        MutationAction::Replay {
            mutation_id,
            scenarios,
        } => {
            let suite: ReplaySuite = serde_json::from_slice(&fs::read(scenarios)?)?;
            let details = store.get_mutation(&mutation_id)?;
            let package = serde_json::from_value(details.mutation.package)?;
            let evaluation = evaluate(&package, &suite)?;
            for event_id in suite
                .scenarios
                .iter()
                .flat_map(|scenario| &scenario.source_event_ids)
            {
                if store.get_event(event_id)?.is_none() {
                    return Err(CliError::MissingReplayEvidence(event_id.clone()));
                }
            }
            let registration = store.register_replay(&ReplayRegistration {
                replay_id: evaluation.replay_id.clone(),
                mutation_id: evaluation.mutation_id.clone(),
                scenario_set_hash: evaluation.scenario_set_hash.clone(),
                report: serde_json::to_value(&evaluation)?,
                passed: evaluation.passed,
                source_event_ids: suite
                    .scenarios
                    .iter()
                    .flat_map(|scenario| scenario.source_event_ids.iter().cloned())
                    .collect(),
            })?;
            Ok(CommandReport::MutationReplay(MutationReplayReport {
                evaluation,
                registration,
            }))
        }
        MutationAction::ReplayDraft {
            mutation_id,
            suite,
            context_events,
            force,
        } => {
            let details = store.get_mutation(&mutation_id)?;
            let package = serde_json::from_value(details.mutation.package)?;
            let events = store.list_events_for_detection(None)?;
            let draft = extract_review_draft(&package, &events, usize::from(context_events))?;
            let intervention_scenarios = draft
                .scenarios
                .iter()
                .filter(|scenario| scenario.expected_action == ExpectedAction::Intervene)
                .count();
            let no_op_scenarios = draft.scenarios.len() - intervention_scenarios;
            let unreviewed_scenarios = draft
                .scenarios
                .iter()
                .filter(|scenario| {
                    scenario.counterfactual_outcome == Some(CounterfactualOutcome::Unknown)
                })
                .count();
            let mut options = fs::OpenOptions::new();
            options.write(true);
            if force {
                options.create(true).truncate(true);
            } else {
                options.create_new(true);
            }
            let mut destination = options.open(&suite)?;
            serde_json::to_writer_pretty(&mut destination, &draft)?;
            writeln!(destination)?;
            Ok(CommandReport::MutationReplayDraft(
                MutationReplayDraftReport {
                    path: suite.to_string_lossy().into_owned(),
                    context_events,
                    scenarios: draft.scenarios.len(),
                    intervention_scenarios,
                    no_op_scenarios,
                    unreviewed_scenarios,
                    draft,
                },
            ))
        }
        MutationAction::Shadow {
            mutation_id,
            observations,
        } => {
            let suite: ShadowSuite = serde_json::from_slice(&fs::read(observations)?)?;
            let details = store.get_mutation(&mutation_id)?;
            let package = serde_json::from_value(details.mutation.package)?;
            let evaluation = evaluate_shadow(&package, &suite)?;
            for event_id in suite
                .observations
                .iter()
                .flat_map(|observation| &observation.source_event_ids)
            {
                if store.get_event(event_id)?.is_none() {
                    return Err(CliError::MissingShadowEvidence(event_id.clone()));
                }
            }
            let registration = store.register_shadow(&ShadowRegistration {
                shadow_id: evaluation.shadow_id.clone(),
                mutation_id: evaluation.mutation_id.clone(),
                observation_set_hash: evaluation.observation_set_hash.clone(),
                report: serde_json::to_value(&evaluation)?,
                passed: evaluation.passed,
                source_event_ids: suite
                    .observations
                    .iter()
                    .flat_map(|observation| observation.source_event_ids.iter().cloned())
                    .collect(),
            })?;
            Ok(CommandReport::MutationShadow(MutationShadowReport {
                evaluation,
                registration,
            }))
        }
        MutationAction::Install {
            mutation_id,
            repository,
            target,
            confirm_permissions,
            dry_run,
        } => {
            if confirm_permissions != "repo-skill-write" {
                return Err(CliError::InstallPermissionConfirmation);
            }
            let details = store.get_mutation(&mutation_id)?;
            if details.mutation.state != "shadow_passed" {
                return Err(StoreError::MutationStateTransition {
                    mutation_id,
                    from_state: details.mutation.state,
                    to_state: "active",
                }
                .into());
            }
            let package = serde_json::from_value(details.mutation.package)?;
            let plan = plan_skill(&package, &repository, target.into())?;
            if dry_run {
                return Ok(CommandReport::MutationInstall(install_report(
                    &plan, true, false, None,
                )));
            }
            let artifact = materialize(&plan)?;
            let registration = InstallationRegistration {
                installation_id: plan.installation_id.clone(),
                mutation_id: plan.mutation_id.clone(),
                target: plan.target.registry_id().to_owned(),
                repository_root: plan.repository_root.to_string_lossy().into_owned(),
                relative_path: portable_relative_path(&plan.relative_path),
                content_hash: plan.content_hash.clone(),
                permission_review: serde_json::json!({
                    "confirmed": "repo-skill-write",
                    "filesystem_write": plan.relative_path,
                    "package_permissions": package.permissions,
                }),
            };
            match store.register_installation(&registration) {
                Ok(transition) => Ok(CommandReport::MutationInstall(install_report(
                    &plan,
                    false,
                    true,
                    Some(transition),
                ))),
                Err(primary) => match unmaterialize(&artifact) {
                    Ok(()) => Err(primary.into()),
                    Err(rollback) => Err(CliError::FilesystemAuditRollback {
                        primary: primary.to_string(),
                        rollback: rollback.to_string(),
                    }),
                },
            }
        }
        MutationAction::Uninstall { mutation_id } => {
            let audit = store.get_installation(&mutation_id)?;
            let details = store.get_mutation(&mutation_id)?;
            let package = serde_json::from_value(details.mutation.package)?;
            let target = InstallTarget::from_registry_id(&audit.target)
                .ok_or(CliError::InstallationAuditMismatch)?;
            let plan = plan_skill(&package, Path::new(&audit.repository_root), target)?;
            if plan.installation_id != audit.installation_id
                || portable_relative_path(&plan.relative_path) != audit.relative_path
                || plan.content_hash != audit.content_hash
                || plan.target.registry_id() != audit.target
            {
                return Err(CliError::InstallationAuditMismatch);
            }
            let artifact = InstalledArtifact {
                mutation_id: mutation_id.clone(),
                repository_root: plan.repository_root.clone(),
                relative_path: plan.relative_path.clone(),
                content_hash: plan.content_hash.clone(),
            };
            unmaterialize(&artifact)?;
            match store.record_uninstall(&mutation_id) {
                Ok(outcome) => Ok(CommandReport::MutationUninstall(outcome)),
                Err(primary) => match materialize(&plan) {
                    Ok(_) => Err(primary.into()),
                    Err(rollback) => Err(CliError::FilesystemAuditRollback {
                        primary: primary.to_string(),
                        rollback: rollback.to_string(),
                    }),
                },
            }
        }
    }
}

fn install_report(
    plan: &SkillPlan,
    dry_run: bool,
    materialized: bool,
    transition: Option<InstallationTransitionOutcome>,
) -> MutationInstallReport {
    MutationInstallReport {
        installation_id: plan.installation_id.clone(),
        target: plan.target.registry_id(),
        repository_root: plan.repository_root.to_string_lossy().into_owned(),
        relative_path: portable_relative_path(&plan.relative_path),
        content_hash: plan.content_hash.clone(),
        required_permission: "repo-skill-write",
        dry_run,
        materialized,
        transition,
    }
}

fn portable_relative_path(path: &Path) -> String {
    path.iter()
        .map(|component| component.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn resolve_database_path(path: Option<PathBuf>) -> Result<PathBuf, CliError> {
    if let Some(path) = path {
        return Ok(path);
    }
    let project = ProjectDirs::from("sh", "autophagy", "Autophagy")
        .ok_or(CliError::DataDirectoryUnavailable)?;
    Ok(project.data_local_dir().join("autophagy.db"))
}

fn open_store(path: &Path) -> Result<EventStore, CliError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    Ok(EventStore::open(path)?)
}

fn open_input(path: &Path) -> Result<Box<dyn BufRead>, CliError> {
    if path == Path::new("-") {
        Ok(Box::new(BufReader::new(io::stdin())))
    } else {
        Ok(Box::new(BufReader::new(File::open(path)?)))
    }
}

fn derive_instance_key(input: &Path) -> Result<String, CliError> {
    if input == Path::new("-") {
        Ok("stdin".to_owned())
    } else {
        let canonical = fs::canonicalize(input)?;
        let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
        let mut encoded = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
        }
        Ok(format!("path:{encoded}"))
    }
}

#[allow(clippy::too_many_lines)]
fn write_report(
    mut writer: impl Write,
    format: OutputFormat,
    report: &CommandReport,
) -> Result<(), CliError> {
    if let CommandReport::Export(events) = report {
        for event in events {
            serde_json::to_writer(&mut writer, event)?;
            writeln!(writer)?;
        }
        return Ok(());
    }
    // The watch loop streams its own per-cycle lines and final summary during
    // execution, so there is no deferred report to render here.
    if matches!(report, CommandReport::Watch(_)) {
        return Ok(());
    }
    match format {
        OutputFormat::Json => {
            serde_json::to_writer_pretty(&mut writer, report)?;
            writeln!(writer)?;
        }
        OutputFormat::Text => match report {
            CommandReport::Import(summary) => match summary {
                ImportReport::Generic(summary) => write_import_summary(&mut writer, summary)?,
                ImportReport::ClaudeCode(summary) => {
                    write_claude_import_summary(&mut writer, summary)?;
                }
                ImportReport::Codex(summary) => write_codex_import_summary(&mut writer, summary)?,
                ImportReport::Pi(summary) => write_pi_import_summary(&mut writer, summary)?,
                ImportReport::Opencode(summary) => {
                    write_opencode_import_summary(&mut writer, summary)?;
                }
            },
            CommandReport::Sessions(sessions) => {
                writeln!(writer, "SESSION\tSOURCE\tEVENTS\tLAST EVENT\tPROJECT")?;
                for session in sessions {
                    writeln!(
                        writer,
                        "{}\t{}\t{}\t{}\t{}",
                        session.session_id,
                        session.adapter,
                        session.event_count,
                        session.last_event_at,
                        session.project_path.as_deref().unwrap_or("-")
                    )?;
                }
            }
            CommandReport::Search(hits) => {
                if hits.is_empty() {
                    writeln!(writer, "no retrieval matches")?;
                }
                for hit in hits {
                    writeln!(
                        writer,
                        "{}\t{}\t{} bps\t{}",
                        hit.event_id,
                        hit.explanation.match_kind.as_str(),
                        hit.explanation.rank_score_bps,
                        hit.snippet
                            .as_deref()
                            .or(hit.signature.as_deref())
                            .unwrap_or("-")
                    )?;
                }
            }
            CommandReport::Digest(report) => write_detection(
                &mut writer,
                "local deterministic digest",
                report.events_scanned,
                report.sessions_scanned,
                report.candidate_signatures,
                &report.findings,
                &report.observations,
            )?,
            CommandReport::Patterns(report) => write_detection(
                &mut writer,
                "deterministic evidence packets",
                report.events_scanned,
                report.sessions_scanned,
                report.candidate_signatures,
                &report.findings,
                &report.observations,
            )?,
            CommandReport::MutationProposal(report) => {
                write_mutation_proposal(&mut writer, report)?;
            }
            CommandReport::MutationSynthesis(report) => {
                write_mutation_synthesis(&mut writer, report)?;
            }
            CommandReport::MutationList(mutations) => {
                if mutations.is_empty() {
                    writeln!(writer, "no registered mutation candidates")?;
                }
                for mutation in mutations {
                    writeln!(
                        writer,
                        "{}\t{}\t{}",
                        mutation.mutation_id,
                        mutation.state,
                        mutation.package["title"].as_str().unwrap_or("untitled")
                    )?;
                }
            }
            CommandReport::MutationShow(details) => {
                writeln!(
                    writer,
                    "{}\t{}\t{}",
                    details.mutation.mutation_id,
                    details.mutation.state,
                    details.mutation.package["title"]
                        .as_str()
                        .unwrap_or("untitled")
                )?;
                for transition in &details.transitions {
                    writeln!(
                        writer,
                        "{}\t{} -> {}\t{}",
                        transition.occurred_at,
                        transition.from_state.as_deref().unwrap_or("none"),
                        transition.to_state,
                        transition.reason
                    )?;
                }
                for replay in &details.replays {
                    writeln!(
                        writer,
                        "{}\treplay {}\tpassed={}",
                        replay.created_at, replay.replay_id, replay.passed
                    )?;
                }
                for shadow in &details.shadows {
                    writeln!(
                        writer,
                        "{}\tshadow {}\tpassed={}",
                        shadow.created_at, shadow.shadow_id, shadow.passed
                    )?;
                }
                for installation in &details.installations {
                    writeln!(
                        writer,
                        "{}\tinstallation {}\t{}\t{}",
                        installation.installed_at,
                        installation.installation_id,
                        installation.state,
                        installation.relative_path
                    )?;
                }
            }
            CommandReport::MutationTransition(outcome) => writeln!(
                writer,
                "{}\t{} -> {}\tchanged={}",
                outcome.mutation_id, outcome.from_state, outcome.to_state, outcome.changed
            )?,
            CommandReport::MutationReplay(report) => writeln!(
                writer,
                "{}\tpassed={}\t{} success · {} no-op · {} contradiction · {} false intervention\tmutation_executed=false",
                report.evaluation.replay_id,
                report.evaluation.passed,
                report.evaluation.summary.successes,
                report.evaluation.summary.no_ops,
                report.evaluation.summary.contradictions,
                report.evaluation.summary.false_interventions
            )?,
            CommandReport::MutationReplayDraft(report) => writeln!(
                writer,
                "{}\t{} scenarios · {} intervention · {} no-op · {} unreviewed\t{}",
                report.draft.mutation_id,
                report.scenarios,
                report.intervention_scenarios,
                report.no_op_scenarios,
                report.unreviewed_scenarios,
                report.path
            )?,
            CommandReport::MutationShadow(report) => writeln!(
                writer,
                "{}\tpassed={}\tprecision={} bps · recall={} bps · {} false positives\tmutation_applied=false",
                report.evaluation.shadow_id,
                report.evaluation.passed,
                report.evaluation.summary.precision_bps,
                report.evaluation.summary.recall_bps,
                report.evaluation.summary.false_positives
            )?,
            CommandReport::MutationInstall(report) => writeln!(
                writer,
                "{}\t{}\t{}\t{}\t{}",
                report.installation_id,
                report.relative_path,
                if report.dry_run {
                    "dry-run"
                } else {
                    "installed"
                },
                report.content_hash,
                report.target
            )?,
            CommandReport::MutationUninstall(outcome) => writeln!(
                writer,
                "{}\t{}\t{}",
                outcome.installation_id, outcome.installation_state, outcome.mutation_state
            )?,
            CommandReport::Prune(summary) => writeln!(
                writer,
                "{} sessions · {} events · {} artifacts · {} mutations{}",
                summary.sessions_deleted,
                summary.events_deleted,
                summary.artifacts_deleted,
                summary.mutations_deleted,
                if summary.dry_run {
                    " · dry run"
                } else {
                    " deleted"
                }
            )?,
            CommandReport::DeleteSession(summary) => writeln!(
                writer,
                "session_deleted={} · {} events · {} artifacts · {} mutations",
                summary.session_deleted,
                summary.events_deleted,
                summary.artifacts_deleted,
                summary.mutations_deleted
            )?,
            CommandReport::DeleteAll(summary) => writeln!(
                writer,
                "{} sources · {} sessions · {} events · {} artifacts · {} conflicts · {} cursors · {} mutations deleted",
                summary.sources_deleted,
                summary.sessions_deleted,
                summary.events_deleted,
                summary.artifacts_deleted,
                summary.conflicts_deleted,
                summary.cursors_deleted,
                summary.mutations_deleted
            )?,
            CommandReport::Export(_) => unreachable!("export handled before format selection"),
            CommandReport::Watch(_) => unreachable!("watch streams its own output"),
            CommandReport::Daemon(report) => daemon::write_text(report, &mut writer)?,
            CommandReport::Reindex(report) => write_reindex(&mut writer, report)?,
            // Setup streams its own guided text during execution, so there is no
            // deferred text to render here.
            CommandReport::Setup(_) => {}
            CommandReport::Status(report) => status::write_text(report, &mut writer)?,
            CommandReport::Config(report) => config::write_text(report, &mut writer)?,
        },
    }
    Ok(())
}

fn write_detection(
    writer: &mut impl Write,
    header: &str,
    events_scanned: usize,
    sessions_scanned: usize,
    candidate_signatures: usize,
    findings: &[EvidencePacket],
    observations: &[Observation],
) -> io::Result<()> {
    writeln!(
        writer,
        "{events_scanned} events · {sessions_scanned} sessions · {candidate_signatures} candidate signatures · {} findings · {header}",
        findings.len()
    )?;
    write_findings(writer, findings)?;
    // Never a silent zero: when nothing qualified, show the strongest recurring
    // candidates and the exact gate each missed so the scan explains itself.
    if findings.is_empty() {
        write_observations(writer, observations)?;
    }
    Ok(())
}

fn write_reindex(writer: &mut impl Write, report: &ReindexReport) -> io::Result<()> {
    writeln!(
        writer,
        "{} events scanned · {} search rows · {} signatures · {} fields redacted",
        report.summary.events_scanned,
        report.summary.search_rows_written,
        report.summary.signatures_written,
        report.summary.redacted_fields
    )?;
    if report.index_tool_input {
        writeln!(
            writer,
            "commands are now searchable by exact signature and free text"
        )?;
    } else {
        writeln!(
            writer,
            "only project paths and tool names are searchable; pass --index-tool-input to rebuild the exact-command index"
        )?;
    }
    Ok(())
}

fn write_findings(writer: &mut impl Write, findings: &[EvidencePacket]) -> io::Result<()> {
    if findings.is_empty() {
        writeln!(writer, "no findings above threshold")?;
    }
    for finding in findings {
        writeln!(
            writer,
            "{}\t{}\t{} bps\t{} evidence\t{} counterexamples",
            finding.finding_id,
            finding.title,
            finding.score.score_bps,
            finding.evidence.len(),
            finding.counterexamples.len()
        )?;
    }
    Ok(())
}

fn write_observations(writer: &mut impl Write, observations: &[Observation]) -> io::Result<()> {
    if observations.is_empty() {
        return Ok(());
    }
    writeln!(
        writer,
        "near-threshold observations (recurring candidates, not findings):"
    )?;
    for observation in observations {
        writeln!(
            writer,
            "{}\t{} occ · {} sessions · {} counterexamples · {} bps support\tunmet: {}",
            observation.title,
            observation.score.occurrences,
            observation.score.distinct_sessions,
            observation.score.counterexamples,
            observation.score.support_ratio_bps,
            observation.unmet_gate.as_str()
        )?;
    }
    Ok(())
}

fn write_mutation_proposal(
    writer: &mut impl Write,
    report: &MutationProposalReport,
) -> io::Result<()> {
    if report.generated.is_empty() {
        writeln!(writer, "no mutation candidates above evidence threshold")?;
    }
    for outcome in &report.generated {
        match outcome {
            GenerationOutcome::Candidate { package } => writeln!(
                writer,
                "{}\t{}\t{} evidence\tzero permissions\tcandidate",
                package.mutation_id,
                package.title,
                package.hypothesis.supporting_event_ids.len()
            )?,
            GenerationOutcome::InsufficientEvidence { finding_id, reason } => {
                writeln!(writer, "{finding_id}\tinsufficient evidence\t{reason}")?;
            }
        }
    }
    if report.dry_run {
        writeln!(writer, "dry run · registry unchanged")?;
    } else if !report.registrations.is_empty() {
        writeln!(writer, "{} registry outcomes", report.registrations.len())?;
    }
    Ok(())
}

fn aggregate_usage(outcomes: &[SynthesisOutcome]) -> (Option<u64>, Option<u64>) {
    let mut prompt: Option<u64> = None;
    let mut completion: Option<u64> = None;
    let add = |total: &mut Option<u64>, value: Option<u64>| {
        if let Some(value) = value {
            *total = Some(total.unwrap_or(0) + value);
        }
    };
    for outcome in outcomes {
        let usage = match outcome {
            SynthesisOutcome::Candidate { usage, .. }
            | SynthesisOutcome::ProviderDeclined { usage, .. }
            | SynthesisOutcome::Rejected { usage, .. } => *usage,
            SynthesisOutcome::InsufficientEvidence { .. }
            | SynthesisOutcome::ProviderError { .. } => TokenUsage::unavailable(),
        };
        add(&mut prompt, usage.prompt_tokens);
        add(&mut completion, usage.completion_tokens);
    }
    (prompt, completion)
}

fn format_tokens(tokens: Option<u64>) -> String {
    tokens.map_or_else(|| "unavailable".to_owned(), |value| value.to_string())
}

fn write_mutation_synthesis(
    writer: &mut impl Write,
    report: &MutationSynthesisReport,
) -> io::Result<()> {
    for warning in &report.warnings {
        writeln!(writer, "warning: {warning}")?;
    }
    writeln!(
        writer,
        "provider={} · model={} · model_used={} · network_used={} · remote_endpoint_allowed={}",
        report.provider,
        report.model,
        report.model_used,
        report.network_used,
        report.remote_endpoint_allowed
    )?;
    if report.model_used {
        writeln!(
            writer,
            "tokens · prompt={} · completion={}",
            format_tokens(report.total_prompt_tokens),
            format_tokens(report.total_completion_tokens)
        )?;
    }
    if report.synthesized.is_empty() {
        writeln!(writer, "no mutation candidates above evidence threshold")?;
    }
    for outcome in &report.synthesized {
        match outcome {
            SynthesisOutcome::Candidate { package, .. } => writeln!(
                writer,
                "{}\t{}\t{} evidence\tzero permissions\tcandidate",
                package.mutation_id,
                package.title,
                package.hypothesis.supporting_event_ids.len()
            )?,
            SynthesisOutcome::InsufficientEvidence { finding_id, reason } => {
                writeln!(writer, "{finding_id}\tinsufficient evidence\t{reason}")?;
            }
            SynthesisOutcome::ProviderDeclined {
                finding_id,
                provider,
                reason,
                ..
            } => {
                writeln!(
                    writer,
                    "{finding_id}\tprovider {provider} declined\t{reason}"
                )?;
            }
            SynthesisOutcome::Rejected {
                finding_id,
                provider,
                diagnostics,
                ..
            } => {
                writeln!(
                    writer,
                    "{finding_id}\tprovider {provider} rejected\t{} violation(s)",
                    diagnostics.len()
                )?;
                for diagnostic in diagnostics {
                    writeln!(
                        writer,
                        "\t{}\t{}\t{}",
                        diagnostic.path, diagnostic.code, diagnostic.message
                    )?;
                }
            }
            SynthesisOutcome::ProviderError {
                finding_id,
                provider,
                message,
            } => {
                writeln!(writer, "{finding_id}\tprovider {provider} error\t{message}")?;
            }
        }
    }
    if report.dry_run {
        writeln!(writer, "dry run · registry unchanged")?;
    } else if !report.registrations.is_empty() {
        writeln!(writer, "{} registry outcomes", report.registrations.len())?;
    }
    Ok(())
}

fn write_codex_import_summary(
    writer: &mut impl Write,
    summary: &CodexImportSummary,
) -> io::Result<()> {
    writeln!(
        writer,
        "{} files · {} records · {} events · {} inserted · {} duplicates · {} conflicts · {} unsupported · {} privacy excluded · {} redacted fields · {} rejected{}",
        summary.discovery.files.len(),
        summary.records_seen,
        summary.events_emitted,
        summary.inserted,
        summary.duplicates,
        summary.conflicts,
        summary.unsupported,
        summary.privacy_skipped,
        summary.redacted_fields,
        summary.rejected,
        if summary.dry_run { " · dry run" } else { "" }
    )?;
    for file in &summary.discovery.files {
        writeln!(writer, "{}\t{} bytes", file.relative_path, file.size_bytes)?;
    }
    for diagnostic in &summary.diagnostics {
        writeln!(
            writer,
            "{}:{} [{}] {}",
            diagnostic.file, diagnostic.line, diagnostic.code, diagnostic.message
        )?;
    }
    if summary.diagnostics_suppressed > 0 {
        writeln!(
            writer,
            "{} additional diagnostics suppressed",
            summary.diagnostics_suppressed
        )?;
    }
    Ok(())
}

fn write_pi_import_summary(writer: &mut impl Write, summary: &PiImportSummary) -> io::Result<()> {
    writeln!(
        writer,
        "{} files · {} records · {} events · {} inserted · {} duplicates · {} conflicts · {} unsupported · {} privacy excluded · {} redacted fields · {} rejected{}",
        summary.discovery.files.len(),
        summary.records_seen,
        summary.events_emitted,
        summary.inserted,
        summary.duplicates,
        summary.conflicts,
        summary.unsupported,
        summary.privacy_skipped,
        summary.redacted_fields,
        summary.rejected,
        if summary.dry_run { " · dry run" } else { "" }
    )?;
    for file in &summary.discovery.files {
        writeln!(writer, "{}\t{} bytes", file.relative_path, file.size_bytes)?;
    }
    for diagnostic in &summary.diagnostics {
        writeln!(
            writer,
            "{}:{} [{}] {}",
            diagnostic.file, diagnostic.line, diagnostic.code, diagnostic.message
        )?;
    }
    if summary.diagnostics_suppressed > 0 {
        writeln!(
            writer,
            "{} additional diagnostics suppressed",
            summary.diagnostics_suppressed
        )?;
    }
    Ok(())
}

fn write_opencode_import_summary(
    writer: &mut impl Write,
    summary: &OpenCodeImportSummary,
) -> io::Result<()> {
    writeln!(
        writer,
        "{} sessions · {} messages · {} parts · {} events · {} inserted · {} duplicates · {} conflicts · {} unsupported · {} privacy excluded · {} redacted fields · {} rejected{}",
        summary.discovery.sessions.len(),
        summary.records_seen,
        summary.parts_seen,
        summary.events_emitted,
        summary.inserted,
        summary.duplicates,
        summary.conflicts,
        summary.unsupported,
        summary.privacy_skipped,
        summary.redacted_fields,
        summary.rejected,
        if summary.dry_run { " · dry run" } else { "" }
    )?;
    for session in &summary.discovery.sessions {
        writeln!(
            writer,
            "{}\t{} messages\t{} bytes",
            session.relative_path, session.message_count, session.size_bytes
        )?;
    }
    for diagnostic in &summary.diagnostics {
        writeln!(
            writer,
            "{} [{}] {}",
            diagnostic.file, diagnostic.code, diagnostic.message
        )?;
    }
    if summary.diagnostics_suppressed > 0 {
        writeln!(
            writer,
            "{} additional diagnostics suppressed",
            summary.diagnostics_suppressed
        )?;
    }
    Ok(())
}

fn write_claude_import_summary(
    writer: &mut impl Write,
    summary: &ClaudeImportSummary,
) -> io::Result<()> {
    writeln!(
        writer,
        "{} files · {} records · {} events · {} inserted · {} duplicates · {} conflicts · {} unsupported · {} privacy excluded · {} redacted fields · {} rejected{}",
        summary.discovery.files.len(),
        summary.records_seen,
        summary.events_emitted,
        summary.inserted,
        summary.duplicates,
        summary.conflicts,
        summary.unsupported,
        summary.privacy_skipped,
        summary.redacted_fields,
        summary.rejected,
        if summary.dry_run { " · dry run" } else { "" }
    )?;
    for file in &summary.discovery.files {
        writeln!(writer, "{}\t{} bytes", file.relative_path, file.size_bytes)?;
    }
    for diagnostic in &summary.diagnostics {
        writeln!(
            writer,
            "{}:{} [{}] {}",
            diagnostic.file, diagnostic.line, diagnostic.code, diagnostic.message
        )?;
    }
    if summary.diagnostics_suppressed > 0 {
        writeln!(
            writer,
            "{} additional diagnostics suppressed",
            summary.diagnostics_suppressed
        )?;
    }
    Ok(())
}

fn write_import_summary(writer: &mut impl Write, summary: &ImportSummary) -> io::Result<()> {
    writeln!(
        writer,
        "{} lines · {} events · {} inserted · {} duplicates · {} conflicts · {} skipped · {} privacy excluded · {} redacted fields · {} rejected{}",
        summary.lines_read,
        summary.events_seen,
        summary.inserted,
        summary.duplicates,
        summary.conflicts,
        summary.skipped,
        summary.privacy_skipped,
        summary.redacted_fields,
        summary.rejected,
        if summary.dry_run { " · dry run" } else { "" }
    )?;
    for diagnostic in &summary.diagnostics {
        writeln!(
            writer,
            "line {} [{}] {}",
            diagnostic.line,
            diagnostic.code.as_str(),
            diagnostic.message
        )?;
    }
    if summary.diagnostics_suppressed > 0 {
        writeln!(
            writer,
            "{} additional diagnostics suppressed",
            summary.diagnostics_suppressed
        )?;
    }
    Ok(())
}
