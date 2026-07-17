//! End-to-end mutation generation and package contract tests.

use std::io::Cursor;

use autophagy_core::{ImportOptions, import_jsonl};
use autophagy_mutations::{
    ADVISORY_EXCLUSION, GenerationOutcome, LEGACY_ADVISORY_UNTIL_REPLAY_EXCLUSION, LifecycleState,
    generate_candidate, generate_candidates,
};
use autophagy_patterns::{DetectorConfig, detect};
use autophagy_store::EventStore;

const CORPUS: &str = include_str!("../../../evals/fixtures/findings/deterministic.jsonl");
const RECOVERY_CORPUS: &str = include_str!("../../../evals/fixtures/findings/recovery-motif.jsonl");

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
fn generated_exclusions_stay_true_at_every_lifecycle_stage() {
    // The exclusion template must not claim a pipeline stage ("advisory until
    // replay and shadow evaluation pass") that is already behind the package
    // by the time it is challenged, replayed, shadowed, or installed — every
    // one of which happens strictly after generation. Assert the new
    // candidates never regress to the old, stage-bound phrasing and instead
    // use the stage-independent `ADVISORY_EXCLUSION` wording.
    let mut saw_advisory_exclusion = false;
    for outcome in generate_candidates(&fixture_findings()) {
        let GenerationOutcome::Candidate { package } = outcome else {
            panic!("fixture finding should generate a candidate")
        };
        assert!(
            package
                .exclusions
                .iter()
                .all(|exclusion| exclusion != LEGACY_ADVISORY_UNTIL_REPLAY_EXCLUSION),
            "newly generated packages must not carry the stale pipeline-stage exclusion"
        );
        saw_advisory_exclusion |= package
            .exclusions
            .iter()
            .any(|exclusion| exclusion == ADVISORY_EXCLUSION);
    }
    assert!(
        saw_advisory_exclusion,
        "the repeated-command-failure fixture candidate should use the stage-independent advisory exclusion"
    );
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
    malformed.signature = "failure/v2|missing-exit".to_owned();
    assert!(matches!(
        generate_candidate(&malformed),
        GenerationOutcome::InsufficientEvidence { .. }
    ));

    // Compatibility (ADR 0014): a well-formed selector minted under the retired
    // v1 grammar no longer parses as a v2 selector, so it generates no candidate.
    // Already-registered v1 mutations stay valid records; they are simply never
    // re-derived under v2.
    let mut legacy = fixture_findings().remove(0);
    legacy.signature = "failure/v1|shell|cargo build|exit:101".to_owned();
    assert!(matches!(
        generate_candidate(&legacy),
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

#[test]
fn recovery_motif_generates_a_conservative_preflight_candidate() {
    let finding = fixture_recovery_findings()
        .into_iter()
        .find(|finding| {
            finding.detector == autophagy_patterns::DetectorKind::RepeatedSuccessfulRecovery
        })
        .expect("recovery finding");
    let GenerationOutcome::Candidate { package } = generate_candidate(&finding) else {
        panic!("recovery candidate")
    };
    package.validate().expect("valid package");
    assert!(package.title.starts_with("Reuse successful recovery"));
    assert_eq!(package.hypothesis.supporting_event_ids.len(), 9);
    assert_eq!(package.hypothesis.counterexample_event_ids.len(), 2);
    assert!(
        package
            .intervention
            .instruction
            .contains("mise run codegen")
    );
    assert!(package.intervention.instruction.contains("otherwise leave"));
    assert!(package.permissions.commands.is_empty());

    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/specs/mutation/0.1/schema.json"))
            .expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    let instance = serde_json::to_value(package).expect("package JSON");
    assert!(validator.is_valid(&instance), "schema rejected {instance}");
}

fn fixture_findings() -> Vec<autophagy_patterns::EvidencePacket> {
    fixture_findings_from(CORPUS, "fixture:mutations")
}

fn fixture_recovery_findings() -> Vec<autophagy_patterns::EvidencePacket> {
    fixture_findings_from(RECOVERY_CORPUS, "fixture:recovery-mutations")
}

fn fixture_findings_from(
    corpus: &str,
    instance_key: &str,
) -> Vec<autophagy_patterns::EvidencePacket> {
    let mut store = EventStore::open_in_memory().expect("store");
    import_jsonl(
        Cursor::new(corpus),
        Some(&mut store),
        &ImportOptions::new(instance_key),
    )
    .expect("import");
    detect(
        &store.list_events_for_detection(None).expect("events"),
        DetectorConfig::default(),
    )
}
