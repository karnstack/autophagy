//! End-to-end deterministic detector and evidence-contract tests.

use std::{collections::BTreeMap, io::Cursor};

use autophagy_core::{ImportOptions, import_jsonl};
use autophagy_events::{Event, EventId, EventKind, SessionId, SpecVersion, ToolCall};
use autophagy_patterns::{DetectorConfig, DetectorKind, UnmetGate, detect, detect_with_report};
use autophagy_store::EventStore;
use serde_json::json;
use time::{Duration, OffsetDateTime};

const CORPUS: &str = include_str!("../../../evals/fixtures/findings/deterministic.jsonl");
const RECOVERY_CORPUS: &str = include_str!("../../../evals/fixtures/findings/recovery-motif.jsonl");

#[test]
fn demo_corpus_produces_two_stable_evidence_linked_findings() {
    let mut store = EventStore::open_in_memory().expect("store");
    let options = ImportOptions::new("fixture:deterministic-findings");
    let imported = import_jsonl(Cursor::new(CORPUS), Some(&mut store), &options).expect("import");
    assert_eq!(imported.inserted, 11);
    assert_eq!(imported.rejected, 0);

    let events = store.list_events_for_detection(None).expect("events");
    let findings = detect(&events, DetectorConfig::default());
    assert_eq!(findings.len(), 2);
    assert_eq!(findings, detect(&events, DetectorConfig::default()));
    let mut reversed = events.clone();
    reversed.reverse();
    assert_eq!(findings, detect(&reversed, DetectorConfig::default()));

    let failure = findings
        .iter()
        .find(|finding| finding.detector == DetectorKind::RepeatedCommandFailure)
        .expect("failure finding");
    assert_eq!(failure.score.occurrences, 3);
    assert_eq!(failure.score.distinct_sessions, 3);
    assert_eq!(failure.score.counterexamples, 1);
    assert_eq!(failure.evidence[0].event_id, "evt_failure_1");
    assert_eq!(failure.counterexamples[0].event_id, "evt_success_1");

    let correction = findings
        .iter()
        .find(|finding| finding.detector == DetectorKind::RepeatedUserCorrection)
        .expect("correction finding");
    assert_eq!(correction.score.occurrences, 3);
    assert_eq!(correction.score.distinct_sessions, 2);
    assert_eq!(correction.score.counterexamples, 1);
    assert!(
        correction
            .evidence
            .iter()
            .all(|item| item.event_id.starts_with("evt_correction_"))
    );

    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/specs/evidence/0.1/schema.json"))
            .expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    for finding in &findings {
        let instance = serde_json::to_value(finding).expect("finding JSON");
        assert!(validator.is_valid(&instance), "schema rejected {instance}");
    }
}

#[test]
fn below_threshold_corpus_produces_no_findings() {
    let mut store = EventStore::open_in_memory().expect("store");
    let options = ImportOptions::new("fixture:below-threshold");
    import_jsonl(Cursor::new(CORPUS), Some(&mut store), &options).expect("import");
    let events = store.list_events_for_detection(None).expect("events");
    let config = DetectorConfig {
        min_occurrences: 4,
        ..DetectorConfig::default()
    };
    assert!(detect(&events, config).is_empty());
}

#[test]
fn exact_project_selection_limits_detector_input() {
    let mut store = EventStore::open_in_memory().expect("store");
    let options = ImportOptions::new("fixture:project-query");
    import_jsonl(Cursor::new(CORPUS), Some(&mut store), &options).expect("import");
    assert_eq!(
        store
            .list_events_for_detection(Some("/workspace/demo"))
            .expect("selected")
            .len(),
        11
    );
    assert!(
        store
            .list_events_for_detection(Some("/workspace/other"))
            .expect("excluded")
            .is_empty()
    );
}

#[test]
fn repeated_successful_recovery_preserves_composite_lineage() {
    let mut store = EventStore::open_in_memory().expect("store");
    import_jsonl(
        Cursor::new(RECOVERY_CORPUS),
        Some(&mut store),
        &ImportOptions::new("fixture:recovery-motif"),
    )
    .expect("import");
    let events = store.list_events_for_detection(None).expect("events");
    let findings = detect(&events, DetectorConfig::default());
    let recovery = findings
        .iter()
        .find(|finding| finding.detector == DetectorKind::RepeatedSuccessfulRecovery)
        .expect("recovery finding");
    assert_eq!(recovery.score.occurrences, 3);
    assert_eq!(recovery.score.distinct_sessions, 3);
    assert_eq!(recovery.score.counterexamples, 1);
    assert_eq!(recovery.evidence.len(), 9);
    assert_eq!(recovery.counterexamples.len(), 2);
    assert_eq!(
        recovery.signature,
        "recovery/v1|shell|mise run check|exit:1|via|shell|mise run codegen"
    );
    assert!(
        recovery
            .evidence
            .iter()
            .any(|reference| reference.event_id == "evt_recovery_a_step")
    );
    let mut reversed = events.clone();
    reversed.reverse();
    assert_eq!(findings, detect(&reversed, DetectorConfig::default()));

    let mut one_session = events;
    for event in &mut one_session {
        event.session_id = SessionId::new("ses_one_recovery_session");
    }
    assert!(
        detect(&one_session, DetectorConfig::default())
            .iter()
            .all(|finding| finding.detector != DetectorKind::RepeatedSuccessfulRecovery)
    );
}

