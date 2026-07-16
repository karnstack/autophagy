use std::{collections::BTreeSet, fmt::Write as _};

use autophagy_mutations::MutationPackage;
use sha2::{Digest, Sha256};

use crate::{
    ShadowDisposition, ShadowPolicy, ShadowReport, ShadowResult, ShadowResultSpecVersion,
    ShadowSuite, ShadowSummary, ShadowThresholdFailure,
};

const MINIMUM_OBSERVATIONS: u32 = 5;

/// Evaluate live annotations without applying the mutation or invoking a model.
///
/// # Errors
/// Returns semantic validation, identity mismatch, or serialization failures.
pub fn evaluate(
    package: &MutationPackage,
    suite: &ShadowSuite,
) -> Result<ShadowReport, ShadowEvaluationError> {
    suite.validate()?;
    if suite.mutation_id != package.mutation_id {
        return Err(ShadowEvaluationError::MutationMismatch {
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
        .observations
        .iter()
        .map(|observation| {
            let would_trigger = observation
                .observed_trigger_selectors
                .iter()
                .any(|selector| selectors.contains(selector.as_str()));
            let disposition = match (would_trigger, observation.intervention_would_help) {
                (true, true) => ShadowDisposition::TruePositive,
                (false, false) => ShadowDisposition::TrueNegative,
                (true, false) => ShadowDisposition::FalsePositive,
                (false, true) => ShadowDisposition::FalseNegative,
            };
            ShadowResult {
                observation_id: observation.observation_id.clone(),
                source_event_ids: observation.source_event_ids.clone(),
                observed_trigger_selectors: observation.observed_trigger_selectors.clone(),
                would_trigger,
                intervention_would_help: observation.intervention_would_help,
                disposition,
            }
        })
        .collect::<Vec<_>>();
    results.sort_by(|left, right| left.observation_id.cmp(&right.observation_id));
    let observations = u32::try_from(results.len()).unwrap_or(u32::MAX);
    let true_positives = count(&results, ShadowDisposition::TruePositive);
    let true_negatives = count(&results, ShadowDisposition::TrueNegative);
    let false_positives = count(&results, ShadowDisposition::FalsePositive);
    let false_negatives = count(&results, ShadowDisposition::FalseNegative);
    let triggered = true_positives.saturating_add(false_positives);
    let useful = true_positives.saturating_add(false_negatives);
    let summary = ShadowSummary {
        observations,
        true_positives,
        true_negatives,
        false_positives,
        false_negatives,
        precision_bps: basis_points(true_positives, triggered),
        false_positive_rate_bps: basis_points(false_positives, triggered),
        recall_bps: basis_points(true_positives, useful),
    };
    let policy = ShadowPolicy {
        minimum_observations: MINIMUM_OBSERVATIONS,
        maximum_false_positive_rate_bps: package.promotion.maximum_false_positive_rate_bps,
    };
    let threshold_failures = threshold_failures(&summary, &policy);
    let passed = threshold_failures.is_empty();
    let suite_json = serde_json::to_vec(suite)?;
    let observation_set_hash = prefixed_hash("shh", &suite_json);
    let shadow_id = prefixed_hash(
        "shr",
        format!("shadow/v1\0{}\0{observation_set_hash}", package.mutation_id).as_bytes(),
    );
    Ok(ShadowReport {
        spec_version: ShadowResultSpecVersion::V0_1,
        shadow_id,
        mutation_id: package.mutation_id.clone(),
        mutation_version: package.version.clone(),
        observation_set_hash,
        results,
        summary,
        policy,
        threshold_failures,
        passed,
        mutation_applied: false,
        model_used: false,
    })
}

fn count(results: &[ShadowResult], disposition: ShadowDisposition) -> u32 {
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

fn threshold_failures(
    summary: &ShadowSummary,
    policy: &ShadowPolicy,
) -> Vec<ShadowThresholdFailure> {
    let mut failures = Vec::new();
    if summary.observations < policy.minimum_observations {
        failures.push(ShadowThresholdFailure::InsufficientObservations);
    }
    if summary.true_positives + summary.false_negatives == 0 {
        failures.push(ShadowThresholdFailure::MissingPositiveCoverage);
    }
    if summary.true_negatives + summary.false_positives == 0 {
        failures.push(ShadowThresholdFailure::MissingNegativeCoverage);
    }
    if summary.true_positives + summary.false_positives == 0 {
        failures.push(ShadowThresholdFailure::MissingTriggerCoverage);
    }
    if summary.false_positive_rate_bps > policy.maximum_false_positive_rate_bps {
        failures.push(ShadowThresholdFailure::FalsePositiveRateAboveMaximum);
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

/// Error produced before shadow evaluation completes.
#[derive(Debug, thiserror::Error)]
pub enum ShadowEvaluationError {
    /// The suite violated its semantic contract.
    #[error("invalid shadow suite: {0}")]
    InvalidSuite(#[from] crate::ShadowValidationErrors),
    /// Suite and immutable mutation identities disagreed.
    #[error(
        "shadow suite targets mutation '{suite_mutation_id}', not package '{package_mutation_id}'"
    )]
    MutationMismatch {
        /// Package mutation identity.
        package_mutation_id: String,
        /// Suite mutation identity.
        suite_mutation_id: String,
    },
    /// The suite could not be canonicalized for hashing.
    #[error("could not serialize shadow suite: {0}")]
    Serialization(#[from] serde_json::Error),
}
