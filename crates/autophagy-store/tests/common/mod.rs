//! Shared support for the schema-squash tests.
//!
//! These helpers reconstruct the exact eight-migration development chain that
//! existed before the first release, from the SQL preserved verbatim under
//! `tests/legacy/`. They let the equivalence and adoption tests build a database
//! that is byte-for-byte the one the migration framework produced at v8.

use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

/// The eight pre-release development migrations, in order: (version,
/// description, SQL). Descriptions mirror the historical `MIGRATIONS` table.
pub const LEGACY_CHAIN: &[(i64, &str, &str)] = &[
    (
        1,
        "initial event store",
        include_str!("../legacy/0001_initial.sql"),
    ),
    (
        2,
        "incremental source cursors",
        include_str!("../legacy/0002_source_cursors.sql"),
    ),
    (
        3,
        "immutable mutation candidate registry",
        include_str!("../legacy/0003_mutation_registry.sql"),
    ),
    (
        4,
        "deterministic mutation replay evaluation",
        include_str!("../legacy/0004_replay_evaluation.sql"),
    ),
    (
        5,
        "shadow evaluation and reversible installation",
        include_str!("../legacy/0005_shadow_install.sql"),
    ),
    (
        6,
        "exact normalized-signature retrieval index",
        include_str!("../legacy/0006_retrieval_signature.sql"),
    ),
    (
        7,
        "claude code repo-skill installation target",
        include_str!("../legacy/0007_claude_code_install_target.sql"),
    ),
    (
        8,
        "accept mutation/0.2 provenance packages",
        include_str!("../legacy/0008_mutation_provenance.sql"),
    ),
];

/// SHA-256 of a migration's raw SQL bytes — the exact checksum the store's
/// migration framework recorded in `schema_migrations`.
#[must_use]
pub fn checksum(sql: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    hasher.finalize().into()
}

/// Apply the full legacy chain to `connection`, reproducing the on-disk ledger
/// the framework wrote: one `schema_migrations` row per migration with its real
/// checksum, and `user_version` left at 8.
pub fn apply_legacy_chain(connection: &Connection) {
    connection
        .execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
               version       INTEGER PRIMARY KEY,
               description   TEXT NOT NULL,
               checksum      BLOB NOT NULL CHECK (length(checksum) = 32),
               applied_at    TEXT NOT NULL
             ) STRICT",
            [],
        )
        .expect("bootstrap schema_migrations");

    for (version, description, sql) in LEGACY_CHAIN {
        connection
            .execute_batch(sql)
            .expect("apply legacy migration");
        connection
            .execute(
                "INSERT INTO schema_migrations(version, description, checksum, applied_at)
                 VALUES (?1, ?2, ?3, '2026-07-16T00:00:00Z')",
                params![version, description, checksum(sql).as_slice()],
            )
            .expect("record legacy migration");
    }
    connection
        .pragma_update(None, "user_version", 8)
        .expect("set legacy user_version");
}
