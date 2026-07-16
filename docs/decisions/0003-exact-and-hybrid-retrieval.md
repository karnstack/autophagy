# ADR 0003: Exact and hybrid retrieval

- Status: accepted
- Date: 2026-07-17

## Context

Evidence and prior recoveries must be recallable predictably without a model.
Full-text search over the redaction-approved projection already existed, but
free-text ranking alone cannot reliably re-find "the same failing command" or
its successful recovery: incidental variation in tool aliases, whitespace, and
project prefixes fragments otherwise identical operations, and bm25 relevance is
not a stable identity. A deterministic, inspectable recall path is required, and
it must not weaken the evidence or privacy boundaries.

## Decision

Add exact normalized-signature lookup alongside FTS5, combined by an
exact-first hybrid ranking with four filters and a versioned ranking
explanation.

- **One normalizer.** The command/tool normalization that the pattern detectors
  already use is extracted into `autophagy-events::signature` as a pure,
  model-free function and shared by both the detectors and the retrieval index.
  There is no second normalizer to drift.
- **Signature index.** A new ordered, immutable migration adds an
  `event_signatures` table keyed by `event_row_id` with a lookup index. Rows are
  written in the same transaction as the event and cascade on event deletion, so
  reimport idempotency, conflict quarantine, prune, and delete-all semantics are
  unchanged.
- **Exact-first ranking.** `exact_signature` contributes 10000 bps and
  `full_text` 5000 bps, so exact-signature matches always outrank
  full-text-only matches. `recency` is an ordering tie-break with a deliberate
  zero score weight, so it can never cross a tier. Ties resolve by score, then
  bm25, then `occurred_at` descending, then `event_id` ascending.
- **Filters.** Repository (exact project path), recency (`since` lower bound),
  event kind (exact AEP types), and outcome (success/failure polarity) narrow
  both match sources identically and are echoed into each hit.
- **Versioned explanation.** The ranking explanation is a new public contract at
  `docs/specs/retrieval/0.1/` (JSON Schema plus valid and invalid fixtures).
  Every hit carries its exact event identifier and the explanation.

## Privacy

The signature embeds normalized command text derived from tool input. It is
therefore added to the searchable index only through the same explicit
redaction-approved search projection that gates free-text tool input, and only
when the source's tool input is approved for indexing. Raw event JSON never
becomes searchable text, and when indexing is not approved the exact-signature
lookup simply returns nothing rather than leaking unapproved content.

## Consequences

- Recall of evidence and prior recoveries is deterministic and model-free; the
  same query returns the same ranked event IDs for a fixed database state.
- Result sets are self-describing: match kind, integer score, contributing
  signals, and applied filters accompany every hit.
- A future ranking change uses a new `retrieval/*` spec version and schema
  directory rather than silently reinterpreting stored explanations.
- The signature index adds one row per approved tool event; it holds no content
  the FTS projection could not already hold.
