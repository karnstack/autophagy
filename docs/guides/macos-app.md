# macOS app

The macOS app is a native, strictly read-only window into a local Autophagy
database. It makes the complete local loop — sessions, deterministic findings,
mutation candidates, and their lifecycle audit — inspectable without JSON output
or a raw SQLite browser. It never writes the database; deletion is delegated to
the `autophagy` CLI.

It runs as a normal Dock application with an always-available menu-bar extra, so
a live summary of the open database is one click away even when the main window
is closed. Everything the menu bar shows is read from the same local database;
nothing new leaves the machine.

The app lives in [`apps/macos`](../../apps/macos) as a Swift Package. Full Xcode
is not required: it builds, tests, and bundles with the Swift toolchain from the
macOS Command Line Tools.

## Build and test

```sh
cd apps/macos
swift build            # compiles AutophagyKit + the SwiftUI app
swift test             # runs the AutophagyKit unit tests
```

`swift test` uses the toolchain-bundled swift-testing framework, so it runs
without Xcode. The tests build fixture databases with raw SQL that matches
[`docs/architecture/database-schema.md`](../architecture/database-schema.md) and
exercise the reader, schema-version tolerance, the read-only guarantee, mutation
package decoding, and CLI-command construction.

## Run

During development, run the built executable directly:

```sh
swift run autophagy-app
```

To launch from Finder, wrap the built binary in a minimal `.app` bundle. The
script needs no Xcode:

```sh
apps/macos/scripts/make-app-bundle.sh                 # release build into apps/macos/build
apps/macos/scripts/make-app-bundle.sh --configuration debug --output /tmp/out
open apps/macos/build/Autophagy.app
```

The script runs `swift build`, copies the executable into
`Autophagy.app/Contents/MacOS`, and writes an `Info.plist` with the bundle
identifier `sh.autophagy.Autophagy`.

## Database selection

On first run the app looks for the default database at the same location the CLI
resolves:

```text
~/Library/Application Support/sh.autophagy.Autophagy/autophagy.db
```

You can open that database with one click, or choose any other `.db` file. The
app validates that the file is an Autophagy database (it must carry the
migration ledger and the session and event tables) before opening it, and
remembers the choice in `UserDefaults` only. Nothing is written into the
repository or the database. Use "Switch database…" in the sidebar footer to
return to selection.

To create a database to inspect, import fixtures with the CLI first:

```sh
mise exec -- cargo run -p autophagy-cli -- \
  --database /tmp/demo.db import evals/fixtures/generic-jsonl/demo.jsonl \
  --instance-key demo
```

## What each view shows

- **Sessions** — every observed session with its source adapter, instance key,
  project path, time range, and event count. Selecting a session shows its
  ordered event timeline: event type, tool, exit code, timestamp, sequence, and
  the exact event ID for each event.
- **Patterns** — the deterministic findings preserved inside registered mutation
  candidates. Each finding shows its detector, hypothesis, expected result, and
  its exact supporting and counterexample event IDs. Findings are not stored as
  their own rows, so this view is empty until `autophagy mutations propose`
  registers candidates.
- **Mutations** — the candidate registry. Each candidate shows its lifecycle
  state, intervention and permissions, evidence lineage (exact supporting and
  counterexample event IDs, flagged if an event is no longer present), the full
  lifecycle audit log, any replay and shadow evaluation records, and any
  filesystem installation record. When a package was enriched by a local model
  provider (a Mutation Package v0.2 with a `provenance` block) the detail view
  shows a **Model provenance** card with the provider, model name, revision, and
  optional digest — model identity only, never an endpoint, key, prompt, or
  payload. Installation records name their target (Codex vs Claude Code repo
  skill). All of this is read-only.
- **Privacy** — where the database lives on disk, its schema version and
  compatibility, and honest counts of what it contains (sources, sessions,
  events, mutations, conflicts, and how many events opted into the search
  projection). It also restates the ingestion-time privacy posture: redaction,
  the redaction-approved full-text projection, local-first operation, and
  retention.

## Menu bar

The app installs a menu-bar extra that is always available, including while the
main window is closed. Clicking it opens a small read-only panel:

- **Connection state** — whether a database is open, its file name, and its
  schema compatibility (a coloured dot mirrors the sidebar's schema badge).
- **Quick stats** — session, event, and candidate counts, plus a breakdown of
  candidates by lifecycle state. These are cheap `COUNT` queries against the
  existing read-only reader.
- **Recent candidates** — the most recent mutation candidates with their state
  and detector.
- **Actions** — "Open Autophagy" brings up (and activates) the main window,
  "Refresh" re-reads the database, and "Quit Autophagy" exits.

The panel refreshes when it opens and when you press Refresh; there is no daemon
and no background polling. Because a menu-bar extra keeps the process alive, the
app keeps running in the menu bar after the main window is closed.

## Preferences

Open Autophagy's Settings (⌘,) to toggle **Run as a menu-bar-only app (hide the
Dock icon)**. When enabled the app runs as an accessory — no Dock icon — while
the menu-bar extra stays available; reopen the main window from the menu bar's
"Open Autophagy". The default is a normal Dock application. The preference is
stored in `UserDefaults` only and is applied at runtime via the app's activation
policy (no `LSUIElement` key is baked into the bundle), so the default bundle is
always a normal Dock app unless you opt in.

## Schema-version tolerance

The app is written to read schema version 1, the squashed release baseline (the
eight development-time migrations were collapsed into one before release; see
ADR 0012). On open it reads `user_version` and the `schema_migrations` ledger
and reports whether the database is fully supported, older but readable, or
newer than the app understands. A newer or older schema is read safely — unknown
tables are skipped and missing tables yield empty views — rather than crashing
or misreading.

One legacy database predates the squash and still carries the development-time
v8 ledger. Until the CLI touches it, the app classifies it as newer-than-known
(8 > 1) and reads it read-only under that heading. The CLI adopts it to the v1
baseline on first open — a one-time, in-place ledger rewrite that preserves all
data — after which the app reports it as fully supported at v1.

A database that the engine left in WAL mode but cleanly checkpointed (its
`-wal`/`-shm` sidecars removed) is opened read-only via SQLite's `immutable`
mode, so the fully checkpointed main file reads correctly as a point-in-time
snapshot. When a live `-wal` sidecar is present the app opens normally so
WAL-resident rows are still visible.

## Read-only guarantee and deletion

Every database connection is opened read-only (`SQLITE_OPEN_READONLY`) and
pinned with `PRAGMA query_only = ON`. The app has no SQL write path.

Deletion (a single session, or all data) is therefore delegated to the CLI. The
app shows the exact command, requires explicit confirmation — including typing
`delete-all` for the destructive whole-database case, mirroring the CLI's own
guard — and then runs the installed `autophagy` binary via a subprocess. If no
CLI binary is found on `PATH`, the app displays the command for you to run
yourself. All deletion flows through the same audited, reversible engine path,
including its refusal to remove evidence for an actively installed mutation.
