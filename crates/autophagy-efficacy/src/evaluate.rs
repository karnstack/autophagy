use std::{collections::BTreeSet, fmt::Write as _};

use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    Coverage, EfficacyObservations, EfficacyReport, EfficacyResultSpecVersion, EfficacyWindows,
    Evidence, FailureOccurrence, InsufficientReason, Verdict, WindowStats,
    validate::EfficacyValidationErrors,
};

/// Minimum post-install observation window. A window shorter than this cannot
/// yield a trustworthy recurrence rate, so the verdict is insufficient.
const MIN_POST_WINDOW_SECONDS: i64 = 7 * 86_400;
/// Minimum combined pre+post occurrences before a comparison is meaningful.
const MIN_TOTAL_OCCURRENCES: u32 = 2;
/// Minimum classifiable-failure coverage (basis points) to trust the counts.
const MIN_COVERAGE_BPS: u32 = 5_000;
/// Neutral band (basis points): a weekly-rate change within ±20% is unchanged.
const UNCHANGED_BAND_BPS: i32 = 2_000;
/// Maximum evidence identifiers listed per window (counts stay exact).
const EVIDENCE_LISTING_CAP: u32 = 50;
/// Seconds in one week.
const WEEK_SECONDS: i64 = 604_800;

/// The symmetric pre/post comparison windows, as instants.
///
/// The post-window runs from `installed_at` to the evaluation clock. The
/// pre-window is the equal-length window immediately before `installed_at`, so
/// the two rates are directly comparable. This is the single source of truth for
/// the window math — the caller derives it to bound its store query and
/// [`evaluate`] derives the same bounds to partition the occurrences.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowBounds {
    /// Start of the pre-window (`installed_at - post_duration`).
    pub pre_start: OffsetDateTime,
    /// Boundary between the two windows.
    pub installed_at: OffsetDateTime,
    /// End of the post-window (the evaluation clock).
    pub now: OffsetDateTime,
}

impl WindowBounds {
    /// Derive the symmetric windows from an install timestamp and a clock.
    ///
    /// # Errors
    /// Returns [`EfficacyError::NonMonotonicClock`] when `now` precedes
    /// `installed_at`.
    pub fn derive(
        installed_at: OffsetDateTime,
        now: OffsetDateTime,
    ) -> Result<Self, EfficacyError> {
        if now < installed_at {
            return Err(EfficacyError::NonMonotonicClock {
                installed_at: rfc3339(installed_at),
                evaluated_at: rfc3339(now),
            });
        }
        let post_duration = now - installed_at;
        Ok(Self {
            pre_start: installed_at - post_duration,
            installed_at,
            now,
        })
    }
}

/// Evaluate post-install recurrence efficacy deterministically, without a model.
///
/// # Errors
/// Returns semantic validation failures, a non-monotonic clock, or
/// serialization failures encountered while deriving the content identity.
pub fn evaluate(
    observations: &EfficacyObservations,
    installed_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<EfficacyReport, EfficacyError> {
    observations.validate()?;
    let bounds = WindowBounds::derive(installed_at, now)?;

    let (pre, post) = partition(&observations.occurrences, &bounds);
    let post_duration = (bounds.now - bounds.installed_at).whole_seconds();
    let pre_duration = (bounds.installed_at - bounds.pre_start).whole_seconds();

    let pre_stats = window_stats(&pre, bounds.pre_start, bounds.installed_at, pre_duration);
    let post_stats = window_stats(&post, bounds.installed_at, bounds.now, post_duration);

    let coverage = coverage(observations);
    let insufficient_reasons =
        insufficient_reasons(post_duration, &pre_stats, &post_stats, &coverage);
    let rate_delta_bps = rate_delta_bps(&pre_stats, &post_stats);
    let verdict = verdict(
        &insufficient_reasons,
        &pre_stats,
        &post_stats,
        rate_delta_bps,
    );
    let evidence = evidence(&pre, &post);

    let mut selectors = observations.signature_selectors.clone();
    selectors.sort();
    selectors.dedup();

    let mut report = EfficacyReport {
        spec_version: EfficacyResultSpecVersion::V0_1,
        efficacy_id: String::new(),
        mutation_id: observations.mutation_id.clone(),
        mutation_version: observations.mutation_version.clone(),
        signature_selectors: selectors,
        matching_rule: observations.matching_rule,
        installed_at: rfc3339(bounds.installed_at),
        evaluated_at: rfc3339(bounds.now),
        windows: EfficacyWindows {
            pre: pre_stats,
            post: post_stats,
        },
        rate_delta_bps,
        coverage,
        verdict,
        insufficient_reasons,
        evidence,
        model_used: false,
    };
    let identity = serde_json::to_vec(&report)?;
    report.efficacy_id = prefixed_hash("eff", &identity);
    Ok(report)
}

/// Partition occurrences into the pre-window `[pre_start, installed_at)` and the
/// post-window `[installed_at, now]`, dropping anything outside the span. Each
/// side is returned in canonical `(occurred_at, event_id)` order.
fn partition<'a>(
    occurrences: &'a [FailureOccurrence],
    bounds: &WindowBounds,
) -> (Vec<&'a FailureOccurrence>, Vec<&'a FailureOccurrence>) {
    let mut pre = Vec::new();
    let mut post = Vec::new();
    for occurrence in occurrences {
        let at = occurrence.occurred_at;
        if at >= bounds.installed_at && at <= bounds.now {
            post.push(occurrence);
        } else if at >= bounds.pre_start && at < bounds.installed_at {
            pre.push(occurrence);
        }
    }
    let order = |left: &&FailureOccurrence, right: &&FailureOccurrence| {
        left.occurred_at
            .cmp(&right.occurred_at)
            .then_with(|| left.event_id.cmp(&right.event_id))
    };
    pre.sort_by(order);
    post.sort_by(order);
    (pre, post)
}

