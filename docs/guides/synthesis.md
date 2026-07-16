# Synthesis boundary

A local model may propose richer mutation candidates than the deterministic
template can, but it can never bypass the Mutation Package v0.1 contract or the
evaluation gates. `mutations synthesize` runs the deterministic detectors,
applies the same evidence gate as `mutations propose`, hands qualifying findings
to a provider, validates every field the provider returns, and registers the
survivors as ordinary review-only candidates.

This is a boundary, not a model integration. No model, runtime, or network
client ships with Autophagy. A built-in deterministic reference provider makes
the whole boundary exercisable offline today.

```sh
autophagy mutations synthesize --provider deterministic \
  --manifest docs/specs/synthesis/0.1/manifest/valid/deterministic.json
autophagy mutations synthesize --provider deterministic \
  --manifest ~/.autophagy/models/qwen3.json --project /workspace/example
autophagy mutations synthesize --provider deterministic \
  --manifest ~/.autophagy/models/qwen3.json --dry-run
```

The normative contracts are
[`docs/specs/synthesis/0.1/`](../specs/synthesis/0.1/README.md). The design
rationale and privacy stance are in
[ADR 0004](../decisions/0004-local-synthesis-boundary.md).

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
(`candidate`, `insufficient_evidence`, `provider_declined`, or `rejected`),
the provider name, whether a model was consulted, and — for rejections — the
exact structured diagnostics. The deterministic reference provider always
reports `model_used: false` and makes no network call.

## Privacy

Evidence handed to a provider stays local. The boundary makes no network call
and declares no network permission; the default path is offline. A future
provider that reaches a remote endpoint would require an explicit, separately
reviewed opt-in. A synthesized candidate carries only the exact evidence
identifiers it was permitted to cite — never raw payloads or secrets.
