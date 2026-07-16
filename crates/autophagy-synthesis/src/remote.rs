//! Local-model-backed synthesis providers over HTTP.
//!
//! Two providers implement [`SynthesisProvider`] against local inference
//! runtimes:
//!
//! - [`OllamaProvider`] posts to Ollama's `/api/chat` with a JSON Schema in the
//!   `format` field so the model must return the synthesis response shape.
//! - [`OpenAiCompatibleProvider`] posts to `/v1/chat/completions` with a
//!   `response_format` of `json_schema` (llama.cpp server, LM Studio, vLLM).
//!
//! Both are strict and defensive. The endpoint must be a loopback host unless a
//! caller opts into remote endpoints; the prompt is built only from the
//! deterministic template fields; the response is capped and parsed into the
//! existing [`SynthesisResponse`]; and unparseable output is an honest decline,
//! never a panic. Every proposed field is still re-validated by the boundary.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    manifest::{ManifestTimeouts, ModelManifest},
    prompt::{self, MAX_COMPLETION_TOKENS},
    provider::{
        ProviderError, ProviderResponse, SynthesisProposal, SynthesisProvider, SynthesisRequest,
        SynthesisResponse, TokenUsage,
    },
};

/// Default connection timeout when the manifest does not override it.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Default total-request timeout when the manifest does not override it.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Whether an endpoint's host is loopback or a remote address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndpointLocality {
    /// The endpoint host is loopback (`localhost`, `127.0.0.0/8`, or `::1`).
    Loopback,
    /// The endpoint host is not loopback; evidence would leave the machine.
    Remote {
        /// The extracted host.
        host: String,
    },
}

/// Classify an endpoint URL as loopback or remote.
///
/// # Errors
/// Returns [`ProviderError::InvalidEndpoint`] when the endpoint is not a usable
/// `http`/`https` URL.
pub fn classify_endpoint(endpoint: &str) -> Result<EndpointLocality, ProviderError> {
    let host = endpoint_host(endpoint).map_err(|reason| ProviderError::InvalidEndpoint {
        endpoint: endpoint.to_owned(),
        reason,
    })?;
    if is_loopback_host(&host) {
        Ok(EndpointLocality::Loopback)
    } else {
        Ok(EndpointLocality::Remote { host })
    }
}

/// Extract the host from an `http`/`https` URL without pulling in a URL crate.
fn endpoint_host(endpoint: &str) -> Result<String, String> {
    let rest = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .ok_or_else(|| "endpoint must start with http:// or https://".to_owned())?;
    // Authority ends at the first path, query, or fragment delimiter.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // Drop any userinfo prefix (`user:pass@host`).
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(after_bracket) = host_port.strip_prefix('[') {
        // Bracketed IPv6 literal: take everything up to the closing bracket.
        after_bracket
            .split(']')
            .next()
            .unwrap_or(after_bracket)
            .to_owned()
    } else {
        // host or host:port.
        host_port.split(':').next().unwrap_or(host_port).to_owned()
    };
    if host.is_empty() {
        return Err("endpoint has no host".to_owned());
    }
    Ok(host)
}

/// Whether a host is a loopback address.
fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// Resolved transport configuration shared by the HTTP providers.
#[derive(Clone, Debug)]
struct HttpConfig {
    endpoint: String,
    model: String,
    connect_timeout: Duration,
    request_timeout: Duration,
    allow_remote: bool,
}

impl HttpConfig {
    fn from_manifest(manifest: &ModelManifest, allow_remote: bool) -> Self {
        let (connect_timeout, request_timeout) = resolve_timeouts(manifest.timeouts);
        Self {
            endpoint: manifest.path.clone(),
            model: manifest.name.clone(),
            connect_timeout,
            request_timeout,
            allow_remote,
        }
    }

    /// Refuse a non-loopback endpoint unless remote endpoints were allowed.
    fn guard_locality(&self) -> Result<(), ProviderError> {
        match classify_endpoint(&self.endpoint)? {
            EndpointLocality::Loopback => Ok(()),
            EndpointLocality::Remote { host } => {
                if self.allow_remote {
                    Ok(())
                } else {
                    Err(ProviderError::NonLoopbackEndpoint {
                        endpoint: self.endpoint.clone(),
                        host,
                    })
                }
            }
        }
    }
}