fn window_stats(
    occurrences: &[&FailureOccurrence],
    start: OffsetDateTime,
    end: OffsetDateTime,
    duration_seconds: i64,
) -> WindowStats {
    let count = u32::try_from(occurrences.len()).unwrap_or(u32::MAX);
    let distinct_sessions = occurrences
        .iter()
        .map(|occurrence| occurrence.session_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    WindowStats {
        start: rfc3339(start),
        end: rfc3339(end),
        duration_seconds,
        occurrences: count,
        distinct_sessions: u32::try_from(distinct_sessions).unwrap_or(u32::MAX),
        rate_per_week_milli: rate_per_week_milli(count, duration_seconds),
    }
}

/// Occurrences per week scaled by 1000, so the report carries no floats.
fn rate_per_week_milli(occurrences: u32, duration_seconds: i64) -> i64 {
    if duration_seconds <= 0 {
        return 0;
    }
    i64::from(occurrences) * WEEK_SECONDS * 1_000 / duration_seconds
}

fn coverage(observations: &EfficacyObservations) -> Coverage {
    let classifiable = observations.coverage.classifiable_failures;
    let total = observations.coverage.total_failures;
    let coverage_bps = if total == 0 {
        // No failures at all in the span: coverage is trivially complete.
        10_000
    } else {
        u32::try_from(u64::from(classifiable) * 10_000 / u64::from(total)).unwrap_or(10_000)
    };
    Coverage {
        classifiable_failures: classifiable,
        total_failures: total,
        coverage_bps,
        complete: classifiable == total,
    }
}

fn insufficient_reasons(
    post_duration_seconds: i64,
    pre: &WindowStats,
    post: &WindowStats,
    coverage: &Coverage,
) -> Vec<InsufficientReason> {
    let mut reasons = Vec::new();
    if post_duration_seconds < MIN_POST_WINDOW_SECONDS {
        reasons.push(InsufficientReason::PostWindowTooShort);
    }
    if pre.occurrences.saturating_add(post.occurrences) < MIN_TOTAL_OCCURRENCES {
        reasons.push(InsufficientReason::SparseOccurrences);
    }
    if coverage.coverage_bps < MIN_COVERAGE_BPS {
        reasons.push(InsufficientReason::PartialIndexCoverage);
    }
    reasons
}

/// Signed relative change in weekly rate, in basis points, when a nonzero
/// baseline exists. Because the windows are equal-length, this reduces to the
/// relative change in raw counts.
fn rate_delta_bps(pre: &WindowStats, post: &WindowStats) -> Option<i32> {
    if pre.rate_per_week_milli == 0 {
        return None;
    }
    let delta =
        (post.rate_per_week_milli - pre.rate_per_week_milli) * 10_000 / pre.rate_per_week_milli;
    Some(i32::try_from(delta).unwrap_or(if delta < 0 { i32::MIN } else { i32::MAX }))
}

fn verdict(
    insufficient_reasons: &[InsufficientReason],
    pre: &WindowStats,
    post: &WindowStats,
    rate_delta_bps: Option<i32>,
) -> Verdict {
    if !insufficient_reasons.is_empty() {
        return Verdict::InsufficientData;
    }
    match rate_delta_bps {
        // No pre-window baseline, yet enough occurrences survived the sparse
        // gate: the recurrence is new after install.
        None => {
            if post.occurrences > pre.occurrences {
                Verdict::Regressed
            } else {
                Verdict::Unchanged
            }
        }
        Some(delta) if delta <= -UNCHANGED_BAND_BPS => Verdict::Improved,
        Some(delta) if delta >= UNCHANGED_BAND_BPS => Verdict::Regressed,
        Some(_) => Verdict::Unchanged,
    }
}

fn evidence(pre: &[&FailureOccurrence], post: &[&FailureOccurrence]) -> Evidence {
    let cap = EVIDENCE_LISTING_CAP as usize;
    let list = |occurrences: &[&FailureOccurrence]| {
        occurrences
            .iter()
            .take(cap)
            .map(|occurrence| occurrence.event_id.clone())
            .collect::<Vec<_>>()
    };
    Evidence {
        pre_event_ids: list(pre),
        post_event_ids: list(post),
        pre_event_count: u32::try_from(pre.len()).unwrap_or(u32::MAX),
        post_event_count: u32::try_from(post.len()).unwrap_or(u32::MAX),
        listing_cap: EVIDENCE_LISTING_CAP,
    }
}

fn rfc3339(instant: OffsetDateTime) -> String {
    instant
        .to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .unwrap_or_else(|_| String::from("0000-01-01T00:00:00Z"))
}

fn prefixed_hash(prefix: &str, bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("{prefix}_{encoded}")
}

/// Error produced before an efficacy report is complete.
#[derive(Debug, thiserror::Error)]
pub enum EfficacyError {
    /// The observation set violated its semantic contract.
    #[error("invalid efficacy observations: {0}")]
    InvalidObservations(#[from] EfficacyValidationErrors),
    /// The evaluation clock preceded the install timestamp.
    #[error("evaluation clock {evaluated_at} precedes install {installed_at}")]
    NonMonotonicClock {
        /// Install timestamp.
        installed_at: String,
        /// Evaluation clock.
        evaluated_at: String,
    },
    /// The report could not be canonicalized for hashing.
    #[error("could not serialize efficacy report: {0}")]
    Serialization(#[from] serde_json::Error),
}
