# Mutation candidates

Phase 2 begins with concrete proposals, not installation. `autophagy mutations`
runs the deterministic detectors and converts qualifying Evidence Packet v0.1
findings into Mutation Package v0.1 candidates.

```sh
autophagy mutations
autophagy --output json mutations --project /workspace/example
```

The normative package schema is
[`docs/specs/mutation/0.1/schema.json`](../specs/mutation/0.1/schema.json).

## Safety boundary

V0.1 supports only `agent_instruction`. Its permission manifest must contain:

- no filesystem reads or writes;
- no commands;
- no environment variables; and
- `network: false`.

The CLI only prints candidates. It has no mutation registry, install command,
hook materializer, active state, or execution path. A package cannot move beyond
`candidate`; challenge, replay, shadow observation, user promotion, and
reversible installation are separate future gates.

## Deterministic templates

Repeated command failures produce an advisory preflight/retry instruction tied
to the exact normalized tool signature and exit code. Repeated explicit user
corrections produce an instruction tied to the explicit correction signature.

Both templates preserve supporting and counterexample event IDs, state a
falsifiable expected result, list likely failure cases, add exclusions, and set
future replay/false-positive promotion thresholds.

The generator returns `insufficient_evidence` when a finding lacks two
independent supports or its signature cannot produce a concrete observable
trigger. It never fills missing semantics with generic model prose.

## Review checklist

Before any future replay or installation, verify:

1. The trigger is observable before the undesirable outcome.
2. Supporting sessions are genuinely comparable.
3. Counterexamples are represented and understood.
4. The instruction is specific enough to change behavior.
5. Exclusions cover legitimate exceptions.
6. Failure cases are testable.
7. The permission manifest remains no broader than necessary.
