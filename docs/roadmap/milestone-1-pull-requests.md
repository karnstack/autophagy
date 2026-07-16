# Milestone 1 pull-request sequence

Each pull request should be independently useful, tested, and reviewable. Later
PRs may refine this sequence through an ADR, but should not collapse the privacy
or evidence boundaries.

## PR 1 — AEP foundation

- Rust workspace and contribution/security baseline
- Normative AEP v0.1 JSON Schema
- Strict Rust event types, parsing, semantic validation, and fixtures
- Repository and database design documents

Exit: valid fixtures round-trip without information loss; invalid event semantics
are rejected with useful paths; formatting, linting, and tests pass.

## PR 2 — SQLite event store

- Immutable migration framework and the documented core schema
- Atomic validated-event insertion and session rollups
- Idempotent duplicate handling and conflict quarantine
- Redaction-safe FTS5 projection, deletion, and store tests

Exit: importing the same corpus twice changes no rows and an ID/content conflict
cannot overwrite evidence.

## PR 3 — Generic JSONL vertical slice

- `autophagy import`, `autophagy sessions`, and `autophagy search`
- Generic AEP JSONL adapter with streaming diagnostics
- Project selection, dry-run, and machine-readable output

Exit: a fixture corpus can be imported, listed, and searched entirely offline.

## PR 4 — Claude Code adapter

- Discovery separated from ingestion
- Native session-to-AEP normalization and capability matrix
- Incremental cursoring, anonymized fixtures, and adapter contract tests

Exit: a user can preview exactly what will be read, import it twice safely, and
trace every normalized event back to its source record.

## PR 5 — Codex adapter and adapter harness

- Shared adapter conformance harness
- Codex history discovery and normalization
- Cross-adapter ordering and provenance tests

Exit: the same store can ingest two agents without losing source provenance.

## PR 6 — Deterministic findings

- Repeated command-failure and repeated user-correction detectors
- Normalized error signatures and recurrence scoring
- Evidence packet v0.1 with exact event IDs and counterexamples

Exit: the demo corpus produces stable, evidence-linked findings and produces no
finding when recurrence thresholds are not met.

## PR 7 — Digestion CLI and privacy controls

- `autophagy digest` and `autophagy patterns`
- Path exclusions, retention policy, secret-redaction rules, export/delete
- End-to-end demo and threat-model updates

Exit: Milestone 1 can demonstrate local import, retrieval, two detectors, and
inspectable evidence without a model or network.
