# Local database schema

Status: implemented through Phase 2 candidate registry (2026-07-16)

SQLite is the single-user source of truth. Foreign keys and WAL mode are enabled
per connection. Timestamps are canonical RFC 3339 UTC strings so exports remain
readable; sortable integer sequence fields break same-timestamp ties.

The authoritative DDL lives in the ordered, immutable files under
[`crates/autophagy-store/migrations`](../../crates/autophagy-store/migrations).
The logical schema and its trust boundaries are summarized here.

```sql
CREATE TABLE schema_migrations (
  version       INTEGER PRIMARY KEY,
  description   TEXT NOT NULL,
  checksum      BLOB NOT NULL CHECK (length(checksum) = 32),
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
  session_id      TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
  occurred_at     TEXT NOT NULL,
  sequence        INTEGER CHECK (sequence IS NULL OR sequence >= 0),
  event_type      TEXT NOT NULL,
  project_path    TEXT,
  parent_event_id TEXT,
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
  event_row_id     INTEGER PRIMARY KEY REFERENCES events(row_id) ON DELETE CASCADE,
  project_path     TEXT,
  tool_name        TEXT,
  tool_input_text  TEXT,
  searchable_text  TEXT NOT NULL DEFAULT ''
) STRICT;

CREATE TABLE event_conflicts (
  conflict_id              INTEGER PRIMARY KEY,
  event_id                 TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  existing_content_hash    BLOB NOT NULL,
  conflicting_content_hash BLOB NOT NULL,
  conflicting_event_json   TEXT NOT NULL CHECK (json_valid(conflicting_event_json)),
  source_adapter           TEXT NOT NULL,
  source_instance_key      TEXT NOT NULL,
  first_seen_at            TEXT NOT NULL,
  last_seen_at             TEXT NOT NULL,
  observation_count        INTEGER NOT NULL DEFAULT 1,
  UNIQUE (event_id, conflicting_content_hash)
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

CREATE TABLE source_cursors (
  adapter       TEXT NOT NULL,
  instance_key  TEXT NOT NULL,
  origin        TEXT NOT NULL,
  byte_offset   INTEGER NOT NULL CHECK (byte_offset >= 0),
  line_number   INTEGER NOT NULL CHECK (line_number >= 0),
  head_hash     BLOB NOT NULL CHECK (length(head_hash) = 32),
  state_json    TEXT NOT NULL CHECK (json_valid(state_json)),
  updated_at    TEXT NOT NULL,
  PRIMARY KEY (adapter, instance_key, origin)
) STRICT;

CREATE TABLE mutation_candidates (
  mutation_id       TEXT PRIMARY KEY,
  source_finding_id TEXT NOT NULL UNIQUE,
  source_detector   TEXT NOT NULL,
  equivalence_key   TEXT NOT NULL UNIQUE,
  spec_version      TEXT NOT NULL,
  semantic_version  TEXT NOT NULL,
  state             TEXT NOT NULL,
  package_json      TEXT NOT NULL,
  content_hash      BLOB NOT NULL,
  challenge_json    TEXT,
  rejection_reason  TEXT,
  created_at        TEXT NOT NULL,
  updated_at        TEXT NOT NULL
) STRICT;

CREATE TABLE mutation_evidence (
  mutation_id TEXT NOT NULL REFERENCES mutation_candidates(mutation_id)
    ON DELETE CASCADE,
  event_id    TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  role        TEXT NOT NULL,
  ordinal     INTEGER NOT NULL,
  PRIMARY KEY (mutation_id, role, ordinal),
  UNIQUE (mutation_id, event_id)
) STRICT;

CREATE TABLE mutation_transitions (
  transition_id INTEGER PRIMARY KEY,
  mutation_id   TEXT NOT NULL REFERENCES mutation_candidates(mutation_id)
    ON DELETE CASCADE,
  from_state    TEXT,
  to_state      TEXT NOT NULL,
  reason        TEXT NOT NULL,
  metadata_json TEXT NOT NULL,
  occurred_at   TEXT NOT NULL
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

Insert, update, and delete triggers keep the external-content FTS5 table aligned
with `events_search`, including foreign-key cascades. `events_search` is populated
in the same transaction as `events`. Project paths and tool names come from the
already policy-processed AEP envelope; tool input and free text require an
explicit redaction-approved search projection. Raw event JSON and raw tool input
are never indexed blindly.

## Idempotency

1. Adapters preserve a native stable identifier when one exists; otherwise they
   derive an AEP event ID from source instance, native session, event position,
   and canonicalized content.
2. `events.event_id` is the primary deduplication boundary.
3. A repeated ID with the same `content_hash` is a no-op.
4. A repeated ID with a different hash is committed to `event_conflicts` with
   its source provenance and observation count. The canonical event is never
   silently overwritten.
5. Source-file fingerprints and cursors avoid rescanning unchanged inputs, but
correctness does not depend on that optimization.

Mutation registration applies the same immutable-content rule. Matching IDs and
content hashes are no-ops; matching IDs with different package content fail.
The unique source-finding and semantic equivalence keys prevent duplicate
proposals from entering the registry.

`source_cursors` stores the last complete byte and physical-line boundary plus
adapter-defined state. The Claude Code adapter includes pending tool calls in
that state so a result appended in a later run can still link to its call. A
bounded prefix hash detects replacement or truncation and resets safely.

## Deletion

Deleting a session cascades through events, conflict records, event-to-artifact
links, and FTS projections. If any cited support or counterexample is removed, a
trigger deletes its mutation candidate; the candidate then cascades through its
remaining evidence links and audit transitions. Unreferenced artifacts are
removed in the same transaction. Connections enable SQLite `secure_delete`;
`VACUUM` remains an explicit user operation because it has performance and
disk-space implications.
