use std::{collections::BTreeSet, fmt};

use crate::EfficacyObservations;

/// One efficacy-observation semantic violation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EfficacyValidationError {
    /// Stable machine-readable code.
    pub code: &'static str,
    /// Field path containing the violation.
    pub field: String,
    /// Human-readable explanation.
    pub message: String,
}

/// All semantic violations found in an efficacy observation set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EfficacyValidationErrors(Vec<EfficacyValidationError>);

impl EfficacyValidationErrors {
    /// Iterate over every violation.
    pub fn iter(&self) -> impl Iterator<Item = &EfficacyValidationError> {
        self.0.iter()
    }
}

impl fmt::Display for EfficacyValidationErrors {
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

impl std::error::Error for EfficacyValidationErrors {}

pub(crate) fn validate(
    observations: &EfficacyObservations,
) -> Result<(), EfficacyValidationErrors> {
    let mut errors = Vec::new();
    if !valid_id(&observations.mutation_id, "mut_") {
        push(&mut errors, "id", "mutation_id", "must begin with mut_");
    }
    if observations.mutation_version.trim().is_empty() {
        push(
            &mut errors,
            "required",
            "mutation_version",
            "must not be blank",
        );
    }
    if observations.signature_selectors.is_empty() {
        push(
            &mut errors,
            "required",
            "signature_selectors",
            "must not be empty",
        );
    }
    let mut seen_selectors = BTreeSet::new();
    for (index, selector) in observations.signature_selectors.iter().enumerate() {
        if selector.trim().is_empty() {
            push(
                &mut errors,
                "invalid",
                format!("signature_selectors[{index}]"),
                "must not be blank",
            );
        } else if !seen_selectors.insert(selector) {
            push(
                &mut errors,
                "duplicate",
                format!("signature_selectors[{index}]"),
                "must be unique",
            );
        }
    }
    let mut seen_events = BTreeSet::new();
    for (index, occurrence) in observations.occurrences.iter().enumerate() {
        let prefix = format!("occurrences[{index}]");
        if !valid_id(&occurrence.event_id, "evt_") {
            push(
                &mut errors,
                "id",
                format!("{prefix}.event_id"),
                "must begin with evt_",
            );
        } else if !seen_events.insert(&occurrence.event_id) {
            push(
                &mut errors,
                "duplicate",
                format!("{prefix}.event_id"),
                "must not be counted twice",
            );
        }
        if occurrence.session_id.trim().is_empty() {
            push(
                &mut errors,
                "required",
                format!("{prefix}.session_id"),
                "must not be blank",
            );
        }
    }
    if observations.coverage.classifiable_failures > observations.coverage.total_failures {
        push(
            &mut errors,
            "invalid",
            "coverage.classifiable_failures",
            "must not exceed total_failures",
        );
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(EfficacyValidationErrors(errors))
    }
}

fn valid_id(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix) && value.len() > prefix.len()
}

fn push(
    errors: &mut Vec<EfficacyValidationError>,
    code: &'static str,
    field: impl Into<String>,
    message: impl Into<String>,
) {
    errors.push(EfficacyValidationError {
        code,
        field: field.into(),
        message: message.into(),
    });
}
