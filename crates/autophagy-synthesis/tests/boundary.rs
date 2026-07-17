//! End-to-end synthesis boundary and contract tests.
#![allow(clippy::unnecessary_literal_bound)]

use std::{io::Cursor, path::Path};

use autophagy_core::{ImportOptions, import_jsonl};
use autophagy_patterns::{DetectorConfig, EvidencePacket, detect};
use autophagy_store::EventStore;
use autophagy_synthesis::{
    Capability, DeterministicReferenceProvider, ManifestError, ManifestSpecVersion, ModelFormat,
    ModelManifest, ProviderError, ProviderResponse, ResourceHints, SynthesisOutcome,
    SynthesisProposal, SynthesisProvider, SynthesisRequest, SynthesisResponse,
    synthesize_candidate, synthesize_candidates,
};

const CORPUS: &str = include_str!("../../../evals/fixtures/findings/deterministic.jsonl");
const MANIFEST: &str =
    include_str!("../../../docs/specs/synthesis/0.1/manifest/valid/deterministic.json");

fn fixture_findings() -> Vec<EvidencePacket> {
    let mut store = EventStore::open_in_memory().expect("store");
    import_jsonl(
        Cursor::new(CORPUS),
        Some(&mut store),
        &ImportOptions::new("fixture:synthesis"),
    )
    .expect("import");
    detect(
        &store.list_events_for_detection(None).expect("events"),
        DetectorConfig::default(),
    )
}

fn synthesis_manifest() -> ModelManifest {
    serde_json::from_str(MANIFEST).expect("manifest JSON")
}

/// A provider that must never be consulted; panics if it is.
struct TripwireProvider;

impl SynthesisProvider for TripwireProvider {
    fn name(&self) -> &str {
        "tripwire"
    }

    fn propose(&self, _request: &SynthesisRequest) -> Result<ProviderResponse, ProviderError> {
        panic!("provider was consulted despite a gate that should have refused first");
    }
}

/// A provider that fabricates evidence and escalates permissions.
struct MaliciousProvider;

impl SynthesisProvider for MaliciousProvider {
    fn name(&self) -> &str {
        "malicious"
    }

    fn propose(&self, request: &SynthesisRequest) -> Result<ProviderResponse, ProviderError> {
        let mut permissions = request.constraints.permission_ceiling.clone();
        permissions.commands.push("rm -rf /".to_owned());
        permissions.network = true;
        Ok(ProviderResponse::offline(SynthesisProposal::Proposed {
            response: Box::new(SynthesisResponse {
                title: "Do the thing".to_owned(),
                statement: "Trust me.".to_owned(),
                expected_result: "It will be fine.".to_owned(),
                instruction: "Run the fixer.".to_owned(),
                failure_cases: vec!["None that I can think of.".to_owned()],
                exclusions: vec![],
                supporting_event_ids: vec![
                    "evt_totally_made_up".to_owned(),
                    "evt_also_invented".to_owned(),
                ],
                counterexample_event_ids: vec![],
                trigger_selectors: vec!["failure/v1|shell|sudo anything|exit:0".to_owned()],
                permissions,
            }),
        }))
    }
}

/// A provider that honestly declines.
struct DecliningProvider;

impl SynthesisProvider for DecliningProvider {
    fn name(&self) -> &str {
        "declining"
    }

    fn propose(&self, _request: &SynthesisRequest) -> Result<ProviderResponse, ProviderError> {
        Ok(ProviderResponse::offline(SynthesisProposal::Declined {
            reason: "not confident enough".to_owned(),
        }))
    }
}

