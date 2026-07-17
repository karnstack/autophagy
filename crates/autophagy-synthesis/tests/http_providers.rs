//! HTTP synthesis provider tests against a hand-rolled mock server.
//!
//! No external HTTP-mock dependency: a tiny `std::net::TcpListener` responder
//! serves one canned reply so the whole provider path — locality guard, prompt
//! construction, request, response parse, token accounting, and boundary
//! re-validation — is exercised offline and deterministically.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use autophagy_mutations::{
    GenerationOutcome, MutationSpecVersion, TriggerKind, generate_candidate,
};
use autophagy_patterns::{DetectorConfig, EvidencePacket, detect};
use autophagy_store::EventStore;
use autophagy_synthesis::{
    Capability, MAX_COMPLETION_TOKENS, ManifestSpecVersion, ManifestTimeouts, ModelFormat,
    ModelManifest, OllamaProvider, OpenAiCompatibleProvider, ResourceHints, SYSTEM_PROMPT,
    SynthesisBaseline, SynthesisConstraints, SynthesisOutcome, SynthesisRequest,
    synthesize_candidate, user_prompt,
};

const CORPUS: &str = include_str!("../../../evals/fixtures/findings/deterministic.jsonl");

fn fixture_findings() -> Vec<EvidencePacket> {
    use std::io::Cursor;

    use autophagy_core::{ImportOptions, import_jsonl};
    let mut store = EventStore::open_in_memory().expect("store");
    import_jsonl(
        Cursor::new(CORPUS),
        Some(&mut store),
        &ImportOptions::new("fixture:http-synthesis"),
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

fn manifest(format: ModelFormat, endpoint: &str, timeout_ms: Option<u64>) -> ModelManifest {
    ModelManifest {
        spec_version: ManifestSpecVersion::V0_2,
        name: "test-model".to_owned(),
        format,
        path: endpoint.to_owned(),
        revision: "test-rev".to_owned(),
        digest: None,
        capabilities: vec![Capability::MutationSynthesis],
        resource_hints: ResourceHints {
            min_memory_mb: 1,
            recommended_memory_mb: None,
            context_window_tokens: None,
        },
        timeouts: timeout_ms.map(|request_ms| ManifestTimeouts {
            connect_ms: Some(1000),
            request_ms: Some(request_ms),
        }),
        api_key_env: None,
        model: None,
    }
}

/// Build a JSON string for a synthesis response that echoes the given evidence.
fn response_content(fixture: &Fixture, permissions_network: bool) -> String {
    let support = serde_json::to_string(&fixture.supporting).unwrap();
    let counter = serde_json::to_string(&fixture.counterexamples).unwrap();
    let selectors = serde_json::to_string(&[&fixture.selector]).unwrap();
    format!(
        "{{\"title\":\"Refined title\",\"statement\":\"A sharper falsifiable statement.\",\
\"expected_result\":\"A sharper observable expectation.\",\"instruction\":\"A specific instruction.\",\
\"failure_cases\":[\"It could be a coincidence.\"],\"exclusions\":[],\
\"supporting_event_ids\":{support},\"counterexample_event_ids\":{counter},\
\"trigger_selectors\":{selectors},\
\"permissions\":{{\"filesystem_read\":[],\"filesystem_write\":[],\"commands\":[],\"environment\":[],\"network\":{permissions_network}}}}}"
    )
}

fn ollama_envelope(content: &str, prompt_tokens: u64, eval_tokens: u64) -> String {
    let content = serde_json::to_string(content).unwrap();
    format!(
        "{{\"model\":\"test-model\",\"message\":{{\"role\":\"assistant\",\"content\":{content}}},\
\"done\":true,\"prompt_eval_count\":{prompt_tokens},\"eval_count\":{eval_tokens}}}"
    )
}

fn openai_envelope(content: &str, prompt_tokens: u64, completion_tokens: u64) -> String {
    let content = serde_json::to_string(content).unwrap();
    format!(
        "{{\"choices\":[{{\"index\":0,\"message\":{{\"role\":\"assistant\",\"content\":{content}}}}}],\
\"usage\":{{\"prompt_tokens\":{prompt_tokens},\"completion_tokens\":{completion_tokens},\"total_tokens\":{}}}}}",
        prompt_tokens + completion_tokens
    )
}

/// A single-shot mock HTTP server. Serves one canned response, returns the raw
/// request it received (for header inspection).
struct MockServer {
    endpoint: String,
    handle: thread::JoinHandle<String>,
}

impl MockServer {
    /// Serve one request with the given status and JSON body.
    fn serve(status: u16, body: String) -> Self {
        Self::serve_after(status, body, Duration::ZERO)
    }

    /// Serve one request with a 307 redirect to `location`, then capture whether
    /// a *second* request ever arrives (it must not: redirects are disabled).
    fn serve_redirect(location: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let endpoint = format!("http://{}", listener.local_addr().expect("addr"));
        let location = location.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let request = read_request(&mut stream);
            let response = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            request
        });
        Self { endpoint, handle }
    }

    /// Serve one request after a delay (to force client-side timeouts).
    fn serve_after(status: u16, body: String, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let endpoint = format!("http://{}", listener.local_addr().expect("addr"));
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let request = read_request(&mut stream);
            if !delay.is_zero() {
                thread::sleep(delay);
            }
            let response = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            // Ignore write failures: a timed-out client may already be gone.
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            request
        });
        Self { endpoint, handle }
    }

    fn received_request(self) -> String {
        self.handle.join().expect("mock server thread")
    }
}

