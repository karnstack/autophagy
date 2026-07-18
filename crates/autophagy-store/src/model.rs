use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;

/// Stable provenance for one adapter installation or history directory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SourceIdentity {
    /// Adapter identifier; must match the AEP event's `source` value.
    pub adapter: String,
    /// Stable identity for this adapter installation or source directory.
    pub instance_key: String,
    /// Optional user-facing label.
    pub display_name: Option<String>,
}

/// Durable position and adapter state for an append-only source file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SourceCursor {
    /// Bytes safely consumed from the beginning of the source.
    pub byte_offset: u64,
    /// Complete physical lines safely consumed.
    pub line_number: u64,
    /// SHA-256 of the source's first bounded block for rotation detection.
    pub head_hash: [u8; 32],
    /// Adapter-defined JSON required to resume normalization correctly.
    pub state: serde_json::Value,
}

impl SourceIdentity {
    /// Create a source identity without a display label.
    #[must_use]
    pub fn new(adapter: impl Into<String>, instance_key: impl Into<String>) -> Self {
        Self {
            adapter: adapter.into(),
            instance_key: instance_key.into(),
            display_name: None,
        }
    }

    /// Attach a user-facing label.
    #[must_use]
    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = Some(display_name.into());
        self
    }
}

/// Redaction-approved free text that may enter the FTS5 index.
///
/// Project paths and tool names come from the already policy-processed AEP
/// envelope. Tool input and event payload text require this explicit projection.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SearchProjection {
    /// Sanitized tool input. Raw event input is not indexed automatically.
    pub tool_input_text: Option<String>,
    /// Sanitized free text extracted by an adapter or digester.
    pub searchable_text: Option<String>,
    /// Redaction-approved normalized operation signature for exact lookup.
    ///
    /// Populated by the caller (never derived from raw JSON inside the store)
    /// only when the source's tool input is already redaction-approved for
    /// indexing. When absent, the event is simply not recalled by exact
    /// signature.
    pub signature: Option<String>,
}

/// Result of attempting to persist one event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InsertOutcome {
    /// A new immutable event row was created.
    Inserted {
        /// Internal `SQLite` row identifier.
        row_id: i64,
    },
    /// The same event ID and content hash already existed; nothing changed.
    Duplicate {
        /// Existing internal `SQLite` row identifier.
        row_id: i64,
    },
    /// The ID existed with different content, which was retained separately.
    ConflictQuarantined {
        /// Stable quarantine record identifier.
        conflict_id: i64,
        /// Number of times this exact conflicting content has been observed.
        observation_count: i64,
    },
}

/// Compact session record returned by storage queries.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionSummary {
    /// AEP session identifier.
    pub session_id: String,
    /// Adapter identifier.
    pub adapter: String,
    /// Stable source-instance identifier.
    pub instance_key: String,
    /// Project path after path-policy processing.
    pub project_path: Option<String>,
    /// Earliest explicit session-start event.
    pub started_at: Option<String>,
    /// Latest explicit session-end event.
    pub ended_at: Option<String>,
    /// Earliest observed event timestamp.
    pub first_event_at: String,
    /// Latest observed event timestamp.
    pub last_event_at: String,
    /// Number of immutable events in the session.
    pub event_count: i64,
}

/// One full-text search result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchHit {
    /// Exact AEP evidence identifier.
    pub event_id: String,
    /// FTS5 BM25 rank; lower values are better matches.
    pub rank: f64,
    /// Context with matching terms surrounded by brackets.
    pub snippet: String,
}

/// Outcome polarity for the retrieval outcome filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalOutcome {
    /// Successful tool or test events (`tool.completed`, `test.passed`).
    Success,
    /// Failed tool or test events (`tool.failed`, `test.failed`).
    Failure,
}

impl RetrievalOutcome {
    /// Event types selected by this outcome, in stable order.
    #[must_use]
    pub const fn event_types(self) -> [&'static str; 2] {
        match self {
            Self::Success => ["tool.completed", "test.passed"],
            Self::Failure => ["tool.failed", "test.failed"],
        }
    }

