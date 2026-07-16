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
exist through PR 5. `autophagy-patterns` and Evidence Packet v0.1 exist through
PR 6. A crate or package is added when its PR contains an executable vertical
slice; empty placeholder crates are avoided.

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
