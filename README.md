# Autophagy

**Your coding AI keeps making the same mistakes. Autophagy notices, and teaches
it to stop.**

Coding assistants like Claude Code and Codex are great at writing code, but
they forget everything between sessions — so the same failed command, the
same wrong assumption, the same fix you had to explain last week happens again
this week. Autophagy quietly watches how your AI assistant works on your own
computer, spots the mistakes it keeps repeating and what actually fixed them,
and turns those into small, reviewed lessons the assistant can use next time.
Nothing leaves your machine unless you decide it should.

## What it does

- **Watches your AI coding sessions**, locally, as they happen — no cloud, no
  account required.
- **Finds the repeats**: the same command failing over and over, the same
  correction you keep giving, and the fix that actually worked afterward.
- **Suggests small "lessons"** — precise, narrowly-scoped improvements, never
  vague advice — each one linked back to the exact moments that justify it.
- **You review and approve everything.** Nothing is installed or changed on
  your behalf without you looking at it first.
- **Tests a lesson against your own history before it's ever used**, so you
  can see whether it would have helped in the past, not just hope it helps in
  the future.
- **Stays on your machine.** Autophagy is offline by default; it only reaches
  out to a model if you explicitly configure one.

## Works with

Claude Code, Codex, [Pi](https://github.com/badlogic/pi-mono), OpenCode, or any
tool that can export a plain JSONL transcript.

**Optional:** plug in a local model (via [Ollama](https://ollama.com)) or a
local OpenAI-compatible server to get richer, more nuanced suggestions.
Autophagy works fully without one — the built-in deterministic engine needs no
model at all. With one, every request's cost is measured in tokens and shown
to you; nothing is sent anywhere without your manifest saying so. See the
[synthesis guide](docs/guides/synthesis.md).

## Quick start

Install [mise](https://mise.jdx.dev/), which pins the exact toolchain, then
build the `autophagy` command-line tool:

```sh
mise install
mise exec -- cargo build --release -p autophagy-cli
```

Point it at a sample history and see what it finds:

```sh
mise exec -- cargo run -p autophagy-cli -- \
  --database /tmp/autophagy-demo.db \
  import evals/fixtures/generic-jsonl/demo.jsonl --instance-key demo

mise exec -- cargo run -p autophagy-cli -- --database /tmp/autophagy-demo.db patterns
```

The first command imports a small anonymized transcript; the second lists the
repeated problems it detected, each with exact evidence.

Import your own real history from Claude Code or Codex instead:

```sh
mise exec -- cargo run -p autophagy-cli -- import --adapter claude-code
mise exec -- cargo run -p autophagy-cli -- import --adapter codex
```

Let it keep watching in the background instead of re-running import by hand:

```sh
mise exec -- cargo run -p autophagy-cli -- watch
```

This checks for new activity on an interval and imports only what's new; see
the [watch and daemon guide](docs/guides/watch-and-daemon.md) for installing it
as a proper background service on macOS or Linux.

Prefer a window over a terminal? Build the native macOS app (read-only, no
Xcode required):

```sh
swift build -c release --package-path apps/macos
apps/macos/scripts/make-app-bundle.sh --configuration release
open apps/macos/build/Autophagy.app
```

It shows your sessions, the patterns found, and every suggested lesson with
its full history — see the [macOS app guide](docs/guides/macos-app.md).

Try the whole loop end to end, offline, in one command:

```sh
mise run demo
```

## Principles

- **Nothing changes without your OK.** Every suggestion sits and waits for
  review; nothing installs itself.
- **It shows its receipts.** Every suggestion links to the exact sessions and
  moments it came from — no "trust me."
- **Silence over guessing.** When the evidence is thin, Autophagy says so
  instead of inventing a confident-sounding answer.
- **Your data stays yours.** Local-first and offline by default; secrets are
  filtered out before anything is stored or searched.
- **Reversible, always.** Anything Autophagy installs, it can cleanly remove.

## Status

The core engine, two reversible installation targets (Codex and Claude Code
skills), exact and hybrid retrieval, an optional local-model synthesis
boundary, continuous watch/daemon ingestion, and a native read-only macOS app
are all implemented and covered by the quality gate below. Version `0.1.0` is
being prepared for release; see [CHANGELOG.md](CHANGELOG.md) for the full
history and [docs/roadmap/](docs/roadmap/) for how it was built, one small
pull request at a time.

## For developers

Autophagy is a Rust workspace (edition 2024) with one strict dependency
direction:

```text
adapters -> events -> store -> patterns -> mutations -> {replay, shadow, install}
                         \        /
                          core --+-- cli
```

| Path | What it is |
| --- | --- |
| `adapters/claude-code/`, `adapters/codex/`, `adapters/pi/`, `adapters/opencode/` | Native transcript discovery and AEP normalization per agent |
| `apps/macos/` | Native read-only SwiftUI database inspector |
| `crates/autophagy-events/` | AEP (Agent Event Protocol) Rust types, parsing, validation |
| `crates/autophagy-store/` | SQLite migrations, idempotency, FTS, quarantine, cascading deletion |
| `crates/autophagy-redaction/` | Secret rules and path policy, applied at ingestion |
| `crates/autophagy-core/` | Streaming import application services |
| `crates/autophagy-patterns/` | Deterministic, model-free recurrence detectors |
| `crates/autophagy-mutations/` | Review-only mutation candidate registry (immutable, audit-logged) |
| `crates/autophagy-replay/` | Non-executable deterministic replay evaluation |
| `crates/autophagy-shadow/` | Observation-only trigger precision measurement |
| `crates/autophagy-synthesis/` | Provider-neutral local-model synthesis boundary |
| `crates/autophagy-install/` | The only crate that writes outside the database: explicit, reversible skill/daemon materialization |
| `crates/autophagy-cli/` | User-facing commands; ties everything together |

Contracts (AEP, evidence, mutation, replay, shadow, retrieval, synthesis) are
versioned JSON Schema plus fixtures in [`docs/specs/`](docs/specs/).
Architecture decisions are recorded in [`docs/decisions/`](docs/decisions/);
the planned repository structure is in
[`docs/architecture/repository-structure.md`](docs/architecture/repository-structure.md);
the full product blueprint is in [`docs/blueprint/`](docs/blueprint/README.md).

Engineering constraints (non-negotiable, from `AGENTS.md`): local-only and
offline-capable by default; never persist secrets or raw cloud payloads
without explicit consent; every derived finding retains exact evidence
identifiers; deterministic and inspectable over model-generated prose;
protocols and schemas are versioned before they change; no autonomous
execution permissions by default.

Per-command usage lives in the [guides](docs/guides/):
[generic JSONL](docs/guides/generic-jsonl.md),
[Claude Code](docs/guides/claude-code.md), [Codex](docs/guides/codex.md),
[Pi](docs/guides/pi.md), [OpenCode](docs/guides/opencode.md),
[deterministic findings](docs/guides/deterministic-findings.md),
[retrieval](docs/guides/retrieval.md),
[mutation candidates](docs/guides/mutation-candidates.md),
[replay](docs/guides/replay.md),
[shadow and installation](docs/guides/shadow-and-installation.md),
[synthesis](docs/guides/synthesis.md),
[watch and daemon](docs/guides/watch-and-daemon.md),
[macOS app](docs/guides/macos-app.md), and
[privacy and lifecycle](docs/guides/privacy-and-lifecycle.md).

Run the full quality gate before proposing changes:

```sh
mise install
mise run check   # fmt + lint + test + docs + actionlint
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

Storage guarantees: AEP validation runs before a transaction writes any rows;
reimporting an identical event is a no-op; reusing an event ID with different
content quarantines rather than overwrites; raw JSON is never copied into
search — only a redaction-approved projection is; deleting a session cascades
through events, conflicts, and search rows, and removes a mutation if any
evidence it cites is deleted.

## Security and privacy

Autophagy processes private developer activity. Cloud processing and
telemetry remain disabled by default. Please read
[SECURITY.md](SECURITY.md) before reporting a vulnerability.

## License

Apache License 2.0. See [LICENSE](LICENSE).