fn resolve_timeouts(timeouts: Option<ManifestTimeouts>) -> (Duration, Duration) {
    let connect = timeouts
        .and_then(|value| value.connect_ms)
        .map_or(DEFAULT_CONNECT_TIMEOUT, Duration::from_millis);
    let request = timeouts
        .and_then(|value| value.request_ms)
        .map_or(DEFAULT_REQUEST_TIMEOUT, Duration::from_millis);
    (connect, request)
}

fn trim_endpoint(endpoint: &str) -> &str {
    endpoint.trim_end_matches('/')
}

/// Post a JSON body to `url` and return the raw response text.
fn send_json(
    config: &HttpConfig,
    url: &str,
    api_key: Option<&str>,
    body: &Value,
) -> Result<String, ProviderError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(config.connect_timeout))
        .timeout_global(Some(config.request_timeout))
        .build()
        .into();
    let payload = serde_json::to_vec(body).map_err(|error| ProviderError::Transport {
        endpoint: config.endpoint.clone(),
        reason: format!("could not encode request body: {error}"),
    })?;
    let mut request = agent.post(url).header("content-type", "application/json");
    if let Some(key) = api_key {
        // The key is used only as an Authorization header. It is never logged,
        // stored, echoed into errors, or serialized anywhere.
        request = request.header("authorization", &format!("Bearer {key}"));
    }
    let mut response = request
        .send(&payload[..])
        .map_err(|error| transport_error(config, &error))?;
    response
        .body_mut()
        .read_to_string()
        .map_err(|error| transport_error(config, &error))
}

/// Convert a transport error into a secret-free [`ProviderError::Transport`].
///
/// `ureq` errors reference the URL and status only — never request headers — so
/// the API key can never leak through this path.
fn transport_error(config: &HttpConfig, error: &ureq::Error) -> ProviderError {
    ProviderError::Transport {
        endpoint: config.endpoint.clone(),
        reason: error.to_string(),
    }
}

/// Parse model output into a proposal, treating unparseable output as an honest
/// decline rather than an error.
fn parse_proposal(content: &str, usage: TokenUsage) -> ProviderResponse {
    match serde_json::from_str::<SynthesisResponse>(content.trim()) {
        Ok(response) => ProviderResponse {
            proposal: SynthesisProposal::Proposed {
                response: Box::new(response),
            },
            usage,
        },
        Err(error) => ProviderResponse {
            proposal: SynthesisProposal::Declined {
                reason: format!(
                    "model returned output that is not a valid synthesis response: {error}"
                ),
            },
            usage,
        },
    }
}

fn missing_content(config: &HttpConfig) -> ProviderError {
    ProviderError::Transport {
        endpoint: config.endpoint.clone(),
        reason: "endpoint response did not contain assistant message content".to_owned(),
    }
}

/// A synthesis provider backed by a local Ollama server.
#[derive(Clone, Debug)]
pub struct OllamaProvider {
    config: HttpConfig,
}

impl OllamaProvider {
    /// Build an Ollama provider from a manifest. The manifest `path` is the
    /// endpoint base URL and `name` is the Ollama model tag.
    #[must_use]
    pub fn from_manifest(manifest: &ModelManifest, allow_remote: bool) -> Self {
        Self {
            config: HttpConfig::from_manifest(manifest, allow_remote),
        }
    }
}

#[derive(Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaMessage>,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
}

#[derive(Deserialize)]
struct OllamaMessage {
    content: Option<String>,
}

impl SynthesisProvider for OllamaProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "ollama"
    }

    fn uses_model(&self) -> bool {
        true
    }

    fn uses_network(&self) -> bool {
        true
    }

    fn propose(&self, request: &SynthesisRequest) -> Result<ProviderResponse, ProviderError> {
        self.config.guard_locality()?;
        let url = format!("{}/api/chat", trim_endpoint(&self.config.endpoint));
        let body = json!({
            "model": self.config.model,
            "stream": false,
            "format": prompt::response_json_schema(),
            "options": { "num_predict": MAX_COMPLETION_TOKENS },
            "messages": [
                { "role": "system", "content": prompt::SYSTEM_PROMPT },
                { "role": "user", "content": prompt::user_prompt(request) }
            ]
        });
        let text = send_json(&self.config, &url, None, &body)?;
        let parsed: OllamaChatResponse =
            serde_json::from_str(&text).map_err(|error| ProviderError::Transport {
                endpoint: self.config.endpoint.clone(),
                reason: format!("endpoint returned an unrecognized response envelope: {error}"),
            })?;
        let usage = TokenUsage {
            prompt_tokens: parsed.prompt_eval_count,
            completion_tokens: parsed.eval_count,
        };
        let content = parsed
            .message
            .and_then(|message| message.content)
            .ok_or_else(|| missing_content(&self.config))?;
        Ok(parse_proposal(&content, usage))
    }
}