/// Read a full HTTP request (headers plus Content-Length body) into a string.
fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut reader = BufReader::new(stream);
    let mut raw = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if let Some(value) = line
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
            .map(str::trim)
        {
            content_length = value.parse().unwrap_or(0);
        }
        raw.extend_from_slice(line.as_bytes());
        if line == "\r\n" {
            break;
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).expect("read body");
    raw.extend_from_slice(&body);
    String::from_utf8_lossy(&raw).into_owned()
}

#[test]
fn ollama_happy_path_produces_a_v0_2_candidate() {
    let fixture = candidate_fixture();
    let content = response_content(&fixture, false);
    let mock = MockServer::serve(200, ollama_envelope(&content, 210, 95));
    let manifest = manifest(ModelFormat::Ollama, &mock.endpoint, None);
    let provider = OllamaProvider::from_manifest(&manifest, false);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let _ = mock.received_request();

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
    assert_eq!(name, "ollama");
    assert!(model_used);
    assert_eq!(package.spec_version, MutationSpecVersion::V0_2);
    let provenance = package.provenance.as_ref().expect("provenance stamped");
    assert_eq!(provenance.provider, "ollama");
    assert_eq!(provenance.model_name, "test-model");
    assert_eq!(provenance.manifest_spec_version, "synthesis-manifest/0.2");
    assert_eq!(package.title, "Refined title");
    assert!(package.permissions.commands.is_empty());
    assert_eq!(usage.prompt_tokens, Some(210));
    assert_eq!(usage.completion_tokens, Some(95));
    package.validate().expect("candidate is contract-valid");
}

#[test]
fn openai_happy_path_produces_a_v0_2_candidate() {
    let fixture = candidate_fixture();
    let content = response_content(&fixture, false);
    let mock = MockServer::serve(200, openai_envelope(&content, 205, 88));
    let manifest = manifest(ModelFormat::OpenAiCompatible, &mock.endpoint, None);
    let provider = OpenAiCompatibleProvider::from_manifest(&manifest, false);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let _ = mock.received_request();

    let SynthesisOutcome::Candidate {
        package,
        usage,
        provider: name,
        ..
    } = outcome
    else {
        panic!("expected a candidate, got {outcome:?}");
    };
    assert_eq!(name, "openai-compatible");
    assert_eq!(package.spec_version, MutationSpecVersion::V0_2);
    assert_eq!(usage.prompt_tokens, Some(205));
    assert_eq!(usage.completion_tokens, Some(88));
    package.validate().expect("candidate is contract-valid");
}

#[test]
fn garbage_json_is_a_structured_decline_not_a_panic() {
    let fixture = candidate_fixture();
    let mock = MockServer::serve(200, ollama_envelope("this is not json at all {{{", 10, 2));
    let manifest = manifest(ModelFormat::Ollama, &mock.endpoint, None);
    let provider = OllamaProvider::from_manifest(&manifest, false);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let _ = mock.received_request();

    let SynthesisOutcome::ProviderDeclined { reason, usage, .. } = outcome else {
        panic!("expected a structured decline, got {outcome:?}");
    };
    assert!(
        reason.contains("not a valid synthesis response"),
        "reason: {reason}"
    );
    // Usage is still surfaced even when the output could not be parsed.
    assert_eq!(usage.prompt_tokens, Some(10));
}

