//! Shared, adapter-neutral conformance checks for incremental imports.

/// Adapter metrics required by the common idempotency contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImportMetrics {
    /// Files selected by metadata-only discovery.
    pub discovered_files: u64,
    /// Source records read during this invocation.
    pub records_seen: u64,
    /// Normalized AEP events produced before filtering.
    pub events_emitted: u64,
    /// Newly stored canonical events.
    pub inserted: u64,
    /// Same-ID, same-content events encountered.
    pub duplicates: u64,
    /// Same-ID, different-content events quarantined.
    pub conflicts: u64,
    /// Invalid source records.
    pub rejected: u64,
}

/// Verify the shared native-adapter import and reimport contract.
///
/// # Errors
/// Returns the first violated discovery, integrity, or idempotency invariant.
pub fn verify_incremental_idempotency(
    first: ImportMetrics,
    second: ImportMetrics,
    stored_after_first: i64,
    stored_after_second: i64,
) -> Result<(), ConformanceError> {
    if first.discovered_files == 0 {
        return Err(ConformanceError::NoFilesDiscovered);
    }
    if first.records_seen == 0 || first.events_emitted == 0 || first.inserted == 0 {
        return Err(ConformanceError::EmptyInitialImport);
    }
    if first.rejected > 0 || first.conflicts > 0 || second.rejected > 0 || second.conflicts > 0 {
        return Err(ConformanceError::IntegrityIssue);
    }
    if second.records_seen != 0 || second.inserted != 0 || second.duplicates != 0 {
        return Err(ConformanceError::ReimportReadSource);
    }
    if stored_after_first != stored_after_second {
        return Err(ConformanceError::StoreChanged {
            before: stored_after_first,
            after: stored_after_second,
        });
    }
    Ok(())
}

/// Native-adapter conformance failure.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConformanceError {
    /// Discovery selected no transcripts.
    #[error("adapter discovery selected no files")]
    NoFilesDiscovered,
    /// Initial fixture import produced no useful evidence.
    #[error("initial import must read records, emit events, and insert evidence")]
    EmptyInitialImport,
    /// A fixture import rejected or quarantined evidence.
    #[error("fixture import produced rejected records or conflicts")]
    IntegrityIssue,
    /// A cursor-backed reimport read source records again.
    #[error("unchanged incremental reimport must not read or insert source records")]
    ReimportReadSource,
    /// Persistent event count changed on reimport.
    #[error("event count changed across reimport from {before} to {after}")]
    StoreChanged {
        /// Count immediately after the first import.
        before: i64,
        /// Count immediately after the second import.
        after: i64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_cursor_backed_noop_reimport() {
        let first = ImportMetrics {
            discovered_files: 1,
            records_seen: 4,
            events_emitted: 5,
            inserted: 5,
            duplicates: 0,
            conflicts: 0,
            rejected: 0,
        };
        let second = ImportMetrics {
            discovered_files: 1,
            records_seen: 0,
            events_emitted: 0,
            inserted: 0,
            duplicates: 0,
            conflicts: 0,
            rejected: 0,
        };
        assert_eq!(verify_incremental_idempotency(first, second, 5, 5), Ok(()));
    }
}
