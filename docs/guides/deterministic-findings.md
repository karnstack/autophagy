# Deterministic findings

Autophagy's first detectors are local, model-free functions over validated AEP
events. They return Evidence Packet v0.1 values; they do not persist findings,
generate prose with a model, or execute a proposed intervention.

The normative output schema is
[`docs/specs/evidence/0.1/schema.json`](../specs/evidence/0.1/schema.json).
Each finding contains its stable detector signature, inspectable integer score,
exact supporting event IDs, and exact counterexample IDs.

## Default recurrence policy

A group becomes a finding only when it has:

- at least three supporting events;
- support in at least two distinct sessions; and
- at least 50% support among support plus explicit counterexamples.

The detector API accepts different thresholds for evaluation, but production
callers should keep the defaults until fixture precision justifies a change.

## Repeated command failures

The failure detector considers only `tool.failed` events with a non-zero exit
code and an inspectable string command. It normalizes common shell tool aliases,
collapses whitespace, replaces the event's exact project prefix with
`$PROJECT`, and groups by normalized operation plus exit code.

A matching `tool.completed` operation is an explicit counterexample. It lowers
both the support ratio and overall score. Different commands, exit codes, and
non-shell structured tools do not get merged speculatively.

## Repeated user corrections

The correction detector considers only explicit `user.corrected_agent` events.
Because native adapters intentionally do not infer corrections from private
prompt text, grouping requires one of these string metadata keys:

- `autophagy.signature`
- `correction_signature`
- `correction_key`

Whitespace and case are normalized. A `decision.recorded` event with the same
signature and an explicit `followed`, `accepted`, or `complied` outcome is a
counterexample. Unclassified corrections remain available as evidence but do
not produce a potentially misleading finding.

## Evaluation corpus

The anonymized corpus at
[`evals/fixtures/findings/deterministic.jsonl`](../../evals/fixtures/findings/deterministic.jsonl)
contains both supported patterns and counterexamples. Contract tests prove that
it emits exactly two stable packets regardless of input order, while a threshold
above its recurrence count emits none.
