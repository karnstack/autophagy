# Codex adapter

The adapter discovers local Codex rollout transcripts, normalizes a conservative
subset into AEP v0.1, and resumes from the last complete JSONL boundary. Codex
stores state under `CODEX_HOME`, which defaults to `~/.codex`; rollout sessions
are under its `sessions` directory. See the official Codex
[environment-variable reference](https://developers.openai.com/codex/config-reference#environment-variables)
and [advanced configuration guide](https://developers.openai.com/codex/config-advanced#config-and-state-locations).

Codex explicitly describes transcript files as a convenience rather than a
stable integration interface. This adapter therefore treats every mapping as a
version-observed capability: unknown records are counted and skipped, fields
are accessed defensively, and no semantic event is guessed. Anonymized fixtures
and a real-history dry-run guard the supported subset.

## Preview and import

Preview the exact metadata-only discovery plan without opening a database:

```sh
mise exec -- cargo run -p autophagy-cli -- --output json \
  import --adapter codex --dry-run
```

Import the default `${CODEX_HOME:-~/.codex}/sessions` tree:

```sh
mise exec -- cargo run -p autophagy-cli -- \
  import --adapter codex
```

Pass one rollout file or another sessions root after `import` when needed.
Repeat `--project /exact/working/directory` for selection. Filtered imports use
independent cursor scopes and do not hide later unfiltered history.

## Privacy and indexing

Prompt text, agent messages, and tool output are omitted from event metadata by
default. `--include-content` opts into local persistence under `codex.content`.
Tool inputs remain structural AEP evidence but enter FTS only with
`--index-tool-input`. Use `--index-metadata codex.content` only when the content
is locally approved for search. Change the content policy only with a fresh
database because it changes canonical event bodies.

## Incremental and cross-adapter guarantees

The cursor advances only past newline-terminated records and retains unmatched
tool calls across invocations. A bounded prefix hash resets safely after file
replacement or truncation. Stable event IDs keep rescans idempotent.

Claude Code and Codex run through the same conformance harness. It requires a
non-empty initial import, zero rejected or conflicting fixture evidence, a
zero-read unchanged reimport, and an unchanged stored event count. CLI tests
also import both adapters into one database and verify their source provenance
remains distinct.

## Capability matrix

| Codex rollout shape | AEP v0.1 event | Notes |
|---|---|---|
| `session_meta` | `session.started` | Native session ID retained as provenance |
| `event_msg.user_message` | `prompt.submitted` | Text omitted unless opted in |
| `event_msg.agent_message` | `decision.recorded` | Avoids duplicate response-item messages |
| `response_item.function_call` | `tool.called` | Embedded JSON arguments decoded when possible |
| `response_item.custom_tool_call` | `tool.called` | Input and file artifact preserved |
| Matching function/custom output | `tool.completed` / `tool.failed` | Failure emitted only from explicit result fields |
| `compacted` | `context.compacted` | Summary omitted unless opted in |
| Response messages, reasoning, token counts, world state, turn context | Unsupported/skipped | Counted without semantic inference |
| Orphaned tool output | Skipped | No tool identity or relationship is invented |
| User correction or rejection | Not inferred | Requires explicit future source evidence |

Every emitted event includes `codex.source_file`, `codex.line`,
`codex.record_type`, and the payload type or call ID when present.
