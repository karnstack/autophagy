//! Pre-release adoption: opening the one known legacy v8 database through the
//! store rewrites its ledger to the v1 baseline in place, preserving all data,
//! and a second open is a no-op. Any other v8-shaped ledger is refused.

mod common;

use autophagy_store::{EventStore, StoreError};
use rusqlite::Connection;

/// Representative rows spanning ingestion and the mutation registry, inserted
/// into an already-migrated legacy database.
const SEED_DATA: &str = "
INSERT INTO sources(source_id, adapter, instance_key, first_seen_at, last_seen_at)
  VALUES (1, 'generic-jsonl', 'demo', '2026-07-16T10:00:00Z', '2026-07-16T10:10:00Z');
INSERT INTO sessions(session_id, source_id, project_path, first_event_at, last_event_at, event_count)
  VALUES ('ses_a', 1, '/workspace/a', '2026-07-16T10:00:00Z', '2026-07-16T10:02:00Z', 2);
INSERT INTO events(row_id, event_id, spec_version, session_id, occurred_at, sequence,
                   event_type, tool_name, exit_code, event_json, content_hash, imported_at)
  VALUES
   (1,'evt_a1','aep/1','ses_a','2026-07-16T10:00:00Z',0,'tool.failed','shell',1,'{}',zeroblob(32),'2026-07-16T10:00:00Z'),
   (2,'evt_a2','aep/1','ses_a','2026-07-16T10:02:00Z',1,'tool.completed','shell',0,'{}',zeroblob(32),'2026-07-16T10:02:00Z');
INSERT INTO mutation_candidates(mutation_id, source_finding_id, source_detector, equivalence_key,
                                spec_version, semantic_version, state, package_json, content_hash,
                                created_at, updated_at)
  VALUES ('mut_1','fnd_1','repeated_command_failure','eqv_1','mutation/0.1','0.1.0','candidate',
          '{}',zeroblob(32),'2026-07-16T11:00:00Z','2026-07-16T11:00:00Z');
INSERT INTO mutation_evidence(mutation_id, event_id, role, ordinal)
  VALUES ('mut_1','evt_a1','support',0);
INSERT INTO mutation_transitions(transition_id, mutation_id, from_state, to_state, reason,
                                 metadata_json, occurred_at)
  VALUES (1,'mut_1',NULL,'candidate','generated from evidence','{}','2026-07-16T11:00:00Z');
";

/// Build a fully-migrated legacy v8 database with representative data.
fn seed_legacy_v8(path: &std::path::Path) {
    let connection = Connection::open(path).expect("open legacy database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("foreign keys");
    common::apply_legacy_chain(&connection);
    connection.execute_batch(SEED_DATA).expect("seed data");
}

#[derive(Debug, PartialEq, Eq)]
struct Counts {
    events: i64,
    sessions: i64,
    candidates: i64,
    evidence: i64,
    transitions: i64,
}

fn data_counts(path: &std::path::Path) -> Counts {
    let connection = Connection::open(path).expect("reopen database");
    let one = |sql: &str| -> i64 {
        connection
            .query_row(sql, [], |row| row.get(0))
            .expect("count query")
    };
    Counts {
        events: one("SELECT count(*) FROM events"),
        sessions: one("SELECT count(*) FROM sessions"),
        candidates: one("SELECT count(*) FROM mutation_candidates"),
        evidence: one("SELECT count(*) FROM mutation_evidence"),
        transitions: one("SELECT count(*) FROM mutation_transitions"),
    }
}

fn ledger_state(path: &std::path::Path) -> (i64, i64, i64) {
    let connection = Connection::open(path).expect("reopen database");
    let rows: i64 = connection
        .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .expect("ledger rows");
    let max_version: i64 = connection
        .query_row(
            "SELECT coalesce(max(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .expect("ledger max version");
    let user_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("user_version");
    (rows, max_version, user_version)
}

#[test]
fn known_legacy_v8_database_is_adopted_and_data_preserved() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("autophagy.db");
    seed_legacy_v8(&path);

    let before = data_counts(&path);
    assert_eq!(
        ledger_state(&path),
        (8, 8, 8),
        "precondition: legacy v8 ledger"
    );

    // First open through the store adopts the baseline, collapsing the eight
    // legacy rows to the v1 baseline, then applies the post-baseline chain
    // (v2, v3) on top — landing on the current released schema.
    {
        let store = EventStore::open(&path).expect("open adopts legacy database");
        assert_eq!(store.schema_version().expect("schema version"), 3);
    }

    assert_eq!(
        ledger_state(&path),
        (3, 3, 3),
        "legacy chain replaced by the released baseline chain and user_version reset"
    );
    assert_eq!(
        data_counts(&path),
        before,
        "all events, sessions, candidates, evidence, and audit rows preserved"
    );

    // Second open is a pure no-op: same schema, same ledger, data intact.
    {
        let store = EventStore::open(&path).expect("second open is a no-op");
        assert_eq!(store.schema_version().expect("schema version"), 3);
    }
    assert_eq!(ledger_state(&path), (3, 3, 3));
    assert_eq!(data_counts(&path), before);
}

#[test]
fn unrecognized_v8_ledger_is_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("autophagy.db");
    seed_legacy_v8(&path);

    // Tamper one recorded checksum so the ledger no longer matches the known
    // legacy chain. Adoption must decline and the open must fail rather than
    // silently rewriting an unrecognised pre-release database.
    {
        let connection = Connection::open(&path).expect("open to tamper");
        connection
            .execute(
                "UPDATE schema_migrations SET checksum = zeroblob(32) WHERE version = 5",
                [],
            )
            .expect("tamper checksum");
    }

    match EventStore::open(&path) {
        Err(StoreError::DatabaseTooNew { version }) => assert_eq!(version, 8),
        Err(other) => {
            panic!("expected DatabaseTooNew for an unrecognised v8 ledger, got {other:?}")
        }
        Ok(_) => panic!("an unrecognised v8 ledger must not open"),
    }
}
