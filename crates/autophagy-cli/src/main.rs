//! Command-line entry point for importing and querying local agent activity.

mod config;
mod daemon;
mod detection;
mod setup;
mod status;
mod watch;

use std::{
    collections::{BTreeMap, BTreeSet},
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
use autophagy_efficacy::{
    CoverageInput, EfficacyError, EfficacyObservations, EfficacyReport, FailureOccurrence,
    InsufficientReason, MatchingRule, Verdict, WindowBounds, evaluate as evaluate_efficacy,
};
use autophagy_events::Event;
use autophagy_install::{
    InstallError, InstallTarget, InstalledArtifact, SkillPlan, SupervisorError, materialize,
    plan_skill, unmaterialize,
};
use autophagy_mutations::{
    GenerationOutcome, MutationPackage, equivalence_key, generate_candidates,
};
use autophagy_patterns::{
    DetectionDiagnostics, DetectionReport, DetectorConfig, EvidencePacket, Observation, UnmetGate,
};
use autophagy_replay::{
    CounterfactualOutcome, ExpectedAction, ReplayDraftError, ReplayEvaluationError, ReplayReport,
    ReplaySuite, evaluate, extract_review_draft,
};
use autophagy_shadow::{
    ShadowDraftError, ShadowEvaluationError, ShadowReport, ShadowSuite,
    evaluate as evaluate_shadow, extract_observation_draft,
};
use autophagy_store::{
    DeleteAllSummary, DeleteSummary, EfficacyRegisterOutcome, EfficacyRegistration, EventStore,
    InstallationRegistration, InstallationTransitionOutcome, MutationDetails, MutationRecord,
    MutationRegisterOutcome, MutationRegistration, MutationTransitionOutcome, PruneSummary,
    ReplayRegisterOutcome, ReplayRegistration, RetrievalHit, RetrievalOutcome, RetrievalQuery,
    SessionSummary, ShadowRegisterOutcome, ShadowRegistration, StoreError,
};
use autophagy_synthesis::{
    AgentCliProvider, Capability, DeterministicReferenceProvider, EndpointLocality, ManifestError,
    ManifestSpecVersion, ModelFormat, ModelManifest, OllamaProvider, OpenAiCompatibleProvider,
    ResourceHints, SynthesisOutcome, SynthesisProvider, TokenUsage, classify_endpoint,
    synthesize_candidates,
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
    #[command(
        after_help = "Text rows abbreviate event ids and clean snippets for scanning. \
        Use --output json for full event ids, raw snippets, and the complete ranking explanation."
    )]
    Search {
        /// FTS5 query expression. Required unless `--signature` is supplied.
        #[arg(required_unless_present = "signature")]
        query: Option<String>,

        /// Exact normalized operation signature, such as
        /// `operation/v2|shell|cargo test`.
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

        /// Ignore any cached findings and run a fresh detection pass.
        #[arg(long)]
        recompute: bool,
    },

    /// List deterministic Evidence Packet v0.1 findings.
    Patterns {
        /// Limit detection to one exact project path.
        #[arg(long, value_name = "PATH")]
        project: Option<String>,

        #[command(flatten)]
        thresholds: ThresholdArgs,

        /// Ignore any cached findings and run a fresh detection pass.
        #[arg(long)]
        recompute: bool,
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

        /// Synthesis model backend for richer suggestions (non-interactive).
        /// Autophagy is fully functional with `none`.
        #[arg(long = "model-backend", value_enum, value_name = "BACKEND")]
        model_backend: Option<setup::SetupModelBackend>,
    },

    /// Show local state: database, imports, index, daemon, and thresholds.
    ///
    /// A fast, read-only snapshot that works against an empty database and with
    /// no config file. Honours `--output json`.
    Status {
        /// Also count deterministic findings at the effective thresholds. This
        /// runs a full detection pass over every event (digest-cost on a large
        /// store), so it is off by default to keep `status` fast.
        #[arg(long)]
        with_findings: bool,
    },

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
    #[arg(long, default_value_t = config::DEFAULT_MIN_OCCURRENCES, value_parser = clap::value_parser!(u32).range(1..), value_name = "COUNT")]
    min_occurrences: u32,

    /// Minimum distinct supporting sessions.
    #[arg(long, default_value_t = config::DEFAULT_MIN_SESSIONS, value_parser = clap::value_parser!(u32).range(1..), value_name = "COUNT")]
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

        /// Local model manifest (synthesis-manifest/0.1, 0.2, or 0.3) JSON file.
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

        /// Annotated Replay Suite v0.1 JSON file (as written by `replay-draft`).
        /// The former `--scenarios` name stays accepted as a hidden alias.
        #[arg(long, alias = "scenarios", value_name = "PATH")]
        suite: PathBuf,
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
    /// Export an evidence-linked Shadow Suite draft for human annotation.
    ShadowDraft {
        /// Stable mutation identity.
        mutation_id: String,

        /// Destination for the Shadow Suite v0.1 JSON draft.
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

        /// Annotated Shadow Suite v0.1 JSON file (as written by `shadow-draft`).
        #[arg(long, alias = "observations", value_name = "PATH")]
        suite: PathBuf,
    },
    /// Measure whether the addressed failure recurs less since install.
    Efficacy {
        /// Stable mutation identity.
        mutation_id: String,

        /// Evaluation clock (RFC 3339). Defaults to the current time; override
        /// for a reproducible or backdated report.
        #[arg(long, value_name = "RFC3339")]
        now: Option<String>,
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
    /// The authenticated Claude Code CLI, run as a subprocess.
    ClaudeCli,
    /// The authenticated Codex CLI, run as a subprocess.
    CodexCli,
}

impl SynthesisProviderChoice {
    /// The manifest format this provider choice requires.
    const fn required_format(self) -> ModelFormat {
        match self {
            Self::Deterministic => ModelFormat::Deterministic,
            Self::Ollama => ModelFormat::Ollama,
            Self::OpenaiCompatible => ModelFormat::OpenAiCompatible,
            Self::ClaudeCli => ModelFormat::ClaudeCli,
            Self::CodexCli => ModelFormat::CodexCli,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic",
            Self::Ollama => "ollama",
            Self::OpenaiCompatible => "openai-compatible",
            Self::ClaudeCli => "claude-cli",
            Self::CodexCli => "codex-cli",
        }
    }

    /// The cloud vendor an agent-CLI provider reaches through the user's
    /// logged-in CLI, or `None` for local/offline providers.
    const fn cloud_vendor(self) -> Option<&'static str> {
        match self {
            Self::ClaudeCli => Some("Anthropic"),
            Self::CodexCli => Some("OpenAI"),
            Self::Deterministic | Self::Ollama | Self::OpenaiCompatible => None,
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
    Search(SearchReport),
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
    MutationTransition(MutationTransitionReport),
    #[serde(rename = "mutations_replay")]
    MutationReplay(MutationReplayReport),
    #[serde(rename = "mutations_replay_draft")]
    MutationReplayDraft(MutationReplayDraftReport),
    #[serde(rename = "mutations_shadow_draft")]
    MutationShadowDraft(MutationShadowDraftReport),
    #[serde(rename = "mutations_shadow")]
    MutationShadow(MutationShadowReport),
    #[serde(rename = "mutations_efficacy")]
    MutationEfficacy(MutationEfficacyReport),
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

/// Retrieval hits paired with whether the database held any events at all.
///
/// The flag lets the text renderer distinguish "nothing was ever imported" from
/// "imported, but this query matched nothing" — two very different situations
/// for a new user. It never reaches the JSON surface: [`Serialize`] emits only
/// the bare hit array, so `--output json` stays an array and machine consumers
/// see no prose (an empty result is still `[]`).
#[derive(Debug)]
struct SearchReport {
    hits: Vec<RetrievalHit>,
    database_empty: bool,
}

impl Serialize for SearchReport {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.hits.serialize(serializer)
    }
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
    // Text-only: the detector thresholds this pass ran under, used to explain in
    // plain language which gate each near-threshold observation missed. Skipped
    // from JSON so the machine surface stays byte-stable.
    #[serde(skip)]
    thresholds: DetectorConfig,
}

/// Build a digest report from one deterministic detection pass. Shared by the
/// `digest` command and `setup`'s immediate digest so both render the same
/// deterministic, model-free report through the same path.
fn digest_report(
    report: DetectionReport,
    thresholds: DetectorConfig,
) -> Result<DigestReport, CliError> {
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
        thresholds,
    })
}