    /// Stable serialized name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

/// A deterministic exact-and-hybrid retrieval request.
///
/// At least one of [`text`](Self::text) or [`signature`](Self::signature) must
/// be present. Every other field is an inclusive filter that narrows both the
/// exact-signature and full-text match sources identically.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RetrievalQuery {
    /// Optional FTS5 query over the redaction-approved projection.
    pub text: Option<String>,
    /// Optional exact normalized operation signature.
    pub signature: Option<String>,
    /// Restrict to one exact project path (repository filter).
    pub project: Option<String>,
    /// Restrict to events at or after this instant (recency filter).
    pub since: Option<OffsetDateTime>,
    /// Restrict to these exact AEP event types (event-kind filter).
    pub event_kinds: Vec<String>,
    /// Restrict to a success or failure outcome polarity.
    pub outcome: Option<RetrievalOutcome>,
    /// Maximum number of ranked results to return.
    pub limit: u32,
}

/// Which match sources produced a retrieval hit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalMatchKind {
    /// The event's stored signature equals the query signature.
    ExactSignature,
    /// The event matched the full-text query only.
    FullText,
    /// The event matched both the exact signature and the full-text query.
    SignatureAndFullText,
}

impl RetrievalMatchKind {
    /// Stable serialized name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExactSignature => "exact_signature",
            Self::FullText => "full_text",
            Self::SignatureAndFullText => "signature_and_full_text",
        }
    }
}

/// A single named contribution to a hit's ranking.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RankingSignalKind {
    /// The exact-signature tier contribution.
    ExactSignature,
    /// The full-text relevance tier contribution.
    FullText,
    /// The recency ordering signal (a within-tier tie-break only).
    Recency,
}

impl RankingSignalKind {
    /// Stable serialized name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExactSignature => "exact_signature",
            Self::FullText => "full_text",
            Self::Recency => "recency",
        }
    }
}

/// One inspectable contribution to a hit's deterministic rank.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RankingSignal {
    /// Which signal contributed.
    pub kind: RankingSignalKind,
    /// Integer score contribution in basis points.
    ///
    /// Recency is a within-tier ordering tie-break with a deliberate zero
    /// score weight, so it can never move a full-text hit above an
    /// exact-signature hit.
    pub contribution_bps: u32,
    /// Human-readable justification, such as the matched signature or bm25.
    pub detail: String,
}

/// Which filter field narrowed a retrieval and to what value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalFilterField {
    /// Exact project-path (repository) filter.
    Project,
    /// Recency lower-bound filter.
    Since,
    /// Event-kind filter.
    EventKind,
    /// Outcome-polarity filter.
    Outcome,
}

impl RetrievalFilterField {
    /// Stable serialized name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Since => "since",
            Self::EventKind => "event_kind",
            Self::Outcome => "outcome",
        }
    }
}

/// One applied retrieval filter, echoed into every hit's explanation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RetrievalFilter {
    /// Which filter field was applied.
    pub field: RetrievalFilterField,
    /// The exact value the filter required.
    pub value: String,
}

/// Versioned, deterministic explanation of why a hit ranked where it did.
///
/// The normative structure lives at `docs/specs/retrieval/0.1/schema.json`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RankingExplanation {
    /// Ranking-explanation contract version (`retrieval/0.1`).
    pub spec_version: &'static str,
    /// Which match sources produced this hit.
    pub match_kind: RetrievalMatchKind,
    /// Sum of the scored signal contributions in basis points.
    pub rank_score_bps: u32,
    /// Ordered contributing signals.
    pub signals: Vec<RankingSignal>,
    /// Filters applied to this retrieval, in stable order.
    pub applied_filters: Vec<RetrievalFilter>,
    /// Stable statement of the total ordering and tie-break rule.
    pub tie_break: &'static str,
}

