# ADR 0002: A versioned, strict AEP event envelope

- Status: accepted
- Date: 2026-07-16

## Context

Multiple coding agents represent sessions, tools, failures, and files
differently. Storage and detectors must not depend on an adapter's native JSON,
but an open protocol also needs a controlled way to retain source-specific data.

## Decision

AEP v0.1 is newline-delimited JSON whose event envelope is validated strictly.
Unknown envelope and nested object fields are rejected. Adapter-specific data is
allowed only inside explicit `metadata` objects. Event and session identifiers
are opaque, stable strings with `evt_` and `ses_` prefixes.

The JSON Schema in `docs/specs/aep/0.1/schema.json` is normative for serialized
data. Rust types and fixtures must remain behaviorally equivalent.

## Consequences

- Bad inputs fail at the adapter boundary instead of poisoning later analysis.
- Existing v0.1 consumers can safely interpret every accepted envelope.
- Adding a top-level field requires a schema change; experimental adapter data
  belongs in `metadata`.
- A future breaking change uses a new `spec_version` and schema directory rather
  than silently reinterpreting stored events.
