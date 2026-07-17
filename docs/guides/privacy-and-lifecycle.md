# Privacy and evidence lifecycle

Milestone 1 enforces privacy at ingestion, before SQLite storage and FTS
projection. Export is not a second redaction boundary: it emits the canonical
events already retained in the database.

## Secret redaction

Default deterministic rules recursively inspect tool input, event/tool/artifact
metadata, and artifact paths and URIs. Recognized values are replaced with
`[REDACTED]`. Built-in rules cover common OpenAI-style keys, GitHub tokens, AWS
access-key IDs, bearer credentials, and assignments to names such as `api_key`,
`access_token`, `password`, and `secret`.

Regex rules cannot recognize every secret. Do not treat a zero-redaction count
as proof that content is safe. Prefer native adapters' default omission of
prompt/response text and avoid `--include-content` unless necessary.

## Path exclusions

Use repeatable glob expressions:

```sh
autophagy import history.jsonl \
  --exclude-path '**/.env' \
  --exclude-path '**/private/**'
```

An event is excluded when its exact project or any artifact path matches.
Import summaries report `privacy_skipped` and `redacted_fields`. Native-adapter
cursors include the sorted exclusion set and privacy-policy version, so later
policy changes use an independent cursor scope.

## Rebuilding the search index

`reindex` rebuilds the derived search artifacts — the free-text FTS projection
and the exact normalized-signature index — from the events already in the
database. It is the way to heal a database imported before signature indexing
existed, or imported without `--index-tool-input`: reimporting cannot fix those,
because an identical event is an idempotent no-op.

```sh
autophagy reindex                     # baseline: project paths and tool names only
autophagy reindex --index-tool-input  # also make redacted commands searchable
```

The consent semantics match `import`. Without `--index-tool-input`, only the
structural project path and tool name become searchable. Passing
`--index-tool-input` makes redacted tool input searchable and rebuilds the
exact-signature index; `--index-metadata <key>` promotes an already-redacted
metadata key to searchable text. The current secret-redaction policy is
re-applied to every event before projection, so a rule added since the original
import removes newly recognized secrets from the rebuilt index.

The rebuild is transactional and idempotent: running it twice yields identical
state. It deletes and rewrites only the derived projection tables — it never
alters events, sessions, source cursors, quarantined conflicts, or evidence, and
quarantined or previously deleted events are never resurrected into the index.
The command reports events scanned, search rows written, signatures written, and
fields redacted.

## Guided setup

`autophagy setup` is the guided first-run path. It detects each local agent,
imports the ones you choose, runs `digest`, and can install background
monitoring. It asks at most two privacy questions — whether to make commands
searchable (`--index-tool-input`) and whether to persist prompt/response/
tool-result text (`--include-content`) — and never deletes anything. When it
finds a database with events but no search index, it routes through `reindex`
rather than a useless reimport. With no terminal, drive the same flow with
`--adapter`, `--index-tool-input`, `--monitor`, and `--yes`. See the
[setup guide](setup.md).

## Evidence inspection

```sh
autophagy --output json patterns --project /workspace/example
autophagy --output json digest --project /workspace/example
```

`digest` is deterministic in Milestone 1 and declares `model_used: false` and
`network_used: false` in JSON output.

## Export

Export writes canonical AEP JSONL to standard output irrespective of the global
display format:

```sh
autophagy export --project /workspace/example > evidence.jsonl
```

The destination becomes another copy of sensitive data and must be protected or
deleted independently.

## Retention and deletion

```sh
autophagy prune --older-than-days 30 --dry-run
autophagy prune --older-than-days 30
autophagy delete session ses_example
autophagy delete all --confirm delete-all
```

Dry-run executes the same retention transaction and rolls it back. Pruning and
session deletion cascade through events, FTS, conflicts, and event-artifact
links, then remove orphaned artifacts. Any mutation candidate citing a deleted
support or counterexample event is removed with its lifecycle audit; deletion
summaries report that count. The same rule applies to events cited by replay
scenarios, preventing a passing evaluation from outliving its local evidence.
Delete-all also removes candidates, sources, import records, and incremental
cursors.

An active mutation has behavior materialized outside SQLite. Deletion and prune
therefore refuse any operation that would remove its evidence until
`autophagy mutations uninstall <mutation-id>` completes. This prevents privacy
deletion from silently orphaning an active repo skill; uninstall first, then
repeat the deletion.

SQLite uses `secure_delete`, but filesystem snapshots, backups, exported files,
and previous database copies remain outside Autophagy's control. `VACUUM` is not
run automatically because it can be expensive and needs additional free space.
