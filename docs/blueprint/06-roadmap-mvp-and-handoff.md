# Roadmap, MVP, and Implementation Handoff

## 12. Execution Roadmap

### 12.1 First 90 days

| Weeks | Goal | Deliverables |
| --- | --- | --- |
| 1–2 | Event foundation | AEP v0.1, Claude Code importer, Codex importer, generic JSONL importer, SQLite schema, CLI import/search. |
| 3–4 | Mac experience | SwiftUI app, onboarding, menu bar, timeline, project exclusions, local database viewer, search. |
| 5–6 | Deterministic digestion | Repeated failures, retries, corrections, recovery motifs, file sequences, lost prior solution detectors. |
| 7–8 | Mutation candidates | SKILL.md, hooks, checklists, command wrappers, evidence packs, permission manifests. |
| 9–10 | Shadow mode | Would-have-triggered observations, precision metrics, review and promotion UI. |
| 11–12 | Initial replay | Replay for command preflights and context-injection skills; public alpha and launch demo. |

### 12.2 Phase roadmap

| Phase | Theme | Exit criterion |
| --- | --- | --- |
| Phase 1 | Addictive importer | A new user imports history and sees at least one evidence-backed repeated pattern. |
| Phase 2 | Mutations | The product generates a useful skill or guardrail and can install it into one supported agent. |
| Phase 3 | Replay | The product demonstrates that a mutation improves historical outcomes with bounded false positives. |
| Phase 4 | Team genome | Multiple developers can safely share verified learning within a repository or organization. |

## 13. MVP Definition

> **MVP test**
>
> Can Autophagy identify one painful repeated agent behavior, prove that a
> concrete intervention improves it, and permanently prevent it?

### 13.1 Required for alpha

- Claude Code session ingestion and one additional adapter.
- Local SQLite storage with exact and semantic search.
- At least three deterministic detectors.
- Evidence-linked pattern view.
- Structured mutation generation with local model support.
- One mutation type: preflight skill or context injection.
- Manual review and installation.
- Shadow mode or a basic replay implementation.
- Privacy controls, redaction, exclusions, and complete deletion.
- A native macOS experience polished enough for a compelling demo.

### 13.2 Explicitly defer

- Windows and Linux GUI
- Marketplace
- Cross-device sync
- Team management
- Complex autonomous scripts
- General-purpose agent benchmark dashboards
- Ten agent integrations
- Knowledge-graph-heavy visualization

## 14. Suggested Initial GitHub Epics and Issues

| Epic | Initial issues |
| --- | --- |
| AEP and storage | Define AEP schema; event validator; SQLite migrations; import idempotency; event retention; fixture corpus. |
| Adapters | Claude Code importer; Codex importer; generic JSONL adapter; adapter test harness; capability matrix. |
| Retrieval | FTS5 search; exact error-signature search; embedding interface; hybrid ranking; repository and recency boosts. |
| Detectors | Repeated failure detector; equivalent retry detector; correction detector; recovery-sequence detector; detector scoring. |
| Digestion | Structured evidence packet; incremental digestion; counterexample retrieval; deduplication; stale pattern decay. |
| Models | llama.cpp integration; model manifest; structured output schema; model download manager; BYOM provider interface. |
| Mutations | Mutation schema; package validator; permission manifest; mutation registry; versioning; install/uninstall adapters. |
| Replay | Decision-point extraction; replay scenario format; worktree runner; outcome metrics; false-positive evaluation. |
| Mac app | Onboarding; menu bar; Today; Patterns; Mutations; Timeline; Lab; permissions and privacy settings. |
| Security | Secret scanner; path exclusions; cloud payload preview; sandbox policy; audit log; delete/export all data. |
| Docs and launch | README; architecture ADRs; contributing guide; demo dataset; 75-second launch video; Homebrew packaging. |

## 15. Implementation Handoff Prompt

The following can be pasted directly into another coding agent to begin work:

```text
You are starting the Autophagy repository. Autophagy is an open-source, local-first self-improvement layer for coding agents. It observes local sessions, detects repeated failures and successful workflows, proposes concrete versioned mutations, evaluates them through replay and shadow mode, and promotes only evidence-backed improvements.

Start by building the foundation, not the full product.

Milestone 1:
1. Create a Rust workspace with crates for events, store, core, CLI, and one Claude Code importer.
2. Define Agent Event Protocol v0.1 as JSON Schema and Rust types.
3. Store normalized events in SQLite with migrations and FTS5.
4. Implement `autophagy import`, `autophagy sessions`, and `autophagy search`.
5. Add deterministic detectors for repeated command failures and repeated user corrections.
6. Produce an evidence packet JSON for each pattern.
7. Add fixture-based tests using anonymized sample sessions.
8. Write architecture decisions and keep all APIs versioned.

Constraints:
- Local-only by default.
- No cloud dependency.
- No generic LLM summaries in milestone 1.
- Every finding must cite exact evidence IDs.
- Import must be idempotent.
- All code should be cross-platform even though the first UI will target macOS.
- Prefer simple, inspectable schemas over premature abstractions.

Deliverables:
- Compiling Rust workspace.
- SQLite schema and migrations.
- AEP v0.1 specification.
- Claude Code importer.
- CLI import/search flow.
- Two deterministic detectors.
- Unit and fixture tests.
- README with setup and demo commands.

Before writing code, propose the exact directory structure, database schema, event schema, and the sequence of small PRs. Then implement PR 1 only.
```
