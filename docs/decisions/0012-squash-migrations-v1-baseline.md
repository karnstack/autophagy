# ADR 0012: Squash migrations into a v1 baseline before first release

- Status: accepted
- Date: 2026-07-17

## Context

The store reached development-time schema v8 through eight ordered migrations
(`0001`..`0008`). Two of them (`0005` and `0007`, and again `0008`) recreate
whole tables via SQLite's recreate-copy-rename dance to alter `CHECK`
constraints, so the final schema is scattered across eight files, several of
which build and then discard intermediate table shapes. `sqlite_master` for the
v8 database even carries the quoted, renamed artifacts of that dance.

Autophagy has never shipped. Exactly one database was ever produced by the eight
migrations — the author's own, at v8 with the full eight-row `schema_migrations`
ledger and matching checksums. There are no external users and no other
databases in existence.

The project's rule that "migrations are ordered and immutable" exists to protect
*released* users: once someone's database has been migrated by a published
binary, editing or deleting the migration that produced it would corrupt their
upgrade path. That protection has no subject before the first release. Carrying
eight fossilized development diffs — including whole-table recreations — into the
release as the permanent origin story makes the schema harder to read, review,
and reason about, for no compatibility benefit.

## Decision

Replace `0001`..`0008` with a single `0001_initial_schema.sql` and start the
released, immutable chain there. The immutability rule's scope begins at the
first release, not at the first development commit.

- **One schema-identical baseline.** The baseline's applied result is identical
  to the old chain's final v8 state — every table, column, order, `CHECK`, `FK`,
  index, trigger, `STRICT`, and the FTS5 definition and its shadow tables. It was
  derived by applying the chain to a scratch database and reconstructing the DDL
  from `sqlite_master` (formatting normalized, the good comments kept), not
  transcribed from memory. The tracked version and `user_version` become 1.
- **A load-bearing equivalence proof.** `tests/schema_equivalence.rs` builds one
  database from the eight legacy migrations (preserved verbatim under
  `tests/legacy/`) and one from the shipped baseline, and asserts their full
  `sqlite_master` (normalized) and per-table `table_info` / `index_list` /
  `foreign_key_list` pragmas are identical. No drift is tolerated.
- **A pre-release adoption shim.** The one known legacy database must not be
  rejected. On open, when the framework finds `user_version` 8 and a
  `schema_migrations` ledger that exactly matches the known legacy v1..=v8 chain
  (each checksum verified against constants embedded in `migration.rs`), it
  adopts the database in a single transaction: the eight ledger rows are replaced
  by the single v1 baseline row (carrying the baseline's real checksum) and
  `user_version` is set to 1. Because the schema is already identical, adoption
  never touches a table — it rewrites only the ledger. A second open is a no-op.
  Any other ledger at an unexpected version is left untouched and surfaces as
  `DatabaseTooNew` (or a checksum mismatch) rather than being silently rewritten.
- **macOS app.** `knownSchemaVersion` becomes 1. A not-yet-adopted v8 legacy
  database classifies as newer-than-known (8 > 1) and is read read-only under the
  app's existing "newer" handling; the CLI adopts it to v1 on first touch.

## Removal plan

The adoption shim and its `LEGACY_V8_CHECKSUMS` constants exist solely to migrate
one database that no longer needs migrating once it has been adopted. They are
dead weight afterward, and they recognize a checksum set that must never match a
legitimately-released database. Remove `adopt_legacy_baseline_if_needed`,
`matches_known_legacy_chain`, and `LEGACY_V8_CHECKSUMS` — together with the
`tests/legacy/` fixtures and the adoption test — in a post-release maintenance
change, once the author's database has been adopted (a good moment is the first
migration added after `0001`, e.g. schema v2). The equivalence test can be
retired at the same time; from the first real migration onward, the ordered,
immutable chain is the only contract that matters.

## Privacy

The change is local-only and touches no data. Adoption rewrites four small
ledger rows into one inside a single transaction on the user's own database; it
copies nothing off the machine, reads no event content, and cannot run against a
database whose ledger it does not recognize byte-for-byte. The squash removes
eight files from the repository and adds one; it changes no stored event,
mutation, or audit row, and preserves every evidence identifier already on disk.
