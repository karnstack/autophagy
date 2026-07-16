//! The built-in deterministic reference provider.
//!
//! This provider consults no model and performs no I/O. It echoes the
//! deterministic template baseline back as a structured response, so the entire
//! synthesis boundary — request construction, provider dispatch, response
//! validation, and contract re-validation — is exercisable offline today. It is
//! also the fixture provider for the boundary's tests.

use crate::provider::{
    ProviderError, ProviderResponse, SynthesisProposal, SynthesisProvider, SynthesisRequest,
    SynthesisResponse,
};

/// A pure, model-free, I/O-free provider that proposes the deterministic
/// template baseline unchanged.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeterministicReferenceProvider;

impl SynthesisProvider for DeterministicReferenceProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "deterministic"
    }

    fn propose(&self, request: &SynthesisRequest) -> Result<ProviderResponse, ProviderError> {
        let baseline = &request.baseline;
        Ok(ProviderResponse::offline(SynthesisProposal::Proposed {
            response: Box::new(SynthesisResponse {
                title: baseline.title.clone(),
                statement: baseline.statement.clone(),
                expected_result: baseline.expected_result.clone(),
                instruction: baseline.instruction.clone(),
                failure_cases: baseline.failure_cases.clone(),
                exclusions: baseline.exclusions.clone(),
                supporting_event_ids: baseline.supporting_event_ids.clone(),
                counterexample_event_ids: baseline.counterexample_event_ids.clone(),
                trigger_selectors: request.constraints.allowed_trigger_selectors.clone(),
                permissions: request.constraints.permission_ceiling.clone(),
            }),
        }))
    }
}
