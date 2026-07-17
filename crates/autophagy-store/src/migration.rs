use std::collections::BTreeMap;

use rusqlite::{Connection, TransactionBehavior, params};

use crate::{StoreError, util};

const BOOTSTRAP_SQL: &str = "
CREATE TABLE IF NOT EXISTS schema_migrations (
  version       INTEGER PRIMARY KEY,
  description   TEXT NOT NULL,
  checksum      BLOB NOT NULL CHECK (length(checksum) = 32),
  applied_at    TEXT NOT NULL
) STRICT;
";

struct Migration {
    version: i64,
    description: &'static str,
    sql: &'static str,
}

/// The released migration chain. It begins at a single squashed v1 baseline.
///
/// The eight development-time migrations that produced this schema were squashed
/// before the first release, while no external database existed; see ADR 0012.
/// The baseline is schema-identical to that chain's final state, proven by
/// `tests/schema_equivalence.rs`. From the first release onward this chain is
/// ordered and immutable — add new migrations, never edit an applied one.
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    description: "initial schema",
    sql: include_str!("../migrations/0001_initial_schema.sql"),
}];

/// SHA-256 checksums of the eight pre-release development migrations, in order
/// (v1..=v8). Exactly one database was ever built by that chain — the author's
/// own, at development-time schema v8. This is the sole legacy ledger the
/// adoption shim recognises before the first release.
///
/// The legacy migration SQL is preserved verbatim under `tests/legacy/` and the
/// schema-equivalence test recomputes these checksums from it, so they cannot
/// silently drift. Remove this constant and the adoption shim once the author's
/// database has been adopted (see ADR 0012).
const LEGACY_V8_CHECKSUMS: [[u8; 32]; 8] = [
    // v1: initial event store
    [
        0xf3, 0x25, 0xb1, 0xee, 0xfb, 0xd9, 0x54, 0x4f, 0x72, 0x2f, 0xfe, 0xf6, 0x01, 0x0d, 0x69,
        0x2a, 0x76, 0xc3, 0xc3, 0xe1, 0xc0, 0xeb, 0x93, 0x46, 0xd8, 0x4b, 0x20, 0xe7, 0x27, 0x82,
        0x34, 0x54,
    ],
    // v2: incremental source cursors
    [
        0xdf, 0x9b, 0x5b, 0x6b, 0xc5, 0xde, 0x5a, 0xd5, 0x0a, 0x23, 0xa8, 0x82, 0xdd, 0x33, 0xd8,
        0x61, 0x39, 0x52, 0x75, 0x1a, 0x26, 0x68, 0x29, 0xa4, 0x75, 0x7f, 0x23, 0x3e, 0x04, 0xd8,
        0x00, 0x4f,
    ],
    // v3: immutable mutation candidate registry
    [
        0xbd, 0x9d, 0x85, 0x5a, 0x38, 0x9c, 0x65, 0x8c, 0xf1, 0xdd, 0x52, 0x7d, 0x83, 0x56, 0xb4,
        0xf4, 0x7f, 0x8b, 0xd4, 0x26, 0x27, 0x68, 0x9b, 0xc5, 0xc0, 0xda, 0x2b, 0x02, 0x18, 0x88,
        0x09, 0xda,
    ],
    // v4: deterministic mutation replay evaluation
    [
        0x80, 0x78, 0x17, 0xb7, 0x66, 0xf5, 0x75, 0x94, 0x00, 0x8b, 0xce, 0x84, 0x51, 0x35, 0xc0,
        0x4b, 0x41, 0x6d, 0x4b, 0xa0, 0x28, 0x57, 0x08, 0xc9, 0xae, 0xcf, 0x13, 0x2f, 0xa5, 0xba,
        0x9a, 0x0e,
    ],
    // v5: shadow evaluation and reversible installation
    [
        0xa6, 0xe8, 0xfa, 0xf1, 0x8c, 0x3e, 0xec, 0x70, 0xf9, 0xa9, 0x0a, 0x4b, 0xeb, 0xf2, 0x7b,
        0x14, 0x08, 0xc4, 0x01, 0x77, 0xca, 0x0a, 0x0d, 0x61, 0x06, 0xa9, 0x0e, 0xc5, 0xf6, 0xb6,
        0xab, 0x6a,
    ],
    // v6: exact normalized-signature retrieval index
    [
        0xd9, 0x0a, 0xd6, 0xc0, 0x18, 0xb7, 0xdc, 0xac, 0x52, 0xcf, 0xcc, 0xee, 0x16, 0x64, 0x44,
        0xb1, 0x30, 0x56, 0xf6, 0x5c, 0x49, 0x0f, 0xe0, 0x67, 0x7f, 0x81, 0xda, 0x84, 0x9b, 0x63,
        0xa9, 0xe3,
    ],
    // v7: claude code repo-skill installation target
    [
        0xba, 0x15, 0xed, 0x1f, 0x53, 0x0b, 0xaa, 0x6a, 0xcb, 0x01, 0x0b, 0x37, 0xcf, 0xba, 0x9a,
        0xe4, 0x8e, 0x51, 0x3f, 0x6a, 0xa9, 0xb1, 0xde, 0xd7, 0x52, 0x1d, 0x78, 0xdf, 0x79, 0x11,
        0xfb, 0x46,
    ],
    // v8: accept mutation/0.2 provenance packages
    [
        0x4a, 0xa3, 0xe7, 0x1c, 0xc6, 0x5e, 0x3e, 0xa5, 0xb1, 0x52, 0xb8, 0xa7, 0x1b, 0x47, 0x10,
        0xf0, 0xe0, 0x23, 0xc0, 0x9f, 0xf8, 0x25, 0xef, 0xc0, 0x94, 0xf0, 0x85, 0x2d, 0xc7, 0xca,
        0xb4, 0x27,
    ],
];

