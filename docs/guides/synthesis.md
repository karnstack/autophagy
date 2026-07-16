# Synthesis boundary

A local model may propose richer mutation candidates than the deterministic
template can, but it can never bypass the Mutation Package v0.1 contract or the
evaluation gates. `mutations synthesize` runs the deterministic detectors,
applies the same evidence gate as `mutations propose`, hands qualifying findings
to a provider, validates every field the provider returns, and registers the
survivors as ordinary review-only candidates.

A built-in deterministic reference provider makes the whole boundary
exercisable offline today, with no model at all. Two model-backed providers can
enrich candidates against a local inference runtime when you want them.

```sh
# Offline, no model — the default path.
autophagy mutations synthesize --provider deterministic \
  --manifest docs/specs/synthesis/0.1/manifest/valid/deterministic.json

# Local Ollama server (loopback endpoint).
autophagy mutations synthesize --provider ollama \
  --manifest ~/.autophagy/models/qwen3-coder.json

# Local OpenAI-compatible server (llama.cpp, LM Studio, vLLM).
autophagy mutations synthesize --provider openai-compatible \
  --manifest ~/.autophagy/models/local-llama.json --dry-run
```

The `--provider` choice must match the manifest `format`
(`deterministic` ↔ `deterministic`, `ollama` ↔ `ollama`,
`openai-compatible` ↔ `openai_compatible`); a mismatch is a precise error.

The normative contracts are
[`docs/specs/synthesis/0.1/`](../specs/synthesis/0.1/README.md) (response) and
[`docs/specs/synthesis/0.2/`](../specs/synthesis/0.2/README.md) (manifest). The
design rationale and privacy stance are in
[ADR 0004](../decisions/0004-local-synthesis-boundary.md) and
[ADR 0006](../decisions/0006-model-synthesis-providers.md).

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
the baseline text, the hard constraints, and the cited event **identifiers**. It
never includes session transcripts or raw event payloads. Measured against the
deterministic fixture corpus, that is roughly **700 prompt tokens per candidate**
(693 at the maximum), and the response is capped at **1024 tokens**. When a
runtime reports usage, Autophagy surfaces the exact `prompt_tokens` and
`completion_tokens` per candidate; when it does not, usage is reported as
unavailable and never estimated.

Cost then follows directly, in tokens (Autophagy quotes no invented prices):

- **Local runtimes (Ollama, llama.cpp, LM Studio, vLLM) have zero marginal
  cost** — you already run the hardware.
- **Hosted endpoints** (only reachable with `--allow-remote-endpoint`) cost
  roughly `candidates × (prompt_tokens + completion_tokens) × provider_rate`.
  With ~700 prompt tokens plus up to 1024 completion tokens per candidate, that
  is on the order of ~1.7k tokens per candidate; multiply by your provider's
  current per-token price from its own pricing page.

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