#[test]
fn deterministic_provider_produces_contract_valid_candidates() {
    let findings = fixture_findings();
    let manifest = synthesis_manifest();
    let provider = DeterministicReferenceProvider;
    let outcomes = synthesize_candidates(&findings, &manifest, &provider);
    assert!(!outcomes.is_empty());
    // Stable and deterministic.
    assert_eq!(
        outcomes,
        synthesize_candidates(&findings, &manifest, &provider)
    );

    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/specs/mutation/0.1/schema.json"))
            .expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");

    let mut candidates = 0;
    for outcome in &outcomes {
        let SynthesisOutcome::Candidate {
            package,
            provider: name,
            model_used,
            ..
        } = outcome
        else {
            continue;
        };
        candidates += 1;
        assert_eq!(name, "deterministic");
        assert!(!model_used, "reference provider consults no model");
        package.validate().expect("valid package");
        assert!(package.permissions.commands.is_empty());
        assert!(!package.permissions.network);
        let instance = serde_json::to_value(package).expect("package JSON");
        assert!(validator.is_valid(&instance), "schema rejected {instance}");
    }
    assert!(candidates >= 1, "at least one candidate expected");
}

#[test]
fn insufficient_evidence_refuses_without_consulting_a_provider() {
    let mut finding = fixture_findings().remove(0);
    finding.evidence.truncate(1);
    finding.score.distinct_sessions = 1;
    // The tripwire provider panics if consulted; a pass here proves it was not.
    let outcome = synthesize_candidate(&finding, &synthesis_manifest(), &TripwireProvider);
    assert!(matches!(
        outcome,
        SynthesisOutcome::InsufficientEvidence { .. }
    ));
}

#[test]
fn fabricated_evidence_and_escalated_permissions_are_rejected() {
    let finding = fixture_findings().remove(0);
    let outcome = synthesize_candidate(&finding, &synthesis_manifest(), &MaliciousProvider);
    let SynthesisOutcome::Rejected { diagnostics, .. } = outcome else {
        panic!("malicious provider response must be rejected");
    };
    let codes = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(codes.contains(&"unknown_evidence"), "codes: {codes:?}");
    assert!(codes.contains(&"unknown_selector"), "codes: {codes:?}");
    assert!(codes.contains(&"excessive_permission"), "codes: {codes:?}");
}

#[test]
fn missing_capability_refuses_without_consulting_a_provider() {
    let finding = fixture_findings().remove(0);
    let manifest = ModelManifest {
        spec_version: ManifestSpecVersion::V0_1,
        name: "embedding-only".to_owned(),
        format: ModelFormat::Gguf,
        path: "/models/embed.gguf".to_owned(),
        revision: "v1".to_owned(),
        digest: None,
        capabilities: vec![Capability::Embedding],
        resource_hints: ResourceHints {
            min_memory_mb: 512,
            recommended_memory_mb: None,
            context_window_tokens: None,
        },
        timeouts: None,
        api_key_env: None,
        model: None,
    };
    // Tripwire provider must not be consulted when the capability is absent.
    let outcome = synthesize_candidate(&finding, &manifest, &TripwireProvider);
    let SynthesisOutcome::Rejected { diagnostics, .. } = outcome else {
        panic!("missing capability must be rejected");
    };
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "missing_capability")
    );
}

#[test]
fn declining_provider_yields_a_structured_refusal() {
    let finding = fixture_findings().remove(0);
    let outcome = synthesize_candidate(&finding, &synthesis_manifest(), &DecliningProvider);
    assert!(matches!(outcome, SynthesisOutcome::ProviderDeclined { .. }));
}

#[test]
fn valid_manifest_fixtures_load() {
    let base =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/synthesis/0.1/manifest/valid");
    for name in [
        "local_gguf.json",
        "ollama_minimal.json",
        "deterministic.json",
    ] {
        let manifest = ModelManifest::from_path(&base.join(name))
            .unwrap_or_else(|error| panic!("{name} should load: {error}"));
        assert!(manifest.declares(Capability::MutationSynthesis), "{name}");
    }
}

