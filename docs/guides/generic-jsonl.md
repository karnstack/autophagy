# Generic AEP JSONL import

The generic importer accepts UTF-8 JSON Lines containing one complete Agent
Event Protocol v0.1 event per nonblank line. It streams input, validates each
record independently, and retains bounded line-addressed diagnostics so one bad
record does not hide later valid evidence.

## Database location

Pass `--database PATH` or set `AUTOPHAGY_DB`. Without either, Autophagy uses the
platform-local application data directory.

## Preview safely

Dry-run parses, validates, and applies project selection without opening or
creating a database:

```sh
autophagy --output json import sessions.jsonl --dry-run
```

## Import a file

Give each persisted input or producer a stable instance key. When omitted, the
CLI stores an opaque hash of the canonical input path rather than the path
itself. Reimporting the same events is safe and reports duplicates without
changing canonical rows.

```sh
autophagy import sessions.jsonl \
  --instance-key laptop-history \
  --display-name "Laptop export"
```

Use repeated exact project selections to import only approved repositories:

```sh
autophagy import sessions.jsonl \
  --instance-key laptop-history \
  --project /work/service-a \
  --project /work/service-b
```

Every selected event passes through default secret redaction before persistence.
Use repeatable `--exclude-path GLOB` values to drop events whose project or
artifact path matches. Exclusion happens before SQLite and FTS writes.

Use `-` or omit the file to read standard input:

```sh
producer | autophagy import - --instance-key live-pipe
```

## Search privacy

Validated, path-policy-processed project paths and tool names are searchable.
Raw event JSON and raw tool input are not copied into FTS5 by default.

Only enable these flags after confirming the selected source fields are already
redacted:

```sh
autophagy import sessions.jsonl \
  --instance-key redacted-export \
  --index-tool-input \
  --index-metadata summary \
  --index-metadata correction
```

Search accepts an SQLite FTS5 query expression:

```sh
autophagy search '"generated client"'
autophagy search 'tool AND failed' --limit 10
```

## Sessions and machine-readable output

```sh
autophagy sessions --limit 25
autophagy --output json sessions
autophagy --output json search stale
```

JSON output is a tagged object with `command` and `result` fields. Import
results include line, event, insertion, duplicate, conflict, project-skip,
privacy-skip, redacted-field, and rejection counts plus bounded diagnostics.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Command completed without rejected or conflicting events. |
| `1` | Fatal I/O, configuration, migration, or database failure. |
| `2` | Import completed, but one or more records were rejected or quarantined. |

Exit code `2` may accompany successful inserts. Read the import summary before
deciding whether to retry, repair the source, or inspect conflict evidence.
