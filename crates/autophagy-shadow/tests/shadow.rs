//! Contract and deterministic metric tests for shadow v0.1.

use std::{collections::BTreeSet, io::Cursor};

use autophagy_core::{ImportOptions, import_jsonl};
use autophagy_events::Event;
use autophagy_mutations::{GenerationOutcome, MutationPackage, generate_candidates};
use autophagy_patterns::{DetectorConfig, detect};
use autophagy_shadow::{
    ShadowDisposition, ShadowSuite, ShadowThresholdFailure, evaluate, extract_observation_draft,
};
use autophagy_store::EventStore;

const CORPUS: &str = include_str!("../../../evals/fixtures/findings/deterministic.jsonl");
const PASSING: &str = include_str!("../../../evals/fixtures/shadow/command-preflight-pass.json");
const FAILING: &str = include_str!("../../../evals/fixtures/shadow/command-preflight-fail.json");

#[test]
fn passing_shadow_is_stable_precise_and_non_applying() {
    let package = command_failure_package();
    let suite: ShadowSuite = serde_json::from_str(PASSING).expect("passing suite");
    suite.validate().expect("valid suite");
    let report = evaluate(&package, &suite).expect("evaluation");
    assert_eq!(report, evaluate(&package, &suite).expect("stable"));
    assert!(report.passed);
    assert!(!report.mutation_applied);
    assert!(!report.model_used);
    assert_eq!(report.summary.observations, 5);
    assert_eq!(report.summary.true_positives, 3);
    assert_eq!(report.summary.true_negatives, 2);
    assert_eq!(report.summary.false_positives, 0);
    assert_eq!(report.summary.false_negatives, 0);
    assert_eq!(report.summary.precision_bps, 10_000);
    assert_eq!(report.summary.recall_bps, 10_000);
    assert!(report.threshold_failures.is_empty());
    validate_schema(
        "../../../docs/specs/shadow/0.1/result.schema.json",
        include_str!("../../../docs/specs/shadow/0.1/result.schema.json"),
        &serde_json::to_value(report).expect("report JSON"),
    );
}

#[test]
fn false_positive_prevents_shadow_pass() {
    let package = command_failure_package();
    let suite: ShadowSuite = serde_json::from_str(FAILING).expect("failing suite");
    let report = evaluate(&package, &suite).expect("evaluation");
    assert!(!report.passed);
    assert_eq!(report.summary.true_positives, 3);
    assert_eq!(report.summary.false_positives, 1);
    assert_eq!(report.summary.false_positive_rate_bps, 2_500);
    assert_eq!(
        report.threshold_failures,
        vec![ShadowThresholdFailure::FalsePositiveRateAboveMaximum]
    );
    assert!(
        report
            .results
            .iter()
            .any(|result| result.disposition == ShadowDisposition::FalsePositive)
    );
}

#[test]
fn suite_schema_and_semantic_independence_are_enforced() {
    let mut suite: ShadowSuite = serde_json::from_str(PASSING).expect("suite");
    validate_schema(
        "../../../docs/specs/shadow/0.1/suite.schema.json",
        include_str!("../../../docs/specs/shadow/0.1/suite.schema.json"),
        &serde_json::to_value(&suite).expect("suite JSON"),
    );
    suite.observations[1].source_event_ids = suite.observations[0].source_event_ids.clone();
    let errors = suite.validate().expect_err("reused event");
    assert!(errors.iter().any(|error| error.code == "duplicate"));
}

#[test]
fn evidence_extraction_produces_stable_schema_valid_shadow_draft() {
    let (package, events) = command_failure_package_and_events();
    let draft = extract_observation_draft(&package, &events, 1).expect("shadow draft");
    draft.validate().expect("structurally valid draft");

    // Deterministic across repeated runs and independent of event input order.
    assert_eq!(
        draft,
        extract_observation_draft(&package, &events, 1).expect("stable draft")
    );
    let mut reversed = events.clone();
    reversed.reverse();
    assert_eq!(
        draft,
        extract_observation_draft(&package, &reversed, 1).expect("stable reversed draft")
    );

    // The draft covers both would-help and legitimate no-op annotations.
    assert!(
        draft
            .observations
            .iter()
            .any(|observation| observation.intervention_would_help)
    );
    assert!(
        draft
            .observations
            .iter()
            .any(|observation| !observation.intervention_would_help)
    );

    // Every observation carries a stable shd_ identity and a review note.
    assert!(
        draft
            .observations
            .iter()
            .all(|observation| observation.observation_id.starts_with("shd_"))
    );
    assert!(
        draft
            .observations
            .iter()
            .all(|observation| observation.note.is_some())
    );

    // Source event ids are globally unique across observations, as the suite
    // contract requires.
    let total: usize = draft
        .observations
        .iter()
        .map(|observation| observation.source_event_ids.len())
        .sum();
    let unique = draft
        .observations
        .iter()
        .flat_map(|observation| observation.source_event_ids.iter())
        .collect::<BTreeSet<_>>();
    assert_eq!(total, unique.len());

    // The draft is a legal input to shadow evaluation.
    evaluate(&package, &draft).expect("draft evaluates");

    validate_schema(
        "../../../docs/specs/shadow/0.1/suite.schema.json",
        include_str!("../../../docs/specs/shadow/0.1/suite.schema.json"),
        &serde_json::to_value(&draft).expect("draft JSON"),
    );
}

fn validate_schema(_path: &str, schema: &str, instance: &serde_json::Value) {
    let schema: serde_json::Value = serde_json::from_str(schema).expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    assert!(validator.is_valid(instance), "schema rejected {instance}");
}

fn command_failure_package() -> MutationPackage {
    command_failure_package_and_events().0
}

fn command_failure_package_and_events() -> (MutationPackage, Vec<Event>) {
    let mut store = EventStore::open_in_memory().expect("store");
    import_jsonl(
        Cursor::new(CORPUS),
        Some(&mut store),
        &ImportOptions::new("fixture:shadow"),
    )
    .expect("import");
    let events = store.list_events_for_detection(None).expect("events");
    let findings = detect(&events, DetectorConfig::default());
    let package = generate_candidates(&findings)
        .into_iter()
        .find_map(|outcome| match outcome {
            GenerationOutcome::Candidate { package }
                if package.mutation_id
                    == "mut_6b51ef819f54c0275db19b15907b0b23c39598241c912828bb64cd5bf824a0ee" =>
            {
                Some(*package)
            }
            _ => None,
        })
        .expect("command failure package");
    (package, events)
}
