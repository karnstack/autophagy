# Retrieval v0.1

Retrieval v0.1 defines the versioned, deterministic ranking explanation attached
to every hit returned by exact-and-hybrid evidence recall.

- [`schema.json`](schema.json) is normative for the ranking-explanation object.
- [`valid/`](valid) holds explanations the schema accepts.
- [`invalid/`](invalid) holds explanations the schema rejects.

Retrieval is model-free and local-only. It recalls evidence by exact normalized
operation signature, by full-text match over the redaction-approved projection,
or by both. No embedding, model, or network call participates.

## Exact-first hybrid ranking

Each hit records a `match_kind` and a deterministic integer `rank_score_bps`
formed by summing its scored signals:

- `exact_signature` contributes 10000 bps.
- `full_text` contributes 5000 bps.
- `recency` contributes 0 bps by design: it is an ordering tie-break, never a
  score. This guarantees an exact-signature match always outranks a
  full-text-only match, so a `signature_and_full_text` hit (15000) precedes an
  `exact_signature` hit (10000), which precedes a `full_text` hit (5000).

The total order — and the value of every hit's `tie_break` string — is:

1. `rank_score_bps` descending;
2. within full-text matches, bm25 ascending (better textual match first);
3. `occurred_at` descending (more recent first);
4. `event_id` ascending (final deterministic tie-break).

## Filters

Repository (exact `project` path), recency (`since` lower bound), event kind
(exact AEP `type` values), and outcome (`success` or `failure` polarity) narrow
both match sources identically. Every applied filter is echoed into each hit's
`applied_filters` so a result set is self-describing.

## Privacy

The exact-signature index is built only from the redaction-approved search
projection, exactly like free-text FTS content. A normalized signature embeds
command text, so it is indexed only when the source's tool input is already
approved for indexing; raw event JSON never becomes searchable text. See
[ADR 0003](../../decisions/0003-exact-and-hybrid-retrieval.md).
