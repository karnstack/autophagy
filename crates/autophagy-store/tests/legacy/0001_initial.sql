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
