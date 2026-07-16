# Autophagy

**The self-improvement layer for local coding agents.**

Autophagy observes local coding-agent sessions, detects repeated failures and
successful recovery paths, and turns them into tested, versioned, reversible
behavioral improvements called mutations.

Memory asks, “What happened before?” Autophagy asks, “What should permanently
change because of what happened?”

## Status

The local-only Milestone 1 engine is implemented: AEP v0.1, transactional
SQLite storage, generic JSONL plus Claude Code and Codex adapters, deterministic
evidence-linked findings, ingestion redaction, retention, export, and deletion.
Phase 2 now generates and retains review-only, zero-permission mutation
candidates in an audited local registry. Users can inspect, challenge, or reject
them. No daemon, replay, installation, autonomous execution, or background
capture ships yet.

## Principles

- Local-first and offline by default
- Evidence over eloquence
- Models propose; deterministic evaluation proves
- Concrete, permission-scoped, reversible behavior only
- Honest silence when evidence is insufficient

## Repository map

```text
adapters/claude-code/      Native transcript discovery and AEP normalization
adapters/codex/            Schema-tolerant Codex rollout normalization
crates/autophagy-adapter-test-support/  Shared native-adapter conformance checks
crates/autophagy-cli/      User-facing import, sessions, and search commands
crates/autophagy-core/     Reusable streaming import application services
crates/autophagy-events/   AEP Rust types, parsing, and validation
crates/autophagy-mutations/ Versioned review-only mutation candidates
crates/autophagy-patterns/ Model-free recurrence detectors and evidence packets
crates/autophagy-redaction/ Secret rules and project/artifact path policy
crates/autophagy-store/    SQLite migrations, idempotency, FTS, and deletion
docs/architecture/        Planned component and storage boundaries
docs/blueprint/           Complete normalized product and implementation brief
docs/decisions/           Architecture decision records
docs/roadmap/              Small pull-request delivery sequence
docs/specs/aep/0.1/       Versioned AEP JSON Schema and examples
docs/specs/evidence/0.1/  Versioned deterministic finding contract
docs/specs/mutation/0.1/  Versioned mutation package contract
```

The intended repository structure is documented in
[`docs/architecture/repository-structure.md`](docs/architecture/repository-structure.md).
The complete product blueprint is available in
[`docs/blueprint/`](docs/blueprint/README.md).

## Try the CLI

Import the anonymized demo corpus into an explicit local database:

```sh
mise exec -- cargo run -p autophagy-cli -- \
  --database /tmp/autophagy-demo.db \
  import evals/fixtures/generic-jsonl/demo.jsonl \
  --instance-key demo \
  --index-metadata summary

mise exec -- cargo run -p autophagy-cli -- \
  --database /tmp/autophagy-demo.db sessions

mise exec -- cargo run -p autophagy-cli -- \
  --database /tmp/autophagy-demo.db search stale
```

See the [generic JSONL guide](docs/guides/generic-jsonl.md) for dry-run,
project selection, standard input, JSON output, privacy controls, and exit-code
semantics.

Preview the exact Claude Code transcripts selected without writing a database:

```sh
mise exec -- cargo run -p autophagy-cli -- --output json \
  import --adapter claude-code --dry-run
```

See the [Claude Code adapter guide](docs/guides/claude-code.md) for incremental
cursoring, subagents, content policy, and the normalization capability matrix.

Preview Codex rollout discovery without changing the database:

```sh
mise exec -- cargo run -p autophagy-cli -- --output json \
  import --adapter codex --dry-run
```

The [Codex adapter guide](docs/guides/codex.md) documents its intentionally
narrow compatibility matrix and the upstream transcript-stability boundary.

The [deterministic findings guide](docs/guides/deterministic-findings.md)
documents recurrence thresholds, signature normalization, counterexamples, and
the versioned Evidence Packet contract.

## Run the offline demo

```sh
mise run demo
```

The demo imports anonymized evidence, emits two deterministic patterns with
exact evidence IDs, produces a digest that confirms no model or network was
used, registers two zero-permission mutation candidates, lists them, and
previews retention deletion. Its temporary database is removed on exit.

Useful privacy and lifecycle commands:

```sh
autophagy import history.jsonl --exclude-path '**/private/**'
autophagy export > autophagy-export.jsonl
autophagy prune --older-than-days 30 --dry-run
autophagy prune --older-than-days 30
autophagy delete session ses_example
autophagy delete all --confirm delete-all
```

See the [privacy and lifecycle guide](docs/guides/privacy-and-lifecycle.md) and
[threat model](docs/security/threat-model.md) for guarantees and limitations.

Register and review candidate packages:

```sh
autophagy mutations propose
autophagy mutations list
autophagy --output json mutations show mut_example
```

See the [mutation candidate guide](docs/guides/mutation-candidates.md) for the
contract, challenge checklist, evidence retention, and unavailable activation
actions.

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

## Storage guarantees

- AEP validation runs before a transaction writes any rows.
- Reimporting an identical event is a no-op.
- Reusing an event ID with different content creates an auditable quarantine
  record and never overwrites canonical evidence.
- Raw JSON is not copied into FTS5; tool input and free text require an explicit
  redaction-approved search projection.
- Session deletion cascades through events, conflicts, and search rows, then
  removes only artifacts that no remaining event references. A mutation is also
  removed if any evidence it cites is deleted.

## Security and privacy

Autophagy processes private developer activity. Cloud processing and telemetry
will remain disabled by default. Please read [SECURITY.md](SECURITY.md) before
reporting a vulnerability.

## License

Apache License 2.0. See [LICENSE](LICENSE).
