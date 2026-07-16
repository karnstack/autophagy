# ADR 0005: macOS read-only application

- Status: accepted
- Date: 2026-07-17

## Context

The alpha needs the complete local loop — sessions, deterministic findings,
mutation candidates, and their lifecycle audit — to be inspectable without JSON
output or a raw SQLite browser. The product wedge is a native macOS experience.
Two constraints shape how it is built now.

First, the engine's guarantees must not be weakened by a second write path. The
database is the single-user source of truth, with idempotency, quarantine,
cascading deletion, and an active-mutation deletion guard all enforced inside
`autophagy-store`. A UI that wrote the database directly would duplicate — and
inevitably drift from — those invariants.

Second, the available toolchain is Swift 6.3 with the macOS SDK via the Command
Line Tools. Full Xcode (and therefore `xcodebuild`, `.xcodeproj`, and the
XCTest framework) is not available in the build environment.

## Decision

Ship `apps/macos/` as a SwiftPM package with a SwiftUI executable target and a
testable `AutophagyKit` library, opened strictly read-only.

- **SwiftPM, not Xcode.** The app is a `Package.swift` with an
  `executableTarget` (SwiftUI `@main`) and a `library` target. It builds and
  tests with `swift build` and `swift test` under the Command Line Tools alone.
  A small `scripts/make-app-bundle.sh` wraps the built binary in a minimal
  `.app` with an `Info.plist` for launching from Finder. No `.xcodeproj` and no
  code generation are introduced. Unit tests use the toolchain-bundled
  swift-testing framework (`import Testing`), because XCTest ships only with
  Xcode.
- **Strict read-only.** Every connection is opened with `SQLITE_OPEN_READONLY`
  and immediately sets `PRAGMA query_only = ON`. The application has no SQL
  write path of any kind; a regression test asserts a write is rejected.
- **CLI-mediated destructive actions.** Deletion (session and delete-all) is not
  implemented by writing the database. The app constructs the exact `autophagy`
  command, shows it, requires explicit multi-step confirmation (including the
  `delete-all` phrase that mirrors the CLI's own guard), and then either runs the
  installed CLI via `Process` or — when no binary is found — displays the command
  for the user to run. All deletion therefore flows through the same audited,
  reversible engine path, including its active-installation guard.
- **Schema-version tolerance.** On open, the app reads `PRAGMA user_version` and
  `max(version)` from `schema_migrations` and classifies the database as
  supported, older-but-readable, newer-than-known, or not-an-Autophagy-database.
  Every query checks for table existence first, so a schema that predates or
  postdates the app's known version (6) degrades to empty results and a clear
  message rather than a crash or a misread.
- **Same default path as the CLI.** The default database location is resolved to
  `~/Library/Application Support/sh.autophagy.Autophagy/autophagy.db`, matching
  the CLI's `directories`-crate resolution. The user's chosen path is remembered
  only in `UserDefaults`; nothing is written into the repository or the database.

## Privacy

The app is a read-only viewer. It opens a local database, sends nothing off the
machine, and cannot alter stored data. It restates the engine's ingestion-time
privacy posture (redaction, the redaction-approved search projection, retention
and forgetting) and surfaces measurable counts, but it performs no redaction of
its own and adds no export path. Because deletion is delegated to the CLI, the
app never becomes an unaudited way to remove — or leak — evidence.

## Consequences

- The complete local loop is inspectable natively without JSON or raw SQLite.
- The database retains exactly one write path (the engine), so its idempotency,
  deletion, and installation guarantees cannot drift into a second surface.
- Findings are surfaced from the Evidence Packet preserved inside each
  registered mutation candidate; findings are not persisted as their own rows,
  so the Patterns view is empty until candidates are proposed.
- A native app that must run headless in CI is validated by unit-testing
  `AutophagyKit` against fixture databases plus a `swift build` of the UI target;
  the SwiftUI layer stays deliberately thin.
- Moving to a signed, notarized build or a menu-bar presentation later is an
  additive packaging change; it does not alter the read-only or CLI-mediation
  boundaries established here.
