use std::{collections::BTreeSet, fmt};

use crate::{ExpectedAction, ReplaySuite};

/// One replay-suite semantic violation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayValidationError {
    /// Stable machine-readable code.
    pub code: &'static str,
    /// Field path containing the violation.
    pub field: String,
    /// Human-readable explanation.
    pub message: String,
}

/// All semantic violations found in a replay suite.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayValidationErrors(Vec<ReplayValidationError>);

impl ReplayValidationErrors {
    /// Iterate over all violations.
    pub fn iter(&self) -> impl Iterator<Item = &ReplayValidationError> {
        self.0.iter()
    }
}

impl fmt::Display for ReplayValidationErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            write!(
                formatter,
                "{} [{}]: {}",
                error.field, error.code, error.message
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for ReplayValidationErrors {}

pub(crate) fn validate(suite: &ReplaySuite) -> Result<(), ReplayValidationErrors> {
    let mut errors = Vec::new();
    if !valid_id(&suite.mutation_id, "mut_") {
        push(&mut errors, "id", "mutation_id", "must begin with mut_");
    }
    if suite.scenarios.is_empty() {
        push(&mut errors, "required", "scenarios", "must not be empty");
    }
    let mut scenario_ids = BTreeSet::new();
    let mut source_event_ids = BTreeSet::new();
    for (index, scenario) in suite.scenarios.iter().enumerate() {
        let prefix = format!("scenarios[{index}]");
        if !valid_id(&scenario.scenario_id, "rps_") {
            push(
                &mut errors,
                "id",
                format!("{prefix}.scenario_id"),
                "must begin with rps_",
            );
        } else if !scenario_ids.insert(&scenario.scenario_id) {
            push(
                &mut errors,
                "duplicate",
                format!("{prefix}.scenario_id"),
                "must be unique within the suite",
            );
        }
        validate_nonempty_unique(
            &mut errors,
            &format!("{prefix}.source_event_ids"),
            &scenario.source_event_ids,
            Some("evt_"),
        );
        for (event_index, event_id) in scenario.source_event_ids.iter().enumerate() {
            if !source_event_ids.insert(event_id) {
                push(
                    &mut errors,
                    "duplicate",
                    format!("{prefix}.source_event_ids[{event_index}]"),
                    "must not be reused by another scenario",
                );
            }
        }
        validate_nonempty_unique(
            &mut errors,
            &format!("{prefix}.observed_trigger_selectors"),
            &scenario.observed_trigger_selectors,
            None,
        );
        match (scenario.expected_action, scenario.counterfactual_outcome) {
            (ExpectedAction::Intervene, None) => push(
                &mut errors,
                "required",
                format!("{prefix}.counterfactual_outcome"),
                "is required when expected_action is intervene",
            ),
            (ExpectedAction::NoOp, Some(_)) => push(
                &mut errors,
                "forbidden",
                format!("{prefix}.counterfactual_outcome"),
                "must be absent when expected_action is no_op",
            ),
            _ => {}
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ReplayValidationErrors(errors))
    }
}

fn validate_nonempty_unique(
    errors: &mut Vec<ReplayValidationError>,
    field: &str,
    values: &[String],
    required_prefix: Option<&str>,
) {
    if values.is_empty() {
        push(errors, "required", field, "must not be empty");
        return;
    }
    let mut seen = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        if value.trim().is_empty()
            || required_prefix.is_some_and(|prefix| !value.starts_with(prefix))
        {
            push(
                errors,
                "invalid",
                format!("{field}[{index}]"),
                "has an invalid value",
            );
        } else if !seen.insert(value) {
            push(
                errors,
                "duplicate",
                format!("{field}[{index}]"),
                "must be unique",
            );
        }
    }
}

fn valid_id(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix) && value.len() > prefix.len()
}

fn push(
    errors: &mut Vec<ReplayValidationError>,
    code: &'static str,
    field: impl Into<String>,
    message: impl Into<String>,
) {
    errors.push(ReplayValidationError {
        code,
        field: field.into(),
        message: message.into(),
    });
}
