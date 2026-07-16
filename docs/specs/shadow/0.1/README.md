# Shadow v0.1

Shadow v0.1 measures where an immutable instruction mutation would have
triggered while guaranteeing that it was not applied. The normative contracts
are [`suite.schema.json`](suite.schema.json) and
[`result.schema.json`](result.schema.json).

Each observation cites exact local AEP event IDs, records selectors visible
before the outcome, and annotates whether intervention would have helped. The
evaluator produces a confusion matrix, precision, false-positive rate, and
recall using integer basis points. Reports always set `mutation_applied: false`
and `model_used: false`.
