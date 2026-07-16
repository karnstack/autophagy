# Shadow and reversible Codex installation

Shadow is the final measurement gate before a user may install an instruction
mutation. It observes where the immutable trigger would fire but never changes
agent context or behavior.

```sh
autophagy mutations shadow mut_example \
  --observations evals/fixtures/shadow/example.json
```

Each Shadow Suite v0.1 observation cites independent local AEP event IDs,
records selectors visible before the outcome, and annotates whether intervention
would have helped. Exact selector matching produces true positives, true
negatives, false positives, and false negatives. Passing requires five
observations, positive and negative coverage, at least one would-be trigger, and
a false-positive rate no greater than the immutable package policy. Reports
always contain `mutation_applied: false` and `model_used: false`.

## Permission preview

The candidate itself remains zero-permission. Installation is a separate user
operation requesting one scoped filesystem effect: create one `SKILL.md` under
the selected repository's `.agents/skills` directory.

```sh
autophagy mutations install mut_example \
  --repository /workspace/project \
  --confirm-permissions repo-skill-write \
  --dry-run
```

The target must be an existing Git repository root. Dry-run reports the
canonical repository, exact relative path, content hash,
target, and required permission without writing or activating anything.

## Codex repo skill target

After removing `--dry-run`, the materializer creates:

```text
<repository>/.agents/skills/autophagy-<stable-id>/SKILL.md
```

This follows Codex's documented repo-scoped skill location and required
`name`/`description` frontmatter ([official Build skills documentation](https://learn.chatgpt.com/docs/build-skills.md)).
Autophagy does not write `$HOME/.agents/skills`, Codex config, hooks, commands,
or network settings.

Codex selects skills from their descriptions and task context. The installed
skill repeats the reviewed selectors as instructions, but it is not a
mechanically enforced pre-tool hook; shadow precision therefore remains
evidence for user judgment rather than a guarantee of identical activation.

Installation requires registry state `shadow_passed` and the exact confirmation
phrase `repo-skill-write`. The materializer refuses existing files and symlink
escapes. After writing, SQLite records the canonical root, relative path,
content SHA-256, target, and permission review while transitioning the mutation
to `active`. If that audit fails, the new file is removed.

## Uninstall

```sh
autophagy mutations uninstall mut_example
```

Uninstall loads the audited target, verifies that its bytes still match the
installation hash, removes `SKILL.md`, and records `active -> retired`. If the
file changed, rollback refuses to delete user edits. If the database update
fails after removal, Autophagy recreates the exact deterministic skill.

Evidence deletion, pruning, and delete-all refuse to proceed while an affected
installation is active. Run uninstall first; this prevents a database cascade
from leaving untracked behavior on disk.
