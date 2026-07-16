# Mutation candidates

Phase 2 begins with concrete proposals, not installation. `mutations propose`
runs the deterministic detectors, converts qualifying Evidence Packet v0.1
findings into Mutation Package v0.1 candidates, and registers them locally.

```sh
autophagy mutations propose
autophagy mutations propose --project /workspace/example
autophagy mutations propose --dry-run
autophagy mutations list
autophagy --output json mutations show mut_example
```

The normative package schema is
[`docs/specs/mutation/0.1/schema.json`](../specs/mutation/0.1/schema.json).

## Registry guarantees

The candidate package is immutable. Registration hashes its canonical JSON and
is idempotent when the same candidate is proposed again. A stable equivalence
key also prevents a second candidate with the same detector, normalized trigger
selectors, and intervention type from being registered under a different ID.

Lifecycle state is stored outside the package. Every registered candidate gets
an initial audit entry, and each challenge or rejection appends another entry.
The only supported transitions are:

```text
candidate -> challenged -> rejected
         \--------------> rejected
```

Repeated requests for the current state are no-ops. A rejected candidate cannot
return to challenged. There is deliberately no replayed, active, install, or
promote state yet.

## Challenge and rejection

Challenge is an adversarial review gate, not approval. All six checks must be
provided in one request:

```sh
autophagy mutations challenge mut_example \
  --check coincidence-considered \
  --check sessions-comparable \
  --check trigger-observable \
  --check legitimate-uses-bounded \
  --check equivalent-searched \
  --check counterexamples-reviewed \
  --note 'reviewed against comparable sessions'
```

The CLI rejects an incomplete checklist and stores the complete structured
assessment in the audit. Rejecting a candidate requires a nonblank reason:

```sh
autophagy mutations reject mut_example --reason 'trigger is too broad'
```

## Evidence retention

Supporting and counterexample event IDs are exact foreign-key links, not copied
labels. If session deletion, pruning, or delete-all removes any cited event, the
candidate and its transition history are removed in the same transaction. This
ensures the registry never presents a proposal whose review evidence is no
longer locally available. Deletion summaries expose `mutations_deleted`.

## Safety boundary

V0.1 supports only `agent_instruction`. Its permission manifest must contain:

- no filesystem reads or writes;
- no commands;
- no environment variables; and
- `network: false`.

The registry stores and reviews packages but has no install command, hook
materializer, active state, or execution path. Challenge, replay, shadow
observation, explicit user promotion, and reversible installation remain
separate gates.

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
