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
    ///
    /// Declining is an expected event, not a failure: a model that returns
    /// unparseable output, or judges the evidence too weak, declines here rather
    /// than fabricating a candidate.
    Declined {
        /// Inspectable reason for declining.
        reason: String,
    },
}

/// Token accounting a provider reports for one request.
///
/// Both counts are optional: when a runtime does not report usage the boundary
/// records it as unavailable rather than estimating it. The deterministic
/// reference provider consults no model, so it always reports both as `None`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Tokens the model consumed for the prompt, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    /// Tokens the model produced in the response, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
}

impl TokenUsage {
    /// Usage that no model produced or reported.
    #[must_use]
    pub const fn unavailable() -> Self {
        Self {
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    /// Whether either count is present.
    #[must_use]
    pub const fn is_reported(&self) -> bool {
        self.prompt_tokens.is_some() || self.completion_tokens.is_some()
    }
}

/// A provider's answer to one request: a proposal plus its token accounting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderResponse {
    /// The structured proposal or honest refusal.
    pub proposal: SynthesisProposal,
    /// Token usage the runtime reported, if any.
    pub usage: TokenUsage,
}

impl ProviderResponse {
    /// A model-free proposal with no token accounting.
    #[must_use]
    pub fn offline(proposal: SynthesisProposal) -> Self {
        Self {
            proposal,
            usage: TokenUsage::unavailable(),
        }
    }
}

/// A provider-transport or configuration failure, distinct from an honest
/// decline. These never carry secrets, prompts, or raw payloads.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// The configured endpoint resolves to a non-loopback host and remote
    /// endpoints were not explicitly allowed. Evidence never leaves the machine
    /// by default.
    #[error(
        "synthesis endpoint '{endpoint}' resolves to non-loopback host '{host}'; \
         refusing to send evidence off this machine without an explicit \
         --allow-remote-endpoint opt-in"
    )]
    NonLoopbackEndpoint {
        /// The configured endpoint.
        endpoint: String,
        /// The extracted non-loopback host.
        host: String,
    },
    /// The configured endpoint is not a usable `http`/`https` URL.
    #[error("synthesis endpoint '{endpoint}' is not a valid http(s) URL: {reason}")]
    InvalidEndpoint {
        /// The configured endpoint.
        endpoint: String,
        /// Why it could not be parsed.
        reason: String,
    },
    /// The manifest named an API-key environment variable that is not set. The
    /// error names the variable but never its value.
    #[error("environment variable '{env_var}' named by the manifest 'api_key_env' is not set")]
    MissingApiKey {
        /// The environment variable name the manifest declared.
        env_var: String,
    },
    /// The request to the endpoint failed (connection, timeout, or bad status).
    /// The reason is derived from the transport and never echoes request headers.
    #[error("synthesis request to '{endpoint}' failed: {reason}")]
    Transport {
        /// The endpoint that was contacted.
        endpoint: String,
        /// A transport-derived, secret-free description of the failure.
        reason: String,
    },
    /// The configured agent-CLI binary could not be launched — most often it is
    /// not installed or not on `PATH`. The reason names the binary and the
    /// underlying cause but never the prompt.
    #[error("agent CLI '{binary}' could not be launched: {reason}")]
    CliSpawn {
        /// The configured binary path or bare name.
        binary: String,
        /// A secret-free description of why the launch failed.
        reason: String,
    },
    /// The agent CLI launched but did not yield a usable response: it exited
    /// non-zero, exceeded the wall-clock timeout and was killed, or produced an
    /// envelope with no assistant message. The reason carries only a bounded,
    /// sanitized diagnostic snippet, never the prompt or a secret.
    #[error("agent CLI '{binary}' did not produce a usable response: {reason}")]
    CliFailure {
        /// The configured binary path or bare name.
        binary: String,
        /// A bounded, sanitized, secret-free description of the failure.
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

    /// Produce a structured proposal (or honest decline) with token accounting,
    /// or a transport/configuration [`ProviderError`].
    ///
    /// A decline — including a model returning unparseable output — is an
    /// expected [`Ok`] result, not an error. Only genuine transport or
    /// configuration failures return [`Err`].
    ///
    /// # Errors
    /// Returns [`ProviderError`] when the endpoint is misconfigured, a required
    /// API key is missing, a remote endpoint is refused, or the request fails.
    fn propose(&self, request: &SynthesisRequest) -> Result<ProviderResponse, ProviderError>;
}
