# ADR 0013: Cache deterministic detection findings in the store

- Status: accepted
- Date: 2026-07-17

## Context

Every findings-consuming command re-runs the full deterministic detection pass
over every stored event on each invocation: `patterns`, `digest`,
`mutations propose`, `mutations synthesize`, and `status --with-findings`. The
pass loads and deserializes each event's canonical JSON and runs all three
recurrence detectors. On a 69,400-event database that is roughly 45 seconds of
work, repeated in full every time, with no output until it finishes — the
command appears hung. Detection is a pure, deterministic function of its inputs,
so recomputing an unchanged result is wasted work.

The stored-schema constraint applies: this adds a table, so it takes an ordered,
immutable migration and this record. Two engineering constraints bound the
design — the default path stays local and offline, and every derived finding
must retain its exact evidence identifiers.

## Decision

Add a `findings_cache` table (migration `0002`) that memoizes the serialized
detection report, and route all five call sites through it.

- **Content-addressed validity, no explicit invalidation.** The cache key is a
  SHA-256 over every input the pass depends on: the detector/signature spec
  version (`DETECTION_SPEC_VERSION`), the effective `DetectorConfig` thresholds
  (`min_occurrences`, `min_sessions`, `min_support_ratio_bps`), the project
  filter, and a cheap content fingerprint of the events in scope — the event
  count, the maximum `row_id`, and the maximum `imported_at` (a monotonic import
  watermark that advances on every insert, distinguishing a delete-then-reimport
  from the original corpus even when count and max row id coincide). The
  fingerprint is computed from indexed columns only, never by deserializing event
  JSON, so the validity check is orders of magnitude cheaper than the pass it
  guards. Any import, delete, or prune moves at least one field, so a stale key
  simply misses; nothing has to remember to invalidate. A reindex changes only
  the derived search projection, not the events detection reads, so it correctly
  leaves the cache valid. Bumping `DETECTION_SPEC_VERSION` invalidates every
  entry, so a future signature-normalization change cannot serve an outdated
  report.
- **Bounded growth by generation.** Each row is tagged with a global corpus
  generation stamp. A write first drops every entry from an older generation,
  then inserts its own, so entries from superseded corpus states never
  accumulate while current-generation entries at different thresholds or projects
  coexist.
- **Store stays type-agnostic.** Detection types live in `autophagy-patterns`,
  which depends on the store, so the store must not depend on them. It treats the
  cached payload as opaque JSON and exposes only fingerprint, generation, and
  get/put primitives; the CLI — which sees both crates — computes the key and
  serializes the report. This preserves the one-way dependency direction.
- **Minimal surface.** `patterns` and `digest` gain a `--recompute` flag to force
  a fresh pass and refresh the entry; the other three call sites always read
  through the cache. When a fresh pass does run, a concise before/after progress
  line is written to stderr (never stdout), so a long pass is visible and JSON
  output stays clean.

This is the first migration added after the `0001` baseline. The ADR 0012
pre-release adoption shim still runs first on open: a recognized legacy database
is adopted to the v1 baseline and then this v2 migration applies on top. Removing
that shim remains future work, out of scope here.

## Privacy

The cache is derived, deterministic, local-only data. It stores no new source
text — only the serialized detection report, whose findings carry the exact
evidence identifiers the detectors already produce from events already on disk.
It never leaves the machine, adds no network path, and is fully reconstructable
at any time by deleting every row (or passing `--recompute`). No secret or raw
cloud payload is persisted.
