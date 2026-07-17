-- Broaden the installation registry to record additional repo-scoped skill
-- targets. The original 0005 table constrained `target` to Codex and the
-- relative path to `.agents/skills/...`. Claude Code repo-scoped skills live
-- under `.claude/skills/...`, so both CHECK constraints are relaxed here.
--
-- SQLite cannot alter a CHECK constraint in place, so the table is recreated
-- and its rows copied, mirroring the pattern used by migration 0005.

CREATE TABLE mutation_installations_v7 (
  installation_id        TEXT PRIMARY KEY CHECK (installation_id LIKE 'ins_%'),
  mutation_id            TEXT NOT NULL UNIQUE REFERENCES mutation_candidates(mutation_id) ON DELETE CASCADE,
  target                 TEXT NOT NULL CHECK (target IN ('codex_repo_skill', 'claude_code_repo_skill')),
  repository_root        TEXT NOT NULL,
  relative_path          TEXT NOT NULL CHECK (
                           relative_path LIKE '.agents/skills/%/SKILL.md'
                           OR relative_path LIKE '.claude/skills/%/SKILL.md'
                         ),
  content_hash           TEXT NOT NULL CHECK (length(content_hash) = 64),
  permission_review_json TEXT NOT NULL CHECK (json_valid(permission_review_json)),
  state                  TEXT NOT NULL CHECK (state IN ('installed', 'uninstalled')),
  installed_at           TEXT NOT NULL,
  uninstalled_at         TEXT
) STRICT;

INSERT INTO mutation_installations_v7 SELECT * FROM mutation_installations;

DROP TABLE mutation_installations;

ALTER TABLE mutation_installations_v7 RENAME TO mutation_installations;
