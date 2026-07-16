//! The synthesis boundary orchestration.
//!
//! [`synthesize_candidate`] wires the invariants together: it runs the
//! deterministic evidence gate first (refusing before any provider is
//! consulted), consults the provider, validates the response deterministically,
//! assembles a mutation candidate, and re-validates it against the exact
//! Mutation Package v0.1 contract.

use autophagy_mutations::{GenerationOutcome, MutationPackage, Trigger, generate_candidate};
use autophagy_patterns::EvidencePacket;
use serde::Serialize;

use crate::{
    manifest::ModelManifest,
    provider::{
        SynthesisBaseline, SynthesisConstraints, SynthesisProposal, SynthesisProvider,
        SynthesisRequest, SynthesisResponse,
    },
    validate::{SynthesisDiagnostic, validate_response},
};

/// The result of synthesizing one finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SynthesisOutcome {
    /// A validated, contract-conformant candidate was produced.
    Candidate {
        /// Provider that produced the candidate.
        provider: String,
        /// Model name from the manifest.
        model: String,
        /// Whether a language model was consulted.
        model_used: bool,
        /// The candidate package, ready for the review-only registry.
        package: Box<MutationPackage>,
    },
    /// Deterministic thresholds were not met; no provider was consulted.
    InsufficientEvidence {
        /// Finding that was considered.
        finding_id: String,
        /// Inspectable refusal reason.
        reason: String,
    },
    /// The provider honestly declined to propose.
    ProviderDeclined {
        /// Finding that was considered.
        finding_id: String,
        /// Provider that declined.
        provider: String,
        /// Inspectable reason for declining.
        reason: String,
    },
    /// The provider proposed, but the response violated the contract.
    Rejected {
        /// Finding that was considered.
        finding_id: String,
        /// Provider whose response was rejected.
        provider: String,
        /// Every detected violation.
        diagnostics: Vec<SynthesisDiagnostic>,
    },
}

/// Synthesize a candidate for one finding through the full boundary.
///
/// The deterministic evidence gate runs first: when it refuses, the provider is
/// never consulted. When a provider proposes, every field it returns is
/// validated against the deterministic template before a candidate is emitted.
#[must_use]
pub fn synthesize_candidate(
    finding: &EvidencePacket,
    manifest: &ModelManifest,
    provider: &dyn SynthesisProvider,
) -> SynthesisOutcome {
    // 1. Deterministic evidence gate. Honest silence over invention.
    let template = match generate_candidate(finding) {
        GenerationOutcome::InsufficientEvidence { finding_id, reason } => {
            return SynthesisOutcome::InsufficientEvidence { finding_id, reason };
        }
        GenerationOutcome::Candidate { package } => package,
    };

    // 2. Capability gate. A provider is only consulted for a capability its
    //    manifest declares.
    let capability = provider.required_capability();
    if !manifest.declares(capability) {
        return SynthesisOutcome::Rejected {
            finding_id: finding.finding_id.clone(),
            provider: provider.name().to_owned(),
            diagnostics: vec![SynthesisDiagnostic {
                path: "manifest.capabilities".to_owned(),
                code: "missing_capability",
                message: format!(
                    "manifest '{}' does not declare the '{}' capability",
                    manifest.name,
                    capability.as_str()
                ),
            }],
        };
    }

    // 3. Build the structured request from the deterministic template.
    let request = build_request(finding, &template);

    // 4. Consult the provider.
    let response = match provider.propose(&request) {
        SynthesisProposal::Declined { reason } => {
            return SynthesisOutcome::ProviderDeclined {
                finding_id: finding.finding_id.clone(),
                provider: provider.name().to_owned(),
                reason,
            };
        }
        SynthesisProposal::Proposed { response } => *response,
    };

    // 5. Validate every returned field against the constraints.
    if let Err(rejection) = validate_response(&request, &response) {
        return SynthesisOutcome::Rejected {
            finding_id: finding.finding_id.clone(),
            provider: provider.name().to_owned(),
            diagnostics: rejection.into_diagnostics(),
        };
    }

    // 6. Assemble the candidate and re-validate it against the mutation
    //    contract. The boundary cannot emit a candidate the contract rejects.
    let package = assemble(&template, &request, &response);
    if let Err(errors) = package.validate() {
        let diagnostics = errors
            .iter()
            .map(|error| SynthesisDiagnostic {
                path: format!("package.{}", error.path),
                code: error.code,
                message: error.message.clone(),
            })
            .collect();
        return SynthesisOutcome::Rejected {
            finding_id: finding.finding_id.clone(),
            provider: provider.name().to_owned(),
            diagnostics,
        };
    }

    SynthesisOutcome::Candidate {
        provider: provider.name().to_owned(),
        model: manifest.name.clone(),
        model_used: provider.uses_model(),
        package: Box::new(package),
    }
}