pub(crate) fn apply(connection: &mut Connection) -> Result<(), StoreError> {
    connection.execute_batch(BOOTSTRAP_SQL)?;
    adopt_legacy_baseline_if_needed(connection)?;

    let applied = load_applied(connection)?;
    let latest = MIGRATIONS.last().map_or(0, |migration| migration.version);
    // Report the highest applied version so an unrecognised pre-release database
    // (a development ledger the adoption shim did not match) fails with a clear
    // "database is newer than this binary supports" verdict rather than pointing
    // at the first out-of-range row.
    if let Some(version) = applied.keys().next_back().copied() {
        if version > latest {
            return Err(StoreError::DatabaseTooNew { version });
        }
    }

    let highest_applied = applied.keys().next_back().copied().unwrap_or(0);
    for migration in MIGRATIONS {
        let checksum = util::sha256(migration.sql.as_bytes());
        if let Some(stored_checksum) = applied.get(&migration.version) {
            if stored_checksum.as_slice() != checksum {
                return Err(StoreError::MigrationDrift {
                    version: migration.version,
                });
            }
            continue;
        }
        if migration.version < highest_applied {
            return Err(StoreError::MissingMigration {
                version: migration.version,
            });
        }

        let applied_at = util::now_timestamp()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(migration.sql)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, description, checksum, applied_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                migration.version,
                migration.description,
                checksum.as_slice(),
                applied_at
            ],
        )?;
        transaction.pragma_update(None, "user_version", migration.version)?;
        transaction.commit()?;
    }

    Ok(())
}

/// Pre-release courtesy shim: adopt the one known legacy database into the v1
/// baseline ledger.
///
/// Exactly one database was ever produced by the eight development-time
/// migrations — the author's own, carrying the full v1..=v8 ledger. Its schema
/// is byte-for-byte identical to the v1 baseline (proven by the schema
/// equivalence test), so adoption never touches a table: it rewrites the ledger
/// to the single baseline row and resets `user_version` to 1, in one
/// transaction. Any other multi-row or mismatched ledger is left untouched and
/// falls through to the normal migration path, where an unexpected version
/// surfaces as [`StoreError::DatabaseTooNew`] or a checksum mismatch. Remove this
/// shim once the author's database has been adopted (see ADR 0012).
fn adopt_legacy_baseline_if_needed(connection: &mut Connection) -> Result<(), StoreError> {
    let applied = load_applied(connection)?;
    if !matches_known_legacy_chain(&applied) {
        return Ok(());
    }

    let baseline = &MIGRATIONS[0];
    let checksum = util::sha256(baseline.sql.as_bytes());
    let applied_at = util::now_timestamp()?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute("DELETE FROM schema_migrations", [])?;
    transaction.execute(
        "INSERT INTO schema_migrations(version, description, checksum, applied_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            baseline.version,
            baseline.description,
            checksum.as_slice(),
            applied_at
        ],
    )?;
    transaction.pragma_update(None, "user_version", baseline.version)?;
    transaction.commit()?;
    Ok(())
}

