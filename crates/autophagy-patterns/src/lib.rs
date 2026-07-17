//! Deterministic pattern discovery over validated AEP events.
//!
//! Detectors never call a model. Every finding carries exact supporting and
//! counterexample event identifiers and uses integer-only recurrence scoring.

mod correction;
mod evidence;
mod failure;
mod recovery;
mod report;
mod score;
mod signature;

pub use evidence::{
    DetectorKind, EvidencePacket, EvidenceReference, EvidenceSpecVersion, RecurrenceScore,
};
pub use report::{DetectionDiagnostics, DetectionReport, Observation, UnmetGate};

use std::collections::BTreeSet;

use autophagy_events::Event;

/// Maximum near-threshold observations retained per detection pass.
const OBSERVATION_LIMIT: usize = 5;

/// Version tag for the deterministic detection and signature-normalization
/// contract, folded into the persisted findings-cache key.
///
/// Bump this whenever detector output or the signature normalization in
/// `autophagy-events` changes, so every previously persisted cache entry is
/// automatically treated as stale rather than served from an outdated pass.
///
/// `v2` accompanies the `v2` signature grammar: detectors now group operations
/// by their volatile-token-normalized shape (see `autophagy_events::signature`
/// and ADR 0014), so every cache entry minted under the `v1` grammar must miss.
pub const DETECTION_SPEC_VERSION: &str = "detection/v2";

/// Thresholds shared by deterministic recurrence detectors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DetectorConfig {
    /// Minimum supporting events required.
    pub min_occurrences: u32,
    /// Minimum distinct sessions containing support.
    pub min_sessions: u32,
    /// Optional anti-noise floor on the support share among support and
    /// counterexamples, in basis points.
    ///
    /// This is **not** a majority-failure gate. Qualification is decided by
    /// recurrence — [`min_occurrences`](Self::min_occurrences) across
    /// [`min_sessions`](Self::min_sessions) — because a repeated failure wastes
    /// effort regardless of how often the same operation also succeeds. The
    /// ratio stays a reported, inspectable score component; setting this floor
    /// above zero only suppresses candidates whose failure share is vanishingly
    /// small. It defaults to zero (disabled).
    pub min_support_ratio_bps: u16,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            min_occurrences: 3,
            min_sessions: 2,
            min_support_ratio_bps: 0,
        }
    }
}

/// A recurrence candidate a detector considered, whether or not it qualified.
///
/// Candidates back both the qualified findings and the diagnostic observations
/// so a single detection pass explains itself without a second scan.
pub(crate) struct Candidate {
    detector: DetectorKind,
    signature: String,
    title: String,
    score: RecurrenceScore,
}

/// Run every Milestone 1 deterministic detector and return stable ordering.
#[must_use]
pub fn detect(events: &[Event], config: DetectorConfig) -> Vec<EvidencePacket> {
    detect_with_report(events, config).findings
}

/// Run every deterministic detector and return findings alongside the
/// deterministic diagnostics from the same pass.
///
/// The diagnostics make a zero-finding scan explainable: they report the scan
/// size, the number of candidate recurrence signatures seen, and the top
/// near-threshold observations with the exact gate each missed. No detection
/// work is repeated — findings and diagnostics are produced together.
#[must_use]
pub fn detect_with_report(events: &[Event], config: DetectorConfig) -> DetectionReport {
    let mut findings = Vec::new();
    let mut candidates = Vec::new();
    for (detected, seen) in [
        failure::detect(events, config),
        correction::detect(events, config),
        recovery::detect(events, config),
    ] {
        findings.extend(detected);
        candidates.extend(seen);
    }
    findings.sort_by(|left, right| left.finding_id.cmp(&right.finding_id));

    let sessions_scanned = events
        .iter()
        .map(|event| event.session_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let candidate_signatures = candidates.len();
    let observations = observations(candidates, config);

    DetectionReport {
        findings,
        diagnostics: DetectionDiagnostics {
            events_scanned: events.len(),
            sessions_scanned,
            candidate_signatures,
            observations,
        },
    }
}

/// Rank the near-threshold candidates and keep the strongest few.
///
/// Only candidates that did not qualify become observations; qualified
/// candidates are already reported as findings. Ordering is deterministic:
/// most occurrences first, then most sessions, then highest score, then
/// signature, so the output is stable across event orderings.
fn observations(candidates: Vec<Candidate>, config: DetectorConfig) -> Vec<Observation> {
    let mut observations = candidates
        .into_iter()
        .filter_map(|candidate| {
            score::unmet_gate(&candidate.score, config).map(|unmet_gate| Observation {
                detector: candidate.detector,
                signature: candidate.signature,
                title: candidate.title,
                score: candidate.score,
                unmet_gate,
            })
        })
        .collect::<Vec<_>>();
    observations.sort_by(|left, right| {
        right
            .score
            .occurrences
            .cmp(&left.score.occurrences)
            .then_with(|| {
                right
                    .score
                    .distinct_sessions
                    .cmp(&left.score.distinct_sessions)
            })
            .then_with(|| right.score.score_bps.cmp(&left.score.score_bps))
            .then_with(|| left.signature.cmp(&right.signature))
    });
    observations.truncate(OBSERVATION_LIMIT);
    observations
}
