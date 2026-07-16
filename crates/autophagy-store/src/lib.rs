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
    DeleteAllSummary, DeleteSummary, InsertOutcome, InstallationRegistration,
    InstallationTransitionOutcome, MutationDetails, MutationInstallationRecord, MutationRecord,
    MutationRegisterOutcome, MutationRegistration, MutationReplayRecord, MutationShadowRecord,
    MutationTransition, MutationTransitionOutcome, PruneSummary, RankingExplanation, RankingSignal,
    RankingSignalKind, ReplayRegisterOutcome, ReplayRegistration, RetrievalFilter,
    RetrievalFilterField, RetrievalHit, RetrievalMatchKind, RetrievalOutcome, RetrievalQuery,
    SearchHit, SearchProjection, SessionSummary, ShadowRegisterOutcome, ShadowRegistration,
    SourceCursor, SourceIdentity, StoreStats,
};
pub use store::EventStore;
