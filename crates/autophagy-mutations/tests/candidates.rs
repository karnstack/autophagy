//! End-to-end mutation generation and package contract tests.

use std::io::Cursor;

use autophagy_core::{ImportOptions, import_jsonl};
use autophagy_mutations::{
    GenerationOutcome, LifecycleState, generate_candidate, generate_candidates,
};
use autophagy_patterns::{DetectorConfig, detect};
use autophagy_store::EventStore;

const CORPUS: &str = include_str!("../../../evals/fixtures/findings/deterministic.jsonl");

#[test]
fn findings_generate_stable_zero_permission_candidates() {
    let findings = fixture_findings();
    let outcomes = generate_candidates(&findings);
    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes, generate_candidates(&findings));

    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/specs/mutation/0.1/schema.json"))
            .expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    for outcome in outcomes {
        let GenerationOutcome::Candidate { package } = outcome else {
            panic!("fixture finding should generate a candidate")
        };
        package.validate().expect("valid package");
        assert!(package.permissions.filesystem_read.is_empty());
        assert!(package.permissions.filesystem_write.is_empty());
        assert!(package.permissions.commands.is_empty());
        assert!(package.permissions.environment.is_empty());
        assert!(!package.permissions.network);
        assert_eq!(package.hypothesis.supporting_event_ids.len(), 3);
        assert_eq!(package.hypothesis.counterexample_event_ids.len(), 1);
        let instance = serde_json::to_value(&package).expect("package JSON");
        assert!(validator.is_valid(&instance), "schema rejected {instance}");
    }
}

#[test]
fn weak_or_malformed_findings_produce_insufficient_evidence() {
    let mut finding = fixture_findings().remove(0);
    finding.evidence.truncate(1);
    finding.score.distinct_sessions = 1;
    assert!(matches!(
        generate_candidate(&finding),
        GenerationOutcome::InsufficientEvidence { .. }
    ));

    let mut malformed = fixture_findings().remove(0);
    malformed.signature = "failure/v1|missing-exit".to_owned();
    assert!(matches!(
        generate_candidate(&malformed),
        GenerationOutcome::InsufficientEvidence { .. }
    ));
}

#[test]
fn validation_rejects_permissions_and_non_candidate_packages() {
    let GenerationOutcome::Candidate { mut package } = generate_candidate(&fixture_findings()[0])
    else {
        panic!("candidate")
    };
    package
        .permissions
        .commands
        .push("dangerous command".to_owned());
    package.state = LifecycleState::Rejected;
    let errors = package.validate().expect_err("invalid package");
    let codes = errors.iter().map(|error| error.code).collect::<Vec<_>>();
    assert!(codes.contains(&"generation_state"));
    assert!(codes.contains(&"excessive"));
}

fn fixture_findings() -> Vec<autophagy_patterns::EvidencePacket> {
    let mut store = EventStore::open_in_memory().expect("store");
    import_jsonl(
        Cursor::new(CORPUS),
        Some(&mut store),
        &ImportOptions::new("fixture:mutations"),
    )
    .expect("import");
    detect(
        &store.list_events_for_detection(None).expect("events"),
        DetectorConfig::default(),
    )
}
