use std::{collections::BTreeSet, fmt};

use crate::ShadowSuite;

/// One shadow-suite semantic violation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShadowValidationError {
    /// Stable machine-readable code.
    pub code: &'static str,
    /// Field path containing the violation.
    pub field: String,
    /// Human-readable explanation.
    pub message: String,
}

/// All semantic violations found in a shadow suite.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShadowValidationErrors(Vec<ShadowValidationError>);

impl ShadowValidationErrors {
    /// Iterate over every violation.
    pub fn iter(&self) -> impl Iterator<Item = &ShadowValidationError> {
        self.0.iter()
    }
}

impl fmt::Display for ShadowValidationErrors {
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

impl std::error::Error for ShadowValidationErrors {}

pub(crate) fn validate(suite: &ShadowSuite) -> Result<(), ShadowValidationErrors> {
    let mut errors = Vec::new();
    if !valid_id(&suite.mutation_id, "mut_") {
        push(&mut errors, "id", "mutation_id", "must begin with mut_");
    }
    if suite.observations.is_empty() {
        push(&mut errors, "required", "observations", "must not be empty");
    }
    let mut observation_ids = BTreeSet::new();
    let mut source_event_ids = BTreeSet::new();
    for (index, observation) in suite.observations.iter().enumerate() {
        let prefix = format!("observations[{index}]");
        if !valid_id(&observation.observation_id, "shd_") {
            push(
                &mut errors,
                "id",
                format!("{prefix}.observation_id"),
                "must begin with shd_",
            );
        } else if !observation_ids.insert(&observation.observation_id) {
            push(
                &mut errors,
                "duplicate",
                format!("{prefix}.observation_id"),
                "must be unique within the suite",
            );
        }
        validate_values(
            &mut errors,
            &format!("{prefix}.source_event_ids"),
            &observation.source_event_ids,
            Some("evt_"),
        );
        for (event_index, event_id) in observation.source_event_ids.iter().enumerate() {
            if !source_event_ids.insert(event_id) {
                push(
                    &mut errors,
                    "duplicate",
                    format!("{prefix}.source_event_ids[{event_index}]"),
                    "must not be reused by another observation",
                );
            }
        }
        validate_values(
            &mut errors,
            &format!("{prefix}.observed_trigger_selectors"),
            &observation.observed_trigger_selectors,
            None,
        );
        if observation
            .note
            .as_ref()
            .is_some_and(|note| note.trim().is_empty())
        {
            push(
                &mut errors,
                "invalid",
                format!("{prefix}.note"),
                "must not be blank",
            );
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ShadowValidationErrors(errors))
    }
}

fn validate_values(
    errors: &mut Vec<ShadowValidationError>,
    field: &str,
    values: &[String],
    prefix: Option<&str>,
) {
    if values.is_empty() {
        push(errors, "required", field, "must not be empty");
        return;
    }
    let mut seen = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        if value.trim().is_empty() || prefix.is_some_and(|prefix| !value.starts_with(prefix)) {
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
    errors: &mut Vec<ShadowValidationError>,
    code: &'static str,
    field: impl Into<String>,
    message: impl Into<String>,
) {
    errors.push(ShadowValidationError {
        code,
        field: field.into(),
        message: message.into(),
    });
}
