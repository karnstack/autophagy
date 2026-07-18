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
    /// A sequence position was already occupied by a different event.
    #[error(
        "session '{session_id}' sequence {sequence} already belongs to event '{existing_event_id}'"
    )]
    SessionSequenceConflict {
        /// Conflicting session identifier.
        session_id: String,
        /// Conflicting sequence position.
        sequence: i64,
        /// Canonical event already stored at this position.
        existing_event_id: String,
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
    /// A mutation evidence position cannot fit `SQLite`'s integer representation.
    #[error("mutation evidence ordinal {ordinal} exceeds SQLite's integer range")]
    MutationEvidenceOrdinalOutOfRange {
        /// Rejected zero-based evidence position.
        ordinal: usize,
    },
    /// A replay evidence position cannot fit `SQLite`'s integer representation.
    #[error("replay evidence ordinal {ordinal} exceeds SQLite's integer range")]
    ReplayEvidenceOrdinalOutOfRange {
        /// Rejected zero-based evidence position.
        ordinal: usize,
    },
    /// A shadow evidence position cannot fit `SQLite`'s integer representation.
    #[error("shadow evidence ordinal {ordinal} exceeds SQLite's integer range")]
    ShadowEvidenceOrdinalOutOfRange {
        /// Rejected zero-based evidence position.
        ordinal: usize,
    },
    /// An efficacy evidence position cannot fit `SQLite`'s integer representation.
    #[error("efficacy evidence ordinal {ordinal} exceeds SQLite's integer range")]
    EfficacyEvidenceOrdinalOutOfRange {
        /// Rejected zero-based evidence position.
        ordinal: usize,
    },
    /// An incremental cursor cannot fit `SQLite`'s signed integer representation.
    #[error("cursor {field} value {value} exceeds SQLite's integer range")]
    CursorOutOfRange {
        /// Rejected cursor field.
        field: &'static str,
        /// Rejected unsigned value.
        value: u64,
    },
    /// An incremental cursor origin was blank.
    #[error("cursor origin must not be empty or whitespace")]
    InvalidCursorOrigin,
    /// Persisted cursor state violated a database invariant.
    #[error("persisted cursor contains an invalid {field} value")]
    CorruptCursor {
        /// Invalid cursor field.
        field: &'static str,
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
    /// A retrieval supplied neither a full-text query nor an exact signature.
    #[error("retrieval requires a text query, an exact signature, or both")]
    EmptyRetrievalQuery,
    /// A mutation ID was not present in the registry.
    #[error("mutation '{mutation_id}' was not found")]
    MutationNotFound {
        /// Missing mutation identity.
        mutation_id: String,
    },
    /// Immutable mutation content changed under the same ID.
    #[error("mutation '{mutation_id}' already exists with different package content")]
    MutationContentConflict {
        /// Conflicting mutation identity.
        mutation_id: String,
    },
    /// A lifecycle transition was not allowed from the current state.
    #[error("mutation '{mutation_id}' cannot transition from '{from_state}' to '{to_state}'")]
    MutationStateTransition {
        /// Mutation identity.
        mutation_id: String,
        /// Current registry state.
        from_state: String,
        /// Requested state.
        to_state: &'static str,
    },
    /// A required lifecycle reason was blank.
    #[error("mutation lifecycle reason must not be blank")]
    InvalidMutationReason,
    /// Replay identity, mutation identity, hash, or pass status disagreed with its report.
    #[error("replay registration does not match its versioned report")]
    InvalidReplayRegistration,
    /// Immutable replay content changed under the same ID.
    #[error("replay '{replay_id}' already exists with different report content")]
    ReplayContentConflict {
        /// Conflicting replay identity.
        replay_id: String,
    },
    /// Shadow identity, mutation identity, hash, pass status, or evidence disagreed.
    #[error("shadow registration does not match its versioned report")]
    InvalidShadowRegistration,
    /// Immutable shadow content changed under the same ID.
    #[error("shadow '{shadow_id}' already exists with different report content")]
    ShadowContentConflict {
        /// Conflicting shadow identity.
        shadow_id: String,
    },
    /// Efficacy identity, mutation identity, hash, or verdict disagreed with its report.
    #[error("efficacy registration does not match its versioned report")]
    InvalidEfficacyRegistration,
    /// Immutable efficacy content changed under the same ID.
    #[error("efficacy '{efficacy_id}' already exists with different report content")]
    EfficacyContentConflict {
        /// Conflicting efficacy identity.
        efficacy_id: String,
    },
    /// An attestation carried an unsupported evaluation kind.
    #[error("attestation kind '{kind}' is not one of 'replay' or 'shadow'")]
    InvalidAttestationKind {
        /// Rejected attestation kind.
        kind: String,
    },
    /// Installation registration violated the supported target contract.
    #[error("installation registration is invalid")]
    InvalidInstallationRegistration,
    /// No installation audit exists for the mutation.
    #[error("mutation '{mutation_id}' has no installation record")]
    InstallationNotFound {
        /// Mutation identity.
        mutation_id: String,
    },
    /// Installation is not currently installed.
    #[error("installation '{installation_id}' is in state '{state}', not 'installed'")]
    InstallationState {
        /// Installation identity.
        installation_id: String,
        /// Current installation state.
        state: String,
    },
    /// Evidence deletion would orphan a materialized active skill.
    #[error("installation '{installation_id}' must be uninstalled before deleting its evidence")]
    ActiveInstallationBlocksEvidenceDeletion {
        /// Active installation requiring rollback first.
        installation_id: String,
    },
}
