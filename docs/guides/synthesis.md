# Synthesis boundary

A local model may propose richer mutation candidates than the deterministic
template can, but it can never bypass the Mutation Package v0.1 contract or the
evaluation gates. `mutations synthesize` runs the deterministic detectors,
applies the same evidence gate as `mutations propose`, hands qualifying findings
to a provider, validates every field the provider returns, and registers the
survivors as ordinary review-only candidates.

A built-in deterministic reference provider makes the whole boundary
exercisable offline today, with no model at all. Model-backed providers can
enrich candidates against a local inference runtime, or — if you already have an
authenticated coding-agent CLI — against `claude` or `codex` with no API key and
no server to run.

```sh
# Offline, no model — the default path. The built-in deterministic provider
# needs no manifest; it falls back to a built-in reference manifest.
autophagy mutations synthesize

# Passing an explicit --manifest still works and is validated strictly.
autophagy mutations synthesize --provider deterministic \
  --manifest docs/specs/synthesis/0.1/manifest/valid/deterministic.json

# Local Ollama server (loopback endpoint).
autophagy mutations synthesize --provider ollama \
  --manifest ~/.autophagy/models/qwen3-coder.json

# Local OpenAI-compatible server (llama.cpp, LM Studio, vLLM).
autophagy mutations synthesize --provider openai-compatible \
  --manifest ~/.autophagy/models/local-llama.json --dry-run

# Your authenticated Claude Code CLI (reaches Anthropic's cloud; opt-in required).
autophagy mutations synthesize --provider claude-cli \
  --manifest ~/.autophagy/models/claude-cli.json --allow-remote-endpoint

# Your authenticated Codex CLI (reaches OpenAI's cloud; opt-in required).
autophagy mutations synthesize --provider codex-cli \
  --manifest ~/.autophagy/models/codex-cli.json --allow-remote-endpoint
```

The `--provider` choice must match the manifest `format`
(`deterministic` ↔ `deterministic`, `ollama` ↔ `ollama`,
`openai-compatible` ↔ `openai_compatible`, `claude-cli` ↔ `claude_cli`,
`codex-cli` ↔ `codex_cli`); a mismatch is a precise error.

The normative contracts are
[`docs/specs/synthesis/0.1/`](../specs/synthesis/0.1/README.md) (response),
[`docs/specs/synthesis/0.2/`](../specs/synthesis/0.2/README.md) (HTTP-provider
manifest), and [`docs/specs/synthesis/0.3/`](../specs/synthesis/0.3/README.md)
(agent-CLI manifest). The design rationale and privacy stance are in
[ADR 0004](../decisions/0004-local-synthesis-boundary.md),
[ADR 0007](../decisions/0007-model-synthesis-providers.md), and
[ADR 0011](../decisions/0011-agent-cli-synthesis-providers.md).

## With and without a model

Autophagy is **fully functional with no model at all**. Deterministic detectors
find repeated behavior and the deterministic template generator turns qualifying
findings into review-only `mutation/0.1` candidates — offline, model-free, and
the default. A model only *enriches* candidate wording; it can never invent
evidence, widen a trigger, escalate permissions, or produce a candidate the
contract rejects. If you never configure a model, you lose nothing but the
enrichment.

A model-backed candidate is recorded as `mutation/0.2` and carries a
`provenance` block naming the provider and model that proposed it. Deterministic
candidates stay `mutation/0.1` with no provenance.

## Model manifest

A provider is configured from a versioned local model manifest — a small JSON
file describing the model and the capabilities it declares:

```json
{
  "spec_version": "synthesis-manifest/0.1",
  "name": "qwen3-8b-q4",
  "format": "gguf",
  "path": "~/.autophagy/models/qwen3-8b-q4.gguf",
  "revision": "q4_k_m",
  "capabilities": ["mutation_synthesis"],
  "resource_hints": { "min_memory_mb": 8192 }
}
```

Loading the manifest never downloads a model or makes a network call. A missing,
malformed, or semantically invalid manifest fails with a precise, actionable
error. A provider is consulted only for a capability its manifest declares, so a
manifest without `mutation_synthesis` cannot drive synthesis.

