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

Review drafts are also Replay Suite v0.1 documents. They use the explicit
`unknown` counterfactual outcome for positive cases extracted from mutation
evidence and nearby session context. Structural validation accepts that state,
but deterministic evaluation rejects it until a reviewer changes every
`unknown` value to `expected_result` or `contradiction`.

The deterministic evaluator performs exact selector matching and produces one
of four classifications: `success`, `no_op`, `contradiction`, or
`false_intervention`. Reports always include `mutation_executed: false` and
`model_used: false`.
