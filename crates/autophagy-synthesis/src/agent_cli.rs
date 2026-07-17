//! Agent-CLI-backed synthesis providers.
//!
//! Many users already have an authenticated coding-agent CLI on their machine —
//! `claude` (Claude Code) or `codex` (Codex). This module reuses that existing
//! login as a synthesis model backend: no API key, no local inference server,
//! no extra install. Each provider spawns the configured CLI as a subprocess,
//! hands it the *same* deterministic prompt used by the HTTP providers
//! ([`prompt::SYSTEM_PROMPT`] plus [`prompt::user_prompt`]), asks for JSON-only
//! output, and parses the CLI's own result envelope into a [`SynthesisResponse`].
//!
//! The invariants match the HTTP providers exactly:
//!
//! - The child is told **not** to execute tools or commands (`claude` runs with
//!   every built-in tool disallowed; `codex exec` runs `--sandbox read-only`),
//!   and the prompt already demands a single JSON object and nothing else.
//! - A hard wall-clock timeout kills a hung child (std threads only, no async).
//! - Unparseable model output is an honest *decline*; a missing binary, a
//!   non-zero exit, or a timeout is a clean, structured [`ProviderError`] —
//!   never a panic. Only a bounded, sanitized stderr snippet is ever surfaced.
//! - Every proposed field is still re-validated by the boundary, and cited
//!   evidence, triggers, and permissions are checked against the deterministic
//!   template regardless of what the model returned.
//!
//! Unlike the HTTP providers these CLIs reach their vendor's cloud, so both
//! report `uses_network() = true`; the CLI layer requires the same explicit
//! remote-endpoint consent before consulting them.

use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::{
    manifest::ModelManifest,
    prompt,
    provider::{ProviderError, ProviderResponse, SynthesisProvider, SynthesisRequest, TokenUsage},
    remote::parse_proposal,
};

/// Default wall-clock timeout for one agent-CLI invocation when the manifest
/// does not override it. Agent CLIs are slower than a raw inference endpoint, so
/// this is more generous than the HTTP request default.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Maximum number of characters of stderr surfaced in a diagnostic. The snippet
/// is sanitized and bounded so a noisy or hostile child cannot flood output.
const MAX_STDERR_SNIPPET: usize = 500;

/// Which authenticated agent CLI a provider drives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentCliKind {
    /// The Claude Code CLI (`claude -p ... --output-format json`).
    Claude,
    /// The Codex CLI (`codex exec --json ...`).
    Codex,
}

impl AgentCliKind {
    const fn provider_name(self) -> &'static str {
        match self {
            Self::Claude => "claude-cli",
            Self::Codex => "codex-cli",
        }
    }
}

/// A synthesis provider backed by an authenticated agent CLI subprocess.
#[derive(Clone, Debug)]
pub struct AgentCliProvider {
    kind: AgentCliKind,
    /// Absolute path or bare name (resolved via `PATH`) of the CLI binary.
    binary: String,
    /// Optional model identifier passed via the CLI's `--model` flag.
    model: Option<String>,
    /// Hard wall-clock timeout for one invocation.
    timeout: Duration,
}

impl AgentCliProvider {
    /// Build an agent-CLI provider of the given kind from a manifest. The
    /// manifest `path` is the binary (absolute path or a bare name resolved via
    /// `PATH`), `model` (when set) is passed via `--model`, and
    /// `timeouts.request_ms` (when set) overrides the wall-clock timeout.
    #[must_use]
    pub fn from_manifest(kind: AgentCliKind, manifest: &ModelManifest) -> Self {
        let timeout = manifest
            .timeouts
            .and_then(|timeouts| timeouts.request_ms)
            .map_or(DEFAULT_TIMEOUT, Duration::from_millis);
        Self {
            kind,
            binary: manifest.path.clone(),
            model: manifest.model.clone(),
            timeout,
        }
    }

    /// Build a Claude Code CLI provider from a manifest.
    #[must_use]
    pub fn claude_from_manifest(manifest: &ModelManifest) -> Self {
        Self::from_manifest(AgentCliKind::Claude, manifest)
    }

