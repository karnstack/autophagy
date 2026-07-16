# Mutation Package v0.1

Mutation Package v0.1 is the language-neutral contract for review-only Phase 2
candidates. The normative JSON Schema is [`schema.json`](schema.json).

V0.1 intentionally supports one intervention: `agent_instruction`. Generated
candidates request no filesystem, command, environment, or network capability.
The package itself carries no installation or execution authority. It remains
permanently in its generated `candidate` state; challenged, evaluated, active,
rejected, and retired lifecycle state belongs to the audited registry outside
this immutable wire object. A separate materializer may install it only after
the registry's challenge, replay, shadow, and user-approval gates pass.

Each package contains:

- a stable mutation ID and semantic package version;
- the exact source finding and detector;
- a falsifiable statement and expected result;
- exact supporting and counterexample event IDs;
- observable triggers and explicit exclusions;
- failure cases that challenge the proposal; and
- replay and false-positive thresholds required before promotion.

The deterministic template generator may return `insufficient_evidence`. A
candidate is not proof that the intervention is correct; it is a concrete claim
ready for challenge and replay.

Its promotion thresholds are consumed by
[Replay Result v0.1](../../replay/0.1/README.md); they are copied into each
report so the pass decision remains inspectable.