#[derive(Debug, Serialize)]
struct PatternsReport {
    events_scanned: usize,
    sessions_scanned: usize,
    candidate_signatures: usize,
    findings: Vec<EvidencePacket>,
    observations: Vec<Observation>,
    // Text-only detector thresholds; see `DigestReport::thresholds`. Skipped from
    // JSON to keep the machine surface byte-stable.
    #[serde(skip)]
    thresholds: DetectorConfig,
}

#[derive(Debug, Serialize)]
struct MutationProposalReport {
    dry_run: bool,
    generated: Vec<GenerationOutcome>,
    registrations: Vec<MutationRegisterOutcome>,
    /// Non-fatal registration notes, currently only immutable-package template
    /// conflicts (see [`template_conflict_warning`]). Skipped from JSON when
    /// empty so the machine surface is unchanged for conflict-free runs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// Each generated candidate's CURRENT registry lifecycle state, keyed by
    /// mutation ID, fetched from the store after the registration attempt
    /// above. Deterministic generation re-derives the same mutation ID for
    /// evidence that was already registered, so a candidate can already be
    /// `shadow_passed`, `retired`, etc.; this map is the source of truth the
    /// text and JSON renderers use instead of assuming every row is still
    /// `candidate`. Absent entries mean the mutation has never been
    /// registered (dry-run preview of a brand-new candidate).
    current_states: BTreeMap<String, String>,
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
    /// See [`MutationProposalReport::current_states`]: each synthesized
    /// candidate's CURRENT registry lifecycle state, fetched after the
    /// registration attempt above, keyed by mutation ID.
    current_states: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
struct ChallengeAssessment {
    spec_version: &'static str,
    checks: Vec<ChallengeCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

/// A lifecycle transition plus the id form the user supplied, so the next-step
/// hint echoes their short id rather than the resolved 64-hex identity. The
/// `requested_id` is text-only (`#[serde(skip)]`) so the JSON surface — the bare
/// [`MutationTransitionOutcome`] fields — stays byte-stable.
#[derive(Debug, Serialize)]
struct MutationTransitionReport {
    #[serde(flatten)]
    outcome: MutationTransitionOutcome,
    #[serde(skip)]
    requested_id: String,
}

#[derive(Debug, Serialize)]
struct MutationReplayReport {
    evaluation: ReplayReport,
    registration: ReplayRegisterOutcome,
    // Text-only: the id form the user typed, echoed by the next-step hint.
    #[serde(skip)]
    requested_id: String,
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
    // Text-only: the id form the user typed, echoed by the next-step hint.
    #[serde(skip)]
    requested_id: String,
}

#[derive(Debug, Serialize)]
struct MutationShadowDraftReport {
    path: String,
    context_events: u8,
    observations: usize,
    intervention_observations: usize,
    no_op_observations: usize,
    draft: ShadowSuite,
    // Text-only: the id form the user typed, echoed by the next-step hint.
    #[serde(skip)]
    requested_id: String,
}

#[derive(Debug, Serialize)]
struct MutationShadowReport {
    evaluation: ShadowReport,
    registration: ShadowRegisterOutcome,
    // Text-only: the id form the user typed, echoed by the next-step hint.
    #[serde(skip)]
    requested_id: String,
}

#[derive(Debug, Serialize)]
struct MutationEfficacyReport {
    evaluation: EfficacyReport,
    registration: EfficacyRegisterOutcome,
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
            Self::MutationEfficacy(report) => {
                matches!(report.evaluation.verdict, Verdict::Regressed)
            }
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
            | Self::MutationShadowDraft(_)
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

