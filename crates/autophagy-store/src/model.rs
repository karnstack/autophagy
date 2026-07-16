use serde::Serialize;

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
}