#[test]
fn fabricated_evidence_flows_into_the_rejection_path() {
    let fixture = candidate_fixture();
    // A well-formed response that cites invented evidence and escalates network.
    let content = "{\"title\":\"t\",\"statement\":\"s\",\"expected_result\":\"e\",\
\"instruction\":\"i\",\"failure_cases\":[\"f\"],\"exclusions\":[],\
\"supporting_event_ids\":[\"evt_invented_one\",\"evt_invented_two\"],\
\"counterexample_event_ids\":[],\"trigger_selectors\":[\"made/up/selector\"],\
\"permissions\":{\"filesystem_read\":[],\"filesystem_write\":[],\"commands\":[],\
\"environment\":[],\"network\":true}}";
    let mock = MockServer::serve(200, ollama_envelope(content, 200, 40));
    let manifest = manifest(ModelFormat::Ollama, &mock.endpoint, None);
    let provider = OllamaProvider::from_manifest(&manifest, false);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let _ = mock.received_request();

    let SynthesisOutcome::Rejected { diagnostics, .. } = outcome else {
        panic!("expected a rejection, got {outcome:?}");
    };
    let codes: Vec<&str> = diagnostics.iter().map(|d| d.code).collect();
    assert!(codes.contains(&"unknown_evidence"), "codes: {codes:?}");
    assert!(codes.contains(&"unknown_selector"), "codes: {codes:?}");
    assert!(codes.contains(&"excessive_permission"), "codes: {codes:?}");
}

#[test]
fn timeout_surfaces_a_clean_provider_error() {
    let fixture = candidate_fixture();
    let content = response_content(&fixture, false);
    // The mock delays well past the 200 ms request timeout.
    let mock = MockServer::serve_after(
        200,
        ollama_envelope(&content, 1, 1),
        Duration::from_millis(1500),
    );
    let manifest = manifest(ModelFormat::Ollama, &mock.endpoint, Some(200));
    let provider = OllamaProvider::from_manifest(&manifest, false);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);

    let SynthesisOutcome::ProviderError {
        message,
        provider: name,
        ..
    } = outcome
    else {
        panic!("expected a provider error, got {outcome:?}");
    };
    assert_eq!(name, "ollama");
    assert!(!message.is_empty());
}

#[test]
fn connection_refused_surfaces_a_clean_provider_error() {
    let fixture = candidate_fixture();
    // Bind then drop the listener so the port is closed: a clean transport error.
    let endpoint = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        format!("http://{}", listener.local_addr().expect("addr"))
    };
    let manifest = manifest(ModelFormat::Ollama, &endpoint, Some(1000));
    let provider = OllamaProvider::from_manifest(&manifest, false);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    assert!(
        matches!(outcome, SynthesisOutcome::ProviderError { .. }),
        "expected a provider error, got {outcome:?}"
    );
}

#[test]
fn loopback_endpoint_redirect_to_remote_host_is_not_followed() {
    // The vuln: a loopback endpoint answers 3xx with a non-loopback Location.
    // If redirects were followed, ureq would re-send the request (evidence IDs
    // and all) to that remote host, bypassing the locality guard. Redirects are
    // disabled, so this must surface a structured transport error, not a follow.
    let fixture = candidate_fixture();
    let mock = MockServer::serve_redirect("http://evil.example.com/api/chat");
    let manifest = manifest(ModelFormat::Ollama, &mock.endpoint, Some(1500));
    let provider = OllamaProvider::from_manifest(&manifest, false);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let _ = mock.received_request();

    let SynthesisOutcome::ProviderError { message, .. } = outcome else {
        panic!("a redirect must produce a structured error, got {outcome:?}");
    };
    // The guard fired (the redirect was returned to us, not followed). Had ureq
    // followed to evil.example.com, the failure would be a connection/DNS error,
    // not a redirect refusal.
    assert!(message.contains("redirect"), "message: {message}");
    assert!(
        !message.contains("evil.example.com"),
        "the redirect target must never be contacted or echoed: {message}"
    );
}

