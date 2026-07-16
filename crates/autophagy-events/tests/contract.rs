//! Contract and fixture tests for AEP v0.1.

use std::fs;

use autophagy_events::{Event, EventParseError};

const VALID_FIXTURES: &[&str] = &[
    "fixtures/valid/session_started.json",
    "fixtures/valid/tool_failed.json",
    "fixtures/valid/user_correction.json",
];

const INVALID_FIXTURES: &[&str] = &[
    "fixtures/invalid/artifact_without_locator.json",
    "fixtures/invalid/called_with_exit.json",
    "fixtures/invalid/completed_with_nonzero_exit.json",
    "fixtures/invalid/failed_with_zero_exit.json",
    "fixtures/invalid/failed_without_tool.json",
    "fixtures/invalid/invalid_event_id.json",
    "fixtures/invalid/self_parent.json",
    "fixtures/invalid/unknown_field.json",
];

const SCHEMA_INVALID_FIXTURES: &[&str] = &[
    "fixtures/invalid/artifact_without_locator.json",
    "fixtures/invalid/called_with_exit.json",
    "fixtures/invalid/completed_with_nonzero_exit.json",
    "fixtures/invalid/failed_with_zero_exit.json",
    "fixtures/invalid/failed_without_tool.json",
    "fixtures/invalid/invalid_event_id.json",
    "fixtures/invalid/unknown_field.json",
];

#[test]
fn valid_fixtures_round_trip_without_information_loss() {
    for fixture in VALID_FIXTURES {
        let source = read_fixture(fixture);
        let event = Event::from_json_str(&source)
            .unwrap_or_else(|error| panic!("{fixture} should be valid: {error}"));
        let event_from_slice = Event::from_json_slice(source.as_bytes())
            .unwrap_or_else(|error| panic!("{fixture} bytes should be valid: {error}"));
        assert_eq!(
            event, event_from_slice,
            "parse methods disagree for {fixture}"
        );
        let serialized = serde_json::to_string(&event).expect("event should serialize");
        let reparsed = Event::from_json_str(&serialized)
            .unwrap_or_else(|error| panic!("serialized {fixture} should be valid: {error}"));
        assert_eq!(event, reparsed, "round trip changed {fixture}");
    }
}

#[test]
fn invalid_fixtures_are_rejected() {
    for fixture in INVALID_FIXTURES {
        let source = read_fixture(fixture);
        assert!(
            Event::from_json_str(&source).is_err(),
            "{fixture} should be invalid"
        );
    }
}

#[test]
fn semantic_errors_include_stable_field_paths_and_codes() {
    let source = read_fixture("fixtures/invalid/failed_with_zero_exit.json");
    let error = Event::from_json_str(&source).expect_err("fixture should fail");
    let EventParseError::Validation(errors) = error else {
        panic!("fixture should fail semantic validation")
    };
    let first = errors.iter().next().expect("at least one error");
    assert_eq!(first.path, "tool.exit_code");
    assert_eq!(first.code, "expected_nonzero");
}

#[test]
fn normative_schema_accepts_and_rejects_contract_fixtures() {
    let schema = include_str!("../../../docs/specs/aep/0.1/schema.json");
    let parsed: serde_json::Value = serde_json::from_str(schema).expect("schema should be JSON");
    assert_eq!(
        parsed["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(parsed["properties"]["spec_version"]["const"], "aep/0.1");

    let validator = jsonschema::validator_for(&parsed).expect("schema should compile");
    for fixture in VALID_FIXTURES {
        let instance: serde_json::Value =
            serde_json::from_str(&read_fixture(fixture)).expect("fixture should be JSON");
        assert!(validator.is_valid(&instance), "schema rejected {fixture}");
    }
    for fixture in SCHEMA_INVALID_FIXTURES {
        let instance: serde_json::Value =
            serde_json::from_str(&read_fixture(fixture)).expect("fixture should be JSON");
        assert!(!validator.is_valid(&instance), "schema accepted {fixture}");
    }
}

fn read_fixture(relative_path: &str) -> String {
    fs::read_to_string(format!(
        "{}/tests/{relative_path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap_or_else(|error| panic!("could not read {relative_path}: {error}"))
}
