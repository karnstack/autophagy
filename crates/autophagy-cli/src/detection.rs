//! Persisted detection-findings cache and detection progress reporting.
//!
//! Every findings-consuming command (`patterns`, `digest`, `mutations propose`,
//! `mutations synthesize`, `status --with-findings`) runs the same deterministic
//! detection pass over every stored event. On a large corpus that pass costs
//! tens of seconds, so [`detect_cached`] memoizes the report in the store keyed
//! by a content fingerprint of its exact inputs. An unchanged corpus at
//! unchanged thresholds returns instantly; any change to the events, thresholds,
//! project filter, or detector spec version misses and recomputes. When a fresh
//! pass does run, a concise before/after progress line is written to stderr so
//! the command never appears hung — and stdout stays a clean report.

use std::{io::Write, time::Instant};

use autophagy_patterns::{
    DETECTION_SPEC_VERSION, DetectionReport, DetectorConfig, detect_with_report,
};
use autophagy_store::{DetectionFingerprint, EventStore};
use sha2::{Digest, Sha256};

use crate::CliError;

/// Return the detection report for `project` at `config`, served from the store
/// cache when the corpus and thresholds are unchanged, otherwise computed fresh
/// and cached. Setting `recompute` forces a fresh pass and refreshes the entry.
///
/// The returned report is byte-for-byte identical whether it came from the cache
/// or a fresh pass, so every caller — findings, diagnostics, and the exact
/// evidence identifiers each finding carries — sees the same deterministic
/// result.
///
/// # Errors
///
/// Returns [`CliError`] when the store cannot be queried or a stored event no
/// longer satisfies the AEP contract.
pub(crate) fn detect_cached(
    store: &EventStore,
    project: Option<&str>,
    config: DetectorConfig,
    recompute: bool,
) -> Result<DetectionReport, CliError> {
    let fingerprint = store.detection_fingerprint(project)?;
    let key = cache_key(project, config, &fingerprint);
    if !recompute {
        if let Some(json) = store.cached_findings(&key)? {
            if let Ok(report) = serde_json::from_str::<DetectionReport>(&json) {
                return Ok(report);
            }
            // A payload from an older cache shape that no longer deserializes is
            // ignored and recomputed below rather than surfaced as an error.
        }
    }
    let report = run_with_progress(store, project, config, &fingerprint)?;
    let generation = store.detection_generation()?;
    let json = serde_json::to_string(&report)?;
    store.store_findings(&key, &generation, &json)?;
    Ok(report)
}

/// Run a fresh detection pass, bracketing it with progress lines on stderr.
fn run_with_progress(
    store: &EventStore,
    project: Option<&str>,
    config: DetectorConfig,
    fingerprint: &DetectionFingerprint,
) -> Result<DetectionReport, CliError> {
    // Progress goes to stderr; stdout stays a clean report so JSON output mode is
    // never polluted.
    let mut stderr = std::io::stderr();
    let _ = writeln!(
        stderr,
        "digesting {} events across {} sessions…",
        group(fingerprint.event_count),
        group(fingerprint.session_count),
    );
    let started = Instant::now();
    let events = store.list_events_for_detection(project)?;
    let report = detect_with_report(&events, config);
    let _ = writeln!(
        stderr,
        "digested in {:.1}s — {} finding(s)",
        started.elapsed().as_secs_f64(),
        report.findings.len(),
    );
    Ok(report)
}

/// Derive the 32-byte cache validity key from every input a detection pass
/// depends on: the detector spec version, the effective thresholds, the project
/// filter, and the cheap content fingerprint (event count, max row id, and the
/// monotonic import watermark). A NUL separator keeps field boundaries
/// unambiguous so distinct inputs cannot collide by concatenation.
fn cache_key(
    project: Option<&str>,
    config: DetectorConfig,
    fingerprint: &DetectionFingerprint,
) -> [u8; 32] {
    let min_occurrences = config.min_occurrences.to_string();
    let min_sessions = config.min_sessions.to_string();
    let min_support_ratio_bps = config.min_support_ratio_bps.to_string();
    let event_count = fingerprint.event_count.to_string();
    let max_row_id = fingerprint.max_row_id.to_string();
    let parts: [&str; 8] = [
        DETECTION_SPEC_VERSION,
        &min_occurrences,
        &min_sessions,
        &min_support_ratio_bps,
        project.unwrap_or(""),
        &event_count,
        &max_row_id,
        &fingerprint.import_watermark,
    ];
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0_u8]);
    }
    hasher.finalize().into()
}

/// Group an integer's digits with thousands separators (`69,400`).
fn group(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if value < 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}