/// One ranked retrieval result with its exact evidence identity.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RetrievalHit {
    /// Exact AEP evidence identifier.
    pub event_id: String,
    /// Session containing the event.
    pub session_id: String,
    /// AEP event type.
    pub event_type: String,
    /// Canonical occurrence timestamp.
    pub occurred_at: String,
    /// Policy-processed project path, when available.
    pub project: Option<String>,
    /// Stored normalized signature, when the event carries one.
    pub signature: Option<String>,
    /// Full-text snippet with matched terms bracketed, when text matched.
    pub snippet: Option<String>,
    /// Deterministic, versioned ranking explanation.
    pub explanation: RankingExplanation,
}

/// Row counts useful for diagnostics and idempotency assertions.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct StoreStats {
    /// Number of source instances.
    pub sources: i64,
    /// Number of sessions.
    pub sessions: i64,
    /// Number of canonical events.
    pub events: i64,
    /// Number of distinct artifacts.
    pub artifacts: i64,
    /// Number of quarantined conflicting payloads.
    pub conflicts: i64,
}

/// Cheap content fingerprint of the events a detection pass would scan.
///
/// Computed from indexed columns only — never by deserializing event JSON — so
/// it is orders of magnitude cheaper than a detection pass. Any import, delete,
/// or prune changes at least one field, which is what lets the derived
/// findings cache key make an unchanged corpus a hit and a changed corpus a
/// miss without explicit invalidation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DetectionFingerprint {
    /// Events in scope (after the optional project filter).
    pub event_count: i64,
    /// Highest `row_id` in scope, or zero when empty.
    pub max_row_id: i64,
    /// Distinct sessions in scope, for human-facing progress only.
    pub session_count: i64,
    /// Latest `imported_at` in scope — a monotonic import watermark that
    /// advances on every insert, distinguishing a delete-then-reimport from the
    /// original corpus even when the count and max row id happen to coincide.
    pub import_watermark: String,
}

/// Per-adapter import activity, for status reporting.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct AdapterActivity {
    /// Stable adapter identifier.
    pub adapter: String,
    /// Distinct sessions imported for this adapter.
    pub sessions: i64,
    /// Canonical events attributed to this adapter's sessions.
    pub events: i64,
    /// Most recent event timestamp across this adapter's sessions.
    pub last_event_at: Option<String>,
    /// Most recent incremental-import cursor update for this adapter.
    pub last_import_at: Option<String>,
}

/// Effect of rebuilding the derived search projections from stored events.
///
/// Returned by [`EventStore::rebuild_search_projection`](crate::EventStore::rebuild_search_projection).
/// Counts describe only the derived free-text and exact-signature index rows
/// that were rewritten; the canonical `events` rows are never altered.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RebuildSummary {
    /// Canonical events scanned and reprojected.
    pub events_scanned: u64,
    /// Free-text search rows written (exactly one per scanned event, so that
    /// project path and tool name stay searchable regardless of index flags).
    pub search_rows_written: u64,
    /// Exact normalized-signature index rows written.
    pub signatures_written: u64,
}

/// Effect of deleting one session.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DeleteSummary {
    /// Whether a session row existed and was removed.
    pub session_deleted: bool,
    /// Number of canonical events removed by the cascade.
    pub events_deleted: i64,
    /// Number of artifacts that became unreferenced and were removed.
    pub artifacts_deleted: i64,
    /// Mutation candidates removed because cited evidence was deleted.
    pub mutations_deleted: i64,
}

/// Effect of deleting all locally persisted Autophagy data.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DeleteAllSummary {
    /// Removed source identities.
    pub sources_deleted: i64,
    /// Removed sessions.
    pub sessions_deleted: i64,
    /// Removed canonical events.
    pub events_deleted: i64,
    /// Removed artifacts.
    pub artifacts_deleted: i64,
    /// Removed conflict records.
    pub conflicts_deleted: i64,
    /// Removed incremental source cursors.
    pub cursors_deleted: i64,
    /// Removed mutation candidates.
    pub mutations_deleted: i64,
}

