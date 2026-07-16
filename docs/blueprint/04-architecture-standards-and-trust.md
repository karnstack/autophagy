# Architecture, Standards, and Trust

## 7. Technical Architecture

### 7.1 Components

| Layer | Technology | Responsibilities |
| --- | --- | --- |
| Core engine | Rust | Collectors, normalization, indexing, redaction, scheduling, detectors, scoring, mutation lifecycle, CLI, MCP server, daemon. |
| macOS application | SwiftUI | Menu bar, onboarding, notifications, review UI, timeline, Lab, permissions, model downloads, privacy controls. |
| Adapter SDK | TypeScript | Claude Code hooks, Codex integrations, community adapters, mutation hook runtime. |
| Storage | SQLite + FTS5 + optional sqlite-vec | Events, evidence graph, analytics, lexical retrieval, vector retrieval. |
| Inference | llama.cpp initially; optional MLX/Ollama endpoints | Local structured generation, embeddings, reranking, and role-based model routing. |
| IPC | Unix domain socket or localhost API | Communication between native app and daemon. |
| Evaluation | Git worktrees + sandboxed command runner | Replay executable mutations against historical repository state where feasible. |

### 7.2 Repository structure

```text
autophagy/
├── README.md
├── LICENSE
├── CONTRIBUTING.md
├── SECURITY.md
├── AGENTS.md
├── Cargo.toml
├── crates/
│   ├── autophagy-core/
│   ├── autophagy-events/
│   ├── autophagy-store/
│   ├── autophagy-retrieval/
│   ├── autophagy-patterns/
│   ├── autophagy-digest/
│   ├── autophagy-mutations/
│   ├── autophagy-replay/
│   ├── autophagy-redaction/
│   ├── autophagy-mcp/
│   ├── autophagy-daemon/
│   └── autophagy-cli/
├── apps/macos/
├── adapters/
│   ├── claude-code/
│   ├── codex/
│   ├── cursor/
│   ├── gemini-cli/
│   ├── opencode/
│   ├── aider/
│   └── generic-jsonl/
├── packages/
│   ├── sdk/
│   ├── mutation-schema/
│   ├── hook-runtime/
│   └── redaction-rules/
├── mutations/
├── evals/
├── docs/specs/
└── website/
```

### 7.3 CLI surface

```text
autophagy init
autophagy doctor
autophagy watch
autophagy import
autophagy digest
autophagy patterns
autophagy autopsy --last
autophagy replay <mutation-id>
autophagy shadow <mutation-id>
autophagy install <mutation-id>
autophagy explain <mutation-id>
autophagy genome
autophagy prune
autophagy serve
```

## 8. Open Standards and Data Model

### 8.1 Agent Event Protocol (AEP)

Define a simple, versioned JSONL protocol so any agent can emit normalized
events and any tool can consume them.

```json
{
  "spec_version": "aep/0.1",
  "event_id": "evt_01...",
  "session_id": "ses_01...",
  "timestamp": "2026-07-16T01:22:31Z",
  "source": "claude-code",
  "type": "tool.failed",
  "project": "/Users/example/project",
  "tool": {
    "name": "bash",
    "input": "pytest tests/translation",
    "exit_code": 1
  },
  "artifacts": [
    { "type": "file", "path": "src/translation/memory.py" }
  ]
}
```

- `session.started`
- `session.ended`
- `prompt.submitted`
- `decision.recorded`
- `tool.called`
- `tool.completed`
- `tool.failed`
- `file.read`
- `file.changed`
- `test.failed`
- `test.passed`
- `user.corrected_agent`
- `user.rejected_action`
- `context.compacted`

### 8.2 Core entities

- Source
- Session
- Event
- Artifact
- Decision
- Failure
- Correction
- Outcome
- Pattern
- Hypothesis
- Mutation
- Replay
- Intervention
- Measurement
- Skill
- Evidence

### 8.3 Traceability graph

```text
Mutation
  ↓ generated because of
Pattern
  ↓ supported by
Evidence
  ↓ extracted from
Session events
  ↓ linked to
Git commits, files, commands, tests, and user corrections
```

## 9. Privacy, Security, and Trust

- No account required for the local product.
- No telemetry by default.
- Project-level include and exclude controls.
- Private-path denylist and configurable retention.
- Secret detection and redaction before persistence or cloud transmission.
- Cloud processing disabled by default and previewable before use.
- One-click deletion and export of all stored data.
- Every stored event and every mutation is inspectable.
- Generated scripts never receive execution permission automatically.
- Mutation packages declare filesystem, command, network, and environment
  permissions.
- All interventions are reversible and auditable.
- Team sharing requires explicit publication and redaction.

```text
Reads:
✓ repository files
✓ git history

May execute:
✓ npm test
✓ pnpm generate

Cannot access:
✗ network
✗ environment secrets
✗ files outside repository
```