#[test]
fn invalid_manifest_fixtures_are_rejected_with_precise_errors() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/specs/synthesis/0.1/manifest/invalid");
    // Structural violations fail to deserialize.
    for name in [
        "unknown_field.json",
        "bad_spec_version.json",
        "bad_format.json",
    ] {
        assert!(
            matches!(
                ModelManifest::from_path(&base.join(name)),
                Err(ManifestError::Malformed { .. })
            ),
            "{name} should be malformed"
        );
    }
    // Semantic violations parse but fail validation.
    for name in [
        "empty_capabilities.json",
        "blank_name.json",
        "zero_memory.json",
    ] {
        assert!(
            matches!(
                ModelManifest::from_path(&base.join(name)),
                Err(ManifestError::Invalid(_))
            ),
            "{name} should be semantically invalid"
        );
    }
}

#[test]
fn missing_manifest_file_reports_a_readable_error() {
    let error = ModelManifest::from_path(Path::new("/no/such/manifest.json"))
        .expect_err("missing file should error");
    assert!(matches!(error, ManifestError::Unreadable { .. }));
}

#[test]
fn manifest_schema_accepts_valid_and_rejects_invalid_fixtures() {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/specs/synthesis/0.1/manifest.schema.json"
    ))
    .expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    let base =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/synthesis/0.1/manifest");
    for name in [
        "local_gguf.json",
        "ollama_minimal.json",
        "deterministic.json",
    ] {
        let instance = read_json(&base.join("valid").join(name));
        assert!(
            validator.is_valid(&instance),
            "schema rejected valid {name}"
        );
    }
    for name in [
        "unknown_field.json",
        "bad_spec_version.json",
        "bad_format.json",
        "empty_capabilities.json",
        "blank_name.json",
        "zero_memory.json",
    ] {
        let instance = read_json(&base.join("invalid").join(name));
        assert!(
            !validator.is_valid(&instance),
            "schema accepted invalid {name}"
        );
    }
}

#[test]
fn response_schema_accepts_valid_and_rejects_invalid_fixtures() {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/specs/synthesis/0.1/response.schema.json"
    ))
    .expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    let base =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/synthesis/0.1/response");
    for name in ["deterministic_echo.json", "enriched.json"] {
        let instance = read_json(&base.join("valid").join(name));
        assert!(
            validator.is_valid(&instance),
            "schema rejected valid {name}"
        );
    }
    for name in [
        "excessive_permissions.json",
        "too_few_supporting.json",
        "bad_event_id.json",
        "empty_failure_cases.json",
        "unknown_field.json",
    ] {
        let instance = read_json(&base.join("invalid").join(name));
        assert!(
            !validator.is_valid(&instance),
            "schema accepted invalid {name}"
        );
    }
}

#[test]
fn mutation_v0_2_schema_accepts_valid_and_rejects_invalid_fixtures() {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/specs/mutation/0.2/schema.json"))
            .expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/mutation/0.2");
    for name in ["model_synthesized.json", "model_synthesized_no_digest.json"] {
        let instance = read_json(&base.join("valid").join(name));
        assert!(
            validator.is_valid(&instance),
            "schema rejected valid {name}"
        );
    }
    for name in [
        "missing_provenance.json",
        "deterministic_generated_by.json",
        "blank_provider.json",
        "unknown_field.json",
        "excessive_permissions.json",
    ] {
        let instance = read_json(&base.join("invalid").join(name));
        assert!(
            !validator.is_valid(&instance),
            "schema accepted invalid {name}"
        );
    }
}

#[test]
fn mutation_v0_1_fixtures_still_validate_against_v0_1_schema() {
    // The v0.1 contract is untouched: a v0.1 package (no provenance) parses,
    // validates, and round-trips byte-for-byte.
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/specs/mutation/0.1/schema.json"))
            .expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    let findings = fixture_findings();
    let manifest = synthesis_manifest();
    let provider = DeterministicReferenceProvider;
    let mut checked = 0;
    for outcome in synthesize_candidates(&findings, &manifest, &provider) {
        let SynthesisOutcome::Candidate { package, .. } = outcome else {
            continue;
        };
        checked += 1;
        // Deterministic candidates stay v0.1 with no provenance.
        assert!(
            package.provenance.is_none(),
            "deterministic must not stamp provenance"
        );
        let value = serde_json::to_value(&*package).expect("package JSON");
        assert!(
            value.get("provenance").is_none(),
            "v0.1 package must not serialize a provenance key"
        );
        assert!(validator.is_valid(&value), "v0.1 schema rejected {value}");
        // Round-trip.
        let round: autophagy_mutations::MutationPackage =
            serde_json::from_value(value.clone()).expect("round-trip");
        assert_eq!(&round, &*package);
    }
    assert!(checked >= 1);
}

