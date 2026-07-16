use std::fmt::Write as _;

use autophagy_patterns::{DetectorKind, EvidencePacket};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    CandidateHypothesis, GeneratedBy, Intervention, InterventionKind, LifecycleState,
    MutationPackage, MutationSpecVersion, PermissionManifest, PromotionPolicy, Trigger,
    TriggerKind,
};

/// Deterministic generation result, including honest refusal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GenerationOutcome {
    /// A valid review-only package was produced.
    Candidate {
        /// Generated candidate package.
        package: Box<MutationPackage>,
    },
    /// The finding could not support a concrete v0.1 candidate.
    InsufficientEvidence {
        /// Finding that was considered.
        finding_id: String,
        /// Inspectable refusal reason.
        reason: String,
    },
}

/// Generate one conservative candidate or an explicit refusal.
#[must_use]
pub fn generate_candidate(finding: &EvidencePacket) -> GenerationOutcome {
    if finding.evidence.len() < 2 || finding.score.distinct_sessions < 2 {
        return insufficient(
            finding,
            "candidate requires two independent supporting events",
        );
    }
    let generated = match finding.detector {
        DetectorKind::RepeatedCommandFailure => failure_candidate(finding),
        DetectorKind::RepeatedUserCorrection => correction_candidate(finding),
        DetectorKind::RepeatedSuccessfulRecovery => recovery_candidate(finding),
    };
    let Some(package) = generated else {
        return insufficient(
            finding,
            "detector signature is not concrete enough for a safe trigger",
        );
    };
    if let Err(errors) = package.validate() {
        return insufficient(
            finding,
            &format!("generated package failed validation: {errors}"),
        );
    }
    GenerationOutcome::Candidate {
        package: Box::new(package),
    }
}

/// Generate stable outcomes for every finding.
#[must_use]
pub fn generate_candidates(findings: &[EvidencePacket]) -> Vec<GenerationOutcome> {
    let mut outcomes = findings.iter().map(generate_candidate).collect::<Vec<_>>();
    outcomes.sort_by(|left, right| outcome_id(left).cmp(outcome_id(right)));
    outcomes
}

