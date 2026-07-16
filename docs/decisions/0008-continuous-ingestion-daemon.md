# ADR 0008: Continuous ingestion and daemon lifecycle

- Status: accepted
- Date: 2026-07-17

## Context

Through 0.1.0-alpha, evidence only enters the store when the user runs
`autophagy import` (or a native-adapter import) by hand. To keep the local
evidence base current, a user has to remember to re-run imports. The 0.1.0
release wants a supervised, hands-off path: watch the native agents' history and
ingest new activity as it appears.

Two constraints shape the design:

- **Ingest-only.** AGENTS.md forbids adding autonomous execution permissions by
  default. A watcher must only discover and import transcripts under the same
  redaction, privacy, and projection gates as one-shot import. It must never
  execute, install, or otherwise act on the evidence it ingests.
- **One out-of-database writer.** The architecture states that
  `autophagy-install` is the only crate that writes outside the SQLite database.
  A background daemon needs a supervisor unit file (a launchd plist or systemd
  unit) written under the user's home directory — an out-of-database write.

## Decision

Add a foreground `autophagy watch` loop and an `autophagy daemon` lifecycle,
placing each piece where the existing dependency direction and invariants already
point.

- **Watch loop lives in `autophagy-core`.** `autophagy-core` gains a model-free
  `run_watch` loop plus a `WatchSource` trait. The loop drives a set of
  `WatchSource`s against one open `EventStore` on an interval, emits a per-cycle
  report, dedupes repeated identical failures, and shuts down gracefully on a
  shared atomic flag without interrupting an in-flight import (and therefore
  without interrupting an in-flight store transaction). Because the loop depends
  only on the store through the trait, `autophagy-core` stays decoupled from the
  concrete native adapters (which live downstream of the store). No new crate is
  introduced: the previously sketched `autophagy-daemon` crate would have been a
  placeholder, and the project adds crates only for an executable vertical slice.
- **Adapters plug in through one seam.** The CLI implements `WatchSource` for the
  native adapters and enumerates them through a single `NativeAdapter` value enum
  (`claude-code`, `codex`). The default watch set, the loop, and the daemon unit's
  `--adapter` arguments all derive from this one type. A new native adapter plugs
  in by adding a variant plus one `build_source` arm — no adapter list is
  hard-coded anywhere else.
- **Supervisor unit generation lives in `autophagy-install`.** Rather than weaken
  the "only `autophagy-install` writes outside the database" invariant, we extend
  that crate's charter from "repo-scoped skills" to "explicit, reversible
  out-of-database filesystem artifacts", which now also covers supervisor units.
  This is the right home because the daemon unit needs exactly the discipline
  `autophagy-install` already embodies: deterministic rendering, a managed-by
  marker so it never clobbers a file Autophagy did not author, and reversible,
  refusal-guarded removal. The crate gains a `supervisor` module
  (`plan_supervisor`, `write_supervisor`, `remove_supervisor`) that renders a
  launchd plist or a systemd user unit and enforces the marker check. It still
  performs no process execution.
- **The CLI orchestrates and shims the init system.** `autophagy daemon
  install|uninstall|status` resolves paths (binary via `current_exe`, plist under
  `~/Library/LaunchAgents`, systemd unit under `${XDG_CONFIG_HOME:-~/.config}/
  systemd/user`, logs under the app-support data dir), calls the install crate to
  write or remove the unit, and drives `launchctl`/`systemctl` through a thin
  `Supervisor` trait. Running those tools is explicit, user-invoked opt-in, not
  autonomous execution — and the trait keeps tests free of a live init system.
  Non-macOS/Linux hosts get an honest "not supported yet; run `autophagy watch`
  under your own supervisor" message rather than a faked install.

No store migration is required: continuous ingestion reuses the existing
source-cursor machinery, so repeat cycles are cheap and idempotent.

## Privacy

Continuous ingestion introduces no new data path. `watch` runs the same native
adapters as `import`, under the same secret redaction, path-exclusion, and
search-projection gates; native prompt/response text is still omitted unless the
operator passes `--include-content`. Nothing leaves the machine, and the daemon
only ingests — it never executes, installs, or acts on evidence.

`daemon install` is explicit opt-in. It writes exactly one supervisor unit under
the user's home directory, prints precisely what it wrote and where, and records
a managed-by marker so it will never overwrite or delete a file Autophagy did not
author. `daemon uninstall` unloads the job and removes that one file, leaving
nothing behind. The launched command is a plain `autophagy watch`, so the daemon
carries no capability the foreground command lacks.
