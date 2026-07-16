# Product Vision and User Experience

## 1. Executive Summary

Autophagy is an open-source, local-first system that continuously improves how
coding agents behave. It captures sessions from tools such as Claude Code,
Codex, Cursor, Gemini CLI, OpenCode, and Aider; detects repeated mistakes and
successful procedures; converts them into versioned “mutations” such as skills,
hooks, runbooks, checklists, or guardrails; evaluates those mutations against
historical sessions; and promotes only changes that demonstrate measurable
value.

> **Category definition**
>
> Memory answers: “What happened before?” Autophagy answers: “What should
> permanently change because of what happened?”

### Product promise

- Observe local agent activity without sending private code or transcripts to a
  hosted service by default.
- Find repeated behavior using deterministic signals before involving an LLM.
- Generate concrete, executable improvements instead of generic summaries.
- Require evidence, replay, shadow mode, and human approval before activation.
- Measure whether an intervention saved time, reduced failed tool calls,
  improved outcomes, or caused false positives.
- Continuously strengthen, revise, decay, or retire learned behavior.

### The initial wedge

Launch as a polished macOS menu-bar application backed by an open-source,
cross-platform Rust engine. The Mac app provides the “wow” onboarding
experience: it discovers existing coding-agent histories, analyses the last 30
days locally, and shows one or two high-confidence improvements with exact
evidence and a replay score.

### The “holy shit” moment

```text
Across 11 sessions in 4 repositories:

Schema changed → build ran → generated client was stale → build failed → generator ran → build passed

Proposed mutation: Schema Change Preflight
Historical replay: prevented 9 of 11 failures
False interventions: 0
Estimated time saved: 58 minutes

[Inspect evidence] [Run in shadow mode] [Install mutation]
```

### Non-goals for v1

- A generic chat interface over session history.
- A vector database wrapper marketed as memory.
- Fully autonomous execution of generated scripts.
- A broad personal productivity coach.
- A marketplace before the mutation format and trust model are proven.
- Cloud accounts as a prerequisite for basic use.

## 2. Product Principles

| Principle | Meaning |
| --- | --- |
| Local first | All capture, indexing, redaction, retrieval, and default analysis run on the user’s machine. |
| Evidence over eloquence | A mutation is never accepted because a model produced a convincing explanation. |
| Model proposes, system proves | LLMs form hypotheses; deterministic checks, replay, tests, and shadow observations decide usefulness. |
| Concrete behavior only | Every insight must map to a trigger, an action, exclusions, and a measurable expected outcome. |
| Reversible by design | Every mutation is versioned, inspectable, permission-scoped, and removable. |
| Aggressive forgetting | Raw history may be retained by policy, but learned behavior decays unless reused or reconfirmed. |
| Cross-agent neutrality | Autophagy becomes the shared learning layer across all local coding agents. |
| Honest silence | “Nothing worth changing was found” is a valid and desirable weekly result. |

## 3. User Experience

### 3.1 Onboarding

1. Install the app with Homebrew or download a signed build.
2. Autophagy scans only known local agent-history directories and displays
   exactly what it found.
3. The user selects repositories, tools, exclusions, retention, and whether any
   cloud model is allowed.
4. The app imports and normalizes session events into a local SQLite database.
5. A first digestion runs against a bounded period, such as the last 30 days.
6. The app presents no more than three high-confidence findings, each linked to
   exact evidence.

```sh
brew install --cask autophagy

autophagy doctor
autophagy import --last 30d
autophagy digest
```

### 3.2 Menu-bar app

```text
AUTOPHAGY

● Claude Code       active · 18m
● Codex             active · 4m

Today
12 sessions observed
3 sessions digested
1 mutation ready
2 repeated failures prevented

[Review mutation]
```

### 3.3 Main navigation

| View | Purpose |
| --- | --- |
| Today | A compact feed of prevented failures, useful recalls, new patterns, and retired mutations. |
| Patterns | Recurring behavior ranked by recurrence, impact, confidence, actionability, and freshness. |
| Mutations | Candidate, replaying, shadow, active, revision-needed, rejected, and retired improvements. |
| Genome | The complete set of learned skills, guardrails, runbooks, conventions, heuristics, and preferences. |
| Timeline | Searchable cross-agent history with sessions, commands, files, failures, corrections, and outcomes. |
| Lab | Counterfactual replay, worktree evaluation, side-by-side session comparison, and mutation testing. |
| Settings | Privacy, model routing, data retention, integrations, permissions, redaction, and export controls. |

### 3.4 High-value experiences

- **Session Autopsy:** explain where a bad session went wrong and propose only
  concrete changes.
- **You Already Solved This:** surface a prior incident at the exact moment a
  similar failure appears.
- **Weekly Evolution Report:** show promoted, revised, and retired mutations
  plus measured impact.
- **Agent Comparison:** compare tools on the user’s own tasks, not a generic
  benchmark.
- **Brutal Mode:** show nothing unless the system can recommend a specific
  behavioral change.