/// Effect or dry-run preview of a retention prune.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PruneSummary {
    /// Sessions older than the cutoff.
    pub sessions_deleted: i64,
    /// Events belonging to selected sessions.
    pub events_deleted: i64,
    /// Artifacts left unreferenced by selected sessions.
    pub artifacts_deleted: i64,
    /// Mutation candidates removed because cited evidence was pruned.
    pub mutations_deleted: i64,
    /// Whether the transaction was intentionally rolled back.
    pub dry_run: bool,
}

/// Owned input for immutable candidate registration.
#[derive(Clone, Debug, PartialEq)]
pub struct MutationRegistration {
    /// Stable mutation identity.
    pub mutation_id: String,
    /// Source Evidence Packet finding.
    pub source_finding_id: String,
    /// Detector family.
    pub source_detector: String,
    /// Stable semantic equivalence hash.
    pub equivalence_key: String,
    /// Package wire version.
    pub spec_version: String,
    /// Package semantic version.
    pub semantic_version: String,
    /// Complete immutable package JSON.
    pub package: Value,
    /// Supporting event IDs in package order.
    pub supporting_event_ids: Vec<String>,
    /// Counterexample event IDs in package order.
    pub counterexample_event_ids: Vec<String>,
}

/// Result of registering one generated package.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MutationRegisterOutcome {
    /// A new immutable candidate was stored.
    Inserted {
        /// Stored mutation identity.
        mutation_id: String,
    },
    /// The same ID and package content already existed.
    Duplicate {
        /// Existing identical mutation identity.
        mutation_id: String,
    },
    /// An equivalent trigger/intervention already exists under another ID.
    EquivalentExisting {
        /// Proposed mutation identity.
        mutation_id: String,
        /// Existing equivalent mutation identity.
        existing_mutation_id: String,
    },
}

/// One immutable package plus mutable audited registry state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MutationRecord {
    /// Stable mutation identity.
    pub mutation_id: String,
    /// Source finding identity.
    pub source_finding_id: String,
    /// Source detector family.
    pub source_detector: String,
    /// Semantic duplicate-detection key.
    pub equivalence_key: String,
    /// Mutation package wire version.
    pub spec_version: String,
    /// Package semantic version.
    pub semantic_version: String,
    /// Current registry lifecycle state.
    pub state: String,
    /// Immutable candidate package.
    pub package: Value,
    /// Completed challenge checklist, when challenged.
    pub challenge: Option<Value>,
    /// User-supplied rejection reason, when rejected.
    pub rejection_reason: Option<String>,
    /// Canonical creation timestamp.
    pub created_at: String,
    /// Canonical last-transition timestamp.
    pub updated_at: String,
}

/// One append-only lifecycle audit entry.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MutationTransition {
    /// Monotonic audit row identity.
    pub transition_id: i64,
    /// Mutation whose state changed.
    pub mutation_id: String,
    /// Previous state; absent for initial generation.
    pub from_state: Option<String>,
    /// New lifecycle state.
    pub to_state: String,
    /// Human-readable transition reason.
    pub reason: String,
    /// Structured checklist or transition context.
    pub metadata: Value,
    /// Canonical transition timestamp.
    pub occurred_at: String,
}

/// Candidate and its complete transition history.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MutationDetails {
    /// Current registry record.
    pub mutation: MutationRecord,
    /// Complete append-only audit history.
    pub transitions: Vec<MutationTransition>,
    /// Immutable deterministic replay reports in creation order.
    pub replays: Vec<MutationReplayRecord>,
    /// Immutable observation-only shadow reports.
    pub shadows: Vec<MutationShadowRecord>,
    /// Installation and rollback audit records.
    pub installations: Vec<MutationInstallationRecord>,
    /// Post-install efficacy reports in creation order (oldest first).
    pub efficacies: Vec<MutationEfficacyRecord>,
}