#[test]
fn transport_error_path_never_echoes_the_api_key() {
    // With an API key configured, a transport failure (connection refused) must
    // still never leak the key value into the error surfaced to the user.
    let fixture = candidate_fixture();
    let key_value = std::env::var("CARGO_PKG_NAME").expect("cargo sets CARGO_PKG_NAME");
    // A closed loopback port: the request fails after the key is attached.
    let mut manifest = manifest(
        ModelFormat::OpenAiCompatible,
        "http://127.0.0.1:9",
        Some(800),
    );
    manifest.api_key_env = Some("CARGO_PKG_NAME".to_owned());
    let provider = OpenAiCompatibleProvider::from_manifest(&manifest, false);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let SynthesisOutcome::ProviderError { ref message, .. } = outcome else {
        panic!("expected a transport error, got {outcome:?}");
    };
    assert!(
        !message.contains(&key_value),
        "transport error must never echo the API key value: {message}"
    );
    let outcome_json = serde_json::to_string(&outcome).expect("serialize outcome");
    assert!(
        !outcome_json.contains(&key_value),
        "serialized outcome must never contain the API key value"
    );
}

#[test]
fn non_loopback_endpoint_is_refused_without_the_flag() {
    let fixture = candidate_fixture();
    let manifest = manifest(ModelFormat::Ollama, "http://model.example.com:11434", None);
    let provider = OllamaProvider::from_manifest(&manifest, false);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let SynthesisOutcome::ProviderError { message, .. } = outcome else {
        panic!("expected refusal, got {outcome:?}");
    };
    assert!(message.contains("non-loopback"), "message: {message}");
    assert!(message.contains("model.example.com"), "message: {message}");
}

#[test]
fn api_key_is_read_from_env_and_sent_as_bearer_never_leaked() {
    let fixture = candidate_fixture();
    let content = response_content(&fixture, false);
    let mock = MockServer::serve(200, openai_envelope(&content, 100, 50));
    // CARGO_PKG_NAME is set by cargo at test runtime; use it as the key source
    // so no forbidden `std::env::set_var` is needed. Its value is the crate name.
    let key_value = std::env::var("CARGO_PKG_NAME").expect("cargo sets CARGO_PKG_NAME");
    let mut manifest = manifest(ModelFormat::OpenAiCompatible, &mock.endpoint, None);
    manifest.api_key_env = Some("CARGO_PKG_NAME".to_owned());
    let provider = OpenAiCompatibleProvider::from_manifest(&manifest, false);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let request = mock.received_request();

    // The key was sent as an Authorization: Bearer header.
    assert!(
        request.to_ascii_lowercase().contains(&format!(
            "authorization: bearer {}",
            key_value.to_ascii_lowercase()
        )),
        "expected a Bearer authorization header in the outbound request"
    );
    // The key never appears in the outcome that is serialized to output/DB.
    let outcome_json = serde_json::to_string(&outcome).expect("serialize outcome");
    assert!(
        !outcome_json.contains(&key_value),
        "the API key must never appear in the synthesis outcome"
    );
    assert!(matches!(outcome, SynthesisOutcome::Candidate { .. }));
}

#[test]
fn missing_api_key_env_is_a_clean_error_that_names_the_var_not_the_key() {
    let fixture = candidate_fixture();
    // No mock needed: the provider fails before any request when the key is absent.
    let mut manifest = manifest(ModelFormat::OpenAiCompatible, "http://localhost:1", None);
    manifest.api_key_env = Some("AUTOPHAGY_UNSET_KEY_VAR_XYZ".to_owned());
    let provider = OpenAiCompatibleProvider::from_manifest(&manifest, false);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let SynthesisOutcome::ProviderError { message, .. } = outcome else {
        panic!("expected a provider error, got {outcome:?}");
    };
    assert!(
        message.contains("AUTOPHAGY_UNSET_KEY_VAR_XYZ"),
        "error should name the missing variable: {message}"
    );
    assert!(
        !message.contains("Bearer"),
        "error must not echo a key: {message}"
    );
}

