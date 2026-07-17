//! Detection diagnostics: the deterministic, model-free explanation a caller
//! can render so a zero-finding scan is never silent.
//!
//! A [`DetectionReport`] pairs the qualified [`EvidencePacket`] findings with a
//! [`DetectionDiagnostics`] summary of the same single pass: how many events and
//! sessions were scanned, how many candidate recurrence signatures were seen,
//! and the top near-threshold [`Observation`]s that recurred but did not qualify
//! (each annotated with the exact gate it missed). Observations are explicitly
//! not findings; they carry no evidence lineage and never feed mutations.

use serde::{Deserialize, Serialize};

use crate::{DetectorKind, RecurrenceScore};

/// The recurrence gate a candidate failed to clear, in evaluation order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnmetGate {
    /// Fewer supporting events than `min_occurrences`.
    MinOccurrences,
    /// Fewer distinct supporting sessions than `min_sessions`.
    MinSessions,
    /// Support share below the optional `min_support_ratio_bps` floor.
    MinSupportRatio,
}

impl UnmetGate {
    /// Stable serialized gate name, matching the CLI threshold flag it maps to.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MinOccurrences => "min_occurrences",
            Self::MinSessions => "min_sessions",
            Self::MinSupportRatio => "min_support_ratio_bps",
        }
    }
}

/// A recurring candidate signature that did not qualify as a finding.
///
/// Observations exist to explain a scan, not to assert one. They expose the same
/// inspectable recurrence statistics a finding would, plus the single gate the
/// candidate missed, but they intentionally omit evidence and counterexample
/// lineage so they can never be mistaken for — or promoted into — a finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// Detector that produced the candidate.
    pub detector: DetectorKind,
    /// Versioned normalized recurrence signature.
    pub signature: String,
    /// Deterministic human-readable label.
    pub title: String,
    /// Inspectable recurrence statistics and score.
    pub score: RecurrenceScore,
    /// The first qualification gate this candidate failed to clear.
    pub unmet_gate: UnmetGate,
}

/// Deterministic summary of a single detection pass.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DetectionDiagnostics {
    /// Events handed to the detectors.
    pub events_scanned: usize,
    /// Distinct sessions represented among the scanned events.
    pub sessions_scanned: usize,
    /// Distinct candidate recurrence signatures seen across all detectors.
    pub candidate_signatures: usize,
    /// Top near-threshold candidates that recurred but did not qualify.
    pub observations: Vec<Observation>,
}

/// Qualified findings paired with the diagnostics from the same pass.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DetectionReport {
    /// Qualified, evidence-linked findings in stable order.
    pub findings: Vec<crate::EvidencePacket>,
    /// Deterministic explanation of the scan.
    pub diagnostics: DetectionDiagnostics,
}
