# Claude Code adapter

The adapter discovers Claude Code session transcripts, normalizes supported
records into AEP v0.1, and resumes at the last complete JSONL boundary. Claude
documents transcripts under `~/.claude/projects/<project>/<session-id>.jsonl`;
`CLAUDE_CONFIG_DIR` changes the configuration root. See Claude's official
[session documentation](https://code.claude.com/docs/en/sessions) and
[environment-variable reference](https://code.claude.com/docs/en/env-vars).

## Preview and import

Preview the exact metadata-only discovery plan. This opens no database and the
JSON result lists every selected path and observed byte size:

```sh
mise exec -- cargo run -p autophagy-cli -- --output json \
  import --adapter claude-code --dry-run
```

Import the default history root:

```sh
mise exec -- cargo run -p autophagy-cli -- \
  import --adapter claude-code
```

Pass a directory or one transcript explicitly after `import`. Primary sessions
are selected by default; add `--include-subagents` for nested
`agent-*.jsonl` transcripts. Repeat `--project /exact/working/directory` to
limit stored events. Project-filtered imports use independent cursor scopes.

## Content and search policy

By default, prompt text, assistant text, and tool-result output are not copied
into normalized event metadata. `--include-content` opts into local persistence
under `claude.content`. Tool inputs remain part of the structural AEP tool call,
but they are excluded from FTS unless `--index-tool-input` is supplied.

Use `--index-metadata claude.content` only with `--include-content` and only when
that local content is approved for search. Changing `--include-content` after a
history has already been imported changes canonical event content; use a fresh
database when intentionally changing that evidence policy.

## Incremental behavior

Only newline-terminated records advance a cursor. An actively written partial
tail is deferred until the next run. Each cursor stores a byte offset, physical
line number, bounded prefix hash, event sequence, and unmatched tool calls.
Truncated or replaced files reset automatically; stable event IDs make the
rescan idempotent.

## Capability matrix

| Claude Code shape | AEP v0.1 event | Notes |
|---|---|---|
| First supported record | `session.started` | Deterministic synthetic boundary |
| User text message | `prompt.submitted` | Text omitted unless opted in |
| Assistant text block(s) | `decision.recorded` | Text omitted unless opted in |
| Assistant `tool_use` | `tool.called` | Input and file artifact preserved |
| User `tool_result` | `tool.completed` / `tool.failed` | Linked when its call is present; orphaned results are skipped rather than guessed |
| Summary or compaction system record | `context.compacted` | Summary omitted unless opted in |
| Subagent transcript | Same mappings | Opt-in discovery; distinct session ID |
| Queue, title, attachment, snapshot, and other metadata | Unsupported/skipped | Counted, never guessed |
| User correction or rejection | Not inferred | Requires explicit future source evidence |

Every emitted event carries `claude.source_file` and `claude.line`; native record
UUID and content-block index are retained when present. Those fields allow an
operator to trace normalized evidence back to the exact local source record.