    /// A single copy-pasteable next command for the mutation lifecycle, printed
    /// to stderr in text mode only. Uses real ids and paths from the outcome so
    /// the user never has to guess the next impassable step. Returns `None` when
    /// there is no unambiguous next action (dry runs, rejections, failed gates).
    fn next_step_hint(&self) -> Option<String> {
        match self {
            Self::MutationProposal(report) => {
                let mutation_id = register_outcome_id(report.registrations.first()?);
                Some(format!(
                    "next: autophagy mutations challenge {mutation_id} \
                     --check coincidence-considered --check sessions-comparable \
                     --check trigger-observable --check legitimate-uses-bounded \
                     --check equivalent-searched --check counterexamples-reviewed"
                ))
            }
            Self::MutationTransition(report) if report.outcome.to_state == "challenged" => {
                Some(format!(
                    "next: autophagy mutations replay-draft {} --suite replay-suite.json",
                    report.requested_id
                ))
            }
            Self::MutationReplayDraft(report) => Some(format!(
                "next: autophagy mutations replay {} --suite {} \
                 (after setting counterfactual_outcome for each intervention scenario)",
                report.requested_id, report.path
            )),
            Self::MutationReplay(report) if report.evaluation.passed => Some(format!(
                "next: autophagy mutations shadow-draft {} --suite shadow-suite.json",
                report.requested_id
            )),
            Self::MutationShadowDraft(report) => Some(format!(
                "next: autophagy mutations shadow {} --suite {} \
                 (after confirming intervention_would_help for each observation)",
                report.requested_id, report.path
            )),
            Self::MutationShadow(report) if report.evaluation.passed => Some(format!(
                "next: autophagy mutations install {} --repository <repo> \
                 --target claude-code --confirm-permissions repo-skill-write",
                report.requested_id
            )),
            _ => None,
        }
    }
}

/// The stored mutation identity behind any registration outcome.
fn register_outcome_id(outcome: &MutationRegisterOutcome) -> &str {
    match outcome {
        MutationRegisterOutcome::Inserted { mutation_id }
        | MutationRegisterOutcome::Duplicate { mutation_id }
        | MutationRegisterOutcome::EquivalentExisting { mutation_id, .. } => mutation_id,
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
    #[error("could not parse a stored timestamp: {0}")]
    TimeParse(#[from] time::error::Parse),
    #[error("deleting all data requires --confirm delete-all")]
    DeleteAllConfirmation,
    #[error("challenge is incomplete; missing checks: {0}")]
    IncompleteChallenge(String),
    #[error(transparent)]
    Replay(#[from] ReplayEvaluationError),
    #[error(transparent)]
    ReplayDraft(#[from] ReplayDraftError),
    #[error(transparent)]
    ShadowDraft(#[from] ShadowDraftError),
    #[error(transparent)]
    Shadow(#[from] ShadowEvaluationError),
    #[error(transparent)]
    Efficacy(#[from] EfficacyError),
    #[error(
        "mutation '{0}' is not installed; efficacy measures post-install recurrence, so install it first"
    )]
    MutationNotInstalled(String),
    #[error("mutation '{0}' carries no trigger selectors to measure")]
    EfficacyNoSelectors(String),
    #[error("could not parse the --now evaluation clock '{0}' as an RFC 3339 timestamp")]
    InvalidNowTimestamp(String),
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
    #[error(
        "provider '{provider}' sends the structured synthesis prompt to {vendor}'s cloud through your logged-in CLI; pass --allow-remote-endpoint to allow it to leave this machine"
    )]
    AgentCliConsentRequired {
        provider: &'static str,
        vendor: &'static str,
    },
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
        "provider '{provider}' needs a manifest; pass --manifest <PATH> or set synthesis.manifest_path in config. \
         For a complete example see docs/specs/synthesis/0.3/manifest/valid/claude_cli.json. \
         (The built-in `deterministic` provider needs no manifest.)"
    )]
    MissingManifest { provider: &'static str },
    #[error(
        "replay suite has {count} unreviewed scenario(s) ({ids}); \
         edit {suite_path} and set counterfactual_outcome to \"expected_result\" or \"contradiction\" for each listed scenario"
    )]
    UnreviewedReplayScenarios {
        count: usize,
        ids: String,
        suite_path: String,
    },
    #[error("id prefix '{query}' is ambiguous; it matches {matches}")]
    AmbiguousId { query: String, matches: String },
}

/// The built-in reference manifest for the offline `deterministic` provider.
///
/// It mirrors `docs/specs/synthesis/0.1/manifest/valid/deterministic.json` so
/// the model-free reference path needs no hand-written manifest: the provider
/// loads no model and performs no I/O, so these fields are pure descriptive
/// metadata. An explicit `--manifest` still overrides this and is validated
/// strictly.
fn builtin_deterministic_manifest() -> ModelManifest {
    ModelManifest {
        spec_version: ManifestSpecVersion::V0_1,
        name: "deterministic-reference".to_owned(),
        format: ModelFormat::Deterministic,
        path: "builtin://deterministic".to_owned(),
        revision: "v1".to_owned(),
        digest: None,
        capabilities: vec![Capability::MutationSynthesis],
        resource_hints: ResourceHints {
            min_memory_mb: 1,
            recommended_memory_mb: None,
            context_window_tokens: None,
        },
        timeouts: None,
        api_key_env: None,
        model: None,
    }
}

/// Number of digest characters kept when abbreviating a stored identifier for a
/// scannable text row. Eight hex digits is enough to stay unique in practice
/// while fitting a terminal column; the full id is always available in JSON.
const SHORT_ID_KEEP: usize = 8;

/// Minimum characters after a type prefix (`mut_`, `ses_`) before a token is
/// treated as a resolvable short-id prefix rather than left for an exact lookup.
const MIN_SHORT_PREFIX: usize = 6;

/// Abbreviate a prefixed identifier for a text row: keep everything up to and
/// including the final `_`, then the first [`SHORT_ID_KEEP`] characters of the
/// digest, marking the elision with a single ellipsis. `evt_claude_<64hex>`
/// becomes `evt_claude_fc1bc1d7…`; `mut_<64hex>` becomes `mut_a1b2c3d4…`. Ids
/// with a short suffix (or no `_`) are returned unchanged.
fn short_id(id: &str) -> String {
    match id.rsplit_once('_') {
        Some((prefix, suffix)) if suffix.chars().count() > SHORT_ID_KEEP => {
            let head: String = suffix.chars().take(SHORT_ID_KEEP).collect();
            format!("{prefix}_{head}…")
        }
        _ => id.to_owned(),
    }
}

/// Render an RFC3339 timestamp as a compact, human-scannable time: a coarse
/// relative age within the last week (`2h ago`), otherwise an absolute
/// `YYYY-MM-DD HH:MM` in UTC. Unparseable input falls back to the raw string so
/// no information is silently dropped.
fn compact_time(timestamp: &str, now: OffsetDateTime) -> String {
    let Ok(then) = OffsetDateTime::parse(timestamp, &Rfc3339) else {
        return timestamp.to_owned();
    };
    let seconds = (now - then).whole_seconds();
    if (0..604_800).contains(&seconds) {
        let seconds = seconds.unsigned_abs();
        return match seconds {
            0..=59 => format!("{seconds}s ago"),
            60..=3599 => format!("{}m ago", seconds / 60),
            3600..=86_399 => format!("{}h ago", seconds / 3600),
            _ => format!("{}d ago", seconds / 86_400),
        };
    }
    let then = then.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        then.year(),
        u8::from(then.month()),
        then.day(),
        then.hour(),
        then.minute()
    )
}

/// Format a basis-points rate as a one-decimal percentage (`7411` → `74.1%`).
/// Percentages are what humans read; the underlying basis-points fields stay in
/// the spec-versioned JSON surface unchanged.
fn percent(bps: u32) -> String {
    format!("{:.1}%", f64::from(bps) / 100.0)
}

/// Format a milli-occurrences-per-week rate for humans (`1810` → `1.8/wk`).
#[allow(clippy::cast_precision_loss)]
fn per_week(milli: i64) -> String {
    format!("{:.1}/wk", milli as f64 / 1000.0)
}

/// Format a signed basis-points change as a signed percentage (`-8700` →
/// `-87.0%`).
#[allow(clippy::cast_precision_loss)]
fn signed_percent(bps: i32) -> String {
    format!("{:+.1}%", f64::from(bps) / 100.0)
}

/// Turn an insufficiency reason into a plain-language "needs" phrase.
const fn efficacy_reason_phrase(reason: InsufficientReason) -> &'static str {
    match reason {
        InsufficientReason::PostWindowTooShort => "a longer post-install window",
        InsufficientReason::SparseOccurrences => "more observed occurrences",
        InsufficientReason::PartialIndexCoverage => "fuller command-index coverage",
    }
}

/// One humanized post-install recurrence summary from a typed efficacy report:
/// verdict, pre/post rates, relative change, and index coverage.
fn efficacy_summary_line(report: &EfficacyReport) -> String {
    let mut parts = vec![format!(
        "{} · {} → {} failures ({} → {})",
        efficacy_verdict_label(report.verdict),
        report.windows.pre.occurrences,
        report.windows.post.occurrences,
        per_week(report.windows.pre.rate_per_week_milli),
        per_week(report.windows.post.rate_per_week_milli),
    )];
    if let Some(delta) = report.rate_delta_bps {
        parts.push(signed_percent(delta));
    } else if report.windows.pre.occurrences == 0 {
        parts.push("no prior baseline".to_owned());
    }
    parts.push(format!(
        "{} classifiable",
        percent(report.coverage.coverage_bps)
    ));
    if report.verdict == Verdict::InsufficientData && !report.insufficient_reasons.is_empty() {
        let needs = report
            .insufficient_reasons
            .iter()
            .map(|reason| efficacy_reason_phrase(*reason))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("needs {needs}"));
    }
    parts.join(" · ")
}

/// Present the verdict enum in prose.
const fn efficacy_verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Improved => "improved",
        Verdict::Regressed => "regressed",
        Verdict::Unchanged => "unchanged",
        Verdict::InsufficientData => "insufficient data",
    }
}

/// Render the `mutations efficacy` result as one humanized line.
fn write_mutation_efficacy(
    writer: &mut impl Write,
    report: &MutationEfficacyReport,
) -> io::Result<()> {
    let duplicate = matches!(
        report.registration,
        EfficacyRegisterOutcome::Duplicate { .. }
    );
    let window_days = report.evaluation.windows.post.duration_seconds / 86_400;
    writeln!(
        writer,
        "{}\t{}\t{window_days}d window\tmodel_used=false{}",
        report.evaluation.efficacy_id,
        efficacy_summary_line(&report.evaluation),
        if duplicate {
            " · already recorded"
        } else {
            ""
        },
    )
}

/// Rewrite an absolute project path into a `~`-relative form when it lives under
/// the user's home directory, so session rows stay short without becoming
/// ambiguous. Paths outside home (and non-UTF-8 homes) are returned unchanged.
fn shorten_project(path: &str) -> String {
    if let Some(home) = directories::BaseDirs::new().and_then(|dirs| {
        dirs.home_dir()
            .to_str()
            .map(std::string::ToString::to_string)
    }) {
        if let Some(rest) = path.strip_prefix(&home) {
            let rest = rest.trim_start_matches('/');
            return if rest.is_empty() {
                "~".to_owned()
            } else {
                format!("~/{rest}")
            };
        }
    }
    path.to_owned()
}

/// The basename (final path component) of a project path, for the compact search
/// row. Falls back to the whole string when there is no separator.
fn project_basename(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
        .to_owned()
}

/// Human-readable label for a retrieval match kind.
fn match_kind_label(kind: autophagy_store::RetrievalMatchKind) -> &'static str {
    use autophagy_store::RetrievalMatchKind::{ExactSignature, FullText, SignatureAndFullText};
    match kind {
        ExactSignature => "signature",
        FullText => "text",
        SignatureAndFullText => "signature+text",
    }
}

/// Clean a raw FTS snippet for a scannable text row while preserving the `[…]`
/// match highlights the store inserts.
///
/// Tool events index their input as a compact JSON object, so the raw snippet
/// reads `{"command":"mise exec -- [cargo] [test] …`. When a `command` field is
/// present its value is extracted (highlights intact); otherwise the JSON
/// framing characters are stripped so plain metadata text remains.
fn clean_snippet(snippet: &str) -> String {
    if let Some(command) = extract_command_value(snippet) {
        let collapsed = collapse_whitespace(&command);
        if !collapsed.is_empty() {
            return collapsed;
        }
    }
    let stripped: String = snippet
        .chars()
        .filter(|character| !matches!(character, '{' | '}' | '"'))
        .collect();
    collapse_whitespace(&stripped)
}

