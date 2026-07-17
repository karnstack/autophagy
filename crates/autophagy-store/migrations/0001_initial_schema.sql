-- Autophagy initial schema (v1 baseline).
--
-- This single migration is the released starting point. It is the squashed
-- equivalent of the eight development-time migrations (0001..0008) that existed
-- before the first release: the applied result is schema-identical to that
-- chain's final state, proven byte-for-byte by the schema-equivalence test in
-- `tests/schema_equivalence.rs`. Development-time migrations were squashed
-- before any external database existed; see ADR 0012. From the first release
-- onward, migrations are ordered and immutable — add new ones, never edit this.
--
-- SQLite foreign keys and WAL are enabled per connection (see `store::configure`).
-- Timestamps are canonical RFC 3339 UTC strings; integer sequence fields break
-- same-timestamp ties. Every constrained table is STRICT.

-- Event ingestion -----------------------------------------------------------

CREATE TABLE sources (
  source_id       INTEGER PRIMARY KEY,
  adapter         TEXT NOT NULL CHECK (length(trim(adapter)) BETWEEN 1 AND 128),
  instance_key    TEXT NOT NULL CHECK (length(trim(instance_key)) > 0),
  display_name    TEXT,
  first_seen_at   TEXT NOT NULL,
  last_seen_at    TEXT NOT NULL,
  UNIQUE (adapter, instance_key)
) STRICT;

CREATE TABLE sessions (
  session_id      TEXT PRIMARY KEY CHECK (session_id GLOB 'ses_?*'),
  source_id       INTEGER NOT NULL REFERENCES sources(source_id),
  project_path    TEXT,
  started_at      TEXT,
  ended_at        TEXT,
  first_event_at  TEXT NOT NULL,
  last_event_at   TEXT NOT NULL,
  event_count     INTEGER NOT NULL DEFAULT 0 CHECK (event_count >= 0),
  metadata_json   TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json))
) STRICT;

CREATE TABLE events (
  row_id          INTEGER PRIMARY KEY,
  event_id        TEXT NOT NULL UNIQUE CHECK (event_id GLOB 'evt_?*'),
  spec_version    TEXT NOT NULL,
  session_id      TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
  occurred_at     TEXT NOT NULL,
  sequence        INTEGER CHECK (sequence IS NULL OR sequence >= 0),
  event_type      TEXT NOT NULL,
  project_path    TEXT,
  parent_event_id TEXT CHECK (parent_event_id IS NULL OR parent_event_id GLOB 'evt_?*'),
  tool_name       TEXT,
  tool_input_text TEXT,
  exit_code       INTEGER,
  event_json      TEXT NOT NULL CHECK (json_valid(event_json)),
  content_hash    BLOB NOT NULL CHECK (length(content_hash) = 32),
  imported_at     TEXT NOT NULL,
  UNIQUE (session_id, sequence)
) STRICT;

CREATE INDEX events_session_time
  ON events(session_id, occurred_at, sequence);
CREATE INDEX events_type_time
  ON events(event_type, occurred_at);
CREATE INDEX events_tool_failure
  ON events(tool_name, exit_code, occurred_at)
  WHERE event_type = 'tool.failed';

CREATE TABLE artifacts (
  artifact_id     INTEGER PRIMARY KEY,
  artifact_type   TEXT NOT NULL,
  path            TEXT,
  uri             TEXT,
  digest          TEXT,
  metadata_json   TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
  CHECK (path IS NOT NULL OR uri IS NOT NULL OR digest IS NOT NULL)
) STRICT;

CREATE UNIQUE INDEX artifacts_identity
  ON artifacts(
    artifact_type,
    coalesce(path, ''),
    coalesce(uri, ''),
    coalesce(digest, '')
  );

CREATE TABLE event_artifacts (
  event_row_id    INTEGER NOT NULL REFERENCES events(row_id) ON DELETE CASCADE,
  artifact_id     INTEGER NOT NULL REFERENCES artifacts(artifact_id),
  ordinal         INTEGER NOT NULL CHECK (ordinal >= 0),
  PRIMARY KEY (event_row_id, ordinal),
  UNIQUE (event_row_id, artifact_id)
) STRICT;

