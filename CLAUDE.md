# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Autophagy observes local coding-agent sessions (Claude Code, Codex, generic JSONL), detects repeated failures and successful recovery paths with deterministic (model-free) detectors, and turns them into evidence-linked, versioned, reversible behavioral improvements called mutations. Local-first, offline by default.

## Commands

mise is the authoritative tool manager — never use an unpinned system Rust toolchain. Tool versions and tasks are pinned in `.mise.toml`.

```sh
mise install          # once, installs pinned toolchain
mise run check        # full PR quality gate: ci + docs + fmt + lint + test
```

Individual tasks:

```sh
mise run fmt          # cargo fmt --all --check
mise run lint         # cargo clippy --workspace --all-targets --all-features -- -D warnings
mise run test         # cargo test --workspace --all-features
mise run docs         # pandoc-validate all docs/**/*.md as GFM
mise run ci           # actionlint on GitHub Actions workflows
mise run demo         # offline end-to-end demonstration (scripts/demo-milestone-1.sh)
```

Single test / single crate:

```sh
mise exec -- cargo test -p autophagy-store some_test_name
mise exec -- cargo test -p autophagy-adapter-claude-code
```

Run the CLI (binary is named `autophagy`, lives in `crates/autophagy-cli`):

```sh
mise exec -- cargo run -p autophagy-cli -- --database /tmp/demo.db import evals/fixtures/generic-jsonl/demo.jsonl --instance-key demo
mise exec -- cargo run -p autophagy-cli -- --output json import --adapter claude-code --dry-run
```

Warnings are errors: clippy runs with `-D warnings` plus `pedantic`, `missing_docs = warn`, and `unsafe_code = forbid` (workspace lints in root `Cargo.toml`).

## Architecture

Rust workspace (edition 2024). Dependency direction is strict and flows one way:

```
adapters -> events -> store -> patterns -> mutations -> {replay, shadow, install}
                         \        /
                          core --+-- cli
```

- `crates/autophagy-events` — AEP (Agent Event Protocol) types, parsing, validation. Depends on nothing else; never couple it to storage, adapters, or models.
- `crates/autophagy-store` — SQLite migrations, transactional writes, idempotency, FTS, quarantine, cascading deletion. Validation runs before any rows are written; reimporting an identical event is a no-op; conflicting reuse of an event ID quarantines rather than overwrites.
- `crates/autophagy-redaction` — secret rules and path policy, applied at ingestion. Raw JSON is never copied into FTS; searchable text requires a redaction-approved projection.
- `crates/autophagy-core` — streaming import application services; the seam between adapters and storage.
- `adapters/claude-code`, `adapters/codex` — native transcript discovery and AEP normalization; `crates/autophagy-adapter-test-support` holds the shared conformance harness both must pass.
- `crates/autophagy-patterns` — deterministic, model-free recurrence detectors producing Evidence Packets with exact evidence IDs.
- `crates/autophagy-mutations` — review-only mutation candidate registry (immutable, audit-logged, lives in store).
- `crates/autophagy-replay` — non-executable deterministic replay evaluation; unreviewed counterfactual outcomes stay explicitly "unknown".
- `crates/autophagy-shadow` — observation-only trigger precision measurement.
- `crates/autophagy-install` — the only crate that writes outside the database: explicit, reversible, repo-scoped Codex skill materialization and rollback.
- `crates/autophagy-cli` — user-facing commands; the only crate that ties everything together.

Contracts live in `docs/specs/` (AEP, evidence, mutation, replay, shadow — each versioned, JSON Schema + fixtures). Architecture decisions in `docs/decisions/`; planned structure in `docs/architecture/repository-structure.md`; full product brief in `docs/blueprint/`.

## Engineering constraints (from AGENTS.md — non-negotiable)

- Keep the default path local-only and offline-capable.
- Never persist secrets or raw cloud payloads without explicit user consent.
- Every derived finding must retain exact evidence identifiers.
- Prefer deterministic, inspectable behavior over model-generated prose.
- Version public protocols and stored schemas before changing them.
- Do not add autonomous execution permissions by default.

A behavior-changing AEP/schema edit requires: a decision record, updated JSON Schema and Rust types, valid and invalid fixtures, and an explicit migration or compatibility story. SQLite migrations are ordered and immutable — add new ones, never edit old ones.

New crates are added only when a PR contains an executable vertical slice — no empty placeholder crates.

## Pull requests

- Title prefix exactly one of `feat:`, `fix:`, `maint:`. No agent or tool labels in titles.
- Keep PRs small enough to review as one coherent claim; explain the claim, the evidence verifying it, and any privacy implications.
- Run `mise run check` before requesting review.
