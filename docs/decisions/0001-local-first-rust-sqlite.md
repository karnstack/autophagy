# ADR 0001: Local-first Rust engine with SQLite

- Status: accepted
- Date: 2026-07-16

## Context

Autophagy reads high-sensitivity coding-agent activity and must work without an
account, network, or hosted model. It needs portable collectors, deterministic
detectors, full-text retrieval, a CLI, a background daemon, and a native macOS
client.

## Decision

Build the cross-platform engine as a Rust workspace and use SQLite with FTS5 as
the local source of truth. Keep the macOS experience native in SwiftUI and
communicate with the daemon through a versioned local IPC boundary.

The engine has no required cloud dependency. Model providers are optional
downstream services; deterministic ingestion, retrieval, and detectors continue
to work when every provider is disabled.

## Consequences

- One portable engine can serve the CLI, daemon, MCP server, and native app.
- SQLite makes inspection, export, transactions, deletion, and FTS available
  without operating a separate service.
- Rust and Swift add a multi-language build, so protocol boundaries must remain
  explicit and tested.
- Vector retrieval remains optional until lexical and exact-signal baselines are
  measured.
