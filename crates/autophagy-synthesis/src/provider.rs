//! The provider-neutral synthesis interface.
//!
//! A provider receives a [`SynthesisRequest`] — the deterministic evidence,
//! the fields it may enrich, and the hard constraints it must respect — and
//! returns a structured [`SynthesisResponse`] or an honest
//! [`SynthesisProposal::Declined`]. There is no free-prose passthrough: a
//! provider fills known, typed fields, and every one of them is validated
//! deterministically before it can become a mutation candidate.

use autophagy_mutations::{PermissionManifest, TriggerKind};
use autophagy_patterns::DetectorKind;
use serde::{Deserialize, Serialize};

use crate::manifest::Capability;

/// Hard limits a provider response must respect. These are derived from the
/// deterministic template, never from the provider.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SynthesisConstraints {
    /// The only event IDs a provider may cite as support.
    pub allowed_supporting_event_ids: Vec<String>,
    /// The only event IDs a provider may cite as counterexamples.
    pub allowed_counterexample_event_ids: Vec<String>,
    /// The only trigger selectors a provider may use.
    pub allowed_trigger_selectors: Vec<String>,
    /// The trigger family the deterministic template selected.
    pub trigger_kind: TriggerKind,
    /// The maximum permission scope; a response may request no more than this.
    pub permission_ceiling: PermissionManifest,
}

/// The deterministic template fields a provider may keep or enrich.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SynthesisBaseline {
    /// Concise candidate title.
    pub title: String,
    /// Falsifiable causal statement.
    pub statement: String,
    /// Observable expected result after intervention.
    pub expected_result: String,
    /// Reviewable agent instruction.
    pub instruction: String,
    /// Conditions under which the intervention could be wrong or harmful.
    pub failure_cases: Vec<String>,
    /// Conditions that suppress the intervention.
    pub exclusions: Vec<String>,
    /// Supporting event IDs from the deterministic template.
    pub supporting_event_ids: Vec<String>,
    /// Counterexample event IDs from the deterministic template.
    pub counterexample_event_ids: Vec<String>,
}

/// A structured synthesis request handed to a provider.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SynthesisRequest {
    /// Source Evidence Packet finding identity.
    pub finding_id: String,
    /// Detector family that produced the finding.
    pub detector: DetectorKind,
    /// Versioned normalized recurrence signature.
    pub signature: String,
    /// Hard constraints the response must respect.
    pub constraints: SynthesisConstraints,
    /// Deterministic baseline the provider may enrich.
    pub baseline: SynthesisBaseline,
}

/// A structured, schema-constrained provider response.
///
/// Every field here is validated before assembly into a mutation candidate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SynthesisResponse {
    /// Concise candidate title.
    pub title: String,
    /// Falsifiable causal statement.
    pub statement: String,
    /// Observable expected result after intervention.
    pub expected_result: String,
    /// Reviewable agent instruction.
    pub instruction: String,
    /// Conditions under which the intervention could be wrong or harmful.
    pub failure_cases: Vec<String>,
    /// Conditions that suppress the intervention.
    pub exclusions: Vec<String>,
    /// Cited supporting event IDs; must be a subset of the allowed set.
    pub supporting_event_ids: Vec<String>,
    /// Cited counterexample event IDs; must be a subset of the allowed set.
    pub counterexample_event_ids: Vec<String>,
    /// Trigger selectors; each must come from the allowed set.
    pub trigger_selectors: Vec<String>,
    /// Requested permissions; may never exceed the ceiling.
    pub permissions: PermissionManifest,
}

/// What a provider returns for one request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SynthesisProposal {
    /// A structured, still-unvalidated proposal.
    Proposed {
        /// The provider's structured response.
        response: Box<SynthesisResponse>,
    },
    /// The provider honestly declined to propose.
    Declined {
        /// Inspectable reason for declining.
        reason: String,
    },
}

/// A source of enriched mutation proposals.
///
/// Implementations range from the built-in deterministic reference provider to
/// a future local-model-backed provider. The boundary treats them identically:
/// it validates every returned field and never trusts a provider's word.
pub trait SynthesisProvider {
    /// Stable provider identifier, recorded in the synthesis outcome.
    fn name(&self) -> &str;

    /// The capability a manifest must declare for this provider to be consulted.
    fn required_capability(&self) -> Capability {
        Capability::MutationSynthesis
    }

    /// Whether this provider consults a language model. The deterministic
    /// reference provider returns `false`.
    fn uses_model(&self) -> bool {
        false
    }

    /// Whether this provider makes network calls. Local-first providers return
    /// `false`.
    fn uses_network(&self) -> bool {
        false
    }

    /// Produce a structured proposal or an honest refusal.
    fn propose(&self, request: &SynthesisRequest) -> SynthesisProposal;
}
