//! Agent-CLI synthesis provider tests against fake CLI scripts.
//!
//! No real `claude` or `codex` process is launched: each test writes a tiny
//! executable shell script that emits a canned envelope (or sleeps, or exits
//! non-zero), points the provider's manifest `path` at it, and exercises the
//! whole subprocess path — argv assembly, timeout-with-kill, envelope parse,
//! token accounting, decline/reject routing, and stderr sanitization. The tests
//! are Unix-only because they rely on executable shell scripts.
#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use autophagy_mutations::{GenerationOutcome, MutationSpecVersion, generate_candidate};
use autophagy_patterns::{DetectorConfig, EvidencePacket, detect};
use autophagy_store::EventStore;
use autophagy_synthesis::{
    AgentCliProvider, Capability, ManifestSpecVersion, ModelFormat, ModelManifest, ResourceHints,
    SynthesisOutcome, synthesize_candidate,
};
use tempfile::TempDir;

const CORPUS: &str = include_str!("../../../evals/fixtures/findings/deterministic.jsonl");

fn fixture_findings() -> Vec<EvidencePacket> {
    use std::io::Cursor;

    use autophagy_core::{ImportOptions, import_jsonl};
    let mut store = EventStore::open_in_memory().expect("store");
    import_jsonl(
        Cursor::new(CORPUS),
        Some(&mut store),
        &ImportOptions::new("fixture:agent-cli-synthesis"),
    )
    .expect("import");
    detect(
        &store.list_events_for_detection(None).expect("events"),
        DetectorConfig::default(),
    )
}

/// A finding that yields a deterministic candidate, plus the exact evidence and
/// selector the boundary will require of any provider response.
struct Fixture {
    finding: EvidencePacket,
    supporting: Vec<String>,
    counterexamples: Vec<String>,
    selector: String,
}

fn candidate_fixture() -> Fixture {
    for finding in fixture_findings() {
        if let GenerationOutcome::Candidate { package } = generate_candidate(&finding) {
            return Fixture {
                supporting: package.hypothesis.supporting_event_ids.clone(),
                counterexamples: package.hypothesis.counterexample_event_ids.clone(),
                selector: package.triggers[0].selector.clone(),
                finding,
            };
        }
    }
    panic!("fixture corpus must yield at least one deterministic candidate");
}

/// A well-formed synthesis response that echoes the given allowed evidence.
fn response_content(fixture: &Fixture) -> String {
    let support = serde_json::to_string(&fixture.supporting).unwrap();
    let counter = serde_json::to_string(&fixture.counterexamples).unwrap();
    let selectors = serde_json::to_string(&[&fixture.selector]).unwrap();
    format!(
        "{{\"title\":\"Refined title\",\"statement\":\"A sharper falsifiable statement.\",\
\"expected_result\":\"A sharper observable expectation.\",\"instruction\":\"A specific instruction.\",\
\"failure_cases\":[\"It could be a coincidence.\"],\"exclusions\":[],\
\"supporting_event_ids\":{support},\"counterexample_event_ids\":{counter},\
\"trigger_selectors\":{selectors},\
\"permissions\":{{\"filesystem_read\":[],\"filesystem_write\":[],\"commands\":[],\"environment\":[],\"network\":false}}}}"
    )
}

/// The Claude Code `--output-format json` envelope carrying `result` as text.
fn claude_envelope(result: &str, input_tokens: u64, output_tokens: u64) -> String {
    serde_json::json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": result,
        "usage": { "input_tokens": input_tokens, "output_tokens": output_tokens }
    })
    .to_string()
}

