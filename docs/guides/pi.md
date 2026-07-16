# Pi adapter

The adapter discovers local Pi coding-agent sessions, normalizes a conservative
subset into AEP v0.1, and resumes from the last complete JSONL boundary. Pi
stores each session as a newline-delimited JSON transcript under
`${PI_HOME:-~/.pi}/agent/sessions/<project>/<timestamp>_<uuid>.jsonl`, where the
project directory encodes the working directory the session ran in.

Pi's transcript layout is an application detail rather than a stable integration
interface, so this adapter treats every mapping as a version-observed
capability: unknown record and content-block types are counted and skipped,
fields are accessed defensively, and no semantic event is guessed. Anonymized
fixtures and a real-history dry-run guard the supported subset.

## Preview and import

Preview the exact metadata-only discovery plan without opening a database:

```sh
mise exec -- cargo run -p autophagy-cli -- --output json \
  import --adapter pi --dry-run
```

Import the default `${PI_HOME:-~/.pi}/agent/sessions` tree:

```sh
mise exec -- cargo run -p autophagy-cli -- \
  import --adapter pi
```

Pass one session file or another sessions root after `import` when needed.
Repeat `--project /exact/working/directory` for selection. Filtered imports use
independent cursor scopes and do not hide later unfiltered history.

## Privacy and indexing

Prompt text, assistant messages, tool arguments, and tool output are omitted
from event metadata by default. `--include-content` opts into local persistence
under `pi.content`. Tool inputs remain structural AEP evidence but enter FTS only
with `--index-tool-input`. Use `--index-metadata pi.content` only when the
content is locally approved for search. Change the content policy only with a
fresh database because it changes canonical event bodies.

Default credential rules redact recognized secrets before persistence. Add
repeatable `--exclude-path GLOB` values for project or artifact exclusions.
Project filters, exclusion globs, and the privacy-policy version are part of the
cursor scope, so changing policy safely rescans instead of losing skipped data.

## Incremental and cross-adapter guarantees

The cursor advances only past newline-terminated records and retains unmatched
tool calls across invocations. A bounded prefix hash resets safely after file
replacement or truncation. Stable event IDs keep rescans idempotent.

Every native adapter runs through the same conformance harness. It requires a
non-empty initial import, zero rejected or conflicting fixture evidence, a
zero-read unchanged reimport, and an unchanged stored event count. CLI tests
also import several adapters into one database and verify their source
provenance remains distinct.

## Capability matrix

| Pi record shape | AEP v0.1 event | Notes |
|---|---|---|
| `session` | `session.started` | Native session ID and version retained as provenance |
| `message` role `user` | `prompt.submitted` | Text omitted unless opted in |
| `message` role `assistant` text | `decision.recorded` | Emitted only when the message has non-empty text |
| `message` role `assistant` `toolCall` block | `tool.called` | One event per call block; file argument preserved |
| `message` role `toolResult` | `tool.completed` / `tool.failed` | Failure derived from `isError` |
| `message` role `bashExecution` | `tool.called` + `tool.completed` / `tool.failed` | Self-contained pair; failure from non-zero `exitCode` or cancellation |
| `assistant` `thinking` block | Dropped | Reasoning is not normalized into events |
| `model_change`, `thinking_level_change`, `custom_message` | Unsupported/skipped | Counted without semantic inference |
| Orphaned `toolResult` | Skipped | No tool identity or relationship is invented |
| User correction or rejection | Not inferred | Requires explicit future source evidence |

Every emitted event includes `pi.source_file` and `pi.line`; record-derived
events additionally carry `pi.record_type`, the message `pi.role`, and the tool
call ID when present.