/// Synthesize stable outcomes for every finding, ordered deterministically.
#[must_use]
pub fn synthesize_candidates(
    findings: &[EvidencePacket],
    manifest: &ModelManifest,
    provider: &dyn SynthesisProvider,
) -> Vec<SynthesisOutcome> {
    let mut outcomes = findings
        .iter()
        .map(|finding| synthesize_candidate(finding, manifest, provider))
        .collect::<Vec<_>>();
    outcomes.sort_by(|left, right| outcome_key(left).cmp(outcome_key(right)));
    outcomes
}

fn build_request(finding: &EvidencePacket, template: &MutationPackage) -> SynthesisRequest {
    let trigger_kind = template
        .triggers
        .first()
        .map_or(autophagy_mutations::TriggerKind::ToolCall, |trigger| {
            trigger.kind
        });
    let allowed_trigger_selectors = template
        .triggers
        .iter()
        .map(|trigger| trigger.selector.clone())
        .collect::<Vec<_>>();
    SynthesisRequest {
        finding_id: finding.finding_id.clone(),
        detector: finding.detector,
        signature: finding.signature.clone(),
        constraints: SynthesisConstraints {
            allowed_supporting_event_ids: template.hypothesis.supporting_event_ids.clone(),
            allowed_counterexample_event_ids: template.hypothesis.counterexample_event_ids.clone(),
            allowed_trigger_selectors,
            trigger_kind,
            permission_ceiling: template.permissions.clone(),
        },
        baseline: SynthesisBaseline {
            title: template.title.clone(),
            statement: template.hypothesis.statement.clone(),
            expected_result: template.hypothesis.expected_result.clone(),
            instruction: template.intervention.instruction.clone(),
            failure_cases: template.hypothesis.failure_cases.clone(),
            exclusions: template.exclusions.clone(),
            supporting_event_ids: template.hypothesis.supporting_event_ids.clone(),
            counterexample_event_ids: template.hypothesis.counterexample_event_ids.clone(),
        },
    }
}

fn assemble(
    template: &MutationPackage,
    request: &SynthesisRequest,
    response: &SynthesisResponse,
) -> MutationPackage {
    // Provenance, identity, versioning, and promotion thresholds are never
    // provider-controlled; they are carried through from the deterministic
    // template. Only the evidence-linked, reviewable content is enriched.
    let mut package = template.clone();
    package.title.clone_from(&response.title);
    package.hypothesis.statement.clone_from(&response.statement);
    package
        .hypothesis
        .expected_result
        .clone_from(&response.expected_result);
    package
        .hypothesis
        .supporting_event_ids
        .clone_from(&response.supporting_event_ids);
    package
        .hypothesis
        .counterexample_event_ids
        .clone_from(&response.counterexample_event_ids);
    package
        .hypothesis
        .failure_cases
        .clone_from(&response.failure_cases);
    package.exclusions.clone_from(&response.exclusions);
    package
        .intervention
        .instruction
        .clone_from(&response.instruction);
    package.triggers = response
        .trigger_selectors
        .iter()
        .map(|selector| Trigger {
            kind: request.constraints.trigger_kind,
            selector: selector.clone(),
        })
        .collect();
    package.permissions.clone_from(&response.permissions);
    package
}

fn outcome_key(outcome: &SynthesisOutcome) -> &str {
    match outcome {
        SynthesisOutcome::Candidate { package, .. } => &package.mutation_id,
        SynthesisOutcome::InsufficientEvidence { finding_id, .. }
        | SynthesisOutcome::ProviderDeclined { finding_id, .. }
        | SynthesisOutcome::Rejected { finding_id, .. } => finding_id,
    }
}