/// Compute the stable duplicate-equivalence key for registry insertion.
///
/// Equivalent candidates share a detector, intervention type, and normalized
/// trigger selectors even when they were derived from different finding IDs.
#[must_use]
pub fn equivalence_key(package: &MutationPackage) -> String {
    let mut material = format!(
        "equivalence/v1\0{}\0agent_instruction",
        package.source_detector.as_str()
    );
    let mut selectors = package
        .triggers
        .iter()
        .map(|trigger| trigger.selector.as_str())
        .collect::<Vec<_>>();
    selectors.sort_unstable();
    for selector in selectors {
        material.push('\0');
        material.push_str(selector);
    }
    let digest = Sha256::digest(material.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("eqv_{encoded}")
}

fn failure_candidate(finding: &EvidencePacket) -> Option<MutationPackage> {
    let signature = finding.signature.strip_prefix("failure/v1|")?;
    let (operation, exit_code) = signature.rsplit_once("|exit:")?;
    let (tool, command) = operation.split_once('|')?;
    if tool.trim().is_empty() || command.trim().is_empty() || exit_code.parse::<i64>().is_err() {
        return None;
    }
    let instruction = format!(
        "Before running `{command}`, verify its required preconditions. If it fails with exit code {exit_code}, do not retry it unchanged; inspect the failure and change the hypothesis or inputs first."
    );
    Some(base_package(
        finding,
        TriggerKind::ToolCall,
        instruction,
        format!(
            "The recurring `{command}` failure is caused or amplified by missing preconditions or unchanged retries."
        ),
        format!(
            "Matching `{command}` attempts avoid repeated exit-code-{exit_code} failures without blocking valid executions."
        ),
        vec![
            "The command can fail transiently even when all preconditions are satisfied.".to_owned(),
            "The same command text can represent different intent or repository state.".to_owned(),
            "The advisory could interrupt a legitimate immediate retry.".to_owned(),
        ],
        vec![
            "Do not block execution; this candidate is advisory until replay and shadow evaluation pass."
                .to_owned(),
            "Do not intervene when equivalent inputs already succeeded in the current context."
                .to_owned(),
        ],
    ))
}

fn correction_candidate(finding: &EvidencePacket) -> Option<MutationPackage> {
    let rule = finding.signature.strip_prefix("correction/v1|")?.trim();
    if rule.is_empty() {
        return None;
    }
    Some(base_package(
        finding,
        TriggerKind::AgentDecision,
        format!("Before the matching action, apply this explicit user-authored rule: {rule}."),
        format!(
            "Applying the explicit rule `{rule}` before the decision will prevent the repeated correction."
        ),
        format!(
            "Matching sessions follow `{rule}` before action and no longer require the same user correction."
        ),
        vec![
            "The rule can be context-specific despite recurring across sessions.".to_owned(),
            "The user can intentionally override the rule for a particular task.".to_owned(),
        ],
        vec![
            "Do not intervene when the user explicitly overrides this rule for the current task."
                .to_owned(),
        ],
    ))
}

fn recovery_candidate(finding: &EvidencePacket) -> Option<MutationPackage> {
    let signature = finding.signature.strip_prefix("recovery/v1|")?;
    let (target, recovery) = signature.split_once("|via|")?;
    let (target_operation, exit_code) = target.rsplit_once("|exit:")?;
    let (_target_tool, target_command) = target_operation.split_once('|')?;
    let (_recovery_tool, recovery_command) = recovery.split_once('|')?;
    if target_command.trim().is_empty()
        || recovery_command.trim().is_empty()
        || exit_code.parse::<i64>().is_err()
    {
        return None;
    }
    let mut package = base_package(
        finding,
        TriggerKind::ToolCall,
        format!(
            "Before running `{target_command}`, check whether the precondition restored by `{recovery_command}` is stale or missing. When it is, run `{recovery_command}` first; otherwise leave the workflow unchanged."
        ),
        format!(
            "The recurring exit-code-{exit_code} failure of `{target_command}` is prevented when the precondition established by `{recovery_command}` is satisfied first."
        ),
        format!(
            "Matching `{target_command}` attempts succeed without an initial failure after the relevant `{recovery_command}` precondition is applied."
        ),
        vec![
            format!(
                "`{recovery_command}` can correlate with recovery without causing it."
            ),
            format!(
                "`{recovery_command}` can have side effects or be invalid for some project states."
            ),
            "The target failure can be transient and recover on direct retry.".to_owned(),
        ],
        vec![
            "Do not run the recovery step when the target inputs or project state differ from the observed motif."
                .to_owned(),
            "Do not run the recovery step without first confirming it is safe in the current repository."
                .to_owned(),
        ],
    );
    package.title = finding.title.replacen(
        "Repeated successful recovery",
        "Reuse successful recovery",
        1,
    );
    Some(package)
}

#[allow(clippy::too_many_arguments)]
fn base_package(
    finding: &EvidencePacket,
    trigger_kind: TriggerKind,
    instruction: String,
    statement: String,
    expected_result: String,
    failure_cases: Vec<String>,
    exclusions: Vec<String>,
) -> MutationPackage {
    MutationPackage {
        spec_version: MutationSpecVersion::V0_1,
        mutation_id: mutation_id(&finding.finding_id),
        version: "0.1.0".to_owned(),
        state: LifecycleState::Candidate,
        generated_by: GeneratedBy::DeterministicTemplateV1,
        source_finding_id: finding.finding_id.clone(),
        source_detector: finding.detector,
        title: finding.title.replacen("Repeated", "Prevent repeated", 1),
        hypothesis: CandidateHypothesis {
            statement,
            expected_result,
            supporting_event_ids: finding
                .evidence
                .iter()
                .map(|item| item.event_id.clone())
                .collect(),
            counterexample_event_ids: finding
                .counterexamples
                .iter()
                .map(|item| item.event_id.clone())
                .collect(),
            failure_cases,
        },
        triggers: vec![Trigger {
            kind: trigger_kind,
            selector: finding.signature.clone(),
        }],
        exclusions,
        intervention: Intervention {
            kind: InterventionKind::AgentInstruction,
            instruction,
        },
        permissions: PermissionManifest::default(),
        promotion: PromotionPolicy {
            minimum_replays: 5,
            minimum_success_rate_bps: 8_000,
            maximum_false_positive_rate_bps: 1_000,
        },
    }
}

fn mutation_id(finding_id: &str) -> String {
    let digest = Sha256::digest(format!("deterministic-template/v1\0{finding_id}").as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("mut_{encoded}")
}

fn insufficient(finding: &EvidencePacket, reason: &str) -> GenerationOutcome {
    GenerationOutcome::InsufficientEvidence {
        finding_id: finding.finding_id.clone(),
        reason: reason.to_owned(),
    }
}

fn outcome_id(outcome: &GenerationOutcome) -> &str {
    match outcome {
        GenerationOutcome::Candidate { package } => &package.mutation_id,
        GenerationOutcome::InsufficientEvidence { finding_id, .. } => finding_id,
    }
}