    /// Build a Codex CLI provider from a manifest.
    #[must_use]
    pub fn codex_from_manifest(manifest: &ModelManifest) -> Self {
        Self::from_manifest(AgentCliKind::Codex, manifest)
    }

    /// The single prompt string handed to the CLI: the shared system prompt and
    /// the deterministic user prompt, joined. This is exactly what leaves the
    /// process — no transcripts, no raw payloads.
    fn combined_prompt(request: &SynthesisRequest) -> String {
        format!(
            "{}\n\n{}",
            prompt::SYSTEM_PROMPT,
            prompt::user_prompt(request)
        )
    }

    /// Assemble the argv for this CLI kind. The prompt is the final element for
    /// Codex (positional) and a `-p` value for Claude.
    fn command(&self, combined_prompt: &str) -> Command {
        let mut command = Command::new(&self.binary);
        match self.kind {
            AgentCliKind::Claude => {
                command
                    .arg("-p")
                    .arg(combined_prompt)
                    .arg("--output-format")
                    .arg("json")
                    // Disable every built-in tool for this run: the task is a
                    // pure text-to-JSON transform and must never touch the
                    // filesystem, shell, or network beyond the model call. A
                    // single comma-separated value avoids clap's variadic
                    // parsing consuming later flags.
                    .arg("--disallowed-tools")
                    .arg(
                        "Bash,Edit,Write,Read,Glob,Grep,WebFetch,WebSearch,\
                         NotebookEdit,Task,TodoWrite",
                    );
                if let Some(model) = &self.model {
                    command.arg("--model").arg(model);
                }
            }
            AgentCliKind::Codex => {
                command
                    .arg("exec")
                    // JSONL events on stdout; the agent's final message and the
                    // token usage both arrive as structured events.
                    .arg("--json")
                    // Never execute model-proposed shell commands.
                    .arg("--sandbox")
                    .arg("read-only")
                    // The prompt is a self-contained transform; do not require a
                    // git repository at the working directory.
                    .arg("--skip-git-repo-check");
                if let Some(model) = &self.model {
                    command.arg("--model").arg(model);
                }
                command.arg(combined_prompt);
            }
        }
        command
    }
}

impl SynthesisProvider for AgentCliProvider {
    fn name(&self) -> &str {
        self.kind.provider_name()
    }

    fn uses_model(&self) -> bool {
        true
    }

    fn uses_network(&self) -> bool {
        true
    }

