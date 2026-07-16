# Milestone 1 database schema

Status: proposed for PR 2 (2026-07-16)

SQLite is the single-user source of truth. Foreign keys and WAL mode are enabled
per connection. Timestamps are canonical RFC 3339 UTC strings so exports remain
readable; sortable integer sequence fields break same-timestamp ties.

PR 2 will implement this schema as an immutable migration, not by executing this
document directly.

```sql
CREATE TABLE schema_migrations (
  version       INTEGER PRIMARY KEY,
  description   TEXT NOT NULL,
  applied_at    TEXT NOT NULL
) STRICT;

CREATE TABLE sources (
  source_id      INTEGER PRIMARY KEY,
  adapter         TEXT NOT NULL,
  instance_key    TEXT NOT NULL,
  display_name    TEXT,
  first_seen_at   TEXT NOT NULL,
  last_seen_at    TEXT NOT NULL,
  UNIQUE (adapter, instance_key)
) STRICT;

CREATE TABLE sessions (
  session_id      TEXT PRIMARY KEY CHECK (session_id LIKE 'ses_%'),
  source_id       INTEGER NOT NULL REFERENCES sources(source_id),
  project_path    TEXT,
  started_at      TEXT,
  ended_at        TEXT,
  first_event_at  TEXT NOT NULL,
  last_event_at   TEXT NOT NULL,
  event_count     INTEGER NOT NULL DEFAULT 0 CHECK (event_count >= 0),
  metadata_json   TEXT NOT NULL DEFAULT '{}',
  UNIQUE (source_id, session_id)
) STRICT;

CREATE TABLE events (
  row_id          INTEGER PRIMARY KEY,
  event_id        TEXT NOT NULL UNIQUE CHECK (event_id LIKE 'evt_%'),
  spec_version    TEXT NOT NULL,
  session_id      TEXT NOT NULL REFERENCES sessions(session_id),
  occurred_at     TEXT NOT NULL,
  sequence        INTEGER CHECK (sequence IS NULL OR sequence >= 0),
  event_type      TEXT NOT NULL,
  project_path    TEXT,
  parent_event_id TEXT,
  tool_name       TEXT,
  tool_input_text TEXT,
  exit_code       INTEGER,
  event_json      TEXT NOT NULL,
  content_hash    BLOB NOT NULL,
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
  metadata_json   TEXT NOT NULL DEFAULT '{}',
  UNIQUE (artifact_type, path, uri, digest)
) STRICT;

CREATE TABLE event_artifacts (
  event_row_id    INTEGER NOT NULL REFERENCES events(row_id) ON DELETE CASCADE,
  artifact_id     INTEGER NOT NULL REFERENCES artifacts(artifact_id),
  ordinal         INTEGER NOT NULL CHECK (ordinal >= 0),
  PRIMARY KEY (event_row_id, ordinal),
  UNIQUE (event_row_id, artifact_id)
) STRICT;

CREATE TABLE imports (
  import_id       INTEGER PRIMARY KEY,
  source_id       INTEGER NOT NULL REFERENCES sources(source_id),
  origin          TEXT NOT NULL,
  fingerprint     BLOB NOT NULL,
  cursor_json     TEXT,
  started_at      TEXT NOT NULL,
  completed_at    TEXT,
  status          TEXT NOT NULL CHECK (status IN ('running', 'complete', 'failed')),
  seen_count      INTEGER NOT NULL DEFAULT 0,
  inserted_count  INTEGER NOT NULL DEFAULT 0,
  rejected_count  INTEGER NOT NULL DEFAULT 0,
  error           TEXT,
  UNIQUE (source_id, origin, fingerprint)
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
```

`events_search` will be a one-to-one projection populated in the same transaction
as `events`; it contains only text approved by the redaction policy. Raw JSON is
never indexed blindly.

## Idempotency

1. Adapters preserve a native stable identifier when one exists; otherwise they
   derive an AEP event ID from source instance, native session, event position,
   and canonicalized content.
2. `events.event_id` is the primary deduplication boundary.
3. A repeated ID with the same `content_hash` is a no-op.
4. A repeated ID with a different hash is quarantined as a contract conflict; it
   is never silently overwritten.
5. Source-file fingerprints and cursors avoid rescanning unchanged inputs, but
   correctness does not depend on that optimization.

## Deletion

Deleting a session cascades through event-to-artifact links and FTS projections.
Unreferenced artifacts are removed in the same transaction. `VACUUM` is an
explicit user operation because it has performance and disk-space implications.
