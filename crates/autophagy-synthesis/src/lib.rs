//! Local synthesis boundary for Autophagy mutation candidates.
//!
//! This crate is a *boundary*, not a model integration. It defines the
//! provider-neutral seam through which any future local model may propose
//! richer mutation candidates without ever being able to bypass the
//! deterministic Mutation Package v0.1 contract or its evaluation gates.
//!
//! Three invariants make the boundary trustworthy:
//!
//! - **Deterministic evidence gate.** A provider is consulted only after the
//!   same deterministic thresholds that gate [`autophagy_mutations`] have been
//!   met. When evidence is insufficient the boundary refuses with a structured
//!   reason and never calls a provider. Honest silence over invention.
//! - **Validation over trust.** Every field a provider returns is validated
//!   deterministically: cited evidence must exist in the packet it was given,
//!   trigger selectors must come from the deterministic template, and requested
//!   permissions may never exceed the template's ceiling. A response that
//!   violates any rule is rejected with a structured diagnostic; nothing is
//!   silently "fixed up".
//! - **One contract.** A synthesized candidate is assembled into an ordinary
//!   [`autophagy_mutations::MutationPackage`] and re-validated against the exact
//!   Mutation Package v0.1 contract. The boundary cannot emit a candidate the
//!   mutation contract would reject.
//!
//! A single [`DeterministicReferenceProvider`] ships with the crate: a pure,
//! model-free, I/O-free provider that lets the whole boundary be exercised
//! offline today. It is the executable vertical slice that justifies the crate.

mod deterministic;
mod manifest;
mod prompt;
mod provider;
mod remote;
mod synthesize;
mod validate;

pub use deterministic::DeterministicReferenceProvider;
pub use manifest::{
    Capability, ManifestError, ManifestSpecVersion, ManifestTimeouts, ModelFormat, ModelManifest,
    ResourceHints,
};
pub use prompt::{MAX_COMPLETION_TOKENS, SYSTEM_PROMPT, response_json_schema, user_prompt};
pub use provider::{
    ProviderError, ProviderResponse, SynthesisBaseline, SynthesisConstraints, SynthesisProposal,
    SynthesisProvider, SynthesisRequest, SynthesisResponse, TokenUsage,
};
pub use remote::{EndpointLocality, OllamaProvider, OpenAiCompatibleProvider, classify_endpoint};
pub use synthesize::{SynthesisOutcome, synthesize_candidate, synthesize_candidates};
pub use validate::{SynthesisDiagnostic, SynthesisRejection};