/// Idempotent lifecycle transition result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MutationTransitionOutcome {
    /// Mutation identity.
    pub mutation_id: String,
    /// State before the request.
    pub from_state: String,
    /// State after the request.
    pub to_state: String,
    /// Whether a new transition was committed.
    pub changed: bool,
}

/// Owned deterministic replay report ready for persistence.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayRegistration {
    /// Stable content-derived replay identity.
    pub replay_id: String,
    /// Evaluated mutation identity.
    pub mutation_id: String,
    /// Stable scenario-suite hash.
    pub scenario_set_hash: String,
    /// Complete versioned replay report.
    pub report: Value,
    /// Whether every coverage and threshold gate passed.
    pub passed: bool,
    /// Exact source events cited across all independent scenarios.
    pub source_event_ids: Vec<String>,
}

/// One persisted immutable replay report.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MutationReplayRecord {
    /// Stable replay identity.
    pub replay_id: String,
    /// Evaluated mutation identity.
    pub mutation_id: String,
    /// Stable scenario-suite hash.
    pub scenario_set_hash: String,
    /// Complete versioned replay report.
    pub report: Value,
    /// Whether every coverage and threshold gate passed.
    pub passed: bool,
    /// Canonical persistence timestamp.
    pub created_at: String,
}

/// Idempotent replay persistence and lifecycle result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReplayRegisterOutcome {
    /// A new replay report was stored.
    Inserted {
        /// Stable replay identity.
        replay_id: String,
        /// Whether this replay advanced the lifecycle.
        advanced: bool,
        /// Registry state after persistence.
        mutation_state: String,
    },
    /// The identical replay report was already stored.
    Duplicate {
        /// Stable replay identity.
        replay_id: String,
        /// Current mutation registry state.
        mutation_state: String,
    },
}

/// Owned observation-only shadow report ready for persistence.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowRegistration {
    /// Stable content-derived shadow identity.
    pub shadow_id: String,
    /// Observed mutation identity.
    pub mutation_id: String,
    /// Stable observation-suite hash.
    pub observation_set_hash: String,
    /// Complete versioned shadow report.
    pub report: Value,
    /// Whether every observation and precision gate passed.
    pub passed: bool,
    /// Exact source events cited across independent observations.
    pub source_event_ids: Vec<String>,
}

/// One persisted immutable shadow report.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MutationShadowRecord {
    /// Stable shadow report identity.
    pub shadow_id: String,
    /// Observed mutation identity.
    pub mutation_id: String,
    /// Stable observation-suite hash.
    pub observation_set_hash: String,
    /// Complete versioned shadow report.
    pub report: Value,
    /// Whether every gate passed.
    pub passed: bool,
    /// Canonical persistence timestamp.
    pub created_at: String,
}

/// Idempotent shadow persistence and lifecycle result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ShadowRegisterOutcome {
    /// A new shadow report was stored.
    Inserted {
        /// Stable shadow report identity.
        shadow_id: String,
        /// Whether this report advanced the lifecycle.
        advanced: bool,
        /// Registry state after persistence.
        mutation_state: String,
    },
    /// The identical report was already stored.
    Duplicate {
        /// Stable shadow report identity.
        shadow_id: String,
        /// Current mutation registry state.
        mutation_state: String,
    },
}

/// Owned post-install efficacy report ready for persistence.
///
/// Registration is append-only and, unlike replay and shadow, performs no
/// lifecycle transition: efficacy observes recurrence, it never gates promotion.
#[derive(Clone, Debug, PartialEq)]
pub struct EfficacyRegistration {
    /// Stable content-derived efficacy identity.
    pub efficacy_id: String,
    /// Measured mutation identity.
    pub mutation_id: String,
    /// Deterministic verdict (`improved`, `regressed`, `unchanged`, or
    /// `insufficient_data`).
    pub verdict: String,
    /// Complete versioned efficacy report.
    pub report: Value,
    /// Exact failure-occurrence events cited across both windows, deduplicated.
    pub source_event_ids: Vec<String>,
}