/// A synthesis provider backed by a local OpenAI-compatible server
/// (llama.cpp server, LM Studio, vLLM, and similar).
#[derive(Clone, Debug)]
pub struct OpenAiCompatibleProvider {
    config: HttpConfig,
    api_key_env: Option<String>,
}

impl OpenAiCompatibleProvider {
    /// Build an OpenAI-compatible provider from a manifest. The manifest `path`
    /// is the endpoint base URL, `name` is the model identifier, and
    /// `api_key_env` (if set) names the environment variable holding the key.
    #[must_use]
    pub fn from_manifest(manifest: &ModelManifest, allow_remote: bool) -> Self {
        Self {
            config: HttpConfig::from_manifest(manifest, allow_remote),
            api_key_env: manifest.api_key_env.clone(),
        }
    }

    /// Resolve the API key from the environment at call time. The manifest only
    /// ever stores the variable name, never the key.
    fn api_key(&self) -> Result<Option<String>, ProviderError> {
        match &self.api_key_env {
            None => Ok(None),
            Some(name) => std::env::var(name)
                .map(Some)
                .map_err(|_| ProviderError::MissingApiKey {
                    env_var: name.clone(),
                }),
        }
    }
}

#[derive(Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
}

impl SynthesisProvider for OpenAiCompatibleProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "openai-compatible"
    }

    fn uses_model(&self) -> bool {
        true
    }

    fn uses_network(&self) -> bool {
        true
    }

    fn propose(&self, request: &SynthesisRequest) -> Result<ProviderResponse, ProviderError> {
        self.config.guard_locality()?;
        let api_key = self.api_key()?;
        let url = format!(
            "{}/v1/chat/completions",
            trim_endpoint(&self.config.endpoint)
        );
        let body = json!({
            "model": self.config.model,
            "max_tokens": MAX_COMPLETION_TOKENS,
            "messages": [
                { "role": "system", "content": prompt::SYSTEM_PROMPT },
                { "role": "user", "content": prompt::user_prompt(request) }
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "synthesis_response",
                    "strict": true,
                    "schema": prompt::response_json_schema()
                }
            }
        });
        let text = send_json(&self.config, &url, api_key.as_deref(), &body)?;
        let parsed: OpenAiChatResponse =
            serde_json::from_str(&text).map_err(|error| ProviderError::Transport {
                endpoint: self.config.endpoint.clone(),
                reason: format!("endpoint returned an unrecognized response envelope: {error}"),
            })?;
        let usage = parsed
            .usage
            .map_or_else(TokenUsage::unavailable, |usage| TokenUsage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
            });
        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .ok_or_else(|| missing_content(&self.config))?;
        Ok(parse_proposal(&content, usage))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts_are_recognized() {
        for endpoint in [
            "http://localhost:11434",
            "http://127.0.0.1:11434",
            "http://127.3.2.1/api",
            "http://[::1]:8080/v1/chat/completions",
            "https://LOCALHOST",
        ] {
            assert_eq!(
                classify_endpoint(endpoint).expect("classify"),
                EndpointLocality::Loopback,
                "{endpoint} should be loopback"
            );
        }
    }

    #[test]
    fn remote_hosts_are_flagged() {
        assert_eq!(
            classify_endpoint("https://api.example.com/v1").expect("classify"),
            EndpointLocality::Remote {
                host: "api.example.com".to_owned()
            }
        );
        assert_eq!(
            classify_endpoint("http://10.0.0.5:11434").expect("classify"),
            EndpointLocality::Remote {
                host: "10.0.0.5".to_owned()
            }
        );
    }

    #[test]
    fn userinfo_does_not_spoof_loopback() {
        // A remote host with loopback-looking userinfo must still be remote.
        assert_eq!(
            classify_endpoint("http://localhost@evil.example.com/api").expect("classify"),
            EndpointLocality::Remote {
                host: "evil.example.com".to_owned()
            }
        );
    }

    #[test]
    fn non_url_endpoints_are_rejected() {
        assert!(matches!(
            classify_endpoint("qwen3-coder:30b"),
            Err(ProviderError::InvalidEndpoint { .. })
        ));
    }
}