/// True only when `applied` is exactly the eight-row pre-release ledger (v1..=v8)
/// with every checksum matching [`LEGACY_V8_CHECKSUMS`].
fn matches_known_legacy_chain(applied: &BTreeMap<i64, Vec<u8>>) -> bool {
    if applied.len() != LEGACY_V8_CHECKSUMS.len() {
        return false;
    }
    LEGACY_V8_CHECKSUMS
        .iter()
        .enumerate()
        .all(|(index, checksum)| {
            let version = i64::try_from(index + 1).expect("legacy chain length fits i64");
            applied
                .get(&version)
                .is_some_and(|stored| stored.as_slice() == checksum)
        })
}

fn load_applied(connection: &Connection) -> Result<BTreeMap<i64, Vec<u8>>, rusqlite::Error> {
    let mut statement =
        connection.prepare("SELECT version, checksum FROM schema_migrations ORDER BY version")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rusqlite::{Connection, params};

    use super::{LEGACY_V8_CHECKSUMS, MIGRATIONS, apply, matches_known_legacy_chain};
    use crate::{StoreError, util};

    #[test]
    fn fresh_database_applies_the_v1_baseline() {
        let mut connection = Connection::open_in_memory().expect("database");
        apply(&mut connection).expect("initial migration");

        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("schema version"),
            1
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM schema_migrations", [], |row| row
                    .get::<_, i64>(0))
                .expect("ledger rows"),
            1
        );
        // A representative table from the squashed tail exists at v1.
        assert!(
            connection
                .query_row(
                    "SELECT 1 FROM sqlite_master \
                     WHERE type = 'table' AND name = 'mutation_installations'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .is_ok()
        );
    }

    #[test]
    fn reapplying_the_baseline_is_a_no_op() {
        let mut connection = Connection::open_in_memory().expect("database");
        apply(&mut connection).expect("initial migration");
        apply(&mut connection).expect("second apply");

        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM schema_migrations", [], |row| row
                    .get::<_, i64>(0))
                .expect("ledger rows"),
            1
        );
    }

    #[test]
    fn changed_applied_migration_is_rejected() {
        let mut connection = Connection::open_in_memory().expect("database");
        apply(&mut connection).expect("initial migration");
        connection
            .execute(
                "UPDATE schema_migrations SET checksum = zeroblob(32) WHERE version = 1",
                [],
            )
            .expect("tamper checksum");

        assert!(matches!(
            apply(&mut connection),
            Err(StoreError::MigrationDrift { version: 1 })
        ));
    }

    #[test]
    fn newer_database_is_rejected() {
        let mut connection = Connection::open_in_memory().expect("database");
        apply(&mut connection).expect("initial migration");
        let future = MIGRATIONS.last().expect("migration").version + 1;
        connection
            .execute(
                "INSERT INTO schema_migrations(version, description, checksum, applied_at)
                 VALUES (?1, 'future', ?2, '2026-07-16T00:00:00Z')",
                params![future, [7_u8; 32].as_slice()],
            )
            .expect("future migration");

        assert!(matches!(
            apply(&mut connection),
            Err(StoreError::DatabaseTooNew { version }) if version == future
        ));
    }

    #[test]
    fn known_legacy_chain_matcher_rejects_partial_and_tampered_ledgers() {
        // The exact eight-row ledger matches.
        let mut full: BTreeMap<i64, Vec<u8>> = BTreeMap::new();
        for (index, checksum) in LEGACY_V8_CHECKSUMS.iter().enumerate() {
            let version = i64::try_from(index + 1).expect("version");
            full.insert(version, checksum.to_vec());
        }
        assert!(matches_known_legacy_chain(&full));

        // A short ledger (fewer than eight rows) never matches.
        let mut partial = full.clone();
        partial.remove(&8);
        assert!(!matches_known_legacy_chain(&partial));

        // A tampered checksum never matches.
        let mut tampered = full.clone();
        tampered.insert(4, vec![0_u8; 32]);
        assert!(!matches_known_legacy_chain(&tampered));

        // The adopted single-row v1 ledger is not a legacy chain, so a second
        // open never re-adopts.
        let mut adopted: BTreeMap<i64, Vec<u8>> = BTreeMap::new();
        adopted.insert(1, util::sha256(MIGRATIONS[0].sql.as_bytes()).to_vec());
        assert!(!matches_known_legacy_chain(&adopted));
    }
}
