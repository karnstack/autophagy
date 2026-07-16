//! Deterministic pattern discovery over validated AEP events.
//!
//! Detectors never call a model. Every finding carries exact supporting and
//! counterexample event identifiers and uses integer-only recurrence scoring.

mod correction;
mod evidence;
mod failure;
mod recovery;
mod score;
mod signature;

pub use evidence::{
    DetectorKind, EvidencePacket, EvidenceReference, EvidenceSpecVersion, RecurrenceScore,
};

use autophagy_events::Event;

/// Thresholds shared by deterministic recurrence detectors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DetectorConfig {
    /// Minimum supporting events required.
    pub min_occurrences: u32,
    /// Minimum distinct sessions containing support.
    pub min_sessions: u32,
    /// Minimum evidence share among support and counterexamples, in basis points.
    pub min_support_ratio_bps: u16,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            min_occurrences: 3,
            min_sessions: 2,
            min_support_ratio_bps: 5_000,
        }
    }
}

/// Run every Milestone 1 deterministic detector and return stable ordering.
#[must_use]
pub fn detect(events: &[Event], config: DetectorConfig) -> Vec<EvidencePacket> {
    let mut findings = failure::detect(events, config);
    findings.extend(correction::detect(events, config));
    findings.extend(recovery::detect(events, config));
    findings.sort_by(|left, right| left.finding_id.cmp(&right.finding_id));
    findings
}
