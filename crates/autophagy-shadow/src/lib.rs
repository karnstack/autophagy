//! Versioned observation-only shadow evaluation.
//!
//! Shadow v0.1 exact-matches immutable mutation trigger selectors against
//! annotated live observations. It never applies the mutation or invokes a
//! model.

mod evaluate;
mod extract;
mod model;
mod validate;

pub use evaluate::{ShadowEvaluationError, evaluate};
pub use extract::{ShadowDraftError, extract_observation_draft};
pub use model::{
    ShadowDisposition, ShadowObservation, ShadowObservationSpecVersion, ShadowPolicy, ShadowReport,
    ShadowResult, ShadowResultSpecVersion, ShadowSuite, ShadowSuiteSpecVersion, ShadowSummary,
    ShadowThresholdFailure,
};
pub use validate::{ShadowValidationError, ShadowValidationErrors};
