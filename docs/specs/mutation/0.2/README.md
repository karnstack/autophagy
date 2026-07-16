# Mutation Package v0.2

Mutation Package v0.2 is an additive revision of
[Mutation Package v0.1](../0.1/README.md). It is a v0.1 package plus one new
block, `provenance`, that records the provider and model that enriched the
candidate through the [synthesis boundary](../../synthesis/0.1/README.md). The
normative JSON Schema is [`schema.json`](schema.json).

## What changed from v0.1

- `spec_version` is `mutation/0.2`.
- `generated_by` is `model_synthesis_v1` (v0.1 uses `deterministic_template_v1`).
- A new required `provenance` object records model identity.

Everything else — the hypothesis, triggers, exclusions, intervention,
zero-permission ceiling, and promotion thresholds — is byte-for-byte the v0.1
contract. A model provider may enrich only the reviewable content
(title, statement, expected result, instruction, failure cases, exclusions) and
may cite only evidence and selectors the deterministic template already
established. The permission ceiling is still empty.

## Provenance

```json
{
  "provider": "ollama",
  "model_name": "qwen3-coder:30b",
  "model_revision": "30b-a3b",
  "model_digest": "sha256:…",
  "manifest_spec_version": "synthesis-manifest/0.2"
}
```

Provenance records model identity only. It never contains an endpoint, an API
key, a prompt, a token count, or any raw payload. `model_digest` is optional;
every other field is required and non-blank.

## Compatibility

The two shapes never mix. A `mutation/0.1` package is deterministic-template
output with no provenance; a `mutation/0.2` package is model-synthesis output
that always carries provenance. A package that omits provenance simply remains a
v0.1 package — presence of provenance is exactly what distinguishes v0.2. The
deterministic reference provider and the deterministic template generator both
continue to emit v0.1 packages unchanged, so nothing about the offline default
path moves to v0.2.

Readers built for v0.1 keep working: the parser treats provenance as optional
and preserves v0.1 packages exactly on round-trip. The registry accepts both
`mutation/0.1` and `mutation/0.2` rows; see
[ADR 0007](../../decisions/0007-model-synthesis-providers.md) for the full
compatibility and migration story.