/// Collapse every run of whitespace — including embedded newlines and tabs —
/// into a single space and trim the ends, so a cleaned snippet always renders on
/// exactly one row. Codex-adapter events carry multiline snippets that would
/// otherwise break the aligned single-row search layout. Display-only: the raw
/// snippet is preserved under `--output json`.
fn collapse_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space && !result.is_empty() {
                result.push(' ');
            }
            pending_space = false;
            result.push(character);
        }
    }
    result
}

/// Extract the `command` field's string value from a (possibly truncated) FTS
/// snippet, keeping the `[…]` highlight markers. Returns `None` when no
/// `command` field is present. The snippet is not valid JSON — the highlight
/// brackets break parsing — so this scans structurally instead of deserializing.
fn extract_command_value(snippet: &str) -> Option<String> {
    let key = snippet.find("command")?;
    let after_key = &snippet[key + "command".len()..];
    let colon = after_key.find(':')?;
    let after_colon = &after_key[colon + 1..];
    let open_quote = after_colon.find('"')?;
    let value_region = &after_colon[open_quote + 1..];
    let mut result = String::new();
    let mut characters = value_region.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.peek() {
                Some('"') => {
                    result.push('"');
                    characters.next();
                }
                Some('\\') => {
                    result.push('\\');
                    characters.next();
                }
                Some('n' | 't' | 'r') => {
                    result.push(' ');
                    characters.next();
                }
                _ => result.push('\\'),
            },
            // The value's closing quote ends the command; a truncated snippet
            // simply runs to the end of the region.
            '"' => break,
            other => result.push(other),
        }
    }
    Some(result)
}

/// Truncate a command string at a word boundary near `limit` characters, marking
/// the elision with a single ellipsis. Never splits mid-token: it trims back to
/// the last whitespace within the budget, falling back to a hard cut only when a
/// single token already exceeds it.
fn truncate_words(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let budget: String = text.chars().take(limit).collect();
    let cut = budget
        .rfind(char::is_whitespace)
        .filter(|boundary| *boundary > 0)
        .unwrap_or(budget.len());
    format!("{}…", budget[..cut].trim_end())
}

/// Resolve a possibly-abbreviated stored identifier against the known ids.
///
/// An exact full-id match always wins. Otherwise, for a token that starts with
/// the expected type prefix and carries at least [`MIN_SHORT_PREFIX`] characters
/// beyond it, a unique prefix match resolves to the full id; an ambiguous prefix
/// is an error listing the candidates. Anything else (too short, no match) is
/// returned unchanged so the caller's exact lookup produces its standard
/// not-found error.
fn resolve_stored_id<I>(candidates: I, query: &str, type_prefix: &str) -> Result<String, CliError>
where
    I: IntoIterator<Item = String>,
{
    let ids: Vec<String> = candidates.into_iter().collect();
    if ids.iter().any(|id| id == query) {
        return Ok(query.to_owned());
    }
    let specific = query
        .strip_prefix(type_prefix)
        .is_some_and(|rest| rest.chars().count() >= MIN_SHORT_PREFIX);
    if !specific {
        return Ok(query.to_owned());
    }
    let mut matches: Vec<String> = ids.into_iter().filter(|id| id.starts_with(query)).collect();
    match matches.len() {
        0 => Ok(query.to_owned()),
        1 => Ok(matches.remove(0)),
        _ => {
            matches.sort();
            Err(CliError::AmbiguousId {
                query: query.to_owned(),
                matches: truncate_ids(&matches),
            })
        }
    }
}

/// Resolve a mutation id, accepting unique `mut_` short-id prefixes.
fn resolve_mutation_id(store: &EventStore, query: &str) -> Result<String, CliError> {
    let candidates = store
        .list_mutations()?
        .into_iter()
        .map(|record| record.mutation_id);
    resolve_stored_id(candidates, query, "mut_")
}

/// Resolve a session id, accepting unique `ses_` short-id prefixes.
fn resolve_session_id(store: &EventStore, query: &str) -> Result<String, CliError> {
    let candidates = store
        .list_sessions(u32::MAX)?
        .into_iter()
        .map(|session| session.session_id);
    resolve_stored_id(candidates, query, "ses_")
}

