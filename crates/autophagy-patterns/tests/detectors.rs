//! End-to-end deterministic detector and evidence-contract tests.

use std::io::Cursor;

use autophagy_core::{ImportOptions, import_jsonl};
use autophagy_patterns::{DetectorConfig, DetectorKind, detect};
use autophagy_store::EventStore;

const CORPUS: &str = include_str!("../../../evals/fixtures/findings/deterministic.jsonl");

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
