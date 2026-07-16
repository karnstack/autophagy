use serde::{Deserialize, Serialize};

use crate::ReplayValidationErrors;

/// Replay suite wire-format version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReplaySuiteSpecVersion {
    /// Initial annotated, non-executable replay suite.
    #[serde(rename = "replay-suite/0.1")]
    V0_1,
}

/// Decision-point wire-format version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReplayScenarioSpecVersion {
    /// Initial annotated decision-point contract.
    #[serde(rename = "replay-scenario/0.1")]
    V0_1,
}

/// Replay report wire-format version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReplayResultSpecVersion {
    /// Initial deterministic replay report contract.
    #[serde(rename = "replay-result/0.1")]
    V0_1,
}

/// Ground-truth action expected at an annotated decision point.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedAction {
    /// The instruction should apply at this decision point.
    Intervene,
    /// The instruction should remain silent.
    NoOp,
}

/// Annotated result of applying an instruction at a positive replay point.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CounterfactualOutcome {
    /// The package's stated expected result was observed in the fixture.
    ExpectedResult,
    /// The fixture contradicts the package's expected result.
    Contradiction,
}

/// One annotated historical decision point.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionPoint {
    /// Decision-point contract version.
    pub spec_version: ReplayScenarioSpecVersion,
    /// Stable scenario identity.
    pub scenario_id: String,
    /// Exact AEP events supporting this annotation.
    pub source_event_ids: Vec<String>,
    /// Versioned trigger selectors observable before the historical outcome.
    pub observed_trigger_selectors: Vec<String>,
    /// Whether the candidate should intervene at this point.
    pub expected_action: ExpectedAction,
    /// Annotated result when intervention is expected; absent for no-op cases.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterfactual_outcome: Option<CounterfactualOutcome>,
    /// Optional human-readable fixture provenance or rationale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Ordered replay inputs for one immutable mutation package.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaySuite {
    /// Suite contract version.
    pub spec_version: ReplaySuiteSpecVersion,
    /// Candidate evaluated by every decision point.
    pub mutation_id: String,
    /// Annotated decision points.
    pub scenarios: Vec<DecisionPoint>,
}

impl ReplaySuite {
    /// Enforce replay-suite semantic invariants.
    ///
    /// # Errors
    /// Returns every invalid field and cross-field relationship.
    pub fn validate(&self) -> Result<(), ReplayValidationErrors> {
        crate::validate::validate(self)
    }
}

/// Deterministic classification for one replay point.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayDisposition {
    /// Triggered where expected and produced the stated result.
    Success,
    /// Correctly remained silent at a negative decision point.
    NoOp,
    /// Missed a positive point or contradicted the expected result.
    Contradiction,
    /// Triggered at a decision point annotated as a no-op.
    FalseIntervention,
}

/// Inspectable result for one scenario.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReplayScenarioResult {
    /// Stable scenario identity.
    pub scenario_id: String,
    /// Exact AEP events providing fixture provenance.
    pub source_event_ids: Vec<String>,
    /// Trigger selectors observed before the annotated outcome.
    pub observed_trigger_selectors: Vec<String>,
    /// Whether an exact package selector was observed.
    pub triggered: bool,
    /// Ground-truth fixture annotation.
    pub expected_action: ExpectedAction,
    /// Deterministic classification.
    pub disposition: ReplayDisposition,
}

/// Integer-only aggregate replay measurements.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReplaySummary {
    /// Total independent decision points evaluated.
    pub scenarios: u32,
    /// Useful expected interventions.
    pub successes: u32,
    /// Correctly silent negative cases.
    pub no_ops: u32,
    /// Missed or outcome-contradicting positive cases.
    pub contradictions: u32,
    /// Interventions on annotated negative cases.
    pub false_interventions: u32,
    /// Successful interventions plus correct no-ops divided by all cases.
    pub success_rate_bps: u16,
    /// False interventions divided by all annotated negative cases.
    pub false_intervention_rate_bps: u16,
}

/// Immutable promotion thresholds copied from the candidate package.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReplayPolicy {
    /// Required independent decision points.
    pub minimum_replays: u32,
    /// Required aggregate success rate.
    pub minimum_success_rate_bps: u16,
    /// Maximum false-intervention rate.
    pub maximum_false_positive_rate_bps: u16,
}

/// Inspectable reason a replay did not pass its gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThresholdFailure {
    /// The suite did not contain enough independent scenarios.
    InsufficientScenarios,
    /// No positive intervention case was present.
    MissingInterventionCoverage,
    /// No negative/no-op case was present.
    MissingNoOpCoverage,
    /// Aggregate correct classifications fell below policy.
    SuccessRateBelowMinimum,
    /// False interventions exceeded policy.
    FalseInterventionRateAboveMaximum,
}

/// Complete deterministic replay report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReplayReport {
    /// Report contract version.
    pub spec_version: ReplayResultSpecVersion,
    /// Stable content-derived replay identity.
    pub replay_id: String,
    /// Evaluated mutation identity.
    pub mutation_id: String,
    /// Immutable mutation semantic version.
    pub mutation_version: String,
    /// Stable content hash of the replay suite.
    pub scenario_set_hash: String,
    /// Per-scenario classifications in canonical scenario-ID order.
    pub results: Vec<ReplayScenarioResult>,
    /// Aggregate integer-only measurements.
    pub summary: ReplaySummary,
    /// Promotion thresholds used for this decision.
    pub policy: ReplayPolicy,
    /// Every unmet gate, empty only when the replay passed.
    pub threshold_failures: Vec<ThresholdFailure>,
    /// Whether all coverage and policy gates passed.
    pub passed: bool,
    /// Execution is always false for replay v0.1.
    pub mutation_executed: bool,
    /// Model use is always false for replay v0.1.
    pub model_used: bool,
}
