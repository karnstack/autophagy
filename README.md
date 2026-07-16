# Autophagy

**The self-improvement layer for local coding agents.**

Autophagy observes local coding-agent sessions, detects repeated failures and
successful recovery paths, and turns them into tested, versioned, reversible
behavioral improvements called mutations.

Memory asks, “What happened before?” Autophagy asks, “What should permanently
change because of what happened?”

## Status

Autophagy is in foundation development. The first pull request defines Agent
Event Protocol (AEP) v0.1 and the Rust types that all collectors, storage, and
detectors will share. No daemon, session importer, or background capture ships
yet.

## Principles

- Local-first and offline by default
- Evidence over eloquence
- Models propose; deterministic evaluation proves
- Concrete, permission-scoped, reversible behavior only
- Honest silence when evidence is insufficient

## Repository map

```text
crates/autophagy-events/   AEP Rust types, parsing, and validation
docs/architecture/        Planned component and storage boundaries
docs/decisions/           Architecture decision records
docs/roadmap/              Small pull-request delivery sequence
docs/specs/aep/0.1/       Versioned AEP JSON Schema and examples
```

The intended repository structure is documented in
[`docs/architecture/repository-structure.md`](docs/architecture/repository-structure.md).

## Try the contract

Install [mise](https://mise.jdx.dev/), then run:

```sh
mise install
mise run check
```

An AEP event looks like this:

```json
{
  "spec_version": "aep/0.1",
  "event_id": "evt_01J2Z3Y4X5W6V7T8S9R0Q1P2N3",
  "session_id": "ses_01J2Z3Y4X5W6V7T8S9R0Q1P2N3",
  "timestamp": "2026-07-16T01:22:31Z",
  "source": "codex",
  "type": "tool.failed",
  "project": "/Users/example/project",
  "tool": {
    "name": "bash",
    "input": "pytest tests/translation",
    "exit_code": 1
  },
  "artifacts": [
    { "type": "file", "path": "src/translation/memory.py" }
  ]
}
```

## Security and privacy

Autophagy processes private developer activity. Cloud processing and telemetry
will remain disabled by default. Please read [SECURITY.md](SECURITY.md) before
reporting a vulnerability.

## License

Apache License 2.0. See [LICENSE](LICENSE).
