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

SQLite uses `secure_delete`, but filesystem snapshots, backups, exported files,
and previous database copies remain outside Autophagy's control. `VACUUM` is not
run automatically because it can be expensive and needs additional free space.