/// Render at most the first three IDs, then summarize the remainder, keeping a
/// long unreviewed list actionable without flooding the terminal.
fn truncate_ids(ids: &[String]) -> String {
    const SHOWN: usize = 3;
    if ids.len() <= SHOWN {
        ids.join(", ")
    } else {
        format!(
            "{}, and {} more",
            ids[..SHOWN].join(", "),
            ids.len() - SHOWN
        )
    }
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
        // Next-step hints guide the user through the mutation lifecycle. They go
        // to stderr and only in text mode, so JSON stdout stays a clean report.
        if output == OutputFormat::Text {
            if let Some(hint) = report.next_step_hint() {
                eprintln!("{hint}");
            }
        }
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
            let hits = store.retrieve(&retrieval)?;
            // A zero-event store means nothing was ever imported, which the text
            // renderer surfaces differently from a query that simply missed.
            let database_empty = store.stats()?.events == 0;
            Ok(CommandReport::Search(SearchReport {
                hits,
                database_empty,
            }))
        }
        Commands::Digest {
            project,
            thresholds,
            recompute,
        } => {
            let database = resolve_database_path(cli.database)?;
            let store = open_store(&database)?;
            let thresholds = config::resolve_thresholds(leaf, thresholds, config);
            let report =
                detection::detect_cached(&store, project.as_deref(), thresholds, recompute)?;
            Ok(CommandReport::Digest(digest_report(report, thresholds)?))
        }
        Commands::Patterns {
            project,
            thresholds,
            recompute,
        } => {
            let database = resolve_database_path(cli.database)?;
            let store = open_store(&database)?;
            let thresholds = config::resolve_thresholds(leaf, thresholds, config);
            let report =
                detection::detect_cached(&store, project.as_deref(), thresholds, recompute)?;
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
                thresholds,
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
                DeleteTarget::Session { session_id } => {
                    let session_id = resolve_session_id(&store, &session_id)?;
                    Ok(CommandReport::DeleteSession(
                        store.delete_session(&session_id)?,
                    ))
                }
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
            model_backend,
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
                    model_backend,
                },
            )?;
            Ok(CommandReport::Setup(report))
        }
        Commands::Status { with_findings } => Ok(CommandReport::Status(Box::new(status::run(
            cli.database,
            config,
            with_findings,
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
            let findings = detection::detect_cached(
                &store,
                project.as_deref(),
                config::resolve_thresholds(matches, thresholds, config),
                false,
            )?
            .findings;
            let generated = generate_candidates(&findings);
            let mut registrations = Vec::new();
            let mut warnings = Vec::new();
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
                    match store.register_mutation(&registration) {
                        Ok(outcome) => registrations.push(outcome),
                        Err(StoreError::MutationContentConflict { mutation_id }) => {
                            warnings.push(template_conflict_warning(&mutation_id));
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            let current_states = mutation_states_by_id(
                &store,
                generated.iter().filter_map(|outcome| match outcome {
                    GenerationOutcome::Candidate { package } => Some(package.mutation_id.as_str()),
                    GenerationOutcome::InsufficientEvidence { .. } => None,
                }),
            )?;
            Ok(CommandReport::MutationProposal(MutationProposalReport {
                dry_run,
                generated,
                registrations,
                warnings,
                current_states,
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
                    Some("claude-cli") => SynthesisProviderChoice::ClaudeCli,
                    Some("codex-cli") => SynthesisProviderChoice::CodexCli,
                    _ => provider,
                }
            };
            // Resolve the manifest under flag < config. An explicit manifest
            // always wins and is validated strictly. When none is given, the
            // built-in `deterministic` provider falls back to its reference
            // manifest; every other provider still requires one.
            let manifest = match manifest {
                Some(path) => Some(path),
                None => config.synthesis_manifest_path.as_ref().map(PathBuf::from),
            };
            let (manifest_path, model_manifest) = match manifest {
                Some(path) => {
                    let manifest_path = path.display().to_string();
                    (manifest_path, ModelManifest::from_path(&path)?)
                }
                None if provider == SynthesisProviderChoice::Deterministic => {
                    let model_manifest = builtin_deterministic_manifest();
                    (model_manifest.path.clone(), model_manifest)
                }
                None => {
                    return Err(CliError::MissingManifest {
                        provider: provider.as_str(),
                    });
                }
            };
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
                SynthesisProviderChoice::ClaudeCli => {
                    Box::new(AgentCliProvider::claude_from_manifest(&model_manifest))
                }
                SynthesisProviderChoice::CodexCli => {
                    Box::new(AgentCliProvider::codex_from_manifest(&model_manifest))
                }
            };
            // Consent gate. Agent-CLI providers always reach a vendor cloud
            // through the user's logged-in CLI, so they require the explicit
            // remote opt-in unconditionally. HTTP providers only require it when
            // their endpoint host is non-loopback. Either way, evidence never
            // leaves the machine without the flag, and the flag prints one clear
            // egress line.
            let mut warnings = Vec::new();
            if let Some(vendor) = provider.cloud_vendor() {
                if allow_remote_endpoint {
                    warnings.push(format!(
                        "the structured synthesis prompt (baseline text, constraints, and cited event IDs) will be sent to {vendor} via your logged-in `{}` CLI because --allow-remote-endpoint is set; usage is billed to your existing {vendor} plan",
                        model_manifest.path
                    ));
                } else {
                    return Err(CliError::AgentCliConsentRequired {
                        provider: provider.as_str(),
                        vendor,
                    });
                }
            } else if synthesis_provider.uses_network() {
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
            let findings = detection::detect_cached(
                &store,
                project.as_deref(),
                config::resolve_thresholds(matches, thresholds, config),
                false,
            )?
            .findings;
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
                    match store.register_mutation(&registration) {
                        Ok(outcome) => registrations.push(outcome),
                        Err(StoreError::MutationContentConflict { mutation_id }) => {
                            warnings.push(template_conflict_warning(&mutation_id));
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            let current_states = mutation_states_by_id(
                &store,
                synthesized.iter().filter_map(|outcome| match outcome {
                    SynthesisOutcome::Candidate { package, .. } => {
                        Some(package.mutation_id.as_str())
                    }
                    _ => None,
                }),
            )?;
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
                current_states,
            }))
        }
        MutationAction::List => Ok(CommandReport::MutationList(store.list_mutations()?)),
        MutationAction::Show { mutation_id } => {
            let mutation_id = resolve_mutation_id(&store, &mutation_id)?;
            Ok(CommandReport::MutationShow(
                store.get_mutation(&mutation_id)?,
            ))
        }
        MutationAction::Challenge {
            mutation_id,
            checks,
            note,
        } => {
            let requested_id = mutation_id.clone();
            let mutation_id = resolve_mutation_id(&store, &mutation_id)?;
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
                MutationTransitionReport {
                    outcome: store
                        .challenge_mutation(&mutation_id, &serde_json::to_value(assessment)?)?,
                    requested_id,
                },
            ))
        }
        MutationAction::Reject {
            mutation_id,
            reason,
        } => {
            let requested_id = mutation_id.clone();
            let mutation_id = resolve_mutation_id(&store, &mutation_id)?;
            Ok(CommandReport::MutationTransition(
                MutationTransitionReport {
                    outcome: store.reject_mutation(&mutation_id, &reason)?,
                    requested_id,
                },
            ))
        }
        MutationAction::Replay { mutation_id, suite } => {
            let requested_id = mutation_id.clone();
            let mutation_id = resolve_mutation_id(&store, &mutation_id)?;
            let suite_path = suite.to_string_lossy().into_owned();
            let suite: ReplaySuite = serde_json::from_slice(&fs::read(&suite)?)?;
            let details = store.get_mutation(&mutation_id)?;
            let package = serde_json::from_value(details.mutation.package)?;
            let evaluation = match evaluate(&package, &suite) {
                Ok(evaluation) => evaluation,
                Err(ReplayEvaluationError::UnreviewedScenarios { .. }) => {
                    let ids = suite
                        .scenarios
                        .iter()
                        .filter(|scenario| {
                            scenario.counterfactual_outcome == Some(CounterfactualOutcome::Unknown)
                        })
                        .map(|scenario| scenario.scenario_id.clone())
                        .collect::<Vec<_>>();
                    return Err(CliError::UnreviewedReplayScenarios {
                        count: ids.len(),
                        ids: truncate_ids(&ids),
                        suite_path,
                    });
                }
                Err(other) => return Err(other.into()),
            };
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
                requested_id,
            }))
        }
        MutationAction::ReplayDraft {
            mutation_id,
            suite,
            context_events,
            force,
        } => {
            let requested_id = mutation_id.clone();
            let mutation_id = resolve_mutation_id(&store, &mutation_id)?;
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
                    requested_id,
                },
            ))
        }
        MutationAction::ShadowDraft {
            mutation_id,
            suite,
            context_events,
            force,
        } => {
            let requested_id = mutation_id.clone();
            let mutation_id = resolve_mutation_id(&store, &mutation_id)?;
            let details = store.get_mutation(&mutation_id)?;
            let package = serde_json::from_value(details.mutation.package)?;
            let events = store.list_events_for_detection(None)?;
            let draft = extract_observation_draft(&package, &events, usize::from(context_events))?;
            let intervention_observations = draft
                .observations
                .iter()
                .filter(|observation| observation.intervention_would_help)
                .count();
            let no_op_observations = draft.observations.len() - intervention_observations;
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
            Ok(CommandReport::MutationShadowDraft(
                MutationShadowDraftReport {
                    path: suite.to_string_lossy().into_owned(),
                    context_events,
                    observations: draft.observations.len(),
                    intervention_observations,
                    no_op_observations,
                    draft,
                    requested_id,
                },
            ))
        }
        MutationAction::Shadow { mutation_id, suite } => {
            let requested_id = mutation_id.clone();
            let mutation_id = resolve_mutation_id(&store, &mutation_id)?;
            let suite: ShadowSuite = serde_json::from_slice(&fs::read(&suite)?)?;
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
                requested_id,
            }))
        }
        MutationAction::Efficacy { mutation_id, now } => {
            let mutation_id = resolve_mutation_id(&store, &mutation_id)?;
            let installation = store.get_installation(&mutation_id)?;
            if installation.state != "installed" {
                return Err(CliError::MutationNotInstalled(mutation_id));
            }
            let installed_at = OffsetDateTime::parse(&installation.installed_at, &Rfc3339)?;
            let now = match now {
                Some(raw) => OffsetDateTime::parse(&raw, &Rfc3339)
                    .map_err(|_| CliError::InvalidNowTimestamp(raw))?,
                None => OffsetDateTime::now_utc(),
            };
            let details = store.get_mutation(&mutation_id)?;
            let package = &details.mutation.package;
            let selectors = package
                .get("triggers")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|trigger| trigger.get("selector").and_then(serde_json::Value::as_str))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if selectors.is_empty() {
                return Err(CliError::EfficacyNoSelectors(mutation_id));
            }
            let mutation_version = package
                .get("version")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("0.0.0")
                .to_owned();
            let bounds = WindowBounds::derive(installed_at, now)?;
            let gathered = store.efficacy_occurrences(&selectors, bounds.pre_start, bounds.now)?;
            let mut occurrences = Vec::with_capacity(gathered.occurrences.len());
            for occurrence in gathered.occurrences {
                let occurred_at = OffsetDateTime::parse(&occurrence.occurred_at, &Rfc3339)?;
                occurrences.push(FailureOccurrence {
                    event_id: occurrence.event_id,
                    session_id: occurrence.session_id,
                    occurred_at,
                });
            }
            let mut source_event_ids = occurrences
                .iter()
                .map(|occurrence| occurrence.event_id.clone())
                .collect::<Vec<_>>();
            source_event_ids.sort();
            let observations = EfficacyObservations {
                mutation_id: mutation_id.clone(),
                mutation_version,
                signature_selectors: selectors,
                matching_rule: MatchingRule::FailureSignatureRecurrence,
                occurrences,
                coverage: CoverageInput {
                    classifiable_failures: gathered.classifiable_failures,
                    total_failures: gathered.total_failures,
                },
            };
            let evaluation = evaluate_efficacy(&observations, installed_at, now)?;
            let report = serde_json::to_value(&evaluation)?;
            let verdict = report
                .get("verdict")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let registration = store.register_efficacy(&EfficacyRegistration {
                efficacy_id: evaluation.efficacy_id.clone(),
                mutation_id: evaluation.mutation_id.clone(),
                verdict,
                report,
                source_event_ids,
            })?;
            Ok(CommandReport::MutationEfficacy(MutationEfficacyReport {
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
            let mutation_id = resolve_mutation_id(&store, &mutation_id)?;
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
            let mutation_id = resolve_mutation_id(&store, &mutation_id)?;
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
                if sessions.is_empty() {
                    writeln!(writer, "no sessions imported yet — run `autophagy setup`")?;
                } else {
                    write_sessions(&mut writer, sessions)?;
                }
            }
            CommandReport::Search(report) => {
                if report.hits.is_empty() {
                    if report.database_empty {
                        writeln!(
                            writer,
                            "no events imported yet — run `autophagy setup` to import your sessions"
                        )?;
                    } else {
                        writeln!(writer, "no retrieval matches")?;
                    }
                }
                write_search_hits(&mut writer, &report.hits)?;
            }
            CommandReport::Digest(report) => write_detection(
                &mut writer,
                "local deterministic digest",
                report.events_scanned,
                report.sessions_scanned,
                report.candidate_signatures,
                &report.findings,
                &report.observations,
                report.thresholds,
            )?,
            CommandReport::Patterns(report) => write_detection(
                &mut writer,
                "deterministic evidence packets",
                report.events_scanned,
                report.sessions_scanned,
                report.candidate_signatures,
                &report.findings,
                &report.observations,
                report.thresholds,
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
                write_mutation_lesson(&mut writer, details)?;
                // The lesson block above answers "what is this?"; a blank line
                // then separates it from the append-only audit trail below.
                if !details.transitions.is_empty()
                    || !details.replays.is_empty()
                    || !details.shadows.is_empty()
                    || !details.installations.is_empty()
                {
                    writeln!(writer)?;
                }
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
                // The latest efficacy report, if any: a one-line post-install
                // recurrence summary in the same audit column.
                if let Some(latest) = details.efficacies.last() {
                    let summary = serde_json::from_value::<EfficacyReport>(latest.report.clone())
                        .map_or_else(
                            |_| latest.verdict.clone(),
                            |report| efficacy_summary_line(&report),
                        );
                    writeln!(
                        writer,
                        "{}\tefficacy {}\t{summary}",
                        compact_time(&latest.created_at, OffsetDateTime::now_utc()),
                        latest.efficacy_id,
                    )?;
                }
            }
            CommandReport::MutationTransition(report) => writeln!(
                writer,
                "{}\t{} -> {}\tchanged={}",
                report.outcome.mutation_id,
                report.outcome.from_state,
                report.outcome.to_state,
                report.outcome.changed
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
            CommandReport::MutationShadowDraft(report) => writeln!(
                writer,
                "{}\t{} observations · {} intervention · {} no-op\t{}",
                report.draft.mutation_id,
                report.observations,
                report.intervention_observations,
                report.no_op_observations,
                report.path
            )?,
            CommandReport::MutationShadow(report) => writeln!(
                writer,
                "{}\tpassed={}\tprecision={} · recall={} · {} false positives\tmutation_applied=false",
                report.evaluation.shadow_id,
                report.evaluation.passed,
                percent(u32::from(report.evaluation.summary.precision_bps)),
                percent(u32::from(report.evaluation.summary.recall_bps)),
                report.evaluation.summary.false_positives
            )?,
            CommandReport::MutationEfficacy(report) => {
                write_mutation_efficacy(&mut writer, report)?;
            }
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

#[allow(clippy::too_many_arguments)]
fn write_detection(
    writer: &mut impl Write,
    header: &str,
    events_scanned: usize,
    sessions_scanned: usize,
    candidate_signatures: usize,
    findings: &[EvidencePacket],
    observations: &[Observation],
    thresholds: DetectorConfig,
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
        write_observations(writer, observations, candidate_signatures, thresholds)?;
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
            "{}\t{}\t{}\t{} evidence\t{} counterexamples",
            finding.finding_id,
            finding.title,
            percent(u32::from(finding.score.score_bps)),
            finding.evidence.len(),
            finding.counterexamples.len()
        )?;
    }
    Ok(())
}

/// Minimum occurrences a candidate must recur before it is worth surfacing as a
/// near-threshold observation. Below this a signature is effectively a one-off,
/// and listing several such giants only buried the real signal.
const NEAR_MISS_MIN_OCCURRENCES: u32 = 2;

/// Most near-threshold observations to list, keeping the digest scannable.
const NEAR_MISS_LIMIT: usize = 5;

/// Character budget for a near-miss command title before word-boundary
/// truncation, chosen to fit a terminal line alongside its statistics.
const NEAR_MISS_TITLE_BUDGET: usize = 72;

fn write_observations(
    writer: &mut impl Write,
    observations: &[Observation],
    candidate_signatures: usize,
    thresholds: DetectorConfig,
) -> io::Result<()> {
    // Only genuinely recurring candidates are worth a reviewer's attention. A
    // one-occurrence signature is noise, however large its command; showing five
    // of them buried the digest, so they are filtered out entirely.
    let recurring: Vec<&Observation> = observations
        .iter()
        .filter(|observation| observation.score.occurrences >= NEAR_MISS_MIN_OCCURRENCES)
        .take(NEAR_MISS_LIMIT)
        .collect();
    if recurring.is_empty() {
        writeln!(
            writer,
            "{candidate_signatures} candidate signatures, none recurring — nothing near threshold"
        )?;
        return Ok(());
    }
    writeln!(
        writer,
        "near-threshold observations (recurring candidates, not findings):"
    )?;
    for observation in recurring {
        writeln!(
            writer,
            "{}\t{} occ · {} sessions · {} counterexamples · {} support\t{}",
            truncate_words(&observation.title, NEAR_MISS_TITLE_BUDGET),
            observation.score.occurrences,
            observation.score.distinct_sessions,
            observation.score.counterexamples,
            percent(u32::from(observation.score.support_ratio_bps)),
            unmet_gate_reason(observation, thresholds),
        )?;
    }
    Ok(())
}

/// Plain-language explanation of the gate a near-threshold candidate missed,
/// naming both the required and observed value (`needs 3+ occurrences, saw 2`)
/// in place of the internal gate identifier.
fn unmet_gate_reason(observation: &Observation, thresholds: DetectorConfig) -> String {
    let score = &observation.score;
    match observation.unmet_gate {
        UnmetGate::MinOccurrences => format!(
            "needs {}+ occurrences, saw {}",
            thresholds.min_occurrences, score.occurrences
        ),
        UnmetGate::MinSessions => format!(
            "needs {}+ sessions, saw {}",
            thresholds.min_sessions, score.distinct_sessions
        ),
        UnmetGate::MinSupportRatio => format!(
            "needs {} support, saw {}",
            percent(u32::from(thresholds.min_support_ratio_bps)),
            percent(u32::from(score.support_ratio_bps))
        ),
    }
}

/// Render the session list as aligned, scannable columns: abbreviated session
/// ids, compact times, event counts, and `~`-relative project paths. Full ids
/// and full timestamps remain available under `--output json`.
fn write_sessions(writer: &mut impl Write, sessions: &[SessionSummary]) -> io::Result<()> {
    let now = OffsetDateTime::now_utc();
    let ids: Vec<String> = sessions
        .iter()
        .map(|session| short_id(&session.session_id))
        .collect();
    let sources: Vec<&str> = sessions
        .iter()
        .map(|session| session.adapter.as_str())
        .collect();
    let events: Vec<String> = sessions
        .iter()
        .map(|session| session.event_count.to_string())
        .collect();
    let lasts: Vec<String> = sessions
        .iter()
        .map(|session| compact_time(&session.last_event_at, now))
        .collect();
    let projects: Vec<String> = sessions
        .iter()
        .map(|session| {
            session
                .project_path
                .as_deref()
                .map_or_else(|| "-".to_owned(), shorten_project)
        })
        .collect();

    let width = |header: &str, column: &[String]| {
        column
            .iter()
            .map(String::len)
            .chain(std::iter::once(header.len()))
            .max()
            .unwrap_or(0)
    };
    let id_w = width("SESSION", &ids);
    let src_w = sources
        .iter()
        .map(|source| source.len())
        .chain(std::iter::once("SOURCE".len()))
        .max()
        .unwrap_or(0);
    let evt_w = width("EVENTS", &events);
    let last_w = width("LAST EVENT", &lasts);

    writeln!(
        writer,
        "{:<id_w$}  {:<src_w$}  {:>evt_w$}  {:<last_w$}  PROJECT",
        "SESSION", "SOURCE", "EVENTS", "LAST EVENT"
    )?;
    for index in 0..sessions.len() {
        writeln!(
            writer,
            "{:<id_w$}  {:<src_w$}  {:>evt_w$}  {:<last_w$}  {}",
            ids[index], sources[index], events[index], lasts[index], projects[index]
        )?;
    }
    Ok(())
}

/// Render retrieval hits as scannable columns: abbreviated event id, compact
/// time, project basename, plain match kind, percentage score, and a cleaned
/// snippet with match highlights preserved. Full ids, raw snippets, and the
/// ranking explanation remain available under `--output json`.
fn write_search_hits(writer: &mut impl Write, hits: &[RetrievalHit]) -> io::Result<()> {
    let now = OffsetDateTime::now_utc();
    let ids: Vec<String> = hits.iter().map(|hit| short_id(&hit.event_id)).collect();
    let times: Vec<String> = hits
        .iter()
        .map(|hit| compact_time(&hit.occurred_at, now))
        .collect();
    let projects: Vec<String> = hits
        .iter()
        .map(|hit| {
            hit.project
                .as_deref()
                .map_or_else(|| "-".to_owned(), project_basename)
        })
        .collect();
    let kinds: Vec<&str> = hits
        .iter()
        .map(|hit| match_kind_label(hit.explanation.match_kind))
        .collect();
    let scores: Vec<String> = hits
        .iter()
        .map(|hit| percent(hit.explanation.rank_score_bps))
        .collect();
    let snippets: Vec<String> = hits
        .iter()
        .map(|hit| {
            hit.snippet
                .as_deref()
                .map(clean_snippet)
                .or_else(|| hit.signature.clone())
                .unwrap_or_else(|| "-".to_owned())
        })
        .collect();

    // Measure width in characters, matching how the formatter pads: `{:<w$}`
    // counts characters, not bytes. Byte length over-counts multibyte cells
    // (mixed adapters, unicode project names), padding the column wider than its
    // content; character counts keep each column exactly as wide as its widest
    // displayed cell.
    let column_width = |column: &[String]| {
        column
            .iter()
            .map(|value| value.chars().count())
            .max()
            .unwrap_or(0)
    };
    let id_w = column_width(&ids);
    let time_w = column_width(&times);
    let project_w = column_width(&projects);
    let kind_w = kinds
        .iter()
        .map(|kind| kind.chars().count())
        .max()
        .unwrap_or(0);
    let score_w = column_width(&scores);

    for index in 0..hits.len() {
        writeln!(
            writer,
            "{:<id_w$}  {:<time_w$}  {:<project_w$}  {:<kind_w$}  {:>score_w$}  {}",
            ids[index], times[index], projects[index], kinds[index], scores[index], snippets[index]
        )?;
    }
    Ok(())
}

/// Width of the label column in the `mutations show` lesson block.
const LESSON_LABEL: usize = 13;

/// Render the mutation package as a compact, labeled lesson block: the title,
/// state, falsifiable hypothesis, proposed intervention, trigger selectors,
/// evidence/counterexample counts, and promotion gates as percentages. This
/// answers the reviewer's core question — "what is this lesson?" — that the raw
/// header-plus-transitions rendering left to the JSON surface.
fn write_mutation_lesson(writer: &mut impl Write, details: &MutationDetails) -> io::Result<()> {
    let record = &details.mutation;
    let row = |label: &str, value: &str| format!("{label:<LESSON_LABEL$} {value}");

    let title = record.package["title"].as_str().unwrap_or("untitled");
    writeln!(writer, "{title}")?;
    writeln!(writer, "{}", row("id", &short_id(&record.mutation_id)))?;
    writeln!(writer, "{}", row("state", &record.state))?;

    match serde_json::from_value::<MutationPackage>(record.package.clone()) {
        Ok(package) => {
            writeln!(
                writer,
                "{}",
                row("hypothesis", &package.hypothesis.statement)
            )?;
            writeln!(
                writer,
                "{}",
                row("intervention", &package.intervention.instruction)
            )?;
            let selectors = package
                .triggers
                .iter()
                .map(|trigger| trigger.selector.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let selectors = if selectors.is_empty() {
                "none".to_owned()
            } else {
                selectors
            };
            writeln!(writer, "{}", row("triggers", &selectors))?;
            writeln!(
                writer,
                "{}",
                row(
                    "evidence",
                    &format!(
                        "{} events · {} counterexamples",
                        package.hypothesis.supporting_event_ids.len(),
                        package.hypothesis.counterexample_event_ids.len()
                    )
                )
            )?;
            writeln!(
                writer,
                "{}",
                row(
                    "gates",
                    &format!(
                        "{}+ replays · {} success · ≤{} false positives",
                        package.promotion.minimum_replays,
                        percent(u32::from(package.promotion.minimum_success_rate_bps)),
                        percent(u32::from(package.promotion.maximum_false_positive_rate_bps)),
                    )
                )
            )?;
        }
        Err(_) => {
            // The stored package should always deserialize; if a future contract
            // makes it momentarily unreadable, still show the header rather than
            // failing the whole command.
            writeln!(
                writer,
                "{}",
                row("note", "package detail unavailable; see --output json")
            )?;
        }
    }
    Ok(())
}

/// Look up a mutation's current display state, falling back to `candidate`
/// only when the ID has no stored state yet (never registered — a dry-run
/// preview of brand-new evidence, which will in fact become `candidate` the
/// moment it is registered).
fn display_state<'a>(
    current_states: &'a BTreeMap<String, String>,
    mutation_id: &'a str,
) -> &'a str {
    current_states
        .get(mutation_id)
        .map_or("candidate", String::as_str)
}

