# Milestone 1 threat model

Status: implemented baseline, 2026-07-16

## Protected assets

- prompts, agent messages, tool inputs, and tool outputs;
- repository names, local paths, diffs, commands, and test failures;
- credentials accidentally embedded in any of those values;
- evidence lineage and findings derived from private work.

## Trust boundaries

```text
local agent transcript
  -> adapter normalization
  -> path exclusion + secret redaction
  -> validated AEP event
  -> SQLite canonical event + explicit FTS projection
  -> deterministic detector
  -> Evidence Packet / local JSONL export
```

Milestone 1 contains no network client, telemetry path, cloud inference, or model
invocation. Native source transcripts remain owned by their agent and are never
modified. The SQLite database and user-created exports are local trust
boundaries.

## Threats and controls

| Threat | Implemented control |
|---|---|
| Accidental credential persistence | Recursive deterministic redaction before store insertion |
| Sensitive project ingestion | Repeatable project/artifact glob exclusions before persistence |
| Private free text entering search | FTS accepts only explicit redaction-approved projections |
| Adapter rescan leaks previously excluded data | Privacy policy and exclusions are cursor-scoped |
| Fabricated behavioral finding | Thresholded deterministic detectors with exact evidence IDs |
| Counterevidence hidden from review | Evidence Packet carries explicit opposite outcomes |
| Accidental full deletion | Exact `--confirm delete-all` phrase required |
| Unverifiable retention effect | Prune dry-run executes and rolls back the real transaction |
| Dangling deleted evidence | Foreign-key cascades and orphan-artifact cleanup |
| Silent ID overwrite | Conflicting immutable event bodies are quarantined |
| Candidate gains execution authority | Mutation Package v0.1 permits only zero-permission instructions |
| Candidate survives evidence deletion | Any removed cited event cascades deletion of the candidate and its audit |
| Unreviewed candidate advances | Challenge requires all six explicit adversarial checks |
| Replay fixture executes untrusted content | Replay v0.1 performs exact string matching only; reports assert no mutation or model execution |
| Easy-only replay creates false confidence | Passing requires intervention and no-op coverage plus package thresholds |
| Shadow mode changes agent behavior | Shadow evaluator only exact-matches annotations and asserts `mutation_applied: false` |
| Installer overwrites user content | Codex skill target uses create-new semantics and refuses existing paths |
| Installer escapes repository | Every created path component is canonicalized and checked against the approved root |
| Uninstall deletes user edits | Rollback verifies the installation SHA-256 and refuses content drift |
| Evidence deletion orphans active behavior | Retention and deletion require audited uninstall first |

## Residual risks

- Pattern-based secret detection has false negatives and false positives.
- Tool inputs are structural evidence and may retain private non-secret data.
- `--include-content` materially expands retained private text.
- Exclusion globs protect only paths represented in normalized event fields.
- Agent transcripts, backups, snapshots, WAL copies, shell history, and exported
  JSONL are outside delete-all's reach.
- A local process with the user's filesystem permissions can read the database.
- Findings demonstrate recurrence, not causality or intervention correctness.
- Shadow usefulness labels remain human-authored annotations, not causal proof.
- Codex skill selection is agent-mediated; exact-selector shadow metrics do not
  guarantee identical implicit skill activation behavior.
- Manual edits or removal of an installed skill create drift that requires user
  resolution before audited uninstall can complete.

## Security invariants

1. Redaction and exclusions run before canonical persistence.
2. No model or network is needed to import, search, detect, digest, export, or
   delete Milestone 1 evidence.
3. Every finding cites exact immutable event IDs.
4. No finding is emitted below configured recurrence and independence thresholds.
5. Destructive operations are explicit, scoped, and report their effect.
6. Candidate generation cannot install content, execute commands, write files,
   read environment variables, or access the network.
7. Candidate packages are immutable; lifecycle state changes are append-only
   audited transitions outside the package.
8. Replay v0.1 never executes mutation content, commands, hooks, or models.
9. Installation requires a shadow-passed state and exact
   `repo-skill-write` confirmation; uninstall is hash-verified and audited.

Report failures of these invariants privately using the process in
[`SECURITY.md`](../../SECURITY.md).