### HTTP provider manifests

For the `ollama` and `openai_compatible` formats the manifest `path` is the
endpoint base URL and `name` is the model identifier sent to it. Optional
timeouts and (for hosted endpoints) an API-key variable name require
`synthesis-manifest/0.2`:

```json
{
  "spec_version": "synthesis-manifest/0.2",
  "name": "qwen3-coder:30b",
  "format": "ollama",
  "path": "http://localhost:11434",
  "revision": "30b-a3b",
  "capabilities": ["mutation_synthesis"],
  "resource_hints": { "min_memory_mb": 24576 },
  "timeouts": { "connect_ms": 2000, "request_ms": 90000 }
}
```

`api_key_env` names an **environment variable** that holds the API key — the key
itself is never stored in the manifest, the database, logs, or output. The
provider reads it from the environment at call time and sends it only as an
`Authorization: Bearer` header. A named-but-unset variable is a clear error.

### The loopback rule

By default the endpoint host must be loopback (`localhost`, `127.0.0.0/8`, or
`::1`). A non-loopback host is **refused** unless you pass
`--allow-remote-endpoint`, which also prints a warning that evidence will leave
the machine. This keeps the default path local-only. Timeouts are always
enforced (defaults: 3 s connect, 60 s total).

### Agent-CLI provider manifests

If you already have an authenticated coding-agent CLI, you can use it as a
synthesis backend with no API key and no local inference server. The
`claude_cli` and `codex_cli` formats (`synthesis-manifest/0.3`) point at the CLI
binary and reuse your existing login:

```json
{
  "spec_version": "synthesis-manifest/0.3",
  "name": "codex-cli-login",
  "format": "codex_cli",
  "path": "/opt/homebrew/bin/codex",
  "revision": "cli-0.x",
  "capabilities": ["mutation_synthesis"],
  "resource_hints": { "min_memory_mb": 512 },
  "timeouts": { "request_ms": 180000 },
  "model": "gpt-5-codex"
}
```

- `path` is the CLI **binary** — an absolute path, or a bare name (`claude`,
  `codex`) resolved via your `PATH`. A binary that cannot be launched is a clean,
  actionable error, never a crash.
- `model` is optional and passed to the CLI's `--model` flag; omit it to use the
  CLI's own configured default model. `name` stays a human-readable label.
- `timeouts.request_ms` overrides the wall-clock timeout (default 120 s). A hung
  CLI is killed at the deadline and reported as a structured provider error.

Autophagy spawns the CLI as a subprocess, hands it the same deterministic prompt
the HTTP providers use, disables tool and command execution for the run
(`--disallowed-tools …` for Claude Code, `--sandbox read-only` for Codex), and
requests JSON-only output. Unparseable output is an honest decline; a non-zero
exit, timeout, or missing binary is a clean provider error carrying only a
bounded, sanitized stderr snippet — never the prompt or a secret.

Because these CLIs reach their **vendor's cloud** through your login, an
agent-CLI provider always requires `--allow-remote-endpoint`; without it,
synthesis refuses before spawning anything. When you pass it, the run prints one
clear line naming the vendor the structured prompt is sent to. Costs are your
existing plan, billed through your CLI login — Autophagy stores no API key for
these providers.

Setup-wizard integration for these providers lands separately; today you point
`--manifest` at a `claude_cli`/`codex_cli` manifest by hand.

## What the boundary guarantees

- **Insufficient evidence refuses silently.** The deterministic evidence gate
  runs first. A finding that does not meet the thresholds produces a structured
  `insufficient_evidence` refusal, and no provider is consulted.
- **Evidence cannot be invented.** Every cited supporting or counterexample event
  must be one the provider was given, and at least two independent supporting
  events are required.
- **Triggers cannot be invented.** Trigger selectors must come from the
  deterministic template.
- **Permissions cannot escalate.** A response may request no capability beyond the
  deterministic template's ceiling, which is empty in v0.1. A synthesized
  candidate stays a zero-permission, review-only agent instruction.