#[test]
fn manifest_v0_2_schema_accepts_valid_and_rejects_invalid_fixtures() {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/specs/synthesis/0.2/manifest.schema.json"
    ))
    .expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    let base =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/synthesis/0.2/manifest");
    for name in [
        "ollama_endpoint.json",
        "openai_compatible_hosted.json",
        "minimal_no_extras.json",
    ] {
        let instance = read_json(&base.join("valid").join(name));
        assert!(
            validator.is_valid(&instance),
            "schema rejected valid {name}"
        );
    }
    for name in [
        "api_key_inline.json",
        "zero_timeout.json",
        "blank_api_key_env.json",
        "wrong_spec_version.json",
    ] {
        let instance = read_json(&base.join("invalid").join(name));
        assert!(
            !validator.is_valid(&instance),
            "schema accepted invalid {name}"
        );
    }
}

#[test]
fn manifest_v0_2_fields_load_and_are_validated() {
    let base =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/synthesis/0.2/manifest");
    let ollama = ModelManifest::from_path(&base.join("valid/ollama_endpoint.json"))
        .expect("ollama endpoint manifest loads");
    assert_eq!(ollama.spec_version, ManifestSpecVersion::V0_2);
    assert_eq!(ollama.format, ModelFormat::Ollama);
    let timeouts = ollama.timeouts.expect("timeouts present");
    assert_eq!(timeouts.connect_ms, Some(2000));
    assert_eq!(timeouts.request_ms, Some(90000));

    let hosted = ModelManifest::from_path(&base.join("valid/openai_compatible_hosted.json"))
        .expect("hosted manifest loads");
    assert_eq!(
        hosted.api_key_env.as_deref(),
        Some("AUTOPHAGY_OPENAI_API_KEY")
    );

    // An inline api_key is an unknown field: it must never be accepted.
    assert!(matches!(
        ModelManifest::from_path(&base.join("invalid/api_key_inline.json")),
        Err(ManifestError::Malformed { .. })
    ));
    // A v0.1 manifest carrying a v0.2-only field is semantically invalid.
    assert!(matches!(
        ModelManifest::from_path(&base.join("invalid/wrong_spec_version.json")),
        Err(ManifestError::Malformed { .. } | ManifestError::Invalid(_))
    ));
    // A blank api_key_env is semantically invalid.
    assert!(matches!(
        ModelManifest::from_path(&base.join("invalid/blank_api_key_env.json")),
        Err(ManifestError::Invalid(_))
    ));
}

#[test]
fn v0_1_manifest_rejects_v0_2_only_fields() {
    let manifest = ModelManifest {
        spec_version: ManifestSpecVersion::V0_1,
        name: "m".to_owned(),
        format: ModelFormat::Ollama,
        path: "http://localhost:11434".to_owned(),
        revision: "v1".to_owned(),
        digest: None,
        capabilities: vec![Capability::MutationSynthesis],
        resource_hints: ResourceHints {
            min_memory_mb: 1,
            recommended_memory_mb: None,
            context_window_tokens: None,
        },
        timeouts: None,
        api_key_env: Some("SOME_VAR".to_owned()),
        model: None,
    };
    assert!(matches!(
        manifest.validate(),
        Err(ManifestError::Invalid(_))
    ));
}

