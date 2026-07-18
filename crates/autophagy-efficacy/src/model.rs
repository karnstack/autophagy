use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::EfficacyValidationErrors;

/// Efficacy report wire-format version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum EfficacyResultSpecVersion {
    /// Initial deterministic recurrence-efficacy report.
    #[serde(rename = "efficacy/0.1")]
    V0_1,
}

/// The deterministic rule used to count failure occurrences for a mutation.
///
/// A mutation's trigger selector is a versioned failure signature
/// (`failure/<v>|<tool>|<command>|exit:<code>`). The store decomposes it into
/// the outcome-independent operation signature (`operation/<v>|<tool>|<command>`)
/// plus an exit code, then counts the `tool.failed` events indexed under that
/// operation signature whose exit code matches. Operation-form selectors
/// (`operation/<v>|…`) match any indexed `tool.failed` event for that operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchingRule {
    /// Failure-signature recurrence over the exact-signature index.
    FailureSignatureRecurrence,
}

/// One failure occurrence counted from the local event store. Input only; never
/// serialized into the report (the report cites `event_id`s under
/// [`Evidence`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailureOccurrence {
    /// Exact AEP event identity.
    pub event_id: String,
    /// Session the failure occurred in.
    pub session_id: String,
    /// Canonical event timestamp.
    pub occurred_at: OffsetDateTime,
}

/// Raw index-coverage counts over the full evaluated span, gathered by the store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoverageInput {
    /// `tool.failed` events in the span that carry an exact-signature index row
    /// (and are therefore classifiable by signature).
    pub classifiable_failures: u32,
    /// All `tool.failed` events in the span, indexed or not.
    pub total_failures: u32,
}

/// Everything [`evaluate`](crate::evaluate) needs, handed in by the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EfficacyObservations {
    /// Installed mutation identity.
    pub mutation_id: String,
    /// Immutable mutation semantic version.
    pub mutation_version: String,
    /// Immutable trigger selectors the recurrence is measured against.
    pub signature_selectors: Vec<String>,
    /// The rule the store applied to count occurrences.
    pub matching_rule: MatchingRule,
    /// Every matched failure occurrence within the full evaluated span.
    pub occurrences: Vec<FailureOccurrence>,
    /// Index-coverage diagnostics over the span.
    pub coverage: CoverageInput,
}

impl EfficacyObservations {
    /// Enforce efficacy-observation semantic invariants.
    ///
    /// # Errors
    /// Returns every invalid field and cross-field relationship.
    pub fn validate(&self) -> Result<(), EfficacyValidationErrors> {
        crate::validate::validate(self)
    }
}

/// Integer-only recurrence measurements for one window.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WindowStats {
    /// Inclusive window start (RFC 3339 UTC).
    pub start: String,
    /// Window end (RFC 3339 UTC): the pre-window ends at `installed_at`, the
    /// post-window ends at the evaluation clock.
    pub end: String,
    /// Window length in whole seconds.
    pub duration_seconds: i64,
    /// Matched failure occurrences in the window.
    pub occurrences: u32,
    /// Distinct sessions those occurrences fell across.
    pub distinct_sessions: u32,
    /// Occurrences per week, scaled by 1000 (milli-occurrences per week) to stay
    /// integer and deterministic. `1810` means `1.81` failures per week.
    pub rate_per_week_milli: i64,
}

/// The symmetric pre/post comparison windows.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EfficacyWindows {
    /// Equal-length window immediately before `installed_at`.
    pub pre: WindowStats,
    /// Observation window since `installed_at`.
    pub post: WindowStats,
}

/// Exact-signature index coverage for the evaluated span.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    /// `tool.failed` events classifiable by signature.
    pub classifiable_failures: u32,
    /// All `tool.failed` events in the span.
    pub total_failures: u32,
    /// Classifiable fraction in basis points (`10000` = fully indexed).
    pub coverage_bps: u32,
    /// Whether every in-span failure was classifiable.
    pub complete: bool,
}

/// Exact evidence identifiers for the counted occurrences.
///
/// Counts are always exact; the listed identifiers are capped at
/// `listing_cap` so a pathological corpus cannot produce an unbounded report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    /// Up to `listing_cap` pre-window occurrence event IDs, in canonical order.
    pub pre_event_ids: Vec<String>,
    /// Up to `listing_cap` post-window occurrence event IDs, in canonical order.
    pub post_event_ids: Vec<String>,
    /// Exact pre-window occurrence count (never capped).
    pub pre_event_count: u32,
    /// Exact post-window occurrence count (never capped).
    pub post_event_count: u32,
    /// Maximum identifiers listed per window.
    pub listing_cap: u32,
}

/// Deterministic efficacy verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The signature recurs meaningfully less after install.
    Improved,
    /// The signature recurs meaningfully more after install.
    Regressed,
    /// The recurrence rate is within the neutral band.
    Unchanged,
    /// Too little observation, evidence, or index coverage to judge.
    InsufficientData,
}

/// Why a verdict is [`Verdict::InsufficientData`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsufficientReason {
    /// The post-install observation window is shorter than the minimum.
    PostWindowTooShort,
    /// Too few total occurrences across both windows to distinguish signal.
    SparseOccurrences,
    /// Too many in-span failures lack an index row to trust the counts.
    PartialIndexCoverage,
}

/// Complete deterministic post-install efficacy report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EfficacyReport {
    /// Report contract version.
    pub spec_version: EfficacyResultSpecVersion,
    /// Stable content-derived report identity (`eff_<sha256-hex>`).
    pub efficacy_id: String,
    /// Installed mutation identity.
    pub mutation_id: String,
    /// Immutable mutation semantic version.
    pub mutation_version: String,
    /// Trigger selectors the recurrence was measured against, in canonical order.
    pub signature_selectors: Vec<String>,
    /// Rule used to count occurrences.
    pub matching_rule: MatchingRule,
    /// Install timestamp anchoring the windows (RFC 3339 UTC).
    pub installed_at: String,
    /// Evaluation clock echoed for reproducibility (RFC 3339 UTC).
    pub evaluated_at: String,
    /// The symmetric pre/post windows and their measurements.
    pub windows: EfficacyWindows,
    /// Signed relative change in weekly rate, in basis points, when a nonzero
    /// baseline exists (`-3300` = a 33% reduction). Absent when the pre-window
    /// held no occurrences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_delta_bps: Option<i32>,
    /// Index-coverage diagnostics.
    pub coverage: Coverage,
    /// Deterministic verdict.
    pub verdict: Verdict,
    /// Every reason the verdict is insufficient; empty otherwise.
    pub insufficient_reasons: Vec<InsufficientReason>,
    /// Exact evidence identifiers counted in each window.
    pub evidence: Evidence,
    /// Always false: efficacy is model-free.
    pub model_used: bool,
}
