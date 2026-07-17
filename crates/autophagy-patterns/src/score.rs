use std::collections::BTreeSet;

use autophagy_events::Event;

use crate::{DetectorConfig, RecurrenceScore, UnmetGate};

pub(crate) fn score(evidence: &[&Event], counterexamples: &[&Event]) -> Option<RecurrenceScore> {
    let occurrences = u32::try_from(evidence.len()).ok()?;
    let counterexample_count = u32::try_from(counterexamples.len()).ok()?;
    let distinct_sessions = u32::try_from(
        evidence
            .iter()
            .map(|event| event.session_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
    )
    .ok()?;
    let total = occurrences.saturating_add(counterexample_count);
    let support_ratio_bps = if total == 0 {
        0
    } else {
        u16::try_from((u64::from(occurrences) * 10_000) / u64::from(total)).unwrap_or(10_000)
    };
    let occurrence_component = occurrences.saturating_sub(1).min(6) * 500;
    let session_component = distinct_sessions.saturating_sub(1).min(4) * 750;
    let score = (u32::from(support_ratio_bps) * 6 / 10)
        .saturating_add(occurrence_component)
        .saturating_add(session_component)
        .min(10_000);
    Some(RecurrenceScore {
        occurrences,
        distinct_sessions,
        counterexamples: counterexample_count,
        support_ratio_bps,
        score_bps: u16::try_from(score).unwrap_or(10_000),
    })
}

pub(crate) const fn qualifies(score: &RecurrenceScore, config: DetectorConfig) -> bool {
    unmet_gate(score, config).is_none()
}

/// The first qualification gate `score` fails to clear under `config`, or
/// `None` when the score qualifies as a finding.
///
/// Gates are evaluated in a fixed order — occurrences, then distinct sessions,
/// then the optional support-ratio floor — so a candidate is always attributed
/// to a single, stable reason. Recurrence (occurrences across distinct sessions)
/// is the qualification signal; the support ratio is an *optional* anti-noise
/// floor that defaults to zero and never gates on the operation's overall
/// failure share.
pub(crate) const fn unmet_gate(
    score: &RecurrenceScore,
    config: DetectorConfig,
) -> Option<UnmetGate> {
    if score.occurrences < config.min_occurrences {
        Some(UnmetGate::MinOccurrences)
    } else if score.distinct_sessions < config.min_sessions {
        Some(UnmetGate::MinSessions)
    } else if score.support_ratio_bps < config.min_support_ratio_bps {
        Some(UnmetGate::MinSupportRatio)
    } else {
        None
    }
}
