# Synthesis v0.1

Synthesis v0.1 defines the provider-neutral boundary through which a local
model may propose richer mutation candidates without bypassing the deterministic
Mutation Package v0.1 contract or its evaluation gates. It is a *boundary*, not
a model integration: no model, runtime, or network client ships with it.

Two normative objects are versioned here:

- [`manifest.schema.json`](manifest.schema.json) — the local model manifest a
  provider is configured from. [`manifest/valid/`](manifest/valid) and
  [`manifest/invalid/`](manifest/invalid) hold accepted and rejected manifests.
- [`response.schema.json`](response.schema.json) — the structured, schema-
  constrained response a provider returns. [`response/valid/`](response/valid)
  and [`response/invalid/`](response/invalid) hold accepted and rejected
  responses.

The design rationale and privacy stance are in
[ADR 0004](../../decisions/0004-local-synthesis-boundary.md).

## Model manifest

A manifest is a small local JSON file describing a model: its `name`, packaging
`format` (`gguf`, `ollama`, `mlx`, `openai_compatible`, or `deterministic`),
local `path` or endpoint, `revision`, optional content `digest`, declared
`capabilities`, and advisory `resource_hints`. Loading a manifest never
downloads a model or makes a network call. A missing, malformed, or
semantically invalid manifest produces a precise, actionable error rather than a
silent default.

A provider is consulted only for a capability its manifest declares. A manifest
that does not declare `mutation_synthesis` cannot drive mutation synthesis.

## Structured response, not prose

There is no free-prose passthrough. A provider fills known, typed fields —
title, hypothesis statement and expected result, agent instruction, failure
cases, exclusions, cited evidence, trigger selectors, and a permission request.
The response carries no self-reported confidence field: promotion confidence is
derived from observed evidence, never asserted by a model.

## Validation over trust

Every field a provider returns is validated deterministically against the
deterministic template the boundary derived from the Evidence Packet:

- **Evidence must exist in the packet.** Every cited supporting or counterexample
  event ID must be one the provider was given. A provider cannot cite evidence
  it wasn't handed, and at least two independent supporting events are required.
- **Selectors must come from the template.** Trigger selectors must be drawn from
  the deterministic template; a provider cannot invent a new trigger.
- **Permissions cannot exceed the ceiling.** A response may request no capability
  beyond the deterministic template's permission scope. In v0.1 that ceiling is
  empty, so a synthesized candidate remains a zero-permission, review-only
  agent instruction.

A response that violates any rule is rejected with a structured diagnostic and
never repaired. A response that passes is assembled into an ordinary Mutation
Package v0.1 and re-validated against that exact contract before it can enter
the review-only registry.

## Insufficient evidence

The deterministic evidence gate runs first. When a finding does not meet the
deterministic thresholds, synthesis refuses with a structured reason and **no
provider is consulted**. Honest silence over invention.
