//! Command-line entry point for importing and querying local agent activity.

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
use autophagy_core::{ImportOptions, ImportSummary, import_jsonl};
use autophagy_events::Event;
use autophagy_install::{
    CodexSkillPlan, InstallError, InstalledArtifact, materialize, plan_codex_skill, unmaterialize,
};
use autophagy_mutations::{GenerationOutcome, equivalence_key, generate_candidates};
use autophagy_patterns::{DetectorConfig, EvidencePacket, detect};
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
    DeterministicReferenceProvider, ManifestError, ModelManifest, SynthesisOutcome,
    SynthesisProvider, synthesize_candidates,
};
use clap::{Parser, Subcommand, ValueEnum};
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
}

#[allow(clippy::struct_field_names)]
#[derive(Clone, Copy, Debug, clap::Args)]
struct ThresholdArgs {
    /// Minimum supporting events.
    #[arg(long, default_value_t = 3, value_name = "COUNT")]
    min_occurrences: u32,

    /// Minimum distinct supporting sessions.
    #[arg(long, default_value_t = 2, value_name = "COUNT")]
    min_sessions: u32,

    /// Minimum support share in basis points (0-10000).
    #[arg(long, default_value_t = 5_000, value_parser = clap::value_parser!(u16).range(0..=10_000), value_name = "BPS")]
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
        /// Synthesis provider to consult.
        #[arg(long, value_enum, default_value_t = SynthesisProviderChoice::Deterministic)]
        provider: SynthesisProviderChoice,

        /// Local model manifest (synthesis-manifest/0.1) JSON file.
        #[arg(long, value_name = "PATH")]
        manifest: PathBuf,

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
    /// Install one shadow-passed mutation as a repo-scoped Codex skill.
    Install {
        /// Stable mutation identity.
        mutation_id: String,

        /// Existing target repository root.
        #[arg(long, value_name = "PATH")]
        repository: PathBuf,

        /// Required phrase acknowledging the scoped filesystem write: `repo-skill-write`.
        #[arg(long, value_name = "PHRASE")]
        confirm_permissions: String,

        /// Preview the exact path and content hash without writing or activating.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove an audited Codex skill and retire its mutation.
    Uninstall {
        /// Stable mutation identity.
        mutation_id: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum SynthesisProviderChoice {
    /// Built-in pure, model-free, offline reference provider.
    Deterministic,
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
    Patterns(Vec<EvidencePacket>),
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
}

#[derive(Debug, Serialize)]
struct DigestReport {
    spec_version: &'static str,
    generated_at: String,
    events_scanned: usize,
    model_used: bool,
    network_used: bool,
    findings: Vec<EvidencePacket>,
}

#[derive(Debug, Serialize)]
struct MutationProposalReport {
    dry_run: bool,
    generated: Vec<GenerationOutcome>,
    registrations: Vec<MutationRegisterOutcome>,
}

#[derive(Debug, Serialize)]
struct MutationSynthesisReport {
    dry_run: bool,
    provider: String,
    model: String,
    model_used: bool,
    network_used: bool,
    manifest_path: String,
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
}

impl ImportReport {
    const fn has_issues(&self) -> bool {
        match self {
            Self::Generic(summary) => summary.has_issues(),
            Self::ClaudeCode(summary) => summary.has_issues(),
            Self::Codex(summary) => summary.has_issues(),
        }
    }
}

impl CommandReport {
    const fn has_issues(&self) -> bool {
        match self {
            Self::Import(summary) => summary.has_issues(),
            Self::MutationReplay(report) => !report.evaluation.passed,
            Self::MutationShadow(report) => !report.evaluation.passed,
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
            | Self::DeleteAll(_) => false,
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
    Manifest(#[from] ManifestError),
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let output = cli.output;
    match execute(cli).and_then(|report| {
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

#[allow(clippy::too_many_lines)]
fn execute(cli: Cli) -> Result<CommandReport, CliError> {
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
        } => match adapter {
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
        },
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
            let findings = detect(&events, thresholds.into());
            Ok(CommandReport::Digest(DigestReport {
                spec_version: "digest/0.1",
                generated_at: OffsetDateTime::now_utc().format(&Rfc3339)?,
                events_scanned: events.len(),
                model_used: false,
                network_used: false,
                findings,
            }))
        }
        Commands::Patterns {
            project,
            thresholds,
        } => {
            let database = resolve_database_path(cli.database)?;
            let store = open_store(&database)?;
            let events = store.list_events_for_detection(project.as_deref())?;
            Ok(CommandReport::Patterns(detect(&events, thresholds.into())))
        }
        Commands::Mutations { action } => execute_mutation_action(cli.database, action),
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
    }
}

#[allow(clippy::too_many_lines)]
fn execute_mutation_action(
    database: Option<PathBuf>,
    action: MutationAction,
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
            let findings = detect(&events, thresholds.into());
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
            project,
            thresholds,
            dry_run,
        } => {
            let manifest_path = manifest.display().to_string();
            let model_manifest = ModelManifest::from_path(&manifest)?;
            let synthesis_provider: Box<dyn SynthesisProvider> = match provider {
                SynthesisProviderChoice::Deterministic => Box::new(DeterministicReferenceProvider),
            };
            let events = store.list_events_for_detection(project.as_deref())?;
            let findings = detect(&events, thresholds.into());
            let synthesized =
                synthesize_candidates(&findings, &model_manifest, synthesis_provider.as_ref());
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
                manifest_path,
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
            let plan = plan_codex_skill(&package, &repository)?;
            if dry_run {
                return Ok(CommandReport::MutationInstall(install_report(
                    &plan, true, false, None,
                )));
            }
            let artifact = materialize(&plan)?;
            let registration = InstallationRegistration {
                installation_id: plan.installation_id.clone(),
                mutation_id: plan.mutation_id.clone(),
                target: "codex_repo_skill".to_owned(),
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
            let plan = plan_codex_skill(&package, Path::new(&audit.repository_root))?;
            if plan.installation_id != audit.installation_id
                || portable_relative_path(&plan.relative_path) != audit.relative_path
                || plan.content_hash != audit.content_hash
                || audit.target != "codex_repo_skill"
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
    plan: &CodexSkillPlan,
    dry_run: bool,
    materialized: bool,
    transition: Option<InstallationTransitionOutcome>,
) -> MutationInstallReport {
    MutationInstallReport {
        installation_id: plan.installation_id.clone(),
        target: "codex_repo_skill",
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
            CommandReport::Digest(report) => {
                writeln!(
                    writer,
                    "{} events · {} findings · local deterministic digest",
                    report.events_scanned,
                    report.findings.len()
                )?;
                write_findings(&mut writer, &report.findings)?;
            }
            CommandReport::Patterns(findings) => write_findings(&mut writer, findings)?,
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
                "{}\t{}\t{}\t{}",
                report.installation_id,
                report.relative_path,
                if report.dry_run {
                    "dry-run"
                } else {
                    "installed"
                },
                report.content_hash
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
        },
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

fn write_mutation_synthesis(
    writer: &mut impl Write,
    report: &MutationSynthesisReport,
) -> io::Result<()> {
    writeln!(
        writer,
        "provider={} · model={} · model_used={} · network_used={}",
        report.provider, report.model, report.model_used, report.network_used
    )?;
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
