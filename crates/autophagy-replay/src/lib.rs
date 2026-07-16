//! Versioned, deterministic replay scenarios and evaluation.
//!
//! Replay v0.1 never executes a mutation or invokes a model. It matches the
//! candidate's exact trigger selectors against annotated historical decision
//! points and measures useful interventions, correct no-ops, contradictions,
//! and false interventions.

mod evaluate;
mod extract;
mod model;
mod validate;

pub use evaluate::{ReplayEvaluationError, evaluate};
pub use extract::{ReplayDraftError, extract_review_draft};
pub use model::{
    CounterfactualOutcome, DecisionPoint, ExpectedAction, ReplayDisposition, ReplayPolicy,
    ReplayReport, ReplayResultSpecVersion, ReplayScenarioResult, ReplayScenarioSpecVersion,
    ReplaySuite, ReplaySuiteSpecVersion, ReplaySummary, ThresholdFailure,
};
pub use validate::{ReplayValidationError, ReplayValidationErrors};