/// Reconstruct the structured request the boundary hands to a provider, so the
/// prompt built from it can be inspected and measured in tests.
fn request_from(finding: &EvidencePacket) -> Option<SynthesisRequest> {
    let GenerationOutcome::Candidate { package } = generate_candidate(finding) else {
        return None;
    };
    let trigger_kind = package
        .triggers
        .first()
        .map_or(TriggerKind::ToolCall, |trigger| trigger.kind);
    let selectors = package
        .triggers
        .iter()
        .map(|trigger| trigger.selector.clone())
        .collect();
    Some(SynthesisRequest {
        finding_id: finding.finding_id.clone(),
        detector: finding.detector,
        signature: finding.signature.clone(),
        constraints: SynthesisConstraints {
            allowed_supporting_event_ids: package.hypothesis.supporting_event_ids.clone(),
            allowed_counterexample_event_ids: package.hypothesis.counterexample_event_ids.clone(),
            allowed_trigger_selectors: selectors,
            trigger_kind,
            permission_ceiling: package.permissions.clone(),
        },
        baseline: SynthesisBaseline {
            title: package.title.clone(),
            statement: package.hypothesis.statement.clone(),
            expected_result: package.hypothesis.expected_result.clone(),
            instruction: package.intervention.instruction.clone(),
            failure_cases: package.hypothesis.failure_cases.clone(),
            exclusions: package.exclusions.clone(),
            supporting_event_ids: package.hypothesis.supporting_event_ids.clone(),
            counterexample_event_ids: package.hypothesis.counterexample_event_ids.clone(),
        },
    })
}

#[test]
fn prompt_is_deterministic_and_carries_only_template_fields() {
    // A raw tool input from the corpus with irregular spacing. The deterministic
    // template normalizes it away, so it must never appear in the prompt.
    const RAW_PAYLOAD_MARKER: &str = "mise   run check";
    let fixture = candidate_fixture();
    let request = request_from(&fixture.finding).expect("request");
    // Deterministic: same request always yields the same prompt.
    assert_eq!(user_prompt(&request), user_prompt(&request));
    let prompt = user_prompt(&request);
    // Cites the exact allowed evidence and selector.
    for event_id in &fixture.supporting {
        assert!(prompt.contains(event_id), "prompt should list {event_id}");
    }
    assert!(prompt.contains(&fixture.selector));
    // The system prompt states the JSON-only, cite-only, zero-permission rules.
    assert!(SYSTEM_PROMPT.contains("JSON"));
    assert!(SYSTEM_PROMPT.contains("permission"));
    // Negative: only normalized, template-derived fields leave the process —
    // never raw payloads. The raw marker is genuine corpus content but must not
    // survive into the prompt.
    assert!(
        CORPUS.contains(RAW_PAYLOAD_MARKER),
        "sanity: the raw marker must be genuine corpus content"
    );
    assert!(
        !prompt.contains(RAW_PAYLOAD_MARKER),
        "raw payload text must never leak into the model prompt"
    );
}

#[test]
fn measured_prompt_size_over_the_fixture_corpus_is_bounded() {
    // Measure the real per-candidate prompt size across the deterministic
    // fixture corpus. Quoted (as approximate tokens) in docs/guides/synthesis.md.
    let mut max_chars = 0usize;
    let mut count = 0usize;
    for finding in fixture_findings() {
        let Some(request) = request_from(&finding) else {
            continue;
        };
        let chars = SYSTEM_PROMPT.len() + user_prompt(&request).len();
        max_chars = max_chars.max(chars);
        count += 1;
    }
    assert!(count >= 1, "expected at least one candidate to measure");
    let approx_tokens = max_chars / 4;
    eprintln!(
        "PROMPT MEASUREMENT: candidates={count} max_prompt_chars={max_chars} \
         approx_prompt_tokens={approx_tokens} response_cap_tokens={MAX_COMPLETION_TOKENS}"
    );
    // A structured, template-only prompt stays small. This bound pins the figure
    // quoted in docs/guides/synthesis.md (~693 approx tokens max); keep the doc
    // and this bound in lockstep so the quoted number cannot drift silently.
    assert!(
        approx_tokens <= 750,
        "prompt exceeded the documented bound ({approx_tokens} approx tokens)"
    );
}

#[test]
fn non_loopback_endpoint_is_allowed_with_the_flag() {
    // With the opt-in, the locality guard permits the remote host; the request
    // then fails at the transport layer (host does not resolve here), which is a
    // clean provider error, not a refusal. The key point: the guard let it past.
    let fixture = candidate_fixture();
    let manifest = manifest(
        ModelFormat::Ollama,
        "http://model.invalid.example:11434",
        Some(1500),
    );
    let provider = OllamaProvider::from_manifest(&manifest, true);

    let outcome = synthesize_candidate(&fixture.finding, &manifest, &provider);
    let SynthesisOutcome::ProviderError { message, .. } = outcome else {
        panic!("expected a transport error, got {outcome:?}");
    };
    // It got past the loopback guard, so the failure is transport, not refusal.
    assert!(!message.contains("non-loopback"), "message: {message}");
}
