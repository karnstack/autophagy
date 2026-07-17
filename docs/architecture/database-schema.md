# Local database schema

Status: squashed to a single v1 baseline before first release (2026-07-17)

SQLite is the single-user source of truth. Foreign keys and WAL mode are enabled
per connection. Timestamps are canonical RFC 3339 UTC strings so exports remain
readable; sortable integer sequence fields break same-timestamp ties.

The authoritative DDL lives in the ordered, immutable files under
[`crates/autophagy-store/migrations`](../../crates/autophagy-store/migrations).
The release baseline is a single migration, `0001_initial_schema.sql`: the eight
development-time migrations were squashed into one v1 baseline before the first
release, while no external database existed (see ADR 0012). From the first
release onward the chain is ordered and immutable — new migrations are added,
never edited. The squash is schema-identical to the old chain's final state,
proven by `tests/schema_equivalence.rs`, and the single legacy database is
adopted to the v1 ledger in place on first open. Migration `0002` then adds the
derived `findings_cache` table (see ADR 0013). The logical schema and its trust
boundaries are summarized here.

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

CREATE TABLE mutation_replays (
  replay_id         TEXT PRIMARY KEY,
  mutation_id       TEXT NOT NULL REFERENCES mutation_candidates(mutation_id)
    ON DELETE CASCADE,
  scenario_set_hash TEXT NOT NULL,
  report_json       TEXT NOT NULL,
  content_hash      BLOB NOT NULL,
  passed            INTEGER NOT NULL,
  created_at        TEXT NOT NULL,
  UNIQUE (mutation_id, scenario_set_hash)
) STRICT;

CREATE TABLE mutation_replay_evidence (
  replay_id TEXT NOT NULL REFERENCES mutation_replays(replay_id) ON DELETE CASCADE,
  event_id  TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  ordinal   INTEGER NOT NULL,
  PRIMARY KEY (replay_id, ordinal),
  UNIQUE (replay_id, event_id)
) STRICT;

CREATE TABLE mutation_shadows (
  shadow_id            TEXT PRIMARY KEY,
  mutation_id          TEXT NOT NULL REFERENCES mutation_candidates(mutation_id)
    ON DELETE CASCADE,
  observation_set_hash TEXT NOT NULL,
  report_json          TEXT NOT NULL,
  content_hash         BLOB NOT NULL,
  passed               INTEGER NOT NULL,
  created_at           TEXT NOT NULL,
  UNIQUE (mutation_id, observation_set_hash)
) STRICT;

CREATE TABLE mutation_shadow_evidence (
  shadow_id TEXT NOT NULL REFERENCES mutation_shadows(shadow_id) ON DELETE CASCADE,
  event_id  TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  ordinal   INTEGER NOT NULL,
  PRIMARY KEY (shadow_id, ordinal),
  UNIQUE (shadow_id, event_id)
) STRICT;

CREATE TABLE mutation_installations (
  installation_id        TEXT PRIMARY KEY,
  mutation_id            TEXT NOT NULL UNIQUE REFERENCES mutation_candidates(mutation_id)
    ON DELETE CASCADE,
  target                 TEXT NOT NULL,
  repository_root        TEXT NOT NULL,
  relative_path          TEXT NOT NULL,
  content_hash           TEXT NOT NULL,
  permission_review_json TEXT NOT NULL,
  state                  TEXT NOT NULL,
  installed_at           TEXT NOT NULL,
  uninstalled_at         TEXT
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

CREATE TABLE event_signatures (
  event_row_id  INTEGER PRIMARY KEY REFERENCES events(row_id) ON DELETE CASCADE,
  signature     TEXT NOT NULL CHECK (length(signature) > 0)
) STRICT;

CREATE INDEX event_signatures_lookup
  ON event_signatures(signature, event_row_id);

-- Migration 0002: derived detection-findings cache (see ADR 0013).
CREATE TABLE findings_cache (
  cache_key    BLOB PRIMARY KEY CHECK (length(cache_key) = 32),
  generation   BLOB NOT NULL CHECK (length(generation) = 32),
  report_json  TEXT NOT NULL CHECK (json_valid(report_json)),
  created_at   TEXT NOT NULL
) STRICT;
```

Insert, update, and delete triggers keep the external-content FTS5 table aligned
with `events_search`, including foreign-key cascades. `events_search` is populated
in the same transaction as `events`. Project paths and tool names come from the
already policy-processed AEP envelope; tool input and free text require an
explicit redaction-approved search projection. Raw event JSON and raw tool input
are never indexed blindly.

`event_signatures` is the exact normalized-signature index behind hybrid
retrieval. A row holds one event's normalized operation signature (for example
`operation/v2|shell|cargo build`), supplied through the same redaction-approved
search projection that gates free-text tool input, and written in the same
transaction as the event. Because a signature embeds command text, it is indexed
only when the source's tool input is approved for indexing. Rows cascade on event
deletion, so quarantine, prune, and delete-all keep the index consistent without
a separate deletion path.

`findings_cache` (migration 0002) memoizes the deterministic detection report so
the findings-consuming commands do not re-run a full detection pass over every
event on each invocation. It is derived, deterministic, local-only data holding
no new source text — only the serialized report, whose findings carry the exact
evidence identifiers the detectors already produce. `cache_key` is a SHA-256 over
every input the pass depends on (detector spec version, effective thresholds,
project filter, and a cheap content fingerprint of the events in scope: event
count, max `row_id`, and the monotonic max `imported_at` watermark), so any
import, delete, or prune changes the key and misses without explicit
invalidation. `generation` tags each row with the global corpus state so a write
collects entries from superseded states. The cache is fully reconstructable at
any time by deleting every row; see ADR 0013.

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

Replay reports use content-derived IDs and suite hashes under the same rule. A
failed report remains attached to a challenged candidate. Only a passing report
updates registry state to `replay_passed` and appends the corresponding
lifecycle transition in the same transaction.

Shadow reports follow the same immutable, evidence-linked design. Passing
advances only `replay_passed -> shadow_passed`. Installation records retain the
canonical target, exact relative path, installed content hash, permission
review, and uninstall timestamp; install and uninstall lifecycle transitions
commit with their audit updates. `target` is one of `codex_repo_skill`
(`.agents/skills/<name>/SKILL.md`) or `claude_code_repo_skill`
(`.claude/skills/<name>/SKILL.md`), and `relative_path` is constrained to match
the recorded target. Uninstall derives the materializer from the stored target,
so rollback always reconstructs the exact deterministic bytes it installed.

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

Replay scenario event IDs are also foreign-key links. Removing any event cited
by a replay removes the candidate, its reports, and its lifecycle audit, so a
`replay_passed` state can never survive its local evaluation evidence.

Shadow observation IDs use the same foreign-key rule. If a mutation is active,
evidence deletion, pruning, and delete-all return an error until its audited
filesystem installation is uninstalled. This prevents database deletion from
leaving an untracked skill active on disk.
