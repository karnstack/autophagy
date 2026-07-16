use serde::Serialize;
use serde_json::Value;

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
