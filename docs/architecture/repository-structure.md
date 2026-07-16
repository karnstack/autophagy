# Repository structure

Status: accepted for Milestone 1 (2026-07-16)

The engine is a Rust workspace. Protocol and package schemas remain language
neutral. Native and adapter code live beside the engine without being coupled to
its release cadence.

```text
autophagy/
├── Cargo.toml
├── crates/
│   ├── autophagy-core/        orchestration and public engine API
│   ├── autophagy-events/      AEP types, parsing, and validation
│   ├── autophagy-store/       SQLite migrations, queries, and retention
│   ├── autophagy-retrieval/   FTS/exact/hybrid retrieval
│   ├── autophagy-patterns/    deterministic detectors and scoring
│   ├── autophagy-digest/      evidence packets and incremental digestion
│   ├── autophagy-mutations/   mutation packages and lifecycle
│   ├── autophagy-replay/      replay scenarios and evaluation
│   ├── autophagy-shadow/      observation-only precision measurement
│   ├── autophagy-install/     reversible agent materializers
│   ├── autophagy-redaction/   secrets and path-policy enforcement
│   ├── autophagy-mcp/         versioned MCP surface
│   ├── autophagy-daemon/      scheduling and local IPC
│   └── autophagy-cli/         user-facing command line
├── apps/
│   └── macos/                 SwiftUI menu-bar application
├── adapters/
│   ├── claude-code/
│   ├── codex/
│   └── generic-jsonl/
├── packages/
│   ├── sdk/                   TypeScript adapter SDK
│   ├── mutation-schema/       language-neutral mutation contract
│   ├── hook-runtime/          permission-scoped hook execution
│   └── redaction-rules/       shared rule fixtures
├── mutations/                 reviewed example mutation packages
├── evals/                     anonymized replay and detector corpora
├── docs/
│   ├── architecture/
│   ├── decisions/
│   ├── roadmap/
│   └── specs/
└── website/
```

`autophagy-events`, `autophagy-store`, `autophagy-core`, `autophagy-cli`, the
native Claude Code and Codex adapters, and their shared conformance harness
exist through PR 5. `autophagy-patterns` and Evidence Packet v0.1 begin in PR 6
and now cover command failures, explicit corrections, and successful recovery
motifs. `autophagy-redaction` and the offline digestion/privacy CLI complete
Milestone 1 through PR 7. `autophagy-mutations` begins Phase 2 with the
review-only Mutation Package v0.1 contract, while `autophagy-store` owns the
immutable candidate registry, evidence links, lifecycle audit, and replay
reports. `autophagy-replay` derives evidence-linked decision-point review
drafts, preserves unknown counterfactuals, and performs deterministic
non-executable evaluation. `autophagy-shadow` measures would-be
trigger precision without intervention, and `autophagy-install` owns the
repo-scoped skill materializers and rollback boundary. A crate or package is
added when its PR contains an executable vertical slice; empty placeholder crates
are avoided.

Continuous ingestion (0.1.0) reuses existing crates rather than introducing the
sketched `autophagy-daemon` crate, which would have been a placeholder. The
model-free watch loop and its `WatchSource` seam live in `autophagy-core`; the
CLI implements the seam for the native adapters and drives the daemon lifecycle.
`autophagy-install` remains the only crate that writes outside the database, and
its charter is extended from repo-scoped skills to explicit, reversible
out-of-database filesystem artifacts generally — which now also covers the
launchd/systemd supervisor unit that runs `autophagy watch` (see ADR 0008). The
CLI shims `launchctl`/`systemctl`; no crate gains process-execution
responsibility.

## Dependency direction

```text
adapters -> events -> store -> retrieval -> patterns -> digest
                    \                         /
                     +------ core -----------+
                              |
                    cli / daemon / MCP / macOS
```

`autophagy-events` has no dependency on storage, models, adapters, or UI.
Storage accepts validated events and owns transactionality and idempotency.
Detectors consume query interfaces rather than SQLite connections. Model-backed
synthesis is downstream of deterministic pattern discovery.

## Versioning boundaries

- AEP versions serialized events independently of crate versions.
- SQLite uses ordered, immutable migrations and a schema version table.
- Mutation packages declare their own format version and semantic version.
- CLI and local IPC responses are versioned before external integrations depend
  on them.
