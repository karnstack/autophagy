# ADR 0007: Local model synthesis providers

- Status: accepted
- Date: 2026-07-17

## Context

[ADR 0004](0004-local-synthesis-boundary.md) built the synthesis boundary: a
provider-neutral seam, exercised offline by a deterministic reference provider,
that lets a provider enrich mutation candidate content but never bypass the
evidence gate, the field validation, or the Mutation Package contract. It
deliberately shipped no model and anticipated this PR:

> When a real model provider lands, that PR will version the mutation contract
> with its own decision record, fixtures, and migration story.

This is that PR. It adds two HTTP-backed providers against local inference
runtimes and records model provenance on the candidates they produce.

## Decision

### Two HTTP providers, loopback by default

`OllamaProvider` posts to Ollama's `/api/chat` with a JSON Schema in `format`;
`OpenAiCompatibleProvider` posts to `/v1/chat/completions` with a
`response_format` of `json_schema` (llama.cpp server, LM Studio, vLLM). Both use
a small blocking client (`ureq`, default features off, `rustls` only for hosted
https) — no async runtime.

For these formats the manifest `path` is the endpoint base URL and `name` is the
model identifier. By default the endpoint host **must be loopback**
(`localhost`, `127.0.0.0/8`, `::1`); a non-loopback host is refused unless the
caller passes `--allow-remote-endpoint`, which also emits a clear warning. This
keeps the default path local-only and offline-capable, per the engineering
constraints. Timeouts are mandatory (3 s connect, 60 s total by default,
manifest-overridable).

### Validation over trust is unchanged

The boundary rules from ADR 0004 hold verbatim: the deterministic evidence gate
runs first (no provider consulted when evidence is insufficient); every
provider-returned field is validated (cited evidence must exist in the packet,
selectors must come from the template, permissions may not exceed the empty
ceiling); and a passing response is re-validated against the mutation contract.
A model that returns unparseable JSON is an honest *decline*, not a crash; a
model that fabricates evidence or escalates permissions flows into the existing
*rejection* path with structured diagnostics; a transport, timeout, missing-key,
or refused-endpoint failure is a clean structured `ProviderError`, never a panic.

### Mutation contract v0.2

A model-backed candidate records which model proposed it. Recording that inside
the immutable package is the behavior-changing schema edit ADR 0004 deferred, so
the mutation contract is versioned:

- **`mutation/0.2`** is `mutation/0.1` plus a required `provenance` block
  (`provider`, `model_name`, `model_revision`, optional `model_digest`,
  `manifest_spec_version`) and `generated_by: model_synthesis_v1`. New JSON
  Schema and valid/invalid fixtures live at `docs/specs/mutation/0.2/`.
- Provenance records model **identity only** — never an endpoint, API key,
  prompt, token count, or raw payload.

### Compatibility story

The two shapes never mix and both remain first-class:

- A `mutation/0.1` package is deterministic-template output with no provenance.
  **The deterministic reference provider and the deterministic template
  generator continue to emit `mutation/0.1` unchanged** — the offline default
  path does not move to v0.2. (A model provider's output is genuinely
  model-derived, so stamping deterministic output with model provenance would be
  a lie; hence deterministic candidates stay v0.1.)
- A `mutation/0.2` package is model-synthesis output that always carries
  provenance.
- The Rust `MutationPackage` gains an `Option<Provenance>` field that defaults to
  absent and is skipped on serialization, so every existing v0.1 package parses,
  validates, and round-trips byte-for-byte. A cross-field validator enforces the
  pairing (`0.1` ⇔ deterministic ⇔ no provenance; `0.2` ⇔ model synthesis ⇔
  provenance present).
- The registry accepts both versions. Migration `0008_mutation_provenance`
  relaxes only the `mutation_candidates.spec_version` CHECK from a single literal
  to `('mutation/0.1', 'mutation/0.2')`, following the immutable-migration rule
  (new ordered migration, old ones untouched) and the established
  recreate-copy-rename procedure. Every existing row is copied through verbatim;
  an unknown spec version is still rejected.

### Manifest v0.2

HTTP providers need per-request timeouts and, for hosted endpoints, auth. The
v0.1 manifest is `additionalProperties: false`, so additive fields require a
version: **`synthesis-manifest/0.2`** adds optional `timeouts` and an optional
`api_key_env`. Schema and fixtures live at `docs/specs/synthesis/0.2/`. The
synthesis *response* wire shape is unchanged and stays at `synthesis/0.1`.

`api_key_env` names an **environment variable**, never the key. The key is read
from the environment at call time and sent only as an `Authorization: Bearer`
header; it never appears in the manifest, the database, logs, output, or error
messages. A named-but-unset variable is a precise, actionable error.

## Privacy

The default path stays offline: a loopback endpoint is required unless remote is
explicitly opted into. What leaves the process is exactly the structured request
— the deterministic template's baseline text, the hard constraints, and the
cited event **identifiers**. No raw event payloads, transcripts, or secrets are
included; the prompt is built solely from template-derived fields (asserted by a
test) and is small (roughly 700 prompt tokens per candidate on the fixture
corpus). Response length is capped. For a non-loopback endpoint that same
structured request leaves the machine, but only under the explicit flag and with
a warning, and API keys travel only as an outbound Authorization header.

## Consequences

- A local model can now propose richer candidates, and each carries auditable
  model provenance — without gaining any power to invent evidence, widen a
  trigger, escalate permissions, or emit a candidate the contract rejects.
- Autophagy remains fully functional with **no model at all**: the deterministic
  path is unchanged and is the default.
- Local (Ollama / llama.cpp) inference has zero marginal token cost; hosted
  endpoints cost the measured per-candidate tokens times the provider's rate.