/// The Codex `exec --json` JSONL event stream (with a non-JSON hook line to
/// prove decoration is skipped).
fn codex_events(message: &str, input_tokens: u64, output_tokens: u64) -> String {
    let item = serde_json::json!({
        "type": "item.completed",
        "item": { "id": "item_0", "type": "agent_message", "text": message }
    })
    .to_string();
    let turn = serde_json::json!({
        "type": "turn.completed",
        "usage": {
            "input_tokens": input_tokens,
            "cached_input_tokens": 0,
            "output_tokens": output_tokens
        }
    })
    .to_string();
    format!("{{\"type\":\"thread.started\"}}\nhook: SessionStart\n{item}\n{turn}\n")
}

/// Write an executable fake CLI: it optionally sleeps, prints `stdout`, prints
/// `stderr`, then exits with `exit_code`. Returns the script path.
fn write_fake_cli(
    dir: &Path,
    name: &str,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
    sleep_secs: u32,
) -> PathBuf {
    let stdout_path = dir.join(format!("{name}.stdout"));
    let stderr_path = dir.join(format!("{name}.stderr"));
    fs::write(&stdout_path, stdout).expect("write stdout payload");
    fs::write(&stderr_path, stderr).expect("write stderr payload");
    let sleep = if sleep_secs > 0 {
        format!("sleep {sleep_secs}\n")
    } else {
        String::new()
    };
    let script = format!(
        "#!/bin/sh\n{sleep}cat \"{}\"\ncat \"{}\" >&2\nexit {exit_code}\n",
        stdout_path.display(),
        stderr_path.display()
    );
    let script_path = dir.join(name);
    fs::write(&script_path, script).expect("write script");
    let mut perms = fs::metadata(&script_path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).expect("chmod");
    script_path
}

fn cli_manifest(format: ModelFormat, binary: &Path, request_ms: Option<u64>) -> ModelManifest {
    let timeouts = request_ms.map(|request_ms| autophagy_synthesis::ManifestTimeouts {
        connect_ms: None,
        request_ms: Some(request_ms),
    });
    ModelManifest {
        spec_version: ManifestSpecVersion::V0_3,
        name: "cli-login".to_owned(),
        format,
        path: binary.display().to_string(),
        revision: "test-rev".to_owned(),
        digest: None,
        capabilities: vec![Capability::MutationSynthesis],
        resource_hints: ResourceHints {
            min_memory_mb: 1,
            recommended_memory_mb: None,
            context_window_tokens: None,
        },
        timeouts,
        api_key_env: None,
        model: None,
    }
}

#[test]
fn claude_cli_happy_path_produces_a_v0_2_candidate() {
    let dir = TempDir::new().expect("tempdir");
    let fixture = candidate_fixture();
    let envelope = claude_envelope(&response_content(&fixture), 700, 120);
    let binary = write_fake_cli(dir.path(), "claude", &envelope, "", 0, 0);
    let manifest = cli_manifest(ModelFormat::ClaudeCli, &binary, None);
    let provider = AgentCliProvider::claude_from_manifest(&manifest);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let SynthesisOutcome::Candidate {
        package,
        usage,
        model_used,
        provider: name,
        ..
    } = outcome
    else {
        panic!("expected a candidate, got {outcome:?}");
    };
    assert_eq!(name, "claude-cli");
    assert!(model_used);
    assert_eq!(package.spec_version, MutationSpecVersion::V0_2);
    let provenance = package.provenance.as_ref().expect("provenance stamped");
    assert_eq!(provenance.provider, "claude-cli");
    assert_eq!(provenance.manifest_spec_version, "synthesis-manifest/0.3");
    assert_eq!(package.title, "Refined title");
    assert!(package.permissions.commands.is_empty());
    assert_eq!(usage.prompt_tokens, Some(700));
    assert_eq!(usage.completion_tokens, Some(120));
    package.validate().expect("candidate is contract-valid");
}

