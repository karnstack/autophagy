# Shadow and reversible installation

Shadow is the final measurement gate before a user may install an instruction
mutation. It observes where the immutable trigger would fire but never changes
agent context or behavior.

## Drafting a Shadow Suite

You do not have to hand-author the Shadow Suite JSON. `mutations shadow-draft`
exports an evidence-linked draft from the mutation's own supporting evidence and
counterexamples, mirroring `replay-draft`:

```sh
autophagy mutations shadow-draft mut_example \
  --suite shadow-review.json \
  --context-events 1
```

Supporting evidence becomes an observation drafted `intervention_would_help:
true`; counterexample evidence becomes one drafted `false`. Each observation
carries a human-readable `note` describing what to confirm against the real
outcome, the exact source event IDs, and the observed trigger selectors.
Observation IDs (`shd_…`) are derived deterministically from their inputs — no
clock, no randomness — so re-running produces byte-for-byte identical output.
Every observation's source event IDs are unique across the suite, as the
contract requires. Drafting only reads the database and writes the one file you
name; it performs no state transition. `--force` overwrites an existing file.

Review each drafted `intervention_would_help` against the real session outcome
before evaluating; the draft is a starting point, not a verdict.

## Evaluating a Shadow Suite

```sh
autophagy mutations shadow mut_example \
  --suite shadow-review.json
```

The canonical flag is `--suite`; the former `--observations` name still works as
a hidden alias. Each Shadow Suite v0.1 observation cites independent local AEP
event IDs,
records selectors visible before the outcome, and annotates whether intervention
would have helped. Exact selector matching produces true positives, true
negatives, false positives, and false negatives. Passing requires five
observations, positive and negative coverage, at least one would-be trigger, and
a false-positive rate no greater than the immutable package policy. Reports
always contain `mutation_applied: false` and `model_used: false`.

## Permission preview

The candidate itself remains zero-permission. Installation is a separate user
operation requesting one scoped filesystem effect: create one `SKILL.md` under
the selected repository's skill directory for the chosen coding agent.

```sh
autophagy mutations install mut_example \
  --repository /workspace/project \
  --target claude-code \
  --confirm-permissions repo-skill-write \
  --dry-run
```

The `--target` selector chooses the coding agent. It defaults to `codex`, so
existing invocations keep their behavior; pass `--target claude-code` for a
Claude Code skill. The target must be an existing Git repository root. Dry-run
reports the canonical repository, exact relative path, content hash, target, and
required permission without writing or activating anything.

## Codex repo skill target

With `--target codex` (the default), after removing `--dry-run` the materializer
creates:

```text
<repository>/.agents/skills/autophagy-<stable-id>/SKILL.md
```

This follows Codex's documented repo-scoped skill location and required
`name`/`description` frontmatter ([official Build skills documentation](https://learn.chatgpt.com/docs/build-skills.md)).
Autophagy does not write `$HOME/.agents/skills`, Codex config, hooks, commands,
or network settings.

## Claude Code repo skill target

With `--target claude-code` the materializer creates:

```text
<repository>/.claude/skills/autophagy-<stable-id>/SKILL.md
```

This is Claude Code's repo-scoped skill location: a `SKILL.md` with `name` and
`description` YAML frontmatter followed by Markdown instructions, which Claude
Code loads automatically for sessions in that repository. Autophagy writes only
that one file inside the selected repository. It does not write `~/.claude`,
`settings.json`, hooks, slash commands, subagents, or any global configuration —
the same repo-scoped stance as the Codex materializer.

The Claude Code skill body carries the reviewed instruction, the exact versioned
trigger selectors, the exclusions, and an evidence footer citing the exact
supporting (and any counterexample) AEP event IDs alongside the mutation ID and
version. Every identifier is reproduced deterministically from the reviewed,
shadow-passed package; nothing is model-generated.

The event-ID evidence footer appears only in the Claude Code body: the Codex
body is kept byte-for-byte identical to earlier releases so its content hash —
and therefore drift detection for Codex skills installed before this change —
stays stable. This is only a difference in the on-disk body; the installation
audit in SQLite retains the mutation link (and thus its exact evidence
identifiers) for both targets.

Both agents select skills from their descriptions and task context. The
installed skill repeats the reviewed selectors as instructions, but it is not a
mechanically enforced pre-tool hook; shadow precision therefore remains evidence
for user judgment rather than a guarantee of identical activation.

## Installation audit

Installation requires registry state `shadow_passed` and the exact confirmation
phrase `repo-skill-write`. The materializer refuses existing files and symlink
escapes. After writing, SQLite records the canonical root, relative path,
content SHA-256, target (`codex_repo_skill` or `claude_code_repo_skill`), and
permission review while transitioning the mutation to `active`. A mutation can
have at most one active installation. If that audit fails, the new file is
removed.

## Uninstall

```sh
autophagy mutations uninstall mut_example
```

Uninstall loads the audited target, reconstructs the materializer from the
stored target identifier, verifies that the file's bytes still match the
installation hash, removes `SKILL.md`, and records `active -> retired`. If the
file changed, rollback refuses to delete user edits. If the database update
fails after removal, Autophagy recreates the exact deterministic skill.

Evidence deletion, pruning, and delete-all refuse to proceed while an affected
installation is active. Run uninstall first; this prevents a database cascade
from leaving untracked behavior on disk.
