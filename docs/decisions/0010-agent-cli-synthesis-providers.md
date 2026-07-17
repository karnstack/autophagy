# ADR 0010: Agent-CLI synthesis providers

- Status: accepted
- Date: 2026-07-17

## Context

[ADR 0007](0007-model-synthesis-providers.md) added HTTP synthesis providers
(Ollama, OpenAI-compatible) against local inference runtimes. Those require the
user to run a local model server or configure a hosted endpoint with an API key.

Many users already have an authenticated coding-agent CLI installed and logged
in — `claude` (Claude Code) or `codex` (Codex). That existing login is a ready
synthesis model backend: no API key to manage, no local inference server to
stand up, no extra install. This ADR adds two providers that drive those CLIs as
subprocesses through the unchanged synthesis boundary.

## Decision

### Two agent-CLI providers

A single `AgentCliProvider` (kind `Claude` or `Codex`) spawns the configured CLI
with `std::process`, hands it the **same** deterministic prompt the HTTP
providers use (`prompt::SYSTEM_PROMPT` + `prompt::user_prompt`, joined verbatim),
requests JSON-only output, disables tool/command execution for the run, parses
the CLI's own result envelope into a `SynthesisResponse`, and returns it through
the boundary — where every field is re-validated exactly as before. No async
runtime is used; a hard wall-clock timeout with kill is implemented with `std`
threads polling `try_wait`.

### Verified CLI invocations

Both CLIs were verified empirically on the implementation machine
(`claude` 2.1.212, `codex-cli` 0.144.5) with trivial non-evidence prompts. The
providers use exactly these shapes:

| Concern | Claude Code (`claude_cli`) | Codex (`codex_cli`) |
| --- | --- | --- |
| Non-interactive | `-p <prompt>` (print mode) | `exec <prompt>` |
| JSON output | `--output-format json` | `--json` (JSONL events) |
| Model text lands in | `.result` (string) | `item.completed` event, `item.type == "agent_message"`, `.item.text` |
| Token usage lands in | `.usage.input_tokens` / `.usage.output_tokens` | `turn.completed` event, `.usage.input_tokens` / `.usage.output_tokens` |
| Model selection | `--model <id>` (optional) | `--model <id>` (optional) |
| Disable tool/command execution | `--disallowed-tools Bash,Edit,Write,Read,Glob,Grep,WebFetch,WebSearch,NotebookEdit,Task,TodoWrite` | `--sandbox read-only` |
| Other | — | `--skip-git-repo-check` (prompt is a self-contained transform) |

Verified working invocations (trivial prompt, not evidence):

```sh
claude -p 'Return exactly {"ok":true} as JSON' --output-format json \
  --disallowed-tools Bash,Edit,Write,Read,Glob,Grep,WebFetch,WebSearch,NotebookEdit,Task,TodoWrite
# → {"type":"result","subtype":"success","is_error":false,...,"result":"{\"ok\":true}",
#    "usage":{"input_tokens":2,"output_tokens":9,...}}

codex exec --json --sandbox read-only --skip-git-repo-check 'Return exactly {"ok":true} as JSON'
# → {"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"{\"ok\":true}"}}
#    {"type":"turn.completed","usage":{"input_tokens":13209,"output_tokens":9,...}}
```

Codex `exec --json` interleaves non-JSON `hook:` lines on stdout; the parser
skips any line that does not parse as a JSON event, so decoration is ignored.

### Binary resolution

The manifest `path` is the CLI binary: an absolute path, or a bare name resolved
via `PATH` (`std::process::Command` does the lookup). A binary that cannot be
launched — most often not installed or not on `PATH` — is a clean
`ProviderError::CliSpawn` with an actionable message, never a panic.

### Failure shapes reuse the existing structured outcomes

The boundary rules from ADR 0004/0007 hold verbatim:

- Unparseable model output (the CLI ran fine but the `result`/agent message is
  not a valid synthesis response) is an honest **decline**, via the same
  `parse_proposal` the HTTP providers use.
- Fabricated evidence, invented selectors, or escalated permissions flow into the
  existing **rejection** path with structured diagnostics.
- A missing binary, a non-zero exit, or a timeout-then-kill is a clean
  `ProviderError` (`CliSpawn` / `CliFailure`). Only a **bounded, sanitized**
  stderr snippet (control characters collapsed, capped at 500 chars) is ever
  surfaced — never the prompt, never a secret.

### Manifest v0.3

The two new `format` values (`claude_cli`, `codex_cli`) change the manifest's
enum, and the CLIs need a model selector, so the manifest is versioned:
**`synthesis-manifest/0.3`** adds the two formats and an optional `model` field
(passed to `--model`; absent means the CLI's own default). Schema and fixtures
live at `docs/specs/synthesis/0.3/`. Both new formats and the `model` field
require `synthesis-manifest/0.3`; using either under an older `spec_version` is a
precise, actionable error. Every existing v0.1 and v0.2 manifest still loads
unchanged through the shared Rust types (asserted by tests). The synthesis
**response** wire shape is unchanged and stays at `synthesis/0.1`. The mutation
contract is likewise unchanged: an agent-CLI candidate is an ordinary
`mutation/0.2` package whose provenance records the provider (`claude-cli` /
`codex-cli`), the model label, and `manifest_spec_version`.

### Consent reuses the single remote flag

Unlike the local HTTP providers, these CLIs reach the **vendor's cloud** through
the user's login, so `uses_network()` is `true` and there is no loopback option.
Rather than introduce a new flag, the providers reuse the existing
`--allow-remote-endpoint` consent flag: an agent-CLI provider requires it
**unconditionally** and, when set, the CLI prints one clear line stating the
structured prompt goes to the vendor (`Anthropic` / `OpenAI`) via the user's
logged-in CLI and that usage is billed to their existing plan. Without the flag,
synthesis refuses before spawning anything. Reusing one flag keeps the consent
surface small and consistent with the HTTP non-loopback path.

## Privacy

What leaves the machine is exactly the structured prompt — the deterministic
template's baseline text, the hard constraints, and the cited event
**identifiers** — the same content the HTTP providers send, built solely from
template-derived fields (asserted by a test). No raw event payloads, transcripts,
or secrets are included. That prompt goes to the vendor's cloud through the
user's own authenticated CLI; Autophagy stores no API key for these providers and
holds no credential. Costs are the user's existing plan, billed through their
CLI login. The child cannot execute tools or shell commands
(`--disallowed-tools` / `--sandbox read-only`), and its stderr is only ever
surfaced as a bounded, sanitized snippet.

## Consequences

- A user with an authenticated `claude` or `codex` CLI can enrich mutation
  candidates with no API key, no local inference server, and no extra install.
- The default path is unchanged and stays offline: agent-CLI providers never run
  without the explicit remote-consent flag, and Autophagy remains fully
  functional with no model at all.
- Setup-wizard integration (offering these providers during guided setup) is a
  separate later change; this PR ships the provider, the manifest contract, the
  CLI wiring, and the documentation only.
