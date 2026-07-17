//! Rebuild the derived search projections from already-stored events.
//!
//! Databases imported before signature indexing existed — or imported without
//! `--index-tool-input` — keep every canonical event (the `events` table still
//! holds each event's redaction-approved `event_json` and tool input) but have
//! an empty exact-signature index and no searchable tool-input text. Reimport
//! cannot heal this: an identical event is an idempotent no-op. `reindex`
//! rebuilds the derived projection tables in place from the stored events,
//! applying the current redaction policy and the same projection gates as
//! import, without touching any canonical row, cursor, or evidence.

use autophagy_events::Event;
use autophagy_redaction::{PrivacyError, PrivacyPolicy};
use autophagy_store::{EventStore, StoreError};
use serde::Serialize;

use crate::generic_jsonl::search_projection_from;

/// Redaction-approved gates for a projection rebuild.
///
/// These mirror `import`'s projection gates exactly. `index_tool_input` makes
/// redacted tool input searchable and rebuilds the exact normalized-operation
/// signature index; `index_metadata` promotes already-redacted metadata keys to
/// searchable text. `exclude_paths` mirrors import's path policy so the rebuilt
/// index never contains text the current policy would now exclude.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReindexOptions {
    /// Index redacted tool input and rebuild the exact-signature index.
    pub index_tool_input: bool,
    /// Already-redacted metadata keys whose values become searchable text.
    pub index_metadata: Vec<String>,
    /// Path globs excluded from the rebuilt projection under current policy.
    pub exclude_paths: Vec<String>,
}

/// Result of a projection rebuild.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ReindexSummary {
    /// Canonical events scanned and reprojected.
    pub events_scanned: u64,
    /// Free-text search rows written (one per scanned event).
    pub search_rows_written: u64,
    /// Exact normalized-signature index rows written.
    pub signatures_written: u64,
    /// String fields changed while re-applying current secret redaction.
    pub redacted_fields: u64,
}

/// Failure modes for [`reindex`].
#[derive(Debug, thiserror::Error)]
pub enum ReindexError {
    /// A transactional storage operation failed or a stored event no longer
    /// satisfies the AEP contract.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A path-exclusion glob was invalid.
    #[error(transparent)]
    Privacy(#[from] PrivacyError),
    /// An option value was malformed.
    #[error("reindex options invalid: {0}")]
    InvalidOptions(String),
}

/// Rebuild the free-text and exact-signature search projections from every
/// stored event, applying the current redaction policy and the supplied gates.
///
/// Transactional and idempotent: it deletes and rewrites only the derived
/// projection tables (the FTS mirror and `event_signatures`), never the
/// canonical `events`, cursors, or evidence, and running it twice yields
/// identical state. Because the projection derives purely from the stored
/// event, the current redaction policy is re-applied to each event before
/// projection, so newly recognized secrets never enter the rebuilt index.
///
/// # Errors
///
/// Returns [`ReindexError`] for invalid options or path globs, or when a
/// storage operation fails.
pub fn reindex(
    store: &mut EventStore,
    options: &ReindexOptions,
) -> Result<ReindexSummary, ReindexError> {
    if options
        .index_metadata
        .iter()
        .any(|key| key.trim().is_empty())
    {
        return Err(ReindexError::InvalidOptions(
            "metadata keys must not be empty".to_owned(),
        ));
    }
    let policy = PrivacyPolicy::new(&options.exclude_paths)?;

    let mut redacted_fields: u64 = 0;
    let rebuilt = store.rebuild_search_projection(|event: &Event| {
        let outcome = policy.apply(event);
        redacted_fields += outcome.redacted_fields;
        // A path the current policy excludes yields no row at all, so nothing
        // about the excluded event stays searchable. The canonical event is
        // left untouched by the store.
        outcome.event.as_ref().map(|sanitized| {
            search_projection_from(sanitized, options.index_tool_input, &options.index_metadata)
        })
    })?;

    Ok(ReindexSummary {
        events_scanned: rebuilt.events_scanned,
        search_rows_written: rebuilt.search_rows_written,
        signatures_written: rebuilt.signatures_written,
        redacted_fields,
    })
}
