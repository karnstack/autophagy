//! Transactional, local-only `SQLite` storage for normalized Autophagy events.
//!
//! The store validates every event before persistence, derives a stable content
//! hash for idempotency, quarantines conflicting identities, and indexes only
//! path-policy-processed structural fields plus caller-approved free text.

mod error;
mod migration;
mod model;
mod store;
mod util;

pub use error::StoreError;
pub use model::{
    DeleteAllSummary, DeleteSummary, InsertOutcome, MutationDetails, MutationRecord,
    MutationRegisterOutcome, MutationRegistration, MutationReplayRecord, MutationTransition,
    MutationTransitionOutcome, PruneSummary, ReplayRegisterOutcome, ReplayRegistration, SearchHit,
    SearchProjection, SessionSummary, SourceCursor, SourceIdentity, StoreStats,
};
pub use store::EventStore;
