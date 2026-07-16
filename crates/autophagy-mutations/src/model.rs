use autophagy_patterns::DetectorKind;
use serde::{Deserialize, Serialize};

use crate::MutationValidationErrors;

/// Mutation Package wire-format version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MutationSpecVersion {
    /// Initial review-only candidate contract.
    #[serde(rename = "mutation/0.1")]
    V0_1,
    /// Additive revision that carries optional model synthesis provenance.
    ///
    /// A v0.2 package is a v0.1 package plus a `provenance` block recording the
    /// provider and model that enriched it. A package with no provenance is a
    /// v0.1 package; the two shapes are otherwise byte-identical.
    #[serde(rename = "mutation/0.2")]
    V0_2,
}

impl MutationSpecVersion {
    /// Stable wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V0_1 => "mutation/0.1",
            Self::V0_2 => "mutation/0.2",
        }
    }
}

/// Auditable lifecycle state external to installation state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    /// Proposed but not challenged or evaluated.
    Candidate,
    /// Challenge checklist passed; replay is the next allowed gate.
    Challenged,
    /// Deterministic replay coverage and thresholds passed.
    ReplayPassed,
    /// Observation-only shadow precision thresholds passed.
    ShadowPassed,
    /// Explicitly installed into a supported agent target.
    Active,
    /// Reversibly uninstalled and no longer active.
    Retired,
    /// A user rejected the candidate before activation.
    Rejected,
}

impl LifecycleState {
    /// Stable database and wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Challenged => "challenged",
            Self::ReplayPassed => "replay_passed",
            Self::ShadowPassed => "shadow_passed",
            Self::Active => "active",
            Self::Retired => "retired",
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
    /// A local model enriched the deterministic template's reviewable content
    /// through the synthesis boundary. The evidence, triggers, and permission
    /// ceiling still come from the deterministic template; only the reviewable
    /// prose was model-enriched, and a `provenance` block records the model.
    ModelSynthesisV1,
}

/// Exact identity of the model that enriched a synthesized candidate.
///
/// Present only on `mutation/0.2` packages produced by a model-backed provider.
/// It never contains secrets, endpoints, prompts, or raw payloads — only stable
/// model identity, so a reviewer can see exactly what proposed the enrichment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// Stable provider identifier (for example `ollama` or `openai-compatible`).
    pub provider: String,
    /// Human-readable model name from the manifest.
    pub model_name: String,
    /// Model revision, tag, or version string from the manifest.
    pub model_revision: String,
    /// Optional content digest of the model weights from the manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_digest: Option<String>,
    /// Manifest contract version the provider was configured from.
    pub manifest_spec_version: String,
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
    /// Model synthesis provenance. Present only on `mutation/0.2` packages; a
    /// package without it is a `mutation/0.1` package and round-trips as one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
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
