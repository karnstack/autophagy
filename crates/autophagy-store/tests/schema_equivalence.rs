//! Load-bearing proof that the squashed v1 baseline is schema-identical to the
//! eight-migration development chain it replaces.
//!
//! One database is built by applying the legacy chain (SQL preserved under
//! `tests/legacy/`), another by applying the shipped `0001_initial_schema.sql`.
//! Their full `sqlite_master` (every table, index, trigger, virtual table, and
//! FTS5 shadow table) and per-table `table_info` / `index_list` /
//! `foreign_key_list` pragmas must match exactly. No drift is tolerated.

mod common;

use std::collections::BTreeMap;

use rusqlite::Connection;

/// The shipped baseline migration — the real file the store embeds.
const BASELINE_SQL: &str = include_str!("../migrations/0001_initial_schema.sql");

/// The framework-managed ledger table, created (in both worlds) before any
/// migration DDL runs. Mirrors `migration::BOOTSTRAP_SQL`.
const BOOTSTRAP_SQL: &str = "
CREATE TABLE IF NOT EXISTS schema_migrations (
  version       INTEGER PRIMARY KEY,
  description   TEXT NOT NULL,
  checksum      BLOB NOT NULL CHECK (length(checksum) = 32),
  applied_at    TEXT NOT NULL
) STRICT;
";

/// Collapse whitespace and drop the identifier quotes `SQLite` adds when a table
/// is renamed (the legacy chain's recreate-copy-rename dance leaves quoted names
/// and quoted foreign-key references in `sqlite_master.sql`). What remains is the
/// effective DDL, formatting aside.
fn normalize(sql: &str) -> String {
    sql.replace('"', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every object's normalized SQL, keyed by (type, name).
fn object_ddl(connection: &Connection) -> BTreeMap<(String, String), Option<String>> {
    let mut statement = connection
        .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
        .expect("query sqlite_master");
    statement
        .query_map([], |row| {
            let kind: String = row.get(0)?;
            let name: String = row.get(1)?;
            let sql: Option<String> = row.get(2)?;
            Ok(((kind, name), sql.as_deref().map(normalize)))
        })
        .expect("map sqlite_master")
        .collect::<Result<BTreeMap<_, _>, _>>()
        .expect("collect sqlite_master")
}

/// A stable textual fingerprint of a table's effective column, index, and
/// foreign-key shape as `SQLite` reports it (captures column order, declared type,
/// NOT NULL, defaults, primary-key position, uniqueness, and FK edges).
fn table_pragmas(connection: &Connection, table: &str) -> String {
    let mut out = String::new();

    let mut ti = connection
        .prepare(&format!("PRAGMA table_info('{table}')"))
        .expect("table_info");
    let cols = ti
        .query_map([], |row| {
            Ok(format!(
                "col[{}] {} {} notnull={} dflt={:?} pk={}",
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .expect("map table_info")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect table_info");
    for col in cols {
        out.push_str(&col);
        out.push('\n');
    }

    let mut il = connection
        .prepare(&format!("PRAGMA index_list('{table}')"))
        .expect("index_list");
    let mut indexes = il
        .query_map([], |row| {
            // Skip the volatile `seq` column (index 0); keep name/unique/origin/partial.
            Ok(format!(
                "index {} unique={} origin={} partial={}",
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .expect("map index_list")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect index_list");
    indexes.sort();
    for index in indexes {
        out.push_str(&index);
        out.push('\n');
    }

    let mut fk = connection
        .prepare(&format!("PRAGMA foreign_key_list('{table}')"))
        .expect("foreign_key_list");
    let mut fks = fk
        .query_map([], |row| {
            Ok(format!(
                "fk -> {}({}) from {} on_delete={} on_update={} match={}",
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                row.get::<_, String>(6)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(7)?,
            ))
        })
        .expect("map foreign_key_list")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect foreign_key_list");
    fks.sort();
    for edge in fks {
        out.push_str(&edge);
        out.push('\n');
    }

    out
}

fn build_from_baseline() -> Connection {
    let connection = Connection::open_in_memory().expect("open memory");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("foreign keys");
    connection
        .execute_batch(BOOTSTRAP_SQL)
        .expect("bootstrap ledger");
    connection
        .execute_batch(BASELINE_SQL)
        .expect("apply baseline");
    connection
}

fn build_from_legacy_chain() -> Connection {
    let connection = Connection::open_in_memory().expect("open memory");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("foreign keys");
    common::apply_legacy_chain(&connection);
    connection
}

#[test]
fn baseline_matches_legacy_chain_sqlite_master() {
    let legacy = build_from_legacy_chain();
    let baseline = build_from_baseline();

    let legacy_ddl = object_ddl(&legacy);
    let baseline_ddl = object_ddl(&baseline);

    let legacy_keys: Vec<_> = legacy_ddl.keys().collect();
    let baseline_keys: Vec<_> = baseline_ddl.keys().collect();
    assert_eq!(
        legacy_keys, baseline_keys,
        "sqlite_master object set differs between the legacy chain and the v1 baseline"
    );

    for key in legacy_ddl.keys() {
        assert_eq!(
            legacy_ddl[key], baseline_ddl[key],
            "normalized DDL differs for {key:?}"
        );
    }
}

#[test]
fn baseline_matches_legacy_chain_table_pragmas() {
    let legacy = build_from_legacy_chain();
    let baseline = build_from_baseline();

    let ddl = object_ddl(&legacy);
    let tables: Vec<String> = ddl
        .keys()
        .filter(|(kind, name)| kind == "table" && !name.starts_with("sqlite_"))
        .map(|(_, name)| name.clone())
        .collect();

    assert!(
        tables.iter().any(|name| name == "mutation_candidates"),
        "expected the mutation registry among the compared tables"
    );

    for table in tables {
        assert_eq!(
            table_pragmas(&legacy, &table),
            table_pragmas(&baseline, &table),
            "table_info/index_list/foreign_key_list differ for {table}"
        );
    }
}
