//! Contract and deterministic classification tests for replay v0.1.

use std::{collections::BTreeSet, io::Cursor};

use autophagy_core::{ImportOptions, import_jsonl};
use autophagy_events::Event;
use autophagy_mutations::{GenerationOutcome, MutationPackage, generate_candidates};
use autophagy_patterns::{DetectorConfig, detect};
use autophagy_replay::{
    CounterfactualOutcome, ExpectedAction, ReplayDisposition, ReplayEvaluationError, ReplaySuite,
    ThresholdFailure, evaluate, extract_review_draft,
};
use autophagy_store::EventStore;

const CORPUS: &str = include_str!("../../../evals/fixtures/findings/deterministic.jsonl");
const DRAFT: &str = include_str!("../../../evals/fixtures/replay/command-preflight-draft.json");
const PASSING: &str = include_str!("../../../evals/fixtures/replay/command-preflight-pass.json");
const FAILING: &str = include_str!("../../../evals/fixtures/replay/command-preflight-fail.json");
const RECOVERY: &str = include_str!("../../../evals/fixtures/findings/recovery-motif.jsonl");

#[test]
fn passing_suite_is_stable_measured_and_schema_valid() {
    let package = command_failure_package();
    let suite: ReplaySuite = serde_json::from_str(PASSING).expect("passing suite");
    suite.validate().expect("valid suite");
    let suite_schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/specs/replay/0.1/suite.schema.json"
    ))
    .expect("suite schema");
    let suite_validator = jsonschema::validator_for(&suite_schema).expect("compile suite schema");
    let suite_instance = serde_json::to_value(&suite).expect("suite JSON");
    assert!(
        suite_validator.is_valid(&suite_instance),
        "schema rejected {suite_instance}"
    );
    validate_scenarios_against_schema(&suite);

    let report = evaluate(&package, &suite).expect("evaluation");
    assert_eq!(
        report,
        evaluate(&package, &suite).expect("stable evaluation")
    );
    assert!(report.passed);
    assert!(!report.mutation_executed);
    assert!(!report.model_used);
    assert_eq!(report.summary.scenarios, 5);
    assert_eq!(report.summary.successes, 3);
    assert_eq!(report.summary.no_ops, 2);
    assert_eq!(report.summary.contradictions, 0);
    assert_eq!(report.summary.false_interventions, 0);
    assert_eq!(report.summary.success_rate_bps, 10_000);
    assert_eq!(report.summary.false_intervention_rate_bps, 0);
    assert!(report.threshold_failures.is_empty());
    assert!(report.replay_id.starts_with("rpl_"));
    assert!(report.scenario_set_hash.starts_with("rsh_"));

    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/specs/replay/0.1/result.schema.json"
    ))
    .expect("result schema");
    let validator = jsonschema::validator_for(&schema).expect("compile result schema");
    let instance = serde_json::to_value(&report).expect("result JSON");
    assert!(validator.is_valid(&instance), "schema rejected {instance}");
}

#[test]
fn failing_suite_reports_contradiction_and_false_intervention() {
    let package = command_failure_package();
    let suite: ReplaySuite = serde_json::from_str(FAILING).expect("failing suite");
    let report = evaluate(&package, &suite).expect("evaluation");

    assert!(!report.passed);
    assert_eq!(report.summary.successes, 2);
    assert_eq!(report.summary.no_ops, 1);
    assert_eq!(report.summary.contradictions, 1);
    assert_eq!(report.summary.false_interventions, 1);
    assert_eq!(report.summary.success_rate_bps, 6_000);
    assert_eq!(report.summary.false_intervention_rate_bps, 5_000);
    assert_eq!(
        report.threshold_failures,
        vec![
            ThresholdFailure::SuccessRateBelowMinimum,
            ThresholdFailure::FalseInterventionRateAboveMaximum,
        ]
    );
    assert!(report.results.iter().any(|result| {
        result.expected_action == ExpectedAction::NoOp
            && result.disposition == ReplayDisposition::FalseIntervention
    }));
}

#[test]
fn validation_rejects_ambiguous_annotations_and_wrong_mutation() {
    let package = command_failure_package();
    let mut suite: ReplaySuite = serde_json::from_str(PASSING).expect("suite");
    suite.scenarios[0].counterfactual_outcome = None;
    let error = suite.validate().expect_err("missing outcome");
    assert!(error.iter().any(|error| error.code == "required"));

    let mut mismatch: ReplaySuite = serde_json::from_str(PASSING).expect("suite");
    mismatch.mutation_id = "mut_some_other_candidate".to_owned();
    assert!(matches!(
        evaluate(&package, &mismatch),
        Err(ReplayEvaluationError::MutationMismatch { .. })
    ));

    let mut reused_event: ReplaySuite = serde_json::from_str(PASSING).expect("suite");
    reused_event.scenarios[1].source_event_ids = reused_event.scenarios[0].source_event_ids.clone();
    let error = reused_event.validate().expect_err("reused source event");
    assert!(error.iter().any(|error| error.code == "duplicate"));
}

