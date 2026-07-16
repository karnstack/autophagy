# ADR 0006: Claude Code installation target

- Status: accepted
- Date: 2026-07-17

## Context

`autophagy-install` could materialize exactly one reversible artifact: a
repo-scoped Codex skill under `.agents/skills`. Most users run Claude Code,
which loads repo-scoped skills from a different location. Shipping 0.1.0 with a
Codex-only installer leaves the common case unserved.

Claude Code's repo-scoped skill format is a `SKILL.md` under
`.claude/skills/<skill-name>/` with `name`/`description` YAML frontmatter
followed by Markdown instructions, loaded automatically for sessions in that
repository. It needs no hooks, no `settings.json` edits, and no global
(`~/.claude`) writes — the same repo-scoped, reversible stance the Codex
materializer already takes.

The installation registry (migration 0005) hard-coded the Codex target: a
`CHECK (target = 'codex_repo_skill')` constraint and a
`CHECK (relative_path LIKE '.agents/skills/%/SKILL.md')` constraint. Recording a
Claude Code installation therefore requires a stored-schema change, which under
the project's rules requires an ordered migration and a decision record.

## Decision

Add a `ClaudeCode` installation target alongside `Codex`, sharing one code path,
and relax the registry constraints with a new migration.

- **Shared target abstraction.** `autophagy-install` gains an `InstallTarget`
  enum (`Codex`, `ClaudeCode`). Planning, materialization, non-overwrite,
  symlink-escape refusal, and drift-checked rollback are all target-agnostic;
  the target only selects the repository-relative skill directory
  (`.agents/skills` vs `.claude/skills`), the persisted registry identifier
  (`codex_repo_skill` vs `claude_code_repo_skill`), and the agent named in the
  rendered guidance. The former `plan_codex_skill` remains as a thin wrapper and
  `CodexSkillPlan` remains as a type alias, so existing callers are unaffected.
- **Deterministic, evidence-linked body.** The Claude Code `SKILL.md` reproduces
  the reviewed instruction, the exact versioned trigger selectors, the
  exclusions, and an evidence footer citing the exact supporting and
  counterexample AEP event IDs plus the mutation ID and version. The Codex body
  is byte-for-byte unchanged. The evidence footer with event IDs is therefore
  deliberately asymmetric — it is emitted only in the Claude Code body, because
  changing the Codex body would alter the content hash of the skill Autophagy
  materializes and break drift detection for skills installed before this
  change; the constraint "every derived finding retains exact evidence
  identifiers" is already satisfied for both targets by the installation audit
  in `mutation_installations`, which links the mutation (and thus its evidence)
  to the on-disk file regardless of target.
- **Migration 0007.** `mutation_installations` is recreated (SQLite cannot alter
  a `CHECK` in place) with `target IN ('codex_repo_skill',
  'claude_code_repo_skill')` and a `relative_path` check that accepts either
  `.agents/skills/%/SKILL.md` or `.claude/skills/%/SKILL.md`. Existing rows copy
  forward unchanged. The store's registration validator accepts both targets and
  their matching path prefix.
- **Target-driven uninstall.** The install audit already records the target, so
  uninstall reconstructs the correct materializer from the stored identifier and
  keeps its byte-exact, drift-refusing rollback. A mutation still has at most one
  active installation.
- **CLI selector with a safe default.** `mutations install` gains
  `--target codex|claude-code`, defaulting to `codex` so existing invocations
  and their `--output json` shape are unchanged (the report now also carries the
  target). `mutations uninstall` needs no flag — it reads the target from the
  audit.

## Privacy

The change preserves the local-first, offline, reversible guarantees. A Claude
Code install writes exactly one file — `.claude/skills/autophagy-<id>/SKILL.md`
— inside the user-selected Git repository, only after the mutation has passed
challenge, replay, and shadow evaluation and the user supplies the exact
`repo-skill-write` confirmation phrase. Nothing is written to `~/.claude`, to
global configuration, to hooks, or off the machine. The file contains only
deterministic, already-redacted, reviewed content plus the exact evidence
identifiers the project requires derived findings to retain. Uninstall removes
the file and directory and refuses to touch content that no longer matches the
recorded hash.