/// Build a synthetic shell tool event. Purely fabricated fixture data — no real
/// history is ever embedded in tests.
fn shell_event(index: u32, session: &str, kind: EventKind, command: &str, exit: i64) -> Event {
    Event {
        spec_version: SpecVersion::V0_1,
        event_id: EventId::new(format!("evt_{session}_{index}")),
        session_id: SessionId::new(session),
        timestamp: OffsetDateTime::UNIX_EPOCH + Duration::seconds(i64::from(index)),
        sequence: Some(u64::from(index)),
        source: "generic-jsonl".to_owned(),
        kind,
        project: Some("/workspace/demo".to_owned()),
        parent_event_id: None,
        tool: Some(ToolCall {
            name: "bash".to_owned(),
            input: Some(json!({ "command": command })),
            exit_code: matches!(kind, EventKind::ToolFailed).then_some(exit),
            duration_ms: None,
            metadata: BTreeMap::new(),
        }),
        artifacts: Vec::new(),
        metadata: BTreeMap::new(),
    }
}

/// Regression for the real-data threshold bug: an operation that succeeds far
/// more often than it fails must still qualify as a repeated failure, because
/// qualification is about cross-session recurrence, not overall failure share.
///
/// The build command fails 4 times across 3 sessions but succeeds 30 times, so
/// its support ratio (~1176 bps) sits far below the old 5000 bps majority gate
/// that silently discarded it.
#[test]
fn mostly_succeeds_but_repeatedly_fails_still_qualifies() {
    let mut events = Vec::new();
    let mut index = 0;
    // 4 failures spread across 3 distinct sessions.
    for (session, count) in [("ses_a", 2), ("ses_b", 1), ("ses_c", 1)] {
        for _ in 0..count {
            events.push(shell_event(
                index,
                session,
                EventKind::ToolFailed,
                "go build ./...",
                1,
            ));
            index += 1;
        }
    }
    // 30 successes of the same operation across the same sessions.
    for session in ["ses_a", "ses_b", "ses_c"] {
        for _ in 0..10 {
            events.push(shell_event(
                index,
                session,
                EventKind::ToolCompleted,
                "go build ./...",
                0,
            ));
            index += 1;
        }
    }

    let findings = detect(&events, DetectorConfig::default());
    let failure = findings
        .iter()
        .find(|finding| finding.detector == DetectorKind::RepeatedCommandFailure)
        .expect("mostly-succeeding operation still yields a repeated-failure finding");
    assert_eq!(failure.score.occurrences, 4);
    assert_eq!(failure.score.distinct_sessions, 3);
    assert_eq!(failure.score.counterexamples, 30);
    // The reported ratio is well under the old majority gate, proving the
    // finding would have been discarded before the fix.
    assert!(failure.score.support_ratio_bps < 5_000);

    // An explicit high support-ratio floor still suppresses it — turning the
    // finding into a near-threshold observation attributed to that exact gate.
    let strict = DetectorConfig {
        min_support_ratio_bps: 5_000,
        ..DetectorConfig::default()
    };
    let report = detect_with_report(&events, strict);
    assert!(report.findings.is_empty());
    let observation = report
        .diagnostics
        .observations
        .iter()
        .find(|observation| observation.detector == DetectorKind::RepeatedCommandFailure)
        .expect("suppressed failure surfaces as an observation");
    assert_eq!(observation.unmet_gate, UnmetGate::MinSupportRatio);
}

/// A zero-finding scan must explain itself: report the scan size, the count of
/// candidate signatures, and the top near-threshold observations with the exact
/// gate each missed.
#[test]
fn zero_findings_scan_reports_near_threshold_observations() {
    // One command fails twice, both times in a single session: it recurs but
    // clears neither the occurrence nor the session gate.
    let events = vec![
        shell_event(0, "ses_solo", EventKind::ToolFailed, "cargo test", 1),
        shell_event(1, "ses_solo", EventKind::ToolFailed, "cargo test", 1),
        shell_event(2, "ses_solo", EventKind::ToolCompleted, "cargo test", 0),
    ];

    let report = detect_with_report(&events, DetectorConfig::default());
    assert!(report.findings.is_empty());
    assert_eq!(report.diagnostics.events_scanned, 3);
    assert_eq!(report.diagnostics.sessions_scanned, 1);
    assert_eq!(report.diagnostics.candidate_signatures, 1);

    let observation = report
        .diagnostics
        .observations
        .first()
        .expect("recurring near-threshold candidate is reported as an observation");
    assert_eq!(observation.detector, DetectorKind::RepeatedCommandFailure);
    assert_eq!(observation.score.occurrences, 2);
    assert_eq!(observation.score.distinct_sessions, 1);
    // Occurrences are checked before sessions, so the attributed gate is stable.
    assert_eq!(observation.unmet_gate, UnmetGate::MinOccurrences);

    // Diagnostics are order-independent, like the findings themselves.
    let mut reversed = events;
    reversed.reverse();
    assert_eq!(
        report.diagnostics,
        detect_with_report(&reversed, DetectorConfig::default()).diagnostics
    );
}
