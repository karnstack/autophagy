//! Contract and deterministic classification tests for replay v0.1.

use std::io::Cursor;

use autophagy_core::{ImportOptions, import_jsonl};
use autophagy_mutations::{GenerationOutcome, MutationPackage, generate_candidates};
use autophagy_patterns::{DetectorConfig, detect};
use autophagy_replay::{
    ExpectedAction, ReplayDisposition, ReplayEvaluationError, ReplaySuite, ThresholdFailure,
    evaluate,
};
use autophagy_store::EventStore;

const CORPUS: &str = include_str!("../../../evals/fixtures/findings/deterministic.jsonl");
const PASSING: &str = include_str!("../../../evals/fixtures/replay/command-preflight-pass.json");
const FAILING: &str = include_str!("../../../evals/fixtures/replay/command-preflight-fail.json");

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
    let mut store = EventStore::open_in_memory().expect("store");
    import_jsonl(
        Cursor::new(CORPUS),
        Some(&mut store),
        &ImportOptions::new("fixture:replay"),
    )
    .expect("import");
    let findings = detect(
        &store.list_events_for_detection(None).expect("events"),
        DetectorConfig::default(),
    );
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
