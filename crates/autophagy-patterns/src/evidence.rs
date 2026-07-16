use autophagy_events::Event;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Evidence packet wire-format version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum EvidenceSpecVersion {
    /// Initial evidence packet contract.
    #[serde(rename = "evidence/0.1")]
    V0_1,
}

/// Deterministic detector that produced a finding.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectorKind {
    /// The same normalized command failed recurrently.
    RepeatedCommandFailure,
    /// The same explicitly classified user correction recurred.
    RepeatedUserCorrection,
}

impl DetectorKind {
    /// Stable serialized detector name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RepeatedCommandFailure => "repeated_command_failure",
            Self::RepeatedUserCorrection => "repeated_user_correction",
        }
    }
}

/// Exact AEP event cited as support or a counterexample.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidenceReference {
    /// Stable event evidence identifier.
    pub event_id: String,
    /// Source session containing the event.
    pub session_id: String,
    /// Canonical occurrence timestamp.
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    /// AEP event kind.
    pub event_type: String,
    /// Policy-processed project path, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl EvidenceReference {
    pub(crate) fn from_event(event: &Event) -> Self {
        Self {
            event_id: event.event_id.as_str().to_owned(),
            session_id: event.session_id.as_str().to_owned(),
            timestamp: event.timestamp,
            event_type: event.kind.as_str().to_owned(),
            project: event.project.clone(),
        }
    }
}

/// Inspectable integer-only recurrence score.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecurrenceScore {
    /// Supporting events.
    pub occurrences: u32,
    /// Distinct sessions containing support.
    pub distinct_sessions: u32,
    /// Events demonstrating the opposite outcome.
    pub counterexamples: u32,
    /// Support share among support and counterexamples, in basis points.
    pub support_ratio_bps: u16,
    /// Overall deterministic score in basis points.
    pub score_bps: u16,
}

/// Versioned, model-free finding with exact evidence lineage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EvidencePacket {
    /// Evidence packet contract version.
    pub spec_version: EvidenceSpecVersion,
    /// Stable content-derived finding identity.
    pub finding_id: String,
    /// Detector implementation that emitted the packet.
    pub detector: DetectorKind,
    /// Versioned normalized recurrence signature.
    pub signature: String,
    /// Deterministic human-readable label.
    pub title: String,
    /// Inspectable recurrence statistics and score.
    pub score: RecurrenceScore,
    /// Supporting events in canonical order.
    pub evidence: Vec<EvidenceReference>,
    /// Opposite-outcome events in canonical order.
    pub counterexamples: Vec<EvidenceReference>,
}