#[test]
fn manifest_v0_3_schema_accepts_valid_and_rejects_invalid_fixtures() {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/specs/synthesis/0.3/manifest.schema.json"
    ))
    .expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    let base =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/synthesis/0.3/manifest");
    for name in [
        "claude_cli.json",
        "codex_cli.json",
        "ollama_carried_forward.json",
    ] {
        let instance = read_json(&base.join("valid").join(name));
        assert!(
            validator.is_valid(&instance),
            "schema rejected valid {name}"
        );
    }
    for name in [
        "wrong_spec_version.json",
        "model_requires_v0_3.json",
        "blank_model.json",
        "unknown_field.json",
    ] {
        let instance = read_json(&base.join("invalid").join(name));
        assert!(
            !validator.is_valid(&instance),
            "schema accepted invalid {name}"
        );
    }
}

#[test]
fn manifest_v0_3_fields_load_and_are_validated() {
    let base =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/synthesis/0.3/manifest");

    let claude = ModelManifest::from_path(&base.join("valid/claude_cli.json"))
        .expect("claude_cli manifest loads");
    assert_eq!(claude.spec_version, ManifestSpecVersion::V0_3);
    assert_eq!(claude.format, ModelFormat::ClaudeCli);
    assert_eq!(claude.path, "claude");
    assert!(claude.model.is_none());

    let codex = ModelManifest::from_path(&base.join("valid/codex_cli.json"))
        .expect("codex_cli manifest loads");
    assert_eq!(codex.format, ModelFormat::CodexCli);
    assert_eq!(codex.model.as_deref(), Some("gpt-5-codex"));
    assert_eq!(codex.timeouts.and_then(|t| t.request_ms), Some(180_000));

    // An agent-CLI format under an older spec version is semantically invalid.
    assert!(matches!(
        ModelManifest::from_path(&base.join("invalid/wrong_spec_version.json")),
        Err(ManifestError::Invalid(_))
    ));
    // A `model` field under an older spec version is semantically invalid.
    assert!(matches!(
        ModelManifest::from_path(&base.join("invalid/model_requires_v0_3.json")),
        Err(ManifestError::Invalid(_))
    ));
    // A blank `model` is semantically invalid.
    assert!(matches!(
        ModelManifest::from_path(&base.join("invalid/blank_model.json")),
        Err(ManifestError::Invalid(_))
    ));
    // An inline api_key (or any unknown field) fails to deserialize.
    assert!(matches!(
        ModelManifest::from_path(&base.join("invalid/unknown_field.json")),
        Err(ManifestError::Malformed { .. })
    ));
}

#[test]
fn older_manifests_still_load_under_the_v0_3_rust_types() {
    // Additive compatibility: v0.1 and v0.2 manifests keep loading unchanged
    // through the same Rust types that now understand v0.3.
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/synthesis");
    let v0_1 = ModelManifest::from_path(&base.join("0.1/manifest/valid/ollama_minimal.json"))
        .expect("v0.1 manifest still loads");
    assert_eq!(v0_1.spec_version, ManifestSpecVersion::V0_1);
    assert!(v0_1.model.is_none());
    let v0_2 = ModelManifest::from_path(&base.join("0.2/manifest/valid/ollama_endpoint.json"))
        .expect("v0.2 manifest still loads");
    assert_eq!(v0_2.spec_version, ManifestSpecVersion::V0_2);
    assert!(v0_2.model.is_none());
}

#[test]
fn pre_v0_3_manifest_rejects_the_model_field() {
    let manifest = ModelManifest {
        spec_version: ManifestSpecVersion::V0_2,
        name: "m".to_owned(),
        format: ModelFormat::Ollama,
        path: "http://localhost:11434".to_owned(),
        revision: "v1".to_owned(),
        digest: None,
        capabilities: vec![Capability::MutationSynthesis],
        resource_hints: ResourceHints {
            min_memory_mb: 1,
            recommended_memory_mb: None,
            context_window_tokens: None,
        },
        timeouts: None,
        api_key_env: None,
        model: Some("some-model".to_owned()),
    };
    assert!(matches!(
        manifest.validate(),
        Err(ManifestError::Invalid(_))
    ));
}

fn read_json(path: &Path) -> serde_json::Value {
    let display = path.display();
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("read {display}: {error}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("parse {display}: {error}"))
}