- **One contract.** A passing response is assembled into a Mutation Package v0.1
  and re-validated against that exact contract before it is registered.

A response that breaks any rule is rejected with a structured diagnostic and
never repaired. Rejections and refusals are reported, not hidden.

## Registration and lifecycle

Synthesized candidates register through the same review-only path as
`mutations propose`: the immutable package is hashed, registration is
idempotent, and the equivalence key still prevents a duplicate detector,
trigger, and intervention from registering twice. Candidates land in the
`candidate` state. Nothing becomes installable or executable through synthesis —
the challenge, replay, shadow, and explicit-approval gates are unchanged.

```sh
autophagy mutations list
autophagy --output json mutations show mut_example
```

## Machine-readable output

`--output json` emits each outcome tagged by `status`
(`candidate`, `insufficient_evidence`, `provider_declined`, `rejected`, or
`provider_error`), the provider name, whether a model was consulted
(`model_used`), whether the network was used (`network_used`), any remote-endpoint
warnings, and per-candidate token `usage`. For rejections it includes the exact
structured diagnostics; for transport, timeout, missing-key, or refused-endpoint
failures it includes a clean, secret-free `provider_error` message (never a
panic). The deterministic reference provider always reports `model_used: false`
and `network_used: false`.

## Token accounting and cost expectations

The prompt is built only from the deterministic template's structured fields —
the baseline text, the hard constraints, and the cited event **identifiers** —
plus a fixed system prompt that spells out the exact response shape (including
the literal permissions object) so a model emits a schema-valid candidate. It
never includes session transcripts or raw event payloads. Measured against the
deterministic fixture corpus, that is roughly **900 prompt tokens per candidate**
(899 at the maximum), and the response is capped at **1024 tokens**. When a
runtime reports usage, Autophagy surfaces the exact `prompt_tokens` and
`completion_tokens` per candidate; when it does not, usage is reported as
unavailable and never estimated.

Cost then follows directly, in tokens (Autophagy quotes no invented prices):

- **Local runtimes (Ollama, llama.cpp, LM Studio, vLLM) have zero marginal
  cost** — you already run the hardware.
- **Hosted endpoints** (only reachable with `--allow-remote-endpoint`) cost
  roughly `candidates × (prompt_tokens + completion_tokens) × provider_rate`.
  With ~900 prompt tokens plus up to 1024 completion tokens per candidate, that
  is on the order of ~1.9k tokens per candidate; multiply by your provider's
  current per-token price from its own pricing page.
- **Agent CLIs** (`claude-cli`, `codex-cli`) have **no separate Autophagy cost**:
  the request goes through your existing CLI login and is billed to your existing
  vendor plan. Both CLIs report per-candidate `prompt_tokens` and
  `completion_tokens`, which Autophagy surfaces unchanged.

## Privacy

For the offline paths — the deterministic provider and any loopback model — no
data leaves the machine. When you opt a model in over a **loopback** endpoint,
the only thing that crosses the process boundary to that local endpoint is the
structured request: the deterministic baseline text, the constraints, and the
cited event identifiers. No raw payloads or secrets are sent, and a synthesized
candidate still carries only the exact evidence identifiers it was permitted to
cite.

Reaching a **non-loopback** endpoint requires the explicit
`--allow-remote-endpoint` opt-in and prints a warning; that same structured
request then leaves the machine to the endpoint you named. An API key, when
configured via `api_key_env`, travels only as an outbound `Authorization: Bearer`
header and never appears in the manifest, database, logs, output, or errors.

The **agent-CLI** providers (`claude-cli`, `codex-cli`) reach their vendor's
cloud through your own logged-in CLI, so they always require
`--allow-remote-endpoint`. When enabled, exactly the same structured request —
the template fields and the cited event **identifiers**, nothing more — is what
that CLI sends to Anthropic or OpenAI on your behalf. Autophagy stores no API key
or credential for these providers; the CLI uses your existing login and your
usage is billed to your existing plan. The spawned CLI cannot execute tools or
shell commands, and its stderr is only ever surfaced as a bounded, sanitized
snippet.