/// One persisted immutable efficacy report.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MutationEfficacyRecord {
    /// Stable efficacy report identity.
    pub efficacy_id: String,
    /// Measured mutation identity.
    pub mutation_id: String,
    /// Deterministic verdict.
    pub verdict: String,
    /// Complete versioned efficacy report.
    pub report: Value,
    /// Canonical persistence timestamp.
    pub created_at: String,
}

/// Idempotent efficacy persistence result. No lifecycle field: efficacy never
/// changes mutation state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EfficacyRegisterOutcome {
    /// A new efficacy report was stored.
    Inserted {
        /// Stable efficacy report identity.
        efficacy_id: String,
    },
    /// The identical report was already stored.
    Duplicate {
        /// Stable efficacy report identity.
        efficacy_id: String,
    },
}

/// One matched failure occurrence gathered for efficacy evaluation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EfficacyOccurrence {
    /// Exact AEP event identity.
    pub event_id: String,
    /// Session the failure occurred in.
    pub session_id: String,
    /// Canonical event timestamp (RFC 3339 UTC).
    pub occurred_at: String,
}

/// Failure occurrences and index-coverage counts for one evaluated span,
/// gathered from the event store for the pure efficacy evaluator to consume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EfficacyOccurrences {
    /// Matched failure occurrences within the span, deduplicated by event.
    pub occurrences: Vec<EfficacyOccurrence>,
    /// In-span `tool.failed` events carrying an exact-signature index row.
    pub classifiable_failures: u32,
    /// All in-span `tool.failed` events.
    pub total_failures: u32,
    /// Selectors that could not be parsed into the failure-matching rule.
    pub unparsed_selectors: Vec<String>,
}

/// Efficacy coverage of the installed mutations, for the `status` summary.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct EfficacyStatusSummary {
    /// Currently-installed mutations.
    pub installed: u32,
    /// Installed mutations whose latest verdict is `improved`.
    pub improved: u32,
    /// Installed mutations whose latest verdict is `regressed`.
    pub regressed: u32,
    /// Installed mutations whose latest verdict is `unchanged`.
    pub unchanged: u32,
    /// Installed mutations whose latest verdict is `insufficient_data`.
    pub insufficient_data: u32,
    /// Installed mutations with no efficacy report yet.
    pub not_measured: u32,
}

/// Audited input for a completed filesystem materialization.
#[derive(Clone, Debug, PartialEq)]
pub struct InstallationRegistration {
    /// Stable installation identity.
    pub installation_id: String,
    /// Installed mutation identity.
    pub mutation_id: String,
    /// Canonical target identifier.
    pub target: String,
    /// Canonical repository root.
    pub repository_root: String,
    /// Installed path relative to the repository root.
    pub relative_path: String,
    /// SHA-256 of installed bytes.
    pub content_hash: String,
    /// Explicit permission review retained in the audit.
    pub permission_review: Value,
}

/// One installation and optional rollback audit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MutationInstallationRecord {
    /// Stable installation identity.
    pub installation_id: String,
    /// Installed mutation identity.
    pub mutation_id: String,
    /// Materializer target.
    pub target: String,
    /// Canonical repository root.
    pub repository_root: String,
    /// Installed relative path.
    pub relative_path: String,
    /// SHA-256 of installed bytes.
    pub content_hash: String,
    /// Explicit permission review.
    pub permission_review: Value,
    /// Current installation state.
    pub state: String,
    /// Canonical install timestamp.
    pub installed_at: String,
    /// Canonical uninstall timestamp, when rolled back.
    pub uninstalled_at: Option<String>,
}

/// Installation lifecycle result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InstallationTransitionOutcome {
    /// Stable installation identity.
    pub installation_id: String,
    /// Mutation registry state after the operation.
    pub mutation_state: String,
    /// Installation state after the operation.
    pub installation_state: String,
}