#[test]
fn codex_cli_happy_path_produces_a_v0_2_candidate() {
    let dir = TempDir::new().expect("tempdir");
    let fixture = candidate_fixture();
    let stream = codex_events(&response_content(&fixture), 13_000, 90);
    let binary = write_fake_cli(dir.path(), "codex", &stream, "", 0, 0);
    let manifest = cli_manifest(ModelFormat::CodexCli, &binary, None);
    let provider = AgentCliProvider::codex_from_manifest(&manifest);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let SynthesisOutcome::Candidate {
        package,
        usage,
        provider: name,
        ..
    } = outcome
    else {
        panic!("expected a candidate, got {outcome:?}");
    };
    assert_eq!(name, "codex-cli");
    assert_eq!(package.spec_version, MutationSpecVersion::V0_2);
    assert_eq!(usage.prompt_tokens, Some(13_000));
    assert_eq!(usage.completion_tokens, Some(90));
    package.validate().expect("candidate is contract-valid");
}

#[test]
fn garbage_output_is_a_structured_decline_not_a_panic() {
    let dir = TempDir::new().expect("tempdir");
    let fixture = candidate_fixture();
    let envelope = claude_envelope("this is not json at all {{{", 42, 7);
    let binary = write_fake_cli(dir.path(), "claude", &envelope, "", 0, 0);
    let manifest = cli_manifest(ModelFormat::ClaudeCli, &binary, None);
    let provider = AgentCliProvider::claude_from_manifest(&manifest);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let SynthesisOutcome::ProviderDeclined { reason, usage, .. } = outcome else {
        panic!("expected a structured decline, got {outcome:?}");
    };
    assert!(
        reason.contains("not a valid synthesis response"),
        "reason: {reason}"
    );
    // Usage is still surfaced even when the model text could not be parsed.
    assert_eq!(usage.prompt_tokens, Some(42));
}

#[test]
fn fabricated_evidence_flows_into_the_rejection_path() {
    let dir = TempDir::new().expect("tempdir");
    let fixture = candidate_fixture();
    // A well-formed response that cites invented evidence and escalates network.
    let fabricated = "{\"title\":\"t\",\"statement\":\"s\",\"expected_result\":\"e\",\
\"instruction\":\"i\",\"failure_cases\":[\"f\"],\"exclusions\":[],\
\"supporting_event_ids\":[\"evt_invented_one\",\"evt_invented_two\"],\
\"counterexample_event_ids\":[],\"trigger_selectors\":[\"made/up/selector\"],\
\"permissions\":{\"filesystem_read\":[],\"filesystem_write\":[],\"commands\":[],\
\"environment\":[],\"network\":true}}";
    let envelope = claude_envelope(fabricated, 200, 40);
    let binary = write_fake_cli(dir.path(), "claude", &envelope, "", 0, 0);
    let manifest = cli_manifest(ModelFormat::ClaudeCli, &binary, None);
    let provider = AgentCliProvider::claude_from_manifest(&manifest);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let SynthesisOutcome::Rejected { diagnostics, .. } = outcome else {
        panic!("expected a rejection, got {outcome:?}");
    };
    let codes: Vec<&str> = diagnostics.iter().map(|d| d.code).collect();
    assert!(codes.contains(&"unknown_evidence"), "codes: {codes:?}");
    assert!(codes.contains(&"unknown_selector"), "codes: {codes:?}");
    assert!(codes.contains(&"excessive_permission"), "codes: {codes:?}");
}

#[test]
fn a_hung_cli_is_killed_and_surfaces_a_timeout_provider_error() {
    let dir = TempDir::new().expect("tempdir");
    let fixture = candidate_fixture();
    // The script sleeps far past the 300 ms wall-clock timeout.
    let binary = write_fake_cli(dir.path(), "codex", "unused", "", 0, 30);
    let manifest = cli_manifest(ModelFormat::CodexCli, &binary, Some(300));
    let provider = AgentCliProvider::codex_from_manifest(&manifest);

    let started = std::time::Instant::now();
    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    // The child must be killed promptly, not waited out for 30 s.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "timeout should fire well before the child's own sleep"
    );
    let SynthesisOutcome::ProviderError {
        message,
        provider: name,
        ..
    } = outcome
    else {
        panic!("expected a provider error, got {outcome:?}");
    };
    assert_eq!(name, "codex-cli");
    assert!(message.contains("timed out"), "message: {message}");
}

