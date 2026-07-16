use std::{collections::BTreeSet, fmt::Write as _};

use autophagy_mutations::MutationPackage;
use sha2::{Digest, Sha256};

use crate::{
    CounterfactualOutcome, ExpectedAction, ReplayDisposition, ReplayPolicy, ReplayReport,
    ReplayResultSpecVersion, ReplayScenarioResult, ReplaySuite, ReplaySummary, ThresholdFailure,
};

/// Evaluate an annotated suite without executing the mutation or invoking a model.
///
/// # Errors
/// Returns semantic validation failures or a mutation identity mismatch.
pub fn evaluate(
    package: &MutationPackage,
    suite: &ReplaySuite,
) -> Result<ReplayReport, ReplayEvaluationError> {
    suite.validate()?;
    let unreviewed_scenario_ids = suite
        .scenarios
        .iter()
        .filter(|scenario| scenario.counterfactual_outcome == Some(CounterfactualOutcome::Unknown))
        .map(|scenario| scenario.scenario_id.clone())
        .collect::<Vec<_>>();
    if !unreviewed_scenario_ids.is_empty() {
        return Err(ReplayEvaluationError::UnreviewedScenarios {
            scenario_ids: unreviewed_scenario_ids.join(", "),
        });
    }
    if suite.mutation_id != package.mutation_id {
        return Err(ReplayEvaluationError::MutationMismatch {
            package_mutation_id: package.mutation_id.clone(),
            suite_mutation_id: suite.mutation_id.clone(),
        });
    }
    let selectors = package
        .triggers
        .iter()
        .map(|trigger| trigger.selector.as_str())
        .collect::<BTreeSet<_>>();
    let mut results = suite
        .scenarios
        .iter()
        .map(|scenario| {
            let triggered = scenario
                .observed_trigger_selectors
                .iter()
                .any(|selector| selectors.contains(selector.as_str()));
            let disposition = match (scenario.expected_action, triggered) {
                (ExpectedAction::NoOp, false) => ReplayDisposition::NoOp,
                (ExpectedAction::NoOp, true) => ReplayDisposition::FalseIntervention,
                (ExpectedAction::Intervene, false) => ReplayDisposition::Contradiction,
                (ExpectedAction::Intervene, true) => {
                    if scenario.counterfactual_outcome
                        == Some(CounterfactualOutcome::ExpectedResult)
                    {
                        ReplayDisposition::Success
                    } else {
                        ReplayDisposition::Contradiction
                    }
                }
            };
            ReplayScenarioResult {
                scenario_id: scenario.scenario_id.clone(),
                source_event_ids: scenario.source_event_ids.clone(),
                observed_trigger_selectors: scenario.observed_trigger_selectors.clone(),
                triggered,
                expected_action: scenario.expected_action,
                disposition,
            }
        })
        .collect::<Vec<_>>();
    results.sort_by(|left, right| left.scenario_id.cmp(&right.scenario_id));

    let scenarios = u32::try_from(results.len()).unwrap_or(u32::MAX);
    let successes = count(&results, ReplayDisposition::Success);
    let no_ops = count(&results, ReplayDisposition::NoOp);
    let contradictions = count(&results, ReplayDisposition::Contradiction);
    let false_interventions = count(&results, ReplayDisposition::FalseIntervention);
    let correct = successes.saturating_add(no_ops);
    let negative = no_ops.saturating_add(false_interventions);
    let summary = ReplaySummary {
        scenarios,
        successes,
        no_ops,
        contradictions,
        false_interventions,
        success_rate_bps: basis_points(correct, scenarios),
        false_intervention_rate_bps: basis_points(false_interventions, negative),
    };
    let policy = ReplayPolicy {
        minimum_replays: package.promotion.minimum_replays,
        minimum_success_rate_bps: package.promotion.minimum_success_rate_bps,
        maximum_false_positive_rate_bps: package.promotion.maximum_false_positive_rate_bps,
    };
    let threshold_failures = threshold_failures(&summary, &policy);
    let passed = threshold_failures.is_empty();
    let suite_json = serde_json::to_vec(suite)?;
    let scenario_set_hash = prefixed_hash("rsh", &suite_json);
    let replay_id = prefixed_hash(
        "rpl",
        format!("replay/v1\0{}\0{scenario_set_hash}", package.mutation_id).as_bytes(),
    );
    Ok(ReplayReport {
        spec_version: ReplayResultSpecVersion::V0_1,
        replay_id,
        mutation_id: package.mutation_id.clone(),
        mutation_version: package.version.clone(),
        scenario_set_hash,
        results,
        summary,
        policy,
        threshold_failures,
        passed,
        mutation_executed: false,
        model_used: false,
    })
}

fn count(results: &[ReplayScenarioResult], disposition: ReplayDisposition) -> u32 {
    u32::try_from(
        results
            .iter()
            .filter(|result| result.disposition == disposition)
            .count(),
    )
    .unwrap_or(u32::MAX)
}

fn basis_points(numerator: u32, denominator: u32) -> u16 {
    if denominator == 0 {
        return 0;
    }
    let scaled = u64::from(numerator) * 10_000 / u64::from(denominator);
    u16::try_from(scaled).expect("a ratio cannot exceed 10,000 basis points")
}

fn threshold_failures(summary: &ReplaySummary, policy: &ReplayPolicy) -> Vec<ThresholdFailure> {
    let mut failures = Vec::new();
    if summary.scenarios < policy.minimum_replays {
        failures.push(ThresholdFailure::InsufficientScenarios);
    }
    if summary.successes + summary.contradictions == 0 {
        failures.push(ThresholdFailure::MissingInterventionCoverage);
    }
    if summary.no_ops + summary.false_interventions == 0 {
        failures.push(ThresholdFailure::MissingNoOpCoverage);
    }
    if summary.success_rate_bps < policy.minimum_success_rate_bps {
        failures.push(ThresholdFailure::SuccessRateBelowMinimum);
    }
    if summary.false_intervention_rate_bps > policy.maximum_false_positive_rate_bps {
        failures.push(ThresholdFailure::FalseInterventionRateAboveMaximum);
    }
    failures
}

fn prefixed_hash(prefix: &str, bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("{prefix}_{encoded}")
}

/// Error produced before a replay can be evaluated.
#[derive(Debug, thiserror::Error)]
pub enum ReplayEvaluationError {
    /// The suite violated its versioned contract.
    #[error("invalid replay suite: {0}")]
    InvalidSuite(#[from] crate::ReplayValidationErrors),
    /// Extracted intervention cases still require a human counterfactual label.
    #[error("replay suite has unreviewed counterfactual outcomes: {scenario_ids}")]
    UnreviewedScenarios {
        /// Stable scenario IDs that must be annotated before evaluation.
        scenario_ids: String,
    },
    /// A versioned suite could not be canonicalized for hashing.
    #[error("could not serialize replay suite: {0}")]
    Serialization(#[from] serde_json::Error),
    /// Suite and immutable package identities disagreed.
    #[error(
        "replay suite targets mutation '{suite_mutation_id}', not package '{package_mutation_id}'"
    )]
    MutationMismatch {
        /// Package mutation identity.
        package_mutation_id: String,
        /// Suite mutation identity.
        suite_mutation_id: String,
    },
}
