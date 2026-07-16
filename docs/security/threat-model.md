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

## Residual risks

- Pattern-based secret detection has false negatives and false positives.
- Tool inputs are structural evidence and may retain private non-secret data.
- `--include-content` materially expands retained private text.
- Exclusion globs protect only paths represented in normalized event fields.
- Agent transcripts, backups, snapshots, WAL copies, shell history, and exported
  JSONL are outside delete-all's reach.
- A local process with the user's filesystem permissions can read the database.
- Findings demonstrate recurrence, not causality or intervention correctness.

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

Report failures of these invariants privately using the process in
[`SECURITY.md`](../../SECURITY.md).