#[test]
fn evidence_extraction_produces_stable_schema_valid_review_draft() {
    let (package, mut events) = command_failure_package_and_events();
    let expected: ReplaySuite = serde_json::from_str(DRAFT).expect("review draft fixture");
    assert_eq!(
        extract_review_draft(&package, &events, 1).expect("fixture review draft"),
        expected
    );
    events.push(
        Event::from_json_str(concat!(
            "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_failure_context\",",
            "\"session_id\":\"ses_failure_1\",\"timestamp\":\"2026-07-16T09:59:00Z\",",
            "\"sequence\":0,\"source\":\"generic-jsonl\",\"type\":\"tool.called\",",
            "\"project\":\"/workspace/demo\",\"tool\":{\"name\":\"bash\",",
            "\"input\":{\"command\":\"mise run check\"}}}"
        ))
        .expect("context event"),
    );

    let draft = extract_review_draft(&package, &events, 1).expect("review draft");
    draft.validate().expect("structurally valid draft");
    assert_eq!(draft.scenarios.len(), 4);
    assert_eq!(
        draft
            .scenarios
            .iter()
            .filter(|scenario| {
                scenario.counterfactual_outcome == Some(CounterfactualOutcome::Unknown)
            })
            .count(),
        3
    );
    assert_eq!(
        draft
            .scenarios
            .iter()
            .filter(|scenario| scenario.expected_action == ExpectedAction::NoOp)
            .count(),
        1
    );
    let contextual = draft
        .scenarios
        .iter()
        .find(|scenario| {
            scenario
                .source_event_ids
                .iter()
                .any(|event_id| event_id == "evt_failure_1")
        })
        .expect("contextual scenario");
    assert_eq!(
        contextual.source_event_ids,
        vec!["evt_failure_context", "evt_failure_1"]
    );
    assert!(
        contextual
            .note
            .as_deref()
            .expect("review note")
            .contains("1 nearby event")
    );

    let mut reversed = events.clone();
    reversed.reverse();
    assert_eq!(
        draft,
        extract_review_draft(&package, &reversed, 1).expect("stable reversed draft")
    );
    assert!(matches!(
        evaluate(&package, &draft),
        Err(ReplayEvaluationError::UnreviewedScenarios { .. })
    ));

    let suite_schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/specs/replay/0.1/suite.schema.json"
    ))
    .expect("suite schema");
    assert!(
        jsonschema::validator_for(&suite_schema)
            .expect("compile suite schema")
            .is_valid(&serde_json::to_value(&draft).expect("draft JSON"))
    );
    validate_scenarios_against_schema(&draft);
}

#[test]
fn mixed_session_evidence_produces_non_overlapping_decision_points() {
    let mut store = EventStore::open_in_memory().expect("store");
    import_jsonl(
        Cursor::new(CORPUS),
        Some(&mut store),
        &ImportOptions::new("fixture:replay-mixed-base"),
    )
    .expect("base import");
    import_jsonl(
        Cursor::new(RECOVERY),
        Some(&mut store),
        &ImportOptions::new("fixture:replay-mixed-recovery"),
    )
    .expect("recovery import");
    let events = store.list_events_for_detection(None).expect("events");
    let package = command_failure_package_from_events(&events);
    let draft = extract_review_draft(&package, &events, 1).expect("mixed review draft");
    draft.validate().expect("valid mixed review draft");
    assert_eq!(draft.scenarios.len(), 12);
    let source_event_ids = draft
        .scenarios
        .iter()
        .flat_map(|scenario| scenario.source_event_ids.iter())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        source_event_ids.len(),
        draft
            .scenarios
            .iter()
            .map(|scenario| scenario.source_event_ids.len())
            .sum::<usize>()
    );
    let direct_retry = draft
        .scenarios
        .iter()
        .filter(|scenario| {
            scenario
                .note
                .as_deref()
                .is_some_and(|note| note.contains("ses_direct_retry"))
        })
        .collect::<Vec<_>>();
    assert_eq!(direct_retry.len(), 2);
    assert!(
        direct_retry
            .iter()
            .all(|scenario| scenario.source_event_ids.len() == 1)
    );
}

fn validate_scenarios_against_schema(suite: &ReplaySuite) {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/specs/replay/0.1/scenario.schema.json"
    ))
    .expect("scenario schema");
    let validator = jsonschema::validator_for(&schema).expect("compile scenario schema");
    for scenario in &suite.scenarios {
        let instance = serde_json::to_value(scenario).expect("scenario JSON");
        assert!(validator.is_valid(&instance), "schema rejected {instance}");
    }
}

fn command_failure_package() -> MutationPackage {
    command_failure_package_and_events().0
}

fn command_failure_package_and_events() -> (MutationPackage, Vec<Event>) {
    let mut store = EventStore::open_in_memory().expect("store");
    import_jsonl(
        Cursor::new(CORPUS),
        Some(&mut store),
        &ImportOptions::new("fixture:replay"),
    )
    .expect("import");
    let events = store.list_events_for_detection(None).expect("events");
    let package = command_failure_package_from_events(&events);
    (package, events)
}

fn command_failure_package_from_events(events: &[Event]) -> MutationPackage {
    let findings = detect(events, DetectorConfig::default());
    generate_candidates(&findings)
        .into_iter()
        .find_map(|outcome| match outcome {
            GenerationOutcome::Candidate { package }
                if package.mutation_id
                    == "mut_d6b7a340eb2fb6f18bee4a20932b9c954adb4975f3ea8136bf0bd264b3ec431c" =>
            {
                Some(*package)
            }
            _ => None,
        })
        .expect("command failure package")
}
