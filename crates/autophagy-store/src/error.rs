use autophagy_events::{EventParseError, ValidationErrors};

/// Error produced by the local event store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// `SQLite` rejected an operation.
    #[error("SQLite operation failed: {0}")]
    Database(#[from] rusqlite::Error),
    /// An event or metadata value could not be serialized.
    #[error("JSON serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    /// An event timestamp could not be formatted canonically.
    #[error("timestamp formatting failed: {0}")]
    TimeFormat(#[from] time::error::Format),
    /// The event violated the AEP contract.
    #[error("event validation failed: {0}")]
    InvalidEvent(#[from] ValidationErrors),
    /// An event read from `SQLite` no longer satisfies the AEP contract.
    #[error("stored event is invalid: {0}")]
    StoredEventInvalid(#[from] EventParseError),
    /// A required source identity component was blank.
    #[error("source {field} must not be empty or whitespace")]
    InvalidSource {
        /// Invalid source field.
        field: &'static str,
    },
    /// The AEP source and storage provenance adapter disagreed.
    #[error("event source '{event_source}' does not match adapter '{adapter}'")]
    SourceMismatch {
        /// Value in the AEP event.
        event_source: String,
        /// Value in storage provenance.
        adapter: String,
    },
    /// An existing session was observed under a different source instance.
    #[error("session '{session_id}' already belongs to a different source instance")]
    SessionSourceConflict {
        /// Conflicting session identifier.
        session_id: String,
    },
    /// An unsigned sequence cannot fit `SQLite`'s signed integer representation.
    #[error("event sequence {sequence} exceeds SQLite's integer range")]
    SequenceOutOfRange {
        /// Rejected sequence.
        sequence: u64,
    },
    /// An artifact position cannot fit `SQLite`'s signed integer representation.
    #[error("artifact ordinal {ordinal} exceeds SQLite's integer range")]
    ArtifactOrdinalOutOfRange {
        /// Rejected zero-based artifact position.
        ordinal: usize,
    },
    /// An applied migration's SQL no longer matches the compiled migration.
    #[error("migration {version} checksum does not match the compiled migration")]
    MigrationDrift {
        /// Migration version whose content changed.
        version: i64,
    },
    /// The database was created by a newer store version.
    #[error("database migration {version} is newer than this binary supports")]
    DatabaseTooNew {
        /// Unsupported migration version.
        version: i64,
    },
    /// An earlier migration record is absent while a later one exists.
    #[error("database is missing migration {version}")]
    MissingMigration {
        /// Missing migration version.
        version: i64,
    },
    /// Full-text search queries cannot be blank.
    #[error("search query must not be empty or whitespace")]
    EmptySearchQuery,
}
