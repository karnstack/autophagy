use autophagy_patterns::DetectorKind;
use serde::{Deserialize, Serialize};

use crate::MutationValidationErrors;

/// Mutation Package wire-format version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MutationSpecVersion {
    /// Initial review-only candidate contract.
    #[serde(rename = "mutation/0.1")]
    V0_1,
}

/// Auditable lifecycle state external to installation state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    /// Proposed but not challenged or evaluated.
    Candidate,
    /// A user rejected the candidate before activation.
    Rejected,
}

impl LifecycleState {
    /// Stable database and wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Rejected => "rejected",
        }
    }
}

/// Origin of candidate package content.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedBy {
    /// Inspectable built-in template version 1; no model involved.
    DeterministicTemplateV1,
}

/// Observable condition at which an intervention may apply.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    /// Trigger category.
    #[serde(rename = "type")]
    pub kind: TriggerKind,
    /// Exact versioned detector signature used as a selector.
    pub selector: String,
}

/// Supported trigger family.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    /// Before a matching tool call.
    ToolCall,
    /// Before an agent decision matching an explicit correction rule.
    AgentDecision,
}

/// Supported intervention category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterventionKind {
    /// Reviewable text intended for a future skill or context-injection adapter.
    AgentInstruction,
}

/// Concrete proposed behavioral change.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Intervention {
    /// Intervention category.
    #[serde(rename = "type")]
    pub kind: InterventionKind,
    /// Exact reviewable instruction; never installed by this crate.
    pub instruction: String,
}

/// Explicit capabilities requested by a mutation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PermissionManifest {
    /// Filesystem paths the mutation may read.
    pub filesystem_read: Vec<String>,
    /// Filesystem paths the mutation may write.
    pub filesystem_write: Vec<String>,
    /// Commands the mutation may execute.
    pub commands: Vec<String>,
    /// Environment variable names the mutation may read.
    pub environment: Vec<String>,
    /// Whether the mutation requests network access.
    pub network: bool,
}

/// Falsifiable claim and its exact lineage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CandidateHypothesis {
    /// Proposed causal claim.
    pub statement: String,
    /// Observable result expected after intervention.
    pub expected_result: String,
    /// Exact supporting AEP event IDs.
    pub supporting_event_ids: Vec<String>,
    /// Exact contradictory or success event IDs.
    pub counterexample_event_ids: Vec<String>,
    /// Conditions under which the proposed intervention could be harmful or wrong.
    pub failure_cases: Vec<String>,
}

/// Minimum future evaluation bars; this crate does not perform promotion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PromotionPolicy {
    /// Required independent replay scenarios.
    pub minimum_replays: u32,
    /// Required replay success rate, in basis points.
    pub minimum_success_rate_bps: u16,
    /// Maximum acceptable false-positive rate, in basis points.
    pub maximum_false_positive_rate_bps: u16,
}

/// Immutable, review-only candidate derived from one Evidence Packet.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MutationPackage {
    /// Mutation Package contract version.
    pub spec_version: MutationSpecVersion,
    /// Stable content-derived mutation identity.
    pub mutation_id: String,
    /// Semantic version of this package content.
    pub version: String,
    /// Candidate lifecycle state at generation time.
    pub state: LifecycleState,
    /// Deterministic or model-backed producer.
    pub generated_by: GeneratedBy,
    /// Exact Evidence Packet finding that caused generation.
    pub source_finding_id: String,
    /// Detector family retained for inspection.
    pub source_detector: DetectorKind,
    /// Concise user-facing title.
    pub title: String,
    /// Falsifiable evidence-linked claim.
    pub hypothesis: CandidateHypothesis,
    /// Observable entry conditions.
    pub triggers: Vec<Trigger>,
    /// Explicit conditions suppressing intervention.
    pub exclusions: Vec<String>,
    /// Proposed behavior.
    pub intervention: Intervention,
    /// Requested capabilities. Deterministic v0.1 candidates request none.
    pub permissions: PermissionManifest,
    /// Future replay/shadow promotion thresholds.
    pub promotion: PromotionPolicy,
}

impl MutationPackage {
    /// Enforce Mutation Package v0.1 semantic invariants.
    ///
    /// # Errors
    /// Returns every detected contract violation.
    pub fn validate(&self) -> Result<(), MutationValidationErrors> {
        crate::validate::validate(self)
    }
}
