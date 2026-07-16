# Changelog

All notable changes to this project are documented in this file.

The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-07-17

The first release. The workspace version is `0.1.0`.

### Added

#### Milestone 1 — local observation engine (offline, model-free)

- Agent Event Protocol (AEP) v0.1: normative JSON Schema, strict Rust event
  types, parsing, semantic validation, and valid/invalid fixtures (#1).
- Transactional SQLite event store: immutable ordered migrations, atomic
  validated-event insertion, session rollups, idempotent duplicate handling,
  ID/content conflict quarantine, a redaction-safe FTS5 projection, and
  cascading deletion (#3).
- Generic JSONL vertical slice with the `autophagy import`, `sessions`, and
  `search` commands, streaming diagnostics, project selection, dry-run, and
  machine-readable output (#4).
- Incremental Claude Code adapter: discovery separated from ingestion, native
  session-to-AEP normalization with a documented capability matrix, incremental
  cursoring, and anonymized fixtures (#5).
- Codex adapter and a shared cross-adapter conformance harness that preserves
  source provenance across agents (#6).
- Deterministic evidence-linked findings: repeated command-failure and repeated
  user-correction detectors, normalized signatures, recurrence scoring, and
  Evidence Packet v0.1 carrying exact event IDs and counterexamples (#7).
- Offline digestion and privacy controls: `autophagy digest` and `patterns`,
  path exclusions, age-based retention (`prune`), secret-redaction rules,
  `export`, `delete`, and an end-to-end offline demo (#8).

#### Phase 2 — review-only mutation lifecycle

- Review-only, zero-permission mutation candidates: Mutation Package v0.1 schema
  and validator, deterministic `agent_instruction` candidates, exact
  evidence/counterexample lineage, and insufficient-evidence refusal (#9).
- Immutable, audited mutation candidate registry: lifecycle transitions,
  duplicate/equivalent detection, a challenge checklist, and rejection reasons
  (#10).
- Non-executable deterministic replay evaluation: versioned decision-point and
  result schemas measuring success, no-op, contradiction, and false-intervention
  outcomes (#11).
- Shadow-gated, reversible Codex skill installation: observation-only trigger
  precision measurement, explicit permission review, an install audit, and
  uninstall rollback (#12).

#### Alpha — recovery, retrieval, synthesis, and inspection

- Repeated successful-recovery-motif detector with direct-retry counterexamples,
  non-inflated occurrence scoring, and a conservative zero-permission preflight
  candidate template (#13).
- Replay decision-point draft extraction from exact mutation evidence and nearby
  session context, preserving unknown counterfactual labels and exporting Replay
  Suite v0.1 annotation drafts (#14).
- Exact and hybrid retrieval: exact normalized-signature lookup alongside FTS5,
  repository/recency/event-kind/outcome filters, and versioned deterministic
  ranking explanations on every result (#16).
- Local synthesis boundary: a provider-neutral structured synthesis interface, a
  local model manifest, explicit insufficient-evidence behavior, and
  deterministic validation of every generated package field (#17).
- Native read-only macOS app: onboarding and database selection; sessions,
  patterns, mutations, and lifecycle-audit views; privacy settings; and
  destructive-action confirmation (#18).

#### 0.1.0 — installation targets, local model providers, more adapters, and continuous ingestion

- Claude Code installation target: shadow-passed mutations can now be
  materialized as repo-scoped Claude Code skills at
  `.claude/skills/autophagy-<id>/SKILL.md`, sharing one
  `InstallTarget`-parameterized planning, materialization, and rollback path
  with the existing Codex target (`--target codex|claude-code`) (#21).
- Local model synthesis providers: an Ollama provider and an OpenAI-compatible
  provider (llama.cpp, LM Studio, vLLM) enrich mutation candidates against a
  loopback-default local inference endpoint, with redirect-safety hardening,
  explicit token accounting, and a mutation/0.2 provenance block recording
  provider, model, and prompt/response token counts on every synthesized
  candidate (#22).
- Pi and OpenCode adapters: two more native session adapters, mirroring the
  Codex adapter's discovery/ingestion separation, incremental cursoring, and
  redaction-gated search projection, so `autophagy import --adapter pi` and
  `--adapter opencode` join Claude Code and Codex (#23).
- macOS menu-bar experience: an always-available menu-bar extra showing
  connection state, quick counts, candidate counts by lifecycle state, and the
  most recent candidates, plus an opt-in menu-bar-only (no Dock icon) runtime
  preference and mutation-detail surfacing of v0.2 synthesis provenance and
  install target (#24).
- Watch mode and daemon lifecycle: a foreground `autophagy watch` loop and an
  `autophagy daemon install|uninstall|status` lifecycle (launchd on macOS,
  a systemd user unit on Linux) for continuous, ingest-only import under the
  same redaction and privacy gates as one-shot import (#25, #26).

### Fixed

- Retrieval CLI and ranking edge cases: `autophagy search` with neither a
  query nor `--signature` now fails fast as an argument error, and dual
  signature-and-full-text matches outside a single source's top-N are no
  longer misclassified or dropped (#19).
- macOS app: read-only open of a cleanly checkpointed WAL database now
  succeeds instead of appearing empty, and refresh re-opens the read-only
  connection so newly written rows become visible (#24).

### Changed

- Normalized the product blueprint into Markdown (#2).
- Added repository agent guidance in `CLAUDE.md` (#15).

[Unreleased]: https://github.com/karnstack/autophagy/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/karnstack/autophagy/releases/tag/v0.1.0
