# Replay v0.1

Replay v0.1 defines portable annotated decision points and deterministic result
reports for zero-permission instruction candidates.

- [`scenario.schema.json`](scenario.schema.json) defines one historical
  decision point.
- [`suite.schema.json`](suite.schema.json) binds ordered scenarios to one
  mutation.
- [`result.schema.json`](result.schema.json) defines the evaluator output.

This version is explicitly non-executable. It does not rerun an agent, invoke a
model, run a command, or claim that an annotation is independently proven. Each
scenario records exact source event IDs, selectors observable before the
outcome, the expected action, and an annotated counterfactual outcome for
positive cases. The CLI requires every cited event to remain in the local store.

The deterministic evaluator performs exact selector matching and produces one
of four classifications: `success`, `no_op`, `contradiction`, or
`false_intervention`. Reports always include `mutation_executed: false` and
`model_used: false`.