-- Redaction-approved search projection and its external-content FTS5 index. Raw
-- event JSON and raw tool input are never indexed blindly; searchable text is a
-- redaction-approved projection populated in the same transaction as the event.

CREATE TABLE events_search (
  event_row_id    INTEGER PRIMARY KEY REFERENCES events(row_id) ON DELETE CASCADE,
  project_path    TEXT,
  tool_name       TEXT,
  tool_input_text TEXT,
  searchable_text TEXT NOT NULL DEFAULT ''
) STRICT;

CREATE VIRTUAL TABLE events_fts USING fts5(
  project_path,
  tool_name,
  tool_input_text,
  searchable_text,
  content='events_search',
  content_rowid='event_row_id',
  tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER events_search_after_insert AFTER INSERT ON events_search BEGIN
  INSERT INTO events_fts(
    rowid,
    project_path,
    tool_name,
    tool_input_text,
    searchable_text
  ) VALUES (
    new.event_row_id,
    new.project_path,
    new.tool_name,
    new.tool_input_text,
    new.searchable_text
  );
END;

CREATE TRIGGER events_search_after_delete AFTER DELETE ON events_search BEGIN
  INSERT INTO events_fts(
    events_fts,
    rowid,
    project_path,
    tool_name,
    tool_input_text,
    searchable_text
  ) VALUES (
    'delete',
    old.event_row_id,
    old.project_path,
    old.tool_name,
    old.tool_input_text,
    old.searchable_text
  );
END;

CREATE TRIGGER events_search_after_update AFTER UPDATE ON events_search BEGIN
  INSERT INTO events_fts(
    events_fts,
    rowid,
    project_path,
    tool_name,
    tool_input_text,
    searchable_text
  ) VALUES (
    'delete',
    old.event_row_id,
    old.project_path,
    old.tool_name,
    old.tool_input_text,
    old.searchable_text
  );
  INSERT INTO events_fts(
    rowid,
    project_path,
    tool_name,
    tool_input_text,
    searchable_text
  ) VALUES (
    new.event_row_id,
    new.project_path,
    new.tool_name,
    new.tool_input_text,
    new.searchable_text
  );
END;

CREATE TABLE event_conflicts (
  conflict_id              INTEGER PRIMARY KEY,
  event_id                 TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  existing_content_hash    BLOB NOT NULL CHECK (length(existing_content_hash) = 32),
  conflicting_content_hash BLOB NOT NULL CHECK (length(conflicting_content_hash) = 32),
  conflicting_event_json   TEXT NOT NULL CHECK (json_valid(conflicting_event_json)),
  source_adapter           TEXT NOT NULL,
  source_instance_key      TEXT NOT NULL,
  first_seen_at            TEXT NOT NULL,
  last_seen_at             TEXT NOT NULL,
  observation_count        INTEGER NOT NULL DEFAULT 1 CHECK (observation_count > 0),
  UNIQUE (event_id, conflicting_content_hash)
) STRICT;

CREATE TABLE imports (
  import_id       INTEGER PRIMARY KEY,
  source_id       INTEGER NOT NULL REFERENCES sources(source_id),
  origin          TEXT NOT NULL,
  fingerprint     BLOB NOT NULL,
  cursor_json     TEXT CHECK (cursor_json IS NULL OR json_valid(cursor_json)),
  started_at      TEXT NOT NULL,
  completed_at    TEXT,
  status          TEXT NOT NULL CHECK (status IN ('running', 'complete', 'failed')),
  seen_count      INTEGER NOT NULL DEFAULT 0 CHECK (seen_count >= 0),
  inserted_count  INTEGER NOT NULL DEFAULT 0 CHECK (inserted_count >= 0),
  rejected_count  INTEGER NOT NULL DEFAULT 0 CHECK (rejected_count >= 0),
  error           TEXT,
  UNIQUE (source_id, origin, fingerprint)
) STRICT;

-- Incremental source cursors. The last complete byte and physical-line boundary
-- plus adapter-defined state; a bounded prefix hash detects truncation.

CREATE TABLE source_cursors (
  adapter         TEXT NOT NULL CHECK (length(trim(adapter)) BETWEEN 1 AND 128),
  instance_key    TEXT NOT NULL CHECK (length(trim(instance_key)) > 0),
  origin          TEXT NOT NULL CHECK (length(trim(origin)) > 0),
  byte_offset     INTEGER NOT NULL CHECK (byte_offset >= 0),
  line_number     INTEGER NOT NULL CHECK (line_number >= 0),
  head_hash       BLOB NOT NULL CHECK (length(head_hash) = 32),
  state_json      TEXT NOT NULL CHECK (json_valid(state_json)),
  updated_at      TEXT NOT NULL,
  PRIMARY KEY (adapter, instance_key, origin)
) STRICT;

-- Mutation candidate registry -----------------------------------------------
--
-- Immutable, audit-logged, evidence-linked. `spec_version` accepts the reviewed
-- Mutation Package v0.1 and the model-provenance-carrying v0.2. The lifecycle is
-- candidate -> challenged -> replay_passed -> shadow_passed -> active, with
-- retired/rejected terminal states.

CREATE TABLE mutation_candidates (
  mutation_id        TEXT PRIMARY KEY CHECK (mutation_id LIKE 'mut_%'),
  source_finding_id  TEXT NOT NULL UNIQUE CHECK (source_finding_id LIKE 'fnd_%'),
  source_detector    TEXT NOT NULL,
  equivalence_key    TEXT NOT NULL UNIQUE CHECK (equivalence_key LIKE 'eqv_%'),
  spec_version       TEXT NOT NULL CHECK (spec_version IN ('mutation/0.1', 'mutation/0.2')),
  semantic_version   TEXT NOT NULL,
  state              TEXT NOT NULL CHECK (state IN ('candidate', 'challenged', 'replay_passed', 'shadow_passed', 'active', 'retired', 'rejected')),
  package_json       TEXT NOT NULL CHECK (json_valid(package_json)),
  content_hash       BLOB NOT NULL CHECK (length(content_hash) = 32),
  challenge_json     TEXT CHECK (challenge_json IS NULL OR json_valid(challenge_json)),
  rejection_reason   TEXT,
  created_at         TEXT NOT NULL,
  updated_at         TEXT NOT NULL
) STRICT;

CREATE TABLE mutation_evidence (
  mutation_id  TEXT NOT NULL REFERENCES mutation_candidates(mutation_id) ON DELETE CASCADE,
  event_id     TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  role         TEXT NOT NULL CHECK (role IN ('support', 'counterexample')),
  ordinal      INTEGER NOT NULL CHECK (ordinal >= 0),
  PRIMARY KEY (mutation_id, role, ordinal),
  UNIQUE (mutation_id, event_id)
) STRICT;

CREATE TABLE mutation_transitions (
  transition_id  INTEGER PRIMARY KEY,
  mutation_id    TEXT NOT NULL REFERENCES mutation_candidates(mutation_id) ON DELETE CASCADE,
  from_state     TEXT CHECK (from_state IS NULL OR from_state IN ('candidate', 'challenged', 'replay_passed', 'shadow_passed', 'active')),
  to_state       TEXT NOT NULL CHECK (to_state IN ('candidate', 'challenged', 'replay_passed', 'shadow_passed', 'active', 'retired', 'rejected')),
  reason         TEXT NOT NULL,
  metadata_json  TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
  occurred_at    TEXT NOT NULL
) STRICT;

-- Deterministic replay evaluation. A failing report stays attached to a
-- challenged candidate; only a passing report advances registry state.

CREATE TABLE mutation_replays (
  replay_id          TEXT PRIMARY KEY CHECK (replay_id LIKE 'rpl_%'),
  mutation_id        TEXT NOT NULL REFERENCES mutation_candidates(mutation_id) ON DELETE CASCADE,
  scenario_set_hash  TEXT NOT NULL CHECK (scenario_set_hash LIKE 'rsh_%'),
  report_json        TEXT NOT NULL CHECK (json_valid(report_json)),
  content_hash       BLOB NOT NULL CHECK (length(content_hash) = 32),
  passed             INTEGER NOT NULL CHECK (passed IN (0, 1)),
  created_at         TEXT NOT NULL,
  UNIQUE (mutation_id, scenario_set_hash)
) STRICT;

CREATE TABLE mutation_replay_evidence (
  replay_id  TEXT NOT NULL REFERENCES mutation_replays(replay_id) ON DELETE CASCADE,
  event_id   TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  ordinal    INTEGER NOT NULL CHECK (ordinal >= 0),
  PRIMARY KEY (replay_id, ordinal),
  UNIQUE (replay_id, event_id)
) STRICT;

-- Observation-only shadow evaluation. Passing advances replay_passed ->
-- shadow_passed only.

CREATE TABLE mutation_shadows (
  shadow_id             TEXT PRIMARY KEY CHECK (shadow_id LIKE 'shr_%'),
  mutation_id           TEXT NOT NULL REFERENCES mutation_candidates(mutation_id) ON DELETE CASCADE,
  observation_set_hash  TEXT NOT NULL CHECK (observation_set_hash LIKE 'shh_%'),
  report_json           TEXT NOT NULL CHECK (json_valid(report_json)),
  content_hash          BLOB NOT NULL CHECK (length(content_hash) = 32),
  passed                INTEGER NOT NULL CHECK (passed IN (0, 1)),
  created_at            TEXT NOT NULL,
  UNIQUE (mutation_id, observation_set_hash)
) STRICT;

CREATE TABLE mutation_shadow_evidence (
  shadow_id  TEXT NOT NULL REFERENCES mutation_shadows(shadow_id) ON DELETE CASCADE,
  event_id   TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  ordinal    INTEGER NOT NULL CHECK (ordinal >= 0),
  PRIMARY KEY (shadow_id, ordinal),
  UNIQUE (shadow_id, event_id)
) STRICT;

-- Reversible, repo-scoped skill installation audit. `target` is one of
-- codex_repo_skill (.agents/skills/<name>/SKILL.md) or claude_code_repo_skill
-- (.claude/skills/<name>/SKILL.md), and `relative_path` is constrained to match.

CREATE TABLE mutation_installations (
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

CREATE INDEX mutation_candidates_state_updated
  ON mutation_candidates(state, updated_at DESC);
CREATE INDEX mutation_transitions_candidate_time
  ON mutation_transitions(mutation_id, occurred_at, transition_id);
CREATE INDEX mutation_replays_candidate_time
  ON mutation_replays(mutation_id, created_at, replay_id);
CREATE INDEX mutation_shadows_candidate_time
  ON mutation_shadows(mutation_id, created_at, shadow_id);

-- Removing any cited support or counterexample deletes the candidate, which then
-- cascades through its remaining evidence, reports, and audit transitions, so a
-- passed state can never survive its local evidence.

CREATE TRIGGER mutation_evidence_removed
AFTER DELETE ON mutation_evidence
BEGIN
  DELETE FROM mutation_candidates WHERE mutation_id = OLD.mutation_id;
END;

CREATE TRIGGER mutation_replay_evidence_removed
AFTER DELETE ON mutation_replay_evidence
BEGIN
  DELETE FROM mutation_candidates
  WHERE mutation_id = (
    SELECT mutation_id FROM mutation_replays WHERE replay_id = OLD.replay_id
  );
END;

CREATE TRIGGER mutation_shadow_evidence_removed
AFTER DELETE ON mutation_shadow_evidence
BEGIN
  DELETE FROM mutation_candidates
  WHERE mutation_id = (
    SELECT mutation_id FROM mutation_shadows WHERE shadow_id = OLD.shadow_id
  );
END;

-- Exact normalized-signature index for deterministic hybrid retrieval. Each row
-- links one canonical event to its redaction-approved normalized operation
-- signature, supplied through the same explicit search projection that gates
-- free-text FTS content. Rows cascade on event deletion.

CREATE TABLE event_signatures (
  event_row_id  INTEGER PRIMARY KEY REFERENCES events(row_id) ON DELETE CASCADE,
  signature     TEXT NOT NULL CHECK (length(signature) > 0)
) STRICT;

CREATE INDEX event_signatures_lookup
  ON event_signatures(signature, event_row_id);