    fn propose(&self, request: &SynthesisRequest) -> Result<ProviderResponse, ProviderError> {
        let combined_prompt = Self::combined_prompt(request);
        let command = self.command(&combined_prompt);
        let output = run_with_timeout(command, self.timeout).map_err(|error| match error {
            RunError::Spawn(source) => ProviderError::CliSpawn {
                binary: self.binary.clone(),
                reason: describe_spawn_error(&source),
            },
            RunError::Wait(source) => ProviderError::CliFailure {
                binary: self.binary.clone(),
                reason: format!("could not wait on the child process: {source}"),
            },
        })?;

        if output.timed_out {
            return Err(ProviderError::CliFailure {
                binary: self.binary.clone(),
                reason: format!(
                    "timed out after {}s and was killed{}",
                    self.timeout.as_secs(),
                    suffix_snippet(&output.stderr)
                ),
            });
        }

        // A non-zero exit means the CLI itself failed (auth, quota, bad flag),
        // distinct from the model returning unusable content.
        let succeeded = output.status.is_some_and(|status| status.success());
        if !succeeded {
            let code = output
                .status
                .and_then(|status| status.code())
                .map_or_else(|| "signal".to_owned(), |code| code.to_string());
            return Err(ProviderError::CliFailure {
                binary: self.binary.clone(),
                reason: format!(
                    "exited with status {code}{}",
                    suffix_snippet(&output.stderr)
                ),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        match self.kind {
            AgentCliKind::Claude => self.parse_claude(&stdout),
            AgentCliKind::Codex => self.parse_codex(&stdout),
        }
    }
}

impl AgentCliProvider {
    /// Parse the Claude Code `--output-format json` envelope. The model's text
    /// lands in `.result`; token usage in `.usage`.
    fn parse_claude(&self, stdout: &str) -> Result<ProviderResponse, ProviderError> {
        let envelope: ClaudeEnvelope =
            serde_json::from_str(stdout.trim()).map_err(|error| ProviderError::CliFailure {
                binary: self.binary.clone(),
                reason: format!("stdout was not a recognized claude JSON envelope: {error}"),
            })?;
        if envelope.is_error.unwrap_or(false) {
            return Err(ProviderError::CliFailure {
                binary: self.binary.clone(),
                reason: format!(
                    "claude reported an error result (subtype {})",
                    envelope.subtype.as_deref().unwrap_or("unknown")
                ),
            });
        }
        let usage = envelope
            .usage
            .map_or_else(TokenUsage::unavailable, |usage| TokenUsage {
                prompt_tokens: usage.input_tokens,
                completion_tokens: usage.output_tokens,
            });
        let content = envelope.result.ok_or_else(|| ProviderError::CliFailure {
            binary: self.binary.clone(),
            reason: "claude envelope carried no `result` text".to_owned(),
        })?;
        Ok(parse_proposal(&content, usage))
    }

    /// Parse the Codex `exec --json` JSONL event stream. The final assistant
    /// message arrives as an `agent_message` item; token usage as the
    /// `turn.completed` event. Non-JSON lines (hook chatter) are ignored.
    fn parse_codex(&self, stdout: &str) -> Result<ProviderResponse, ProviderError> {
        let mut last_message: Option<String> = None;
        let mut usage = TokenUsage::unavailable();
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<CodexEvent>(line) else {
                // Hook output and other decoration are not JSON; skip them.
                continue;
            };
            match event.event_type.as_deref() {
                Some("item.completed") => {
                    if let Some(item) = event.item {
                        if item.item_type.as_deref() == Some("agent_message") {
                            if let Some(text) = item.text {
                                last_message = Some(text);
                            }
                        }
                    }
                }
                Some("turn.completed") => {
                    if let Some(reported) = event.usage {
                        usage = TokenUsage {
                            prompt_tokens: reported.input_tokens,
                            completion_tokens: reported.output_tokens,
                        };
                    }
                }
                _ => {}
            }
        }
        let content = last_message.ok_or_else(|| ProviderError::CliFailure {
            binary: self.binary.clone(),
            reason: "codex event stream carried no agent message".to_owned(),
        })?;
        Ok(parse_proposal(&content, usage))
    }
}

/// The Claude Code print-mode JSON envelope (only the fields we consume).
#[derive(Deserialize)]
struct ClaudeEnvelope {
    #[serde(default)]
    is_error: Option<bool>,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

/// One Codex `exec --json` event (only the fields we consume).
#[derive(Deserialize)]
struct CodexEvent {
    #[serde(rename = "type", default)]
    event_type: Option<String>,
    #[serde(default)]
    item: Option<CodexItem>,
    #[serde(default)]
    usage: Option<CodexUsage>,
}

#[derive(Deserialize)]
struct CodexItem {
    #[serde(rename = "type", default)]
    item_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct CodexUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

/// Captured child output plus whether the wall-clock timeout fired.
struct CliOutput {
    status: Option<std::process::ExitStatus>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

/// A failure launching or waiting on the child, before any output is available.
enum RunError {
    Spawn(std::io::Error),
    Wait(std::io::Error),
}

/// How long to wait for a reader thread to deliver its drained buffer after the
/// child is reaped. A leaked grandchild can hold a pipe open past the child's
/// exit, so this wait is time-boxed: on expiry we proceed with whatever arrived.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Run a command with a hard wall-clock timeout, killing the child if it exceeds
/// the deadline. Uses only `std`.
///
/// Two hazards drive the design, because both vendor CLIs spawn helper
/// subprocesses that inherit the child's stdout/stderr pipe write-ends:
///
/// - On timeout, killing only the direct child leaves those grandchildren alive
///   holding the pipe open, so the reader threads never see EOF. On unix the
///   child is therefore placed in its own process group and the whole group is
///   killed at the deadline.
/// - As defense in depth, the reader threads deliver their buffers over channels
///   and are never joined. After the child is reaped we `recv_timeout` each
///   stream; if a leaked reader is still blocked it stays detached rather than
///   hanging the caller forever.
fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<CliOutput, RunError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Lead a new process group so the whole tree can be signalled on timeout.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(RunError::Spawn)?;

    let mut stdout_pipe = child.stdout.take().expect("stdout piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr piped");
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    // Detached readers: they own the read-end and send the drained buffer at EOF.
    // We never join them — a reader blocked on a leaked pipe must not hang us.
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buffer);
        let _ = stdout_tx.send(buffer);
    });
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buffer);
        let _ = stderr_tx.send(buffer);
    });

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_process_tree(&mut child);
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(source) => return Err(RunError::Wait(source)),
        }
    };

    // The child is reaped. Collect whatever the readers drained without ever
    // blocking forever: on the (post-group-kill) rare chance a grandchild still
    // holds a pipe, the recv times out and we proceed with what arrived.
    let stdout = stdout_rx.recv_timeout(DRAIN_TIMEOUT).unwrap_or_default();
    let stderr = stderr_rx.recv_timeout(DRAIN_TIMEOUT).unwrap_or_default();
    Ok(CliOutput {
        status,
        stdout,
        stderr,
        timed_out,
    })
}

