use serde::{Deserialize, Serialize};

use crate::ShadowValidationErrors;

/// Shadow suite wire-format version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ShadowSuiteSpecVersion {
    /// Initial annotated observation suite.
    #[serde(rename = "shadow-suite/0.1")]
    V0_1,
}

/// Shadow observation wire-format version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ShadowObservationSpecVersion {
    /// Initial observation-only contract.
    #[serde(rename = "shadow-observation/0.1")]
    V0_1,
}

/// Shadow report wire-format version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ShadowResultSpecVersion {
    /// Initial deterministic shadow report.
    #[serde(rename = "shadow-result/0.1")]
    V0_1,
}

/// One annotated live observation collected without applying a mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowObservation {
    /// Observation contract version.
    pub spec_version: ShadowObservationSpecVersion,
    /// Stable observation identity.
    pub observation_id: String,
    /// Exact AEP events supporting the annotation.
    pub source_event_ids: Vec<String>,
    /// Trigger selectors observable before the outcome.
    pub observed_trigger_selectors: Vec<String>,
    /// Whether review of the actual outcome says intervention would have helped.
    pub intervention_would_help: bool,
    /// Optional annotation rationale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Ordered shadow observations for one immutable mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowSuite {
    /// Suite contract version.
    pub spec_version: ShadowSuiteSpecVersion,
    /// Mutation observed by this suite.
    pub mutation_id: String,
    /// Independent live observations.
    pub observations: Vec<ShadowObservation>,
}

impl ShadowSuite {
    /// Enforce shadow-suite semantic invariants.
    ///
    /// # Errors
    /// Returns every invalid field and cross-field relationship.
    pub fn validate(&self) -> Result<(), ShadowValidationErrors> {
        crate::validate::validate(self)
    }
}

/// Confusion-matrix classification for one observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowDisposition {
    /// Would trigger where intervention was useful.
    TruePositive,
    /// Would remain silent where intervention was unnecessary.
    TrueNegative,
    /// Would trigger where intervention was unnecessary.
    FalsePositive,
    /// Would remain silent where intervention was useful.
    FalseNegative,
}

/// Inspectable deterministic shadow result for one observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ShadowResult {
    /// Stable observation identity.
    pub observation_id: String,
    /// Exact AEP event provenance.
    pub source_event_ids: Vec<String>,
    /// Selectors observed before the outcome.
    pub observed_trigger_selectors: Vec<String>,
    /// Whether the mutation would have triggered.
    pub would_trigger: bool,
    /// Human-reviewed usefulness annotation.
    pub intervention_would_help: bool,
    /// Deterministic confusion-matrix class.
    pub disposition: ShadowDisposition,
}

/// Integer-only shadow metrics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ShadowSummary {
    /// Total independent observations.
    pub observations: u32,
    /// Useful would-be interventions.
    pub true_positives: u32,
    /// Correct would-be silence.
    pub true_negatives: u32,
    /// Unnecessary would-be interventions.
    pub false_positives: u32,
    /// Useful interventions the trigger would miss.
    pub false_negatives: u32,
    /// True positives divided by all would-be interventions.
    pub precision_bps: u16,
    /// False positives divided by all would-be interventions.
    pub false_positive_rate_bps: u16,
    /// True positives divided by all useful observations.
    pub recall_bps: u16,
}

/// Shadow promotion policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ShadowPolicy {
    /// Required independent live observations in v0.1.
    pub minimum_observations: u32,
    /// Maximum rate copied from the immutable mutation package.
    pub maximum_false_positive_rate_bps: u16,
}

/// Reason a shadow suite did not pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowThresholdFailure {
    /// Too few independent observations were supplied.
    InsufficientObservations,
    /// No observation was annotated as useful intervention.
    MissingPositiveCoverage,
    /// No observation was annotated as a legitimate no-op.
    MissingNegativeCoverage,
    /// No trigger fired, so precision cannot be measured.
    MissingTriggerCoverage,
    /// Unnecessary would-be interventions exceeded policy.
    FalsePositiveRateAboveMaximum,
}

/// Complete deterministic observation-only shadow report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ShadowReport {
    /// Report contract version.
    pub spec_version: ShadowResultSpecVersion,
    /// Stable content-derived report identity.
    pub shadow_id: String,
    /// Observed mutation identity.
    pub mutation_id: String,
    /// Immutable mutation semantic version.
    pub mutation_version: String,
    /// Stable suite hash.
    pub observation_set_hash: String,
    /// Per-observation classifications in canonical identity order.
    pub results: Vec<ShadowResult>,
    /// Aggregate precision and recall measurements.
    pub summary: ShadowSummary,
    /// Policy used for the pass decision.
    pub policy: ShadowPolicy,
    /// Every unmet gate.
    pub threshold_failures: Vec<ShadowThresholdFailure>,
    /// Whether all observation and precision gates passed.
    pub passed: bool,
    /// Always false for shadow v0.1.
    pub mutation_applied: bool,
    /// Always false for shadow v0.1.
    pub model_used: bool,
}
