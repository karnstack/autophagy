# OpenCode adapter

The adapter discovers local [OpenCode](https://github.com/sst/opencode) sessions
and normalizes a conservative subset into AEP v0.1. OpenCode persists each
conversation as many small JSON files under a `storage/` tree, defaulting to
`${XDG_DATA_HOME:-~/.local/share}/opencode/storage`:

```text
storage/session/<projectID>/<sessionID>.json   session info
storage/message/<sessionID>/<messageID>.json   one message
storage/part/<messageID>/<partID>.json         one message part
```

The adapter targets this file-based JSON storage layout. Message and part
identifiers ascend over time, so a session is the incremental unit: the cursor
records the highest message identifier already normalized and resumes past it.
Recent OpenCode builds are migrating this data into a single SQLite database;
that backing store is out of scope for v0.1 and is neither read nor written.

OpenCode's on-disk layout is an application detail rather than a stable
integration interface, so this adapter treats every mapping as a version-observed
capability: unknown message roles and part types are counted and skipped, fields
are accessed defensively, and no semantic event is guessed. Anonymized fixtures
guard the supported subset.

## Preview and import

Preview the exact metadata-only discovery plan without opening a database:

```sh
mise exec -- cargo run -p autophagy-cli -- --output json \
  import --adapter opencode --dry-run
```

Import the default storage tree:

```sh
mise exec -- cargo run -p autophagy-cli -- \
  import --adapter opencode
```

Pass another `storage/` root after `import` when needed. Repeat
`--project /exact/working/directory` for selection. Filtered imports use
independent cursor scopes and do not hide later unfiltered history.

## Privacy and indexing

Prompt text, assistant messages, tool input, and tool output are omitted from
event metadata by default. `--include-content` opts into local persistence under
`opencode.content`. Tool inputs remain structural AEP evidence but enter FTS only
with `--index-tool-input`. Use `--index-metadata opencode.tool` only when the
content is locally approved for search. Change the content policy only with a
fresh database because it changes canonical event bodies.

Default credential rules redact recognized secrets before persistence. Add
repeatable `--exclude-path GLOB` values for project or artifact exclusions.
Project filters, exclusion globs, and the privacy-policy version are part of the
cursor scope, so changing policy safely rescans instead of losing skipped data.

## Incremental and cross-adapter guarantees

The per-session cursor advances only past the highest message identifier read,
so an unchanged reimport reads no message records and inserts nothing. The
session-start event is emitted once and tracked in cursor state. Stable event IDs
keep rescans idempotent.

Every native adapter runs through the same conformance harness. It requires a
non-empty initial import, zero rejected or conflicting fixture evidence, a
zero-read unchanged reimport, and an unchanged stored event count. CLI tests
also import several adapters into one database and verify their source provenance
remains distinct.

## Capability matrix

| OpenCode record shape | AEP v0.1 event | Notes |
|---|---|---|
| Session info | `session.started` | Native session ID and working directory retained |
| Message role `user` | `prompt.submitted` | Text collected from text parts, omitted unless opted in |
| Message role `assistant` text parts | `decision.recorded` | Emitted only when text parts are non-empty |
| `tool` part `state.status` `completed` | `tool.called` + `tool.completed` | Self-contained pair; file input preserved |
| `tool` part `state.status` `error` | `tool.called` + `tool.failed` | Failure carried from the error state |
| `tool` part `state.status` `pending` / `running` | `tool.called` | No terminal outcome is invented |
| `reasoning`, `file`, `step-start`, `step-finish`, `snapshot`, `patch`, `agent` parts | Dropped | Not normalized into events |
| Unknown message role | Unsupported/skipped | Counted without semantic inference |
| SQLite-backed storage | Not read | v0.1 targets the file-based JSON layout only |
| User correction or rejection | Not inferred | Requires explicit future source evidence |

Every emitted event includes `opencode.project_id`; message-derived events
additionally carry `opencode.message_id`, `opencode.role`, and the tool name and
call ID for tool events.
