//! Bundle build/parse behavior and `genome/0.1` schema conformance.

use autophagy_events::Event;
use autophagy_genome::{
    AttestationInput, AttestationKind, GENOME_SPEC_VERSION, GenomeBuildError, GenomeParseError,
    GenomeSource, GenomeTransition, build, parse,
};
use autophagy_mutations::MutationPackage;
use autophagy_redaction::PrivacyPolicy;
use serde_json::{Value, json};

const BUNDLE_SCHEMA: &str = include_str!("../../../docs/specs/genome/0.1/bundle.schema.json");
const VALID: &[&str] = &[
    include_str!("../../../docs/specs/genome/0.1/valid/full.json"),
    include_str!("../../../docs/specs/genome/0.1/valid/minimal_no_attestations.json"),
];
const INVALID: &[&str] = &[
    include_str!("../../../docs/specs/genome/0.1/invalid/bad_spec_version.json"),
    include_str!("../../../docs/specs/genome/0.1/invalid/bad_genome_id.json"),
    include_str!("../../../docs/specs/genome/0.1/invalid/unknown_field.json"),
    include_str!("../../../docs/specs/genome/0.1/invalid/missing_redaction.json"),
    include_str!("../../../docs/specs/genome/0.1/invalid/bad_attestation_kind.json"),
    include_str!("../../../docs/specs/genome/0.1/invalid/bad_content_hash.json"),
];

fn package() -> MutationPackage {
    let value = json!({
        "spec_version": "mutation/0.1",
        "mutation_id": "mut_a87b7c83aa9bf78169c01c2ad73b02c1ad7705f976fc2c94194f397a3091ce9d",
        "version": "0.1.0",
        "state": "candidate",
        "generated_by": "deterministic_template_v1",
        "source_finding_id": "find_examplefinding",
        "source_detector": "repeated_command_failure",
        "title": "Prevent repeated command failure: shell: go build ./...",
        "hypothesis": {
            "statement": "The recurring go build failure is caused by missing preconditions.",
            "expected_result": "Matching go build attempts avoid repeated exit-code-1 failures.",
            "supporting_event_ids": ["evt_support_one", "evt_support_two"],
            "counterexample_event_ids": [],
            "failure_cases": ["The command can fail transiently even when preconditions hold."]
        },
        "triggers": [
            { "type": "tool_call", "selector": "failure/v2|shell|go build ./...|exit:1" }
        ],
        "exclusions": ["Do not block execution; this instruction is advisory."],
        "intervention": {
            "type": "agent_instruction",
            "instruction": "Before running `go build ./...`, verify its required preconditions."
        },
        "permissions": {
            "filesystem_read": [], "filesystem_write": [], "commands": [],
            "environment": [], "network": false
        },
        "promotion": {
            "minimum_replays": 5,
            "minimum_success_rate_bps": 8000,
            "maximum_false_positive_rate_bps": 1000
        }
    });
    serde_json::from_value(value).expect("package")
}

fn event(event_id: &str, project: &str) -> Event {
    let value = json!({
        "spec_version": "aep/0.1",
        "event_id": event_id,
        "session_id": "ses_alpha",
        "timestamp": "2026-06-01T09:00:00Z",
        "source": "claude-code",
        "type": "tool.failed",
        "project": project,
        "tool": { "name": "shell", "input": { "command": "go build ./..." }, "exit_code": 1 }
    });
    serde_json::from_value(value).expect("event")
}

fn source() -> GenomeSource {
    GenomeSource {
        origin_instance_key: "laptop-abc".to_owned(),
        autophagy_version: "0.1.0".to_owned(),
        exported_at: "2026-07-19T12:00:00Z".to_owned(),
        package: package(),
        events: vec![
            event("evt_support_one", "/repo/public"),
            event("evt_support_two", "/repo/public"),
        ],
        attestations: vec![AttestationInput {
            kind: AttestationKind::Replay,
            id: "rep_examplereplay".to_owned(),
            set_hash: "scenarioset0001".to_owned(),
            report: json!({
                "spec_version": "replay/0.1",
                "replay_id": "rep_examplereplay",
                "passed": true
            }),
            passed: true,
            created_at: "2026-07-10T10:00:00Z".to_owned(),
        }],
        transitions: vec![GenomeTransition {
            from_state: None,
            to_state: "candidate".to_owned(),
            reason: "generated from evidence".to_owned(),
            occurred_at: "2026-06-03T09:00:00Z".to_owned(),
        }],
    }
}