/// Kill the child and any inherited helper subprocesses, then reap the direct
/// child. On unix the child leads its own process group (see `run_with_timeout`),
/// so the whole group is signalled first with `kill -9 -<pgid>`; otherwise a
/// grandchild would survive and keep the stdout/stderr pipes open. Non-unix
/// falls back to killing only the direct child.
fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // The child's PID is its process-group id because it was spawned with
        // `process_group(0)`. `kill -9 -<pgid>` signals every process in it.
        let pgid = child.id();
        let _ = Command::new("kill")
            .arg("-9")
            .arg(format!("-{pgid}"))
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Turn a spawn error into an actionable, secret-free description.
fn describe_spawn_error(source: &std::io::Error) -> String {
    if source.kind() == std::io::ErrorKind::NotFound {
        "binary not found — install the CLI or set the manifest `path` to its \
         absolute location"
            .to_owned()
    } else {
        source.to_string()
    }
}

/// A bounded, sanitized stderr snippet suffixed onto a diagnostic, or empty when
/// the child wrote nothing to stderr.
fn suffix_snippet(stderr: &[u8]) -> String {
    let snippet = sanitize_stderr(stderr);
    if snippet.is_empty() {
        String::new()
    } else {
        format!("; stderr: {snippet}")
    }
}

/// Collapse control characters and runs of whitespace, then bound the length.
/// Keeps diagnostics readable and prevents a noisy child from flooding output.
fn sanitize_stderr(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    // Map every control character (ESC and other non-printing bytes included, not
    // just whitespace) to a space, then collapse runs of whitespace. This strips
    // ANSI escape introducers and newlines so the snippet stays single-line.
    let defanged: String = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let collapsed = defanged.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut snippet: String = collapsed.chars().take(MAX_STDERR_SNIPPET).collect();
    if collapsed.chars().count() > MAX_STDERR_SNIPPET {
        snippet.push('…');
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_bounds_and_strips_control_characters() {
        let raw = b"line one\n\tline two\r\n\x1b[31mred\x1b[0m";
        let snippet = sanitize_stderr(raw);
        assert!(!snippet.contains('\n'));
        assert!(!snippet.contains('\t'));
        assert!(snippet.contains("line one line two"));
    }

    #[test]
    fn sanitize_truncates_long_output_with_ellipsis() {
        let raw = "x".repeat(MAX_STDERR_SNIPPET + 50);
        let snippet = sanitize_stderr(raw.as_bytes());
        assert_eq!(snippet.chars().count(), MAX_STDERR_SNIPPET + 1);
        assert!(snippet.ends_with('…'));
    }

    #[test]
    fn empty_stderr_yields_no_suffix() {
        assert_eq!(suffix_snippet(b"   \n\t "), "");
    }
}
