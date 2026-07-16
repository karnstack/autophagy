# Agent Event Protocol v0.1

AEP is a transport-neutral contract for normalized local coding-agent activity.
An AEP stream is UTF-8 JSON Lines: one complete event per line, ordered by
`sequence` when present and then by file order.

The normative schema is [`schema.json`](schema.json). The Rust implementation in
`autophagy-events` adds the same semantic checks and returns field-addressed
diagnostics.

## Compatibility

- Consumers must reject unsupported `spec_version` values.
- Producers must not add undeclared envelope fields. Source-specific values go
  in `metadata`.
- IDs are opaque and stable. Consumers must not infer ULID or UUID semantics.
- A producer should emit `sequence` whenever its native source has a stable
  ordering. Sequence values are scoped to a session.
- Timestamps use RFC 3339. Producers should normalize to UTC when possible.

## Event semantics

- `tool.called` describes an invocation and cannot have an exit code.
- `tool.completed` may omit an exit code; when present it is zero.
- `tool.failed` has a non-zero exit code.
- Tool lifecycle events require a `tool` object.
- An artifact needs at least one stable locator: `path`, `uri`, or `digest`.
- `parent_event_id` cannot equal the event's own ID.

## Privacy

AEP defines structure, not permission to collect it. Adapters must apply project
selection and redaction policy before an event crosses the ingestion boundary.
Secret-bearing source fields should be omitted or represented by a digest, not
copied into `metadata`.
