# Improvement Loop and Mutations

## 4. The Autophagy Loop

Observe → Digest → Detect patterns → Propose mutation → Challenge hypothesis →
Replay → Shadow mode → Promote → Measure → Revise or retire

### 4.1 Observe

Adapters emit normalized events from local coding agents. Initial capture should
favor native hooks and persisted session files over screen scraping.

- Session lifecycle
- Prompts and model responses where available
- Tool calls and exit codes
- File reads and writes
- Git status, diffs, commits, and branches
- Test outcomes
- User corrections and rejected actions
- Context compaction and retrieval events

### 4.2 Digest

Raw transcripts are transformed into compact, structured facts: decisions,
failures, recovery sequences, corrections, outcomes, artifacts, and repeated
motifs. Digestion should be incremental and idempotent.

### 4.3 Detect patterns

V1 should begin with deterministic detectors, not an open-ended “find insights”
prompt.

| Detector | Example signal |
| --- | --- |
| Repeated command failure | Same normalized command or error signature fails across multiple sessions. |
| Retry equivalence | The agent repeats semantically equivalent tool calls without changing the hypothesis. |
| Correction recurrence | The user corrects the same behavior in independent sessions. |
| Successful recovery motif | A repeated sequence consistently resolves a recurring failure. |
| Missing preflight | Successful sessions contain a step that failed sessions omit before the same action. |
| Lost prior solution | A relevant successful prior session existed but was not retrieved. |
| Repeated investigation path | The same files, dashboards, logs, or commands are traversed repeatedly. |
| Stale knowledge | A mutation has not triggered, been recalled, or produced value within its decay window. |

### 4.4 Propose a mutation

An LLM receives a compact evidence packet and produces a strictly structured
hypothesis with trigger conditions, exclusions, action, expected result,
supporting event IDs, and failure cases. It must be allowed to return
“insufficient evidence.”

### 4.5 Challenge

- Could the pattern be coincidental?
- Are the sessions genuinely comparable?
- Is the trigger observable before the failure?
- Would the intervention interrupt legitimate workflows?
- Does an equivalent mutation already exist?
- Is contradictory or successful counterexample evidence being ignored?

### 4.6 Replay

Replay is the technical heart of the product. Autophagy evaluates the proposed
mutation against relevant historical decision points. For executable changes,
it may create temporary Git worktrees and run tests. For instruction or
retrieval changes, it simulates the decision point and compares predicted action
quality, tool failures, outcome, and unnecessary interventions.

```text
Relevant sessions:             9
Failures potentially avoided:  7
Correct no-ops:                 2
False interventions:           0
Contradictory cases:            0
```

### 4.7 Shadow mode

The mutation observes live sessions but does not alter behavior. It records
where it would have triggered and compares that recommendation with the actual
outcome. Promotion requires a minimum observation count and acceptable
false-positive rate.

### 4.8 Promote, measure, and prune

A user approves promotion. The mutation becomes active with explicit
permissions. Autophagy continues measuring value and retires mutations that
become stale, harmful, redundant, or unused.

## 5. What Is a Mutation?

A mutation is a durable, versioned behavioral change produced from experience.
It is not limited to prompts.

- Agent skill or SKILL.md
- Pre-tool or post-tool hook
- Repository instruction
- Runbook or decision tree
- Checklist or preflight
- Command wrapper
- MCP tool
- Small script
- Test or invariant
- Context retrieval rule
- Guardrail or block condition
- Prompt fragment with explicit trigger and exit conditions

### 5.1 Mutation lifecycle

```text
Candidate → Challenged → Replay passed → Shadow → Active → Strengthened / Revised / Rejected / Retired
```

### 5.2 Mutation package

```text
schema-change-preflight/
├── mutation.yaml
├── SKILL.md
├── permissions.json
├── evidence.json
├── evaluation.json
├── hooks/
│   └── pre_tool_use.ts
└── tests/
    ├── stale-client.json
    └── unrelated-change.json
```

```yaml
id: schema-change-preflight
version: 1.2.0
description: Regenerate clients after schema changes

triggers:
  changed_files:
    - "**/*.graphql"
    - "**/openapi.yaml"
    - "**/schema.prisma"

intervention:
  type: agent_instruction
  skill: SKILL.md

permissions:
  filesystem: read
  commands:
    - "pnpm generate"
    - "npm run generate"
    - "go generate ./..."

promotion:
  minimum_replays: 5
  minimum_success_rate: 0.8
  maximum_false_positive_rate: 0.1
```
