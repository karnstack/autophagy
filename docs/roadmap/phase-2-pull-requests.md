# Phase 2 pull-request sequence

Phase 2 turns findings into interventions without collapsing the review,
evaluation, and permission boundaries.

## PR 1 — Mutation candidate contract

Status: complete.

- Mutation Package v0.1 schema and Rust validator
- deterministic zero-permission `agent_instruction` candidates
- exact evidence/counterexample lineage and insufficient-evidence refusal
- read-only `autophagy mutations` output

Exit: the demo findings produce stable, schema-valid candidates that cannot be
installed or executed.

## PR 2 — Candidate registry and challenge

Status: complete.

- immutable package registry and audited lifecycle transitions
- duplicate/equivalent candidate detection
- challenge checklist, rejection reason, and evidence-retention semantics

Exit: a user can retain, inspect, challenge, or reject a candidate, but cannot
activate it.

## PR 3 — Replay scenario contract

- versioned decision-point and replay-result schemas
- non-executable instruction replay fixtures
- success, no-op, contradiction, and false-intervention measurement

Exit: candidates advance only after deterministic evaluation thresholds pass.

## PR 4 — Shadow and reversible installation

- observation-only triggers and precision measurement
- one agent skill/context-injection materializer
- explicit permission review, install audit, and uninstall rollback

Exit: a user can manually promote one replay-passed candidate and remove it
without residual behavior.
