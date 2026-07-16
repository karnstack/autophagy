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

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "initial event store",
        sql: include_str!("../migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        description: "incremental source cursors",
        sql: include_str!("../migrations/0002_source_cursors.sql"),
    },
];

pub(crate) fn apply(connection: &mut Connection) -> Result<(), StoreError> {
    connection.execute_batch(BOOTSTRAP_SQL)?;

    let applied = load_applied(connection)?;
    let latest = MIGRATIONS.last().map_or(0, |migration| migration.version);
    if let Some(version) = applied.keys().copied().find(|version| *version > latest) {
        return Err(StoreError::DatabaseTooNew { version });
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

fn load_applied(connection: &Connection) -> Result<BTreeMap<i64, Vec<u8>>, rusqlite::Error> {
    let mut statement =
        connection.prepare("SELECT version, checksum FROM schema_migrations ORDER BY version")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};

    use super::apply;
    use crate::StoreError;

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
        connection
            .execute(
                "INSERT INTO schema_migrations(version, description, checksum, applied_at)
                 VALUES (3, 'future', ?1, '2026-07-16T00:00:00Z')",
                params![[7_u8; 32].as_slice()],
            )
            .expect("future migration");

        assert!(matches!(
            apply(&mut connection),
            Err(StoreError::DatabaseTooNew { version: 3 })
        ));
    }
}
