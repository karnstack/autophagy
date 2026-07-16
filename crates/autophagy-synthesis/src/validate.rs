//! Deterministic validation of provider responses.
//!
//! Nothing a provider returns is trusted. Cited evidence must exist in the
//! packet the provider was given, trigger selectors must come from the
//! deterministic template, requested permissions may never exceed the ceiling,
//! and every text field must be present. A violation produces a structured
//! diagnostic; the response is rejected, never repaired.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::provider::{SynthesisRequest, SynthesisResponse};

/// One stable synthesis-boundary violation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SynthesisDiagnostic {
    /// Field path containing the violation.
    pub path: String,
    /// Stable machine-readable violation category.
    pub code: &'static str,
    /// Human-readable detail.
    pub message: String,
}

/// The complete set of violations for one rejected response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SynthesisRejection(Vec<SynthesisDiagnostic>);

impl SynthesisRejection {
    /// Iterate over violations in detection order.
    pub fn iter(&self) -> impl Iterator<Item = &SynthesisDiagnostic> {
        self.0.iter()
    }

    /// Consume the rejection into its diagnostics.
    #[must_use]
    pub fn into_diagnostics(self) -> Vec<SynthesisDiagnostic> {
        self.0
    }
}

impl std::fmt::Display for SynthesisRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} synthesis rejection(s)", self.0.len())
    }
}

impl std::error::Error for SynthesisRejection {}

/// Validate a provider response against the constraints it was given.
///
/// # Errors
/// Returns every detected violation as a [`SynthesisRejection`].
pub(crate) fn validate_response(
    request: &SynthesisRequest,
    response: &SynthesisResponse,
) -> Result<(), SynthesisRejection> {
    let mut errors = Vec::new();
    let constraints = &request.constraints;

    nonblank(&response.title, "title", &mut errors);
    nonblank(&response.statement, "statement", &mut errors);
    nonblank(&response.expected_result, "expected_result", &mut errors);
    nonblank(&response.instruction, "instruction", &mut errors);

    if response.failure_cases.is_empty() {
        push(
            &mut errors,
            "failure_cases",
            "required",
            "at least one falsification or harm case is required",
        );
    }
    for (index, case) in response.failure_cases.iter().enumerate() {
        nonblank(case, &format!("failure_cases[{index}]"), &mut errors);
    }
    for (index, exclusion) in response.exclusions.iter().enumerate() {
        nonblank(exclusion, &format!("exclusions[{index}]"), &mut errors);
    }

    validate_evidence(
        &response.supporting_event_ids,
        &constraints.allowed_supporting_event_ids,
        "supporting_event_ids",
        true,
        &mut errors,
    );
    validate_evidence(
        &response.counterexample_event_ids,
        &constraints.allowed_counterexample_event_ids,
        "counterexample_event_ids",
        false,
        &mut errors,
    );

    let support: BTreeSet<&String> = response.supporting_event_ids.iter().collect();
    if response
        .counterexample_event_ids
        .iter()
        .any(|event_id| support.contains(event_id))
    {
        push(
            &mut errors,
            "counterexample_event_ids",
            "overlap",
            "counterexamples cannot also be supporting evidence",
        );
    }

    validate_triggers(request, response, &mut errors);
    validate_permissions(request, response, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(SynthesisRejection(errors))
    }
}

fn validate_evidence(
    cited: &[String],
    allowed: &[String],
    path: &str,
    require_minimum: bool,
    errors: &mut Vec<SynthesisDiagnostic>,
) {
    let allowed_set: BTreeSet<&String> = allowed.iter().collect();
    let mut seen = BTreeSet::new();
    for (index, event_id) in cited.iter().enumerate() {
        if !allowed_set.contains(event_id) {
            push(
                errors,
                &format!("{path}[{index}]"),
                "unknown_evidence",
                &format!(
                    "event '{event_id}' was not in the evidence packet the provider was given"
                ),
            );
        }
        if !seen.insert(event_id) {
            push(
                errors,
                &format!("{path}[{index}]"),
                "duplicate",
                &format!("event '{event_id}' is cited more than once"),
            );
        }
    }
    if require_minimum && cited.len() < 2 {
        push(
            errors,
            path,
            "insufficient_evidence",
            "at least two supporting events are required",
        );
    }
}

fn validate_triggers(
    request: &SynthesisRequest,
    response: &SynthesisResponse,
    errors: &mut Vec<SynthesisDiagnostic>,
) {
    if response.trigger_selectors.is_empty() {
        push(
            errors,
            "trigger_selectors",
            "required",
            "at least one trigger selector is required",
        );
    }
    let allowed: BTreeSet<&String> = request
        .constraints
        .allowed_trigger_selectors
        .iter()
        .collect();
    for (index, selector) in response.trigger_selectors.iter().enumerate() {
        if !allowed.contains(selector) {
            push(
                errors,
                &format!("trigger_selectors[{index}]"),
                "unknown_selector",
                &format!("selector '{selector}' was not derived from the deterministic template"),
            );
        }
    }
}

fn validate_permissions(
    request: &SynthesisRequest,
    response: &SynthesisResponse,
    errors: &mut Vec<SynthesisDiagnostic>,
) {
    let ceiling = &request.constraints.permission_ceiling;
    exceeds(
        &response.permissions.filesystem_read,
        &ceiling.filesystem_read,
        "permissions.filesystem_read",
        errors,
    );
    exceeds(
        &response.permissions.filesystem_write,
        &ceiling.filesystem_write,
        "permissions.filesystem_write",
        errors,
    );
    exceeds(
        &response.permissions.commands,
        &ceiling.commands,
        "permissions.commands",
        errors,
    );
    exceeds(
        &response.permissions.environment,
        &ceiling.environment,
        "permissions.environment",
        errors,
    );
    if response.permissions.network && !ceiling.network {
        push(
            errors,
            "permissions.network",
            "excessive_permission",
            "network access exceeds the deterministic template ceiling",
        );
    }
}

fn exceeds(
    requested: &[String],
    ceiling: &[String],
    path: &str,
    errors: &mut Vec<SynthesisDiagnostic>,
) {
    let allowed: BTreeSet<&String> = ceiling.iter().collect();
    for (index, entry) in requested.iter().enumerate() {
        if !allowed.contains(entry) {
            push(
                errors,
                &format!("{path}[{index}]"),
                "excessive_permission",
                &format!("'{entry}' exceeds the deterministic template permission ceiling"),
            );
        }
    }
}

fn nonblank(value: &str, path: &str, errors: &mut Vec<SynthesisDiagnostic>) {
    if value.trim().is_empty() {
        push(errors, path, "blank", "must not be blank");
    }
}

fn push(errors: &mut Vec<SynthesisDiagnostic>, path: &str, code: &'static str, message: &str) {
    errors.push(SynthesisDiagnostic {
        path: path.to_owned(),
        code,
        message: message.to_owned(),
    });
}
