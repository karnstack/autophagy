# Retrieval

Autophagy recalls evidence and prior recoveries deterministically, without a
model. The `autophagy search` command combines exact normalized-signature
lookup with full-text search over the redaction-approved projection, applies
repository, recency, event-kind, and outcome filters, and attaches a versioned
ranking explanation to every result.

The normative explanation schema is
[`docs/specs/retrieval/0.1/schema.json`](../specs/retrieval/0.1/schema.json).
The design rationale and privacy stance are in
[ADR 0003](../decisions/0003-exact-and-hybrid-retrieval.md).

## Exact normalized signatures

Every tool event with an inspectable command is indexed under a normalized
operation signature such as `operation/v1|shell|cargo build`. The same
model-free normalizer that powers the deterministic detectors collapses tool
aliases (`bash`, `exec_command`, `shell`), whitespace, and the exact project
prefix (`$PROJECT`), so incidental variation does not fragment identical
operations.

Look one up exactly:

```sh
autophagy search --signature 'operation/v1|shell|cargo build'
```

Exact-signature lookup does not require a text query. Combine it with a text
query for hybrid recall, or use text alone for classic search.

## Exact-first hybrid ranking

Results are ranked deterministically. Each hit carries an integer
`rank_score_bps`:

- an exact-signature match contributes 10000 bps;
- a full-text match contributes 5000 bps;
- recency contributes 0 bps — it only breaks ties within a tier.

So an exact-signature match always outranks a full-text-only match. The full
total order, restated in each hit's `tie_break` field, is: score descending,
then bm25 ascending among full-text matches, then `occurred_at` descending, then
`event_id` ascending. For a fixed database the same query returns the same
ordered event IDs every time.

## Filters

Each filter narrows both the exact and full-text match sources identically:

```sh
# Repository filter (exact project path).
autophagy search --signature 'operation/v1|shell|cargo build' --project /repo/alpha

# Recency filter (events within the last 14 days).
autophagy search 'linker error' --since-days 14

# Event-kind filter (repeatable; exact AEP types).
autophagy search 'pytest' --event-kind tool.failed --event-kind test.failed

# Outcome filter (success or failure polarity).
autophagy search --signature 'operation/v1|shell|cargo build' --outcome failure
```

Applied filters are echoed into every hit's explanation, so a result set is
self-describing.

## Machine-readable output

`--output json` emits each hit with its exact `event_id`, `session_id`,
`event_type`, `occurred_at`, `signature`, optional `snippet`, and the full
`explanation` object:

```sh
autophagy --output json search 'succeeded' --signature 'operation/v1|shell|cargo build'
```

## Privacy

A signature embeds normalized command text, so it enters the searchable index
only through the same redaction-approved projection that gates free-text tool
input. Import with `--index-tool-input` only after confirming the source is
already redacted. When tool-input indexing is not approved, no signature is
indexed and exact-signature lookup simply returns nothing — raw event JSON never
becomes searchable text.