#[test]
fn a_leaked_grandchild_holding_stdout_does_not_hang_propose() {
    // Regression: both vendor CLIs spawn helpers that inherit the stdout pipe.
    // If, on timeout, only the direct child is killed, a surviving grandchild
    // keeps the pipe open, the reader never sees EOF, and propose() hangs. The
    // provider must kill the whole process group and return within the timeout.
    let dir = TempDir::new().expect("tempdir");
    // Grandchild `sleep 300` inherits stdout; the foreground `sleep 300` keeps
    // the direct child alive so the provider must time out and kill the group.
    let script = "#!/bin/sh\nsleep 300 &\nsleep 300\n";
    let script_path = dir.path().join("codex");
    fs::write(&script_path, script).expect("write script");
    let mut perms = fs::metadata(&script_path).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).expect("chmod");

    let manifest = cli_manifest(ModelFormat::CodexCli, &script_path, Some(500));

    // Bound the whole thing with an outer watchdog so a regression fails fast
    // instead of hanging the test suite forever.
    let (tx, rx) = std::sync::mpsc::channel();
    let finding = candidate_fixture().finding;
    std::thread::spawn(move || {
        let provider = AgentCliProvider::codex_from_manifest(&manifest);
        let outcome = synthesize_candidate(&finding, &manifest, &provider);
        let _ = tx.send(outcome);
    });
    let outcome = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("propose() must return within the timeout, not hang on a leaked grandchild");
    let SynthesisOutcome::ProviderError { message, .. } = outcome else {
        panic!("expected a provider error, got {outcome:?}");
    };
    assert!(message.contains("timed out"), "message: {message}");
}

#[test]
fn a_missing_binary_is_a_clean_provider_error() {
    let fixture = candidate_fixture();
    let manifest = cli_manifest(
        ModelFormat::ClaudeCli,
        Path::new("/no/such/autophagy-fake-agent-cli"),
        None,
    );
    let provider = AgentCliProvider::claude_from_manifest(&manifest);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let SynthesisOutcome::ProviderError { message, .. } = outcome else {
        panic!("expected a provider error, got {outcome:?}");
    };
    assert!(
        message.contains("could not be launched") || message.contains("not found"),
        "message: {message}"
    );
}

#[test]
fn a_nonzero_exit_surfaces_a_bounded_sanitized_stderr_snippet() {
    let dir = TempDir::new().expect("tempdir");
    let fixture = candidate_fixture();
    // Noisy, multi-line, control-character-laden stderr, well over the cap.
    let noisy = format!(
        "auth error line one\n\tline two with tab\r\n\x1b[31mred\x1b[0m {}",
        "x".repeat(2000)
    );
    let binary = write_fake_cli(dir.path(), "claude", "", &noisy, 1, 0);
    let manifest = cli_manifest(ModelFormat::ClaudeCli, &binary, None);
    let provider = AgentCliProvider::claude_from_manifest(&manifest);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let SynthesisOutcome::ProviderError { message, .. } = outcome else {
        panic!("expected a provider error, got {outcome:?}");
    };
    assert!(
        message.contains("exited with status 1"),
        "message: {message}"
    );
    // Sanitized: newlines, tabs, and escape sequences are collapsed away.
    assert!(!message.contains('\n'), "message must be single-line");
    assert!(!message.contains('\t'), "message must not carry tabs");
    assert!(!message.contains('\x1b'), "message must strip escape codes");
    assert!(
        message.contains("auth error line one"),
        "message: {message}"
    );
    // Bounded: the 2000-char run must have been truncated with an ellipsis.
    assert!(message.contains('…'), "long stderr should be truncated");
    assert!(
        message.len() < 800,
        "sanitized snippet must be bounded, was {}",
        message.len()
    );
}
