# Efficacy v0.1

Efficacy v0.1 measures, deterministically and observationally, whether the
failure signature an *installed* mutation addresses recurs less after it was
installed. It makes **no causal claim**, invokes **no model**, and never changes
a mutation's lifecycle state. The normative contract is
[`result.schema.json`](result.schema.json); `valid/` and `invalid/` hold
fixtures the evaluator is tested against.

Every report sets `model_used: false` and cites exact local AEP event IDs — never
event content.

## Windows

Efficacy compares two equal-length windows anchored on `installed_at`:

- **post-window** `[installed_at, evaluated_at]` — the observation period since
  install.
- **pre-window** `[installed_at − post_duration, installed_at)` — the equal-length
  window immediately before install.

Both windows report raw `occurrences`, `distinct_sessions`, and a weekly rate.
The rate is `rate_per_week_milli` — occurrences per week scaled by 1000, so the
report carries no floats (`1810` means `1.81` failures/week). The evaluation
clock (`evaluated_at`) is passed in and echoed, so a report is fully reproducible
from `(database-state, installed_at, evaluated_at)`.

## Matching rule

`matching_rule: failure_signature_recurrence` is the only rule in v0.1. A
mutation's immutable trigger selector is a versioned failure signature; the store
decomposes it and counts the matching failure events:

- A `failure/<v>|<tool>|<command>|exit:<code>` selector is split into the
  outcome-independent operation signature `operation/<v>|<tool>|<command>` and an
  exit code. It matches an event when the event is a `tool.failed`, its exit code
  equals `<code>`, and it is indexed under that operation signature. (Command
  text may contain `|`, so the failure form is parsed by trimming the trailing
  `|exit:<code>`, never by splitting on `|`.)
- An `operation/<v>|<tool>|<command>` selector matches any `tool.failed` event
  indexed under that operation signature, regardless of exit code.

The exact-signature index is the same redaction-approved projection that gates
free-text search, so a failure event is only counted when its operation
signature was indexed. This is why coverage is reported explicitly (below) rather
than assumed complete.

## Coverage diagnostics

The exact-signature index is partial by construction — old imports may predate
indexing, and events whose command cannot be projected carry no signature row.
`coverage` reports, over the full evaluated span:

- `total_failures` — every `tool.failed` event in the span.
- `classifiable_failures` — how many of those carry an index row (and can
  therefore be matched by signature at all).
- `coverage_bps` — `classifiable / total`, in basis points.
- `complete` — whether every in-span failure was classifiable.

Coverage never silently undercounts: when it is too low to trust, the verdict is
`insufficient_data` with reason `partial_index_coverage`.

## Verdict

`verdict` is one of `improved`, `regressed`, `unchanged`, `insufficient_data`,
computed by these exact, inspectable thresholds:

1. `insufficient_data` when **any** of these hold (all triggered reasons are
   listed in `insufficient_reasons`):
   - `post_window_too_short` — the post-window is shorter than **7 days**.
   - `sparse_occurrences` — fewer than **2** total occurrences across both
     windows.
   - `partial_index_coverage` — `coverage_bps` below **5000** (50%).
   - `selector_grammar_mismatch` — a trigger selector's signature grammar is
     older than the newest grammar present in the index, and the index holds no
     rows of that older grammar (as after `reindex --index-tool-input` re-mints
     every row under the current grammar). The selector's operation key then
     matches zero events, so the counts are blind to the failures it was minted
     against — not evidence they stopped. A selector cannot be silently
     translated to the current grammar (the current grammar's normalization
     cannot be derived from the old string), so the honest resolution is to
     re-propose the mutation from current findings.
2. Otherwise, from the change in weekly rate:
   - With no pre-window baseline (pre rate 0), a nonzero post rate is
     `regressed` (a new recurrence appeared); `rate_delta_bps` is omitted.
   - With a nonzero baseline, `rate_delta_bps` is the signed relative change
     (`-3300` = a 33% reduction). A change of **−2000 bps or more** (≥20%
     reduction) is `improved`; **+2000 bps or more** is `regressed`; anything
     inside the ±20% band is `unchanged`.

Because the windows are equal length, the rate comparison reduces to comparing
raw counts, but rates are reported so the numbers stay legible.

## Evidence

`evidence` lists the exact `event_id`s counted in each window, in canonical
order. Counts (`pre_event_count`, `post_event_count`) are always exact; the
listed identifiers are capped at `listing_cap` (50) so a pathological corpus
cannot produce an unbounded report.
