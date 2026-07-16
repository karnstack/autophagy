# Deterministic replay

Replay v0.1 tests challenged instruction candidates against annotated
historical decision points without executing the instruction or invoking a
model.

Start from an evidence-linked review draft instead of hand-authoring scenario
structure:

```sh
autophagy mutations replay-draft mut_example \
  --suite replay-review.json \
  --context-events 1
```

The extractor groups the immutable package's supporting and counterexample
event IDs by session, retains a bounded window of nearby structural events,
copies exact positive trigger selectors, and derives stable scenario IDs. It
does not infer causal outcomes. Every positive decision point is exported with
`counterfactual_outcome: unknown`; change each one to `expected_result` or
`contradiction` after reviewing the cited local events. Existing destinations
are refused unless `--force` is supplied. When one session contains both
support and counterevidence, the extractor emits separate decision points and
omits nearby context that would duplicate event provenance between them.

```sh
autophagy mutations replay mut_example \
  --scenarios evals/fixtures/replay/example.json
```

The command evaluates the suite, persists the immutable report, and returns
exit code `2` when thresholds do not pass. A failed report leaves the mutation
`challenged`. A passing report creates one audited `challenged -> replay_passed`
transition. Repeating the identical replay is a storage no-op.

## Scenario annotations

Each Replay Scenario v0.1 contains:

- exact AEP event IDs providing fixture provenance;
- trigger selectors that were observable before the historical outcome;
- `expected_action: intervene` or `no_op`;
- an `expected_result` or `contradiction` annotation for intervention cases;
  and
- an optional note explaining the annotation.

Annotations are reviewable claims, not synthetic agent executions. Fixture
authors remain responsible for comparable scenarios and honest counterfactual
labels. `unknown` is valid only during review: the evaluator refuses to create
a report until every intervention case has a reviewed outcome. The CLI refuses
missing event IDs and the suite validator prevents one
source event from being reused as multiple supposedly independent scenarios.
Persisted reports retain the IDs and observed selectors for later inspection.
Replay event IDs are foreign-key bound: deleting any cited event removes the
candidate and its replay audit rather than leaving a `replay_passed` claim with
missing evidence.

## Classification

The evaluator exact-matches the package's versioned trigger selectors:

| Expected action | Trigger matched | Annotated outcome | Classification |
| --- | --- | --- | --- |
| Intervene | Yes | Expected result | Success |
| Intervene | No | Any | Contradiction |
| Intervene | Yes | Contradiction | Contradiction |
| No-op | No | Absent | Correct no-op |
| No-op | Yes | Absent | False intervention |

The aggregate success rate is successes plus correct no-ops divided by all
scenarios. The false-intervention rate is false interventions divided by all
negative scenarios. All arithmetic uses integer basis points.

## Pass gate

A report passes only when all conditions hold:

1. scenario count meets `minimum_replays` from the immutable package;
2. at least one intervention case and one no-op case are present;
3. aggregate success meets `minimum_success_rate_bps`; and
4. false interventions do not exceed `maximum_false_positive_rate_bps`.

The report copies the policy and lists every failed threshold. Passing replay is
still not permission to install: observation-only shadow measurement and an
explicit install confirmation remain separate gates.
