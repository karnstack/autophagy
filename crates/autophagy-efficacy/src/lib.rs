//! Versioned, deterministic post-install efficacy evaluation.
//!
//! Efficacy v0.1 answers one empirical, model-free question about an installed
//! mutation: does the exact failure signature it addresses recur *less* in the
//! observation window since `installed_at` than it did in an equal window
//! immediately before? It makes no causal claim, invokes no model, and never
//! changes a mutation's lifecycle state — it only measures recurrence.
//!
//! This crate is a pure function of the inputs handed to it. Gathering the
//! failure occurrences and index-coverage counts from the local event store is
//! the store's job; wiring the two together is the CLI's. Given the same
//! observations, `installed_at`, and evaluation clock, [`evaluate`] always
//! produces the same report and the same content-derived `efficacy_id`.

mod evaluate;
mod model;
mod validate;

pub use evaluate::{EfficacyError, WindowBounds, evaluate};
pub use model::{
    Coverage, CoverageInput, EfficacyObservations, EfficacyReport, EfficacyResultSpecVersion,
    EfficacyWindows, Evidence, FailureOccurrence, InsufficientReason, MatchingRule, Verdict,
    WindowStats,
};
pub use validate::{EfficacyValidationError, EfficacyValidationErrors};
