# ADR 0004: Local synthesis boundary

- Status: accepted
- Date: 2026-07-17

## Context

The product's decision rule is "the model proposes, Autophagy proves, the user
promotes." Deterministic detectors and hybrid retrieval already find candidate
repeated behavior, and the deterministic template generator already turns
qualifying Evidence Packets into review-only Mutation Package v0.1 candidates.
The next gap is the seam for the "scientist": a local model that can propose a
richer, more specific mutation than a fixed template can.

The risk is obvious. A model can hallucinate evidence, over-broaden a trigger,
request capabilities it was never granted, or emit confident prose that reads as
proof. If any of that could reach the registry, the model would be bypassing the
evidence and privacy boundaries the rest of the system depends on. The exit
criterion for this slice is therefore precise: a local model may propose richer
mutations but must not be able to bypass contracts or evaluation gates.

## Decision

Add `crates/autophagy-synthesis`: a provider-neutral **boundary**, not a model
integration. It ships no model, no llama.cpp, no download, no network client. A
new crate is justified because it ships an executable vertical slice — a
deterministic reference provider exercisable offline today — and it respects the
one-way dependency direction (`patterns -> mutations -> synthesis`; it never
touches `autophagy-events`).

- **Boundary first.** The crate defines the `SynthesisProvider` trait: a
  structured request (evidence, the fields a provider may enrich, and the hard
  constraints it must respect) in, a structured response or an honest
  `Declined` out. Building the seam before any model means the validation and
  refusal semantics are settled and tested before a model can exploit a gap.
- **Provider neutrality.** Nothing in the boundary assumes a runtime. A future
  llama.cpp, Ollama, MLX, or OpenAI-compatible provider implements the same
  trait and is validated by the same rules as the built-in deterministic
  reference provider. The reference provider (pure function, no model, no I/O)
  is the shipped vertical slice and the fixture provider for tests.
- **Versioned manifest.** A provider is configured from a versioned local model
  manifest (`synthesis-manifest/0.1`): name, format, path, revision, optional
  digest, declared capabilities, and resource hints. It is a new public contract
  at `docs/specs/synthesis/0.1/` with a JSON Schema and valid/invalid fixtures,
  loaded and validated from a local file. Missing, malformed, or semantically
  invalid manifests fail with a precise, actionable error. A provider is
  consulted only for a capability its manifest declares.
- **Insufficient-evidence refusal.** The deterministic evidence gate — the same
  thresholds that gate `autophagy-mutations` — runs first. When evidence is
  insufficient, synthesis refuses with a structured reason and **no provider is
  consulted**. Honest silence over invention.
- **Validation over trust.** Every provider-returned field is validated
  deterministically. Cited evidence IDs must exist in the packet the provider
  was given (a provider cannot cite evidence it wasn't handed); at least two
  independent supporting events are required; trigger selectors must come from
  the deterministic template; and requested permissions may never exceed the
  template's permission ceiling (empty in v0.1). A violation is rejected with a
  structured diagnostic and never silently repaired. A passing response is
  assembled into an ordinary Mutation Package v0.1 and re-validated against that
  exact contract before it can enter the review-only registry.

## The contract is unchanged

A synthesized candidate flows through the existing `mutation/0.1` contract
verbatim. This PR deliberately does **not** widen that schema. The only shipped
provider is the deterministic reference provider, whose output is genuinely
`deterministic_template_v1`; recording a distinct model provenance value inside
the immutable package would be a behavior-changing schema edit, and we version a
public schema only when a change actually requires it. Provider identity and
whether a model was consulted are recorded in the synthesis *outcome* envelope
(alongside lifecycle state, which already lives outside the immutable package),
not inside the package. When a real model provider lands, that PR will version
the mutation contract with its own decision record, fixtures, and migration
story.

## Privacy

Evidence content handed to a provider stays local. The boundary makes no network
call and declares no network permission; the default path remains offline. The
deterministic reference provider performs no I/O at all. A future provider that
reaches a remote endpoint would require an explicit, separately reviewed opt-in;
raw cloud payloads and secrets never leave the machine by default, and a
synthesized candidate — like every deterministic candidate — carries only the
exact evidence identifiers it was permitted to cite, never raw payloads.

## Consequences

- A local model can enrich candidate content but cannot invent evidence, widen a
  trigger, escalate permissions, or emit a candidate the mutation contract would
  reject. The exit criterion holds by construction.
- The boundary is exercisable offline today through the deterministic reference
  provider and a `mutations synthesize --provider deterministic` CLI surface;
  candidates land in the same review-only registry states, and nothing becomes
  installable or executable by this PR.
- A future model integration is an additive change: implement the trait, ship a
  manifest, and — if it needs to record model provenance in the package — version
  the mutation contract at that point.