fn policy(exclude: &[&str]) -> PrivacyPolicy {
    let patterns = exclude.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>();
    PrivacyPolicy::new(&patterns).expect("policy")
}

#[test]
fn a_built_bundle_conforms_to_its_own_schema() {
    let bundle = build(source(), &policy(&[])).expect("build");
    assert_eq!(bundle.spec_version, GENOME_SPEC_VERSION);
    assert!(bundle.genome_id.starts_with("gen_"));
    let schema: Value = serde_json::from_str(BUNDLE_SCHEMA).expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    let instance = serde_json::to_value(&bundle).expect("bundle value");
    assert!(
        validator.is_valid(&instance),
        "schema rejected a freshly built bundle"
    );
}

#[test]
fn a_built_bundle_round_trips_through_parse() {
    let bundle = build(source(), &policy(&[])).expect("build");
    let bytes = serde_json::to_vec(&bundle).expect("serialize");
    let parsed = parse(&bytes).expect("parse");
    assert_eq!(parsed, bundle);
    assert_eq!(parsed.evidence_events.len(), 2);
    assert!(parsed.attestations[0].hash_matches());
}

#[test]
fn tampering_with_content_is_detected_by_the_genome_id() {
    let bundle = build(source(), &policy(&[])).expect("build");
    let mut value = serde_json::to_value(&bundle).expect("value");
    // Alter the mutation title without touching genome_id.
    value["mutation"]["title"] = json!("A different, tampered title");
    let bytes = serde_json::to_vec(&value).expect("serialize");
    assert!(matches!(
        parse(&bytes),
        Err(GenomeParseError::GenomeIdMismatch { .. })
    ));
}

#[test]
fn tampering_with_an_attestation_report_is_detected_by_its_hash() {
    let mut bundle = build(source(), &policy(&[])).expect("build");
    assert!(bundle.attestations[0].hash_matches());
    bundle.attestations[0].report_json["passed"] = json!(false);
    assert!(
        !bundle.attestations[0].hash_matches(),
        "a mutated report must not match its carried content hash"
    );
}

#[test]
fn a_path_excluded_event_aborts_the_build() {
    let mut source = source();
    source.events[1] = event("evt_support_two", "/repo/private/client");
    let error = build(source, &policy(&["**/private/**"])).expect_err("must abort");
    assert!(matches!(
        error,
        GenomeBuildError::PathExcludedEvent { event_id } if event_id == "evt_support_two"
    ));
}

#[test]
fn secrets_in_package_text_are_scrubbed_and_counted() {
    let mut source = source();
    source.package.intervention.instruction =
        "Use token sk-abcdefghijklmnop to authenticate before building.".to_owned();
    let bundle = build(source, &policy(&[])).expect("build");
    assert!(
        !bundle
            .mutation
            .intervention
            .instruction
            .contains("sk-abcdefghijklmnop")
    );
    assert!(bundle.redaction.redacted_fields >= 1);
}

#[test]
fn unsupported_spec_version_is_rejected() {
    let bundle = build(source(), &policy(&[])).expect("build");
    let mut value = serde_json::to_value(&bundle).expect("value");
    value["spec_version"] = json!("genome/9.9");
    let bytes = serde_json::to_vec(&value).expect("serialize");
    assert!(matches!(
        parse(&bytes),
        Err(GenomeParseError::UnsupportedSpecVersion { .. })
    ));
}

#[test]
fn fixtures_round_trip_through_the_schema() {
    let schema: Value = serde_json::from_str(BUNDLE_SCHEMA).expect("schema JSON");
    assert_eq!(
        schema["properties"]["spec_version"]["const"],
        GENOME_SPEC_VERSION
    );
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    for fixture in VALID {
        let instance: Value = serde_json::from_str(fixture).expect("valid fixture JSON");
        assert!(validator.is_valid(&instance), "schema rejected {fixture}");
    }
    for fixture in INVALID {
        let instance: Value = serde_json::from_str(fixture).expect("invalid fixture JSON");
        assert!(!validator.is_valid(&instance), "schema accepted {fixture}");
    }
}