/// Warning for a candidate whose evidence re-derived an already-registered
/// mutation ID with different package content. Registered packages are
/// immutable, so the stored package stays authoritative; this happens when the
/// deterministic template evolved between the original registration and this
/// run, and it must not abort the rest of the batch.
fn template_conflict_warning(mutation_id: &str) -> String {
    format!(
        "mutation '{mutation_id}' is already registered with earlier-template content; \
         the stored immutable package is kept"
    )
}

fn write_mutation_proposal(
    writer: &mut impl Write,
    report: &MutationProposalReport,
) -> io::Result<()> {
    for warning in &report.warnings {
        writeln!(writer, "warning: {warning}")?;
    }
    if report.generated.is_empty() {
        writeln!(writer, "no mutation candidates above evidence threshold")?;
    }
    for outcome in &report.generated {
        match outcome {
            GenerationOutcome::Candidate { package } => writeln!(
                writer,
                "{}\t{}\t{} evidence\tzero permissions\t{}",
                package.mutation_id,
                package.title,
                package.hypothesis.supporting_event_ids.len(),
                display_state(&report.current_states, &package.mutation_id),
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

/// Fetch each mutation's CURRENT registry lifecycle state, keyed by mutation
/// ID, for every ID a `propose`/`synthesize` pass generated a candidate for.
///
/// Generation is deterministic: the same finding always re-derives the same
/// mutation ID, so re-running `propose`/`synthesize` over evidence that was
/// registered in an earlier pass reproduces an identical `GenerationOutcome::
/// Candidate`/`SynthesisOutcome::Candidate` even after that mutation has been
/// challenged, replayed, shadow-evaluated, promoted, or retired. Looking the
/// state up fresh from the store — rather than assuming every generated row
/// is still `candidate` — is what keeps the displayed state honest. IDs never
/// registered (a dry-run preview of brand-new evidence) are simply absent.
///
/// # Errors
/// Returns [`CliError`] when the store cannot be queried.
fn mutation_states_by_id<'a>(
    store: &EventStore,
    mutation_ids: impl Iterator<Item = &'a str>,
) -> Result<BTreeMap<String, String>, CliError> {
    let mut states = BTreeMap::new();
    for mutation_id in mutation_ids {
        if let Some(state) = store.mutation_state(mutation_id)? {
            states.insert(mutation_id.to_owned(), state);
        }
    }
    Ok(states)
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
                "{}\t{}\t{} evidence\tzero permissions\t{}",
                package.mutation_id,
                package.title,
                package.hypothesis.supporting_event_ids.len(),
                display_state(&report.current_states, &package.mutation_id),
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

#[cfg(test)]
mod tests {
    use super::{
        MIN_SHORT_PREFIX, clean_snippet, collapse_whitespace, percent, project_basename,
        resolve_stored_id, short_id, shorten_project, truncate_words,
    };

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn short_id_keeps_prefix_and_eight_digest_chars() {
        assert_eq!(
            short_id("evt_claude_fc1bc1d7aaaaaaaaaaaaaaaa"),
            "evt_claude_fc1bc1d7…"
        );
        assert_eq!(
            short_id("ses_claude_d19bfbd8bbbbbbbbbbbbbbbb"),
            "ses_claude_d19bfbd8…"
        );
        assert_eq!(short_id(&format!("mut_{HASH}")), "mut_01234567…");
    }

    #[test]
    fn short_id_leaves_already_short_ids_untouched() {
        // Suffix at or below the keep width, or no separator at all.
        assert_eq!(short_id("evt_cli_failure"), "evt_cli_failure");
        assert_eq!(short_id("ses_demo"), "ses_demo");
        assert_eq!(short_id("bare"), "bare");
    }

    #[test]
    fn percent_renders_basis_points_to_one_decimal() {
        assert_eq!(percent(7411), "74.1%");
        assert_eq!(percent(5000), "50.0%");
        assert_eq!(percent(10_000), "100.0%");
        assert_eq!(percent(0), "0.0%");
    }

    #[test]
    fn clean_snippet_extracts_command_and_preserves_highlights() {
        let raw = r#"{"command":"mise exec -- [cargo] [test] -p autophagy-store"}"#;
        assert_eq!(
            clean_snippet(raw),
            "mise exec -- [cargo] [test] -p autophagy-store"
        );
    }

    #[test]
    fn clean_snippet_handles_truncated_command_payload() {
        // A snippet cut off mid-value (no closing quote) still yields the text.
        let raw = r#"{"command":"mise exec -- [cargo] test -p autophagy"#;
        assert_eq!(clean_snippet(raw), "mise exec -- [cargo] test -p autophagy");
    }

    #[test]
    fn clean_snippet_strips_json_framing_without_a_command_field() {
        let raw = r#"{"path":"/workspace/[demo]"}"#;
        // No command field: JSON framing is stripped, highlights preserved.
        assert_eq!(clean_snippet(raw), "path:/workspace/[demo]");
    }

    #[test]
    fn clean_snippet_leaves_plain_text_alone() {
        assert_eq!(clean_snippet("cargo [test] failed"), "cargo [test] failed");
    }

    #[test]
    fn clean_snippet_collapses_multiline_content_to_one_row() {
        // Codex-adapter snippets embed literal newlines and tabs; they must
        // collapse to single spaces so the result stays on one row.
        let raw = "first line\n\tsecond   line\r\nthird";
        let cleaned = clean_snippet(raw);
        assert_eq!(cleaned, "first line second line third");
        assert!(!cleaned.contains(['\n', '\r', '\t']));
    }

    #[test]
    fn clean_snippet_collapses_newlines_inside_command_value() {
        let raw = "{\"command\":\"echo one\necho [two]\"}";
        assert_eq!(clean_snippet(raw), "echo one echo [two]");
    }

    #[test]
    fn collapse_whitespace_trims_and_squeezes_runs() {
        assert_eq!(collapse_whitespace("  a \n\n b\t\tc  "), "a b c");
        assert_eq!(collapse_whitespace("\n\t "), "");
        assert_eq!(collapse_whitespace("solo"), "solo");
    }

    #[test]
    fn column_width_counts_characters_not_bytes() {
        // `format!` pads a `&str` by character count, so the width budget the
        // renderer computes must too. A byte-length budget over-counts a
        // multibyte cell and pads the column wider than its displayed content;
        // a character-count budget equal to the value's own width adds nothing.
        let value = "café—münchen".to_owned();
        let char_width = value.chars().count();
        assert!(char_width < value.len(), "sample must be multibyte");
        assert_eq!(format!("{value:<char_width$}|"), format!("{value}|"));
        // The byte-length budget would inject spurious trailing spaces.
        let byte_width = value.len();
        assert!(format!("{value:<byte_width$}|").ends_with(" |"));
    }

    #[test]
    fn truncate_words_cuts_at_a_word_boundary() {
        let text = "mise exec -- cargo test --workspace --all-features some-really-long-token";
        let truncated = truncate_words(text, 30);
        assert!(
            truncated.ends_with('…'),
            "truncation marks elision: {truncated}"
        );
        assert!(
            !truncated.contains("--all-features"),
            "must not keep a token past the budget: {truncated}"
        );
        // The kept portion is a clean whole-token prefix, never mid-word.
        let kept = truncated.trim_end_matches('…');
        assert!(text.starts_with(kept), "kept portion is a clean prefix");
        assert!(!kept.ends_with('-'), "no dangling partial token: {kept}");
    }

    #[test]
    fn truncate_words_leaves_short_text_unchanged() {
        assert_eq!(truncate_words("cargo test", 40), "cargo test");
    }

    #[test]
    fn shorten_project_and_basename() {
        assert_eq!(project_basename("/workspace/demo/repo"), "repo");
        assert_eq!(project_basename("repo"), "repo");
        // A non-home path is returned verbatim (unambiguous).
        assert_eq!(shorten_project("/opt/things/x"), "/opt/things/x");
    }

    #[test]
    fn resolve_stored_id_matches_a_unique_prefix() {
        let ids = vec![format!("mut_{HASH}"), "mut_ff00ff00deadbeef".to_owned()];
        let resolved =
            resolve_stored_id(ids.clone(), "mut_012345", "mut_").expect("unique prefix resolves");
        assert_eq!(resolved, format!("mut_{HASH}"));
    }

    #[test]
    fn resolve_stored_id_passes_through_an_exact_full_id() {
        let full = format!("mut_{HASH}");
        let ids = vec![full.clone(), "mut_ff00ff00deadbeef".to_owned()];
        assert_eq!(
            resolve_stored_id(ids, &full, "mut_").expect("exact id resolves"),
            full
        );
    }

    #[test]
    fn resolve_stored_id_reports_an_ambiguous_prefix() {
        let ids = vec!["mut_abcdef001111".to_owned(), "mut_abcdef002222".to_owned()];
        let error =
            resolve_stored_id(ids, "mut_abcdef", "mut_").expect_err("an ambiguous prefix errors");
        let rendered = error.to_string();
        assert!(
            rendered.contains("ambiguous"),
            "message names ambiguity: {rendered}"
        );
        assert!(
            rendered.contains("mut_abcdef001111") && rendered.contains("mut_abcdef002222"),
            "message lists the candidates: {rendered}"
        );
    }

    #[test]
    fn resolve_stored_id_passes_missing_and_too_short_prefixes_through() {
        let ids = vec![format!("mut_{HASH}")];
        // No match: returned unchanged so the caller's exact lookup 404s.
        assert_eq!(
            resolve_stored_id(ids.clone(), "mut_ffffff", "mut_").expect("no match passes through"),
            "mut_ffffff"
        );
        // Below the minimum prefix length: not treated as a resolvable prefix.
        let short = format!("mut_{}", &HASH[..MIN_SHORT_PREFIX - 1]);
        assert_eq!(
            resolve_stored_id(ids, &short, "mut_").expect("too-short passes through"),
            short
        );
    }
}
