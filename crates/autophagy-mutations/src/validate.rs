use std::collections::BTreeSet;

use semver::Version;

use crate::{InterventionKind, LifecycleState, MutationPackage};

/// One stable mutation contract violation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationValidationError {
    /// Field path containing the error.
    pub path: String,
    /// Stable machine-readable error category.
    pub code: &'static str,
    /// Human-readable detail.
    pub message: String,
}

/// Complete validation result for one package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationValidationErrors(Vec<MutationValidationError>);

impl MutationValidationErrors {
    /// Iterate over violations in validation order.
    pub fn iter(&self) -> impl Iterator<Item = &MutationValidationError> {
        self.0.iter()
    }
}

impl std::fmt::Display for MutationValidationErrors {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} mutation validation error(s)", self.0.len())
    }
}

impl std::error::Error for MutationValidationErrors {}

#[allow(clippy::too_many_lines)]
pub(crate) fn validate(package: &MutationPackage) -> Result<(), MutationValidationErrors> {
    let mut errors = Vec::new();
    validate_id(&package.mutation_id, "mut_", "mutation_id", &mut errors);
    validate_id(
        &package.source_finding_id,
        "fnd_",
        "source_finding_id",
        &mut errors,
    );
    if Version::parse(&package.version).is_err() {
        push(
            &mut errors,
            "version",
            "invalid_semver",
            "must be valid semantic versioning",
        );
    }
    if package.state != LifecycleState::Candidate {
        push(
            &mut errors,
            "state",
            "generation_state",
            "new packages must be candidates",
        );
    }
    nonblank(&package.title, "title", &mut errors);
    nonblank(
        &package.hypothesis.statement,
        "hypothesis.statement",
        &mut errors,
    );
    nonblank(
        &package.hypothesis.expected_result,
        "hypothesis.expected_result",
        &mut errors,
    );
    nonblank(
        &package.intervention.instruction,
        "intervention.instruction",
        &mut errors,
    );
    if package.triggers.is_empty() {
        push(
            &mut errors,
            "triggers",
            "required",
            "at least one trigger is required",
        );
    }
    for (index, trigger) in package.triggers.iter().enumerate() {
        nonblank(
            &trigger.selector,
            &format!("triggers[{index}].selector"),
            &mut errors,
        );
    }
    if package.hypothesis.supporting_event_ids.len() < 2 {
        push(
            &mut errors,
            "hypothesis.supporting_event_ids",
            "insufficient_evidence",
            "at least two supporting events are required",
        );
    }
    let support = package
        .hypothesis
        .supporting_event_ids
        .iter()
        .collect::<BTreeSet<_>>();
    for (index, event_id) in package.hypothesis.supporting_event_ids.iter().enumerate() {
        validate_id(
            event_id,
            "evt_",
            &format!("hypothesis.supporting_event_ids[{index}]"),
            &mut errors,
        );
    }
    if support.len() != package.hypothesis.supporting_event_ids.len() {
        push(
            &mut errors,
            "hypothesis.supporting_event_ids",
            "duplicate",
            "supporting event IDs must be unique",
        );
    }
    if package
        .hypothesis
        .counterexample_event_ids
        .iter()
        .any(|event_id| support.contains(event_id))
    {
        push(
            &mut errors,
            "hypothesis.counterexample_event_ids",
            "overlap",
            "counterexamples cannot also be supporting evidence",
        );
    }
    let counterexamples = package
        .hypothesis
        .counterexample_event_ids
        .iter()
        .collect::<BTreeSet<_>>();
    if counterexamples.len() != package.hypothesis.counterexample_event_ids.len() {
        push(
            &mut errors,
            "hypothesis.counterexample_event_ids",
            "duplicate",
            "counterexample event IDs must be unique",
        );
    }
    for (index, event_id) in package
        .hypothesis
        .counterexample_event_ids
        .iter()
        .enumerate()
    {
        validate_id(
            event_id,
            "evt_",
            &format!("hypothesis.counterexample_event_ids[{index}]"),
            &mut errors,
        );
    }
    if package.hypothesis.failure_cases.is_empty() {
        push(
            &mut errors,
            "hypothesis.failure_cases",
            "required",
            "at least one falsification or harm case is required",
        );
    }
    for (index, exclusion) in package.exclusions.iter().enumerate() {
        nonblank(exclusion, &format!("exclusions[{index}]"), &mut errors);
    }
    if package.intervention.kind == InterventionKind::AgentInstruction
        && (!package.permissions.filesystem_read.is_empty()
            || !package.permissions.filesystem_write.is_empty()
            || !package.permissions.commands.is_empty()
            || !package.permissions.environment.is_empty()
            || package.permissions.network)
    {
        push(
            &mut errors,
            "permissions",
            "excessive",
            "review-only agent instructions must request no capabilities",
        );
    }
    if package.promotion.minimum_replays == 0 {
        push(
            &mut errors,
            "promotion.minimum_replays",
            "minimum",
            "must be greater than zero",
        );
    }
    if package.promotion.minimum_success_rate_bps > 10_000
        || package.promotion.maximum_false_positive_rate_bps > 10_000
    {
        push(
            &mut errors,
            "promotion",
            "basis_points",
            "rates must be between 0 and 10000",
        );
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(MutationValidationErrors(errors))
    }
}

fn validate_id(value: &str, prefix: &str, path: &str, errors: &mut Vec<MutationValidationError>) {
    let valid = value.strip_prefix(prefix).is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.len() <= 127
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    });
    if !valid {
        push(
            errors,
            path,
            "invalid_id",
            &format!("must start with {prefix} and use safe ID characters"),
        );
    }
}

fn nonblank(value: &str, path: &str, errors: &mut Vec<MutationValidationError>) {
    if value.trim().is_empty() {
        push(errors, path, "blank", "must not be blank");
    }
}

fn push(errors: &mut Vec<MutationValidationError>, path: &str, code: &'static str, message: &str) {
    errors.push(MutationValidationError {
        path: path.to_owned(),
        code,
        message: message.to_owned(),
    });
}
