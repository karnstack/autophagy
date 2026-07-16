import Foundation
import SQLite3
@testable import AutophagyKit

/// Helper that builds a minimal Autophagy database on disk using raw SQL that
/// matches `docs/architecture/database-schema.md`. This is test-fixture setup,
/// not an application write path: the app itself only ever opens read-only.
enum FixtureDatabase {
    /// Create a temporary database file, populate it via `setup`, and return its
    /// path. The caller is responsible for deletion (see `TempPath`).
    static func make(schemaVersion: Int, _ setup: (OpaquePointer) -> Void) -> String {
        let path = NSTemporaryDirectory() + "autophagy-test-\(UUID().uuidString).db"
        var db: OpaquePointer?
        precondition(sqlite3_open(path, &db) == SQLITE_OK, "open failed")
        defer { sqlite3_close(db) }
        exec(db!, "PRAGMA journal_mode = WAL;")
        setup(db!)
        exec(db!, "PRAGMA user_version = \(schemaVersion);")
        return path
    }

    /// A database carrying the full v6 schema subset the app reads, with one
    /// source, several sessions/events, and one challenged + one rejected
    /// mutation candidate (with evidence links and transitions).
    static func populated() -> String {
        make(schemaVersion: 8) { db in
            createSchema(db)
            exec(db, """
            INSERT INTO schema_migrations(version, description, checksum, applied_at) VALUES
              (1,'initial',zeroblob(32),'2026-07-16T00:00:00Z'),
              (6,'retrieval',zeroblob(32),'2026-07-16T00:00:00Z'),
              (7,'claude_code_install',zeroblob(32),'2026-07-16T00:00:00Z'),
              (8,'mutation_provenance',zeroblob(32),'2026-07-16T00:00:00Z');
            """)
            exec(db, """
            INSERT INTO sources(source_id, adapter, instance_key, display_name,
                                first_seen_at, last_seen_at)
            VALUES (1,'generic-jsonl','demo','Demo',
                    '2026-07-16T10:00:00Z','2026-07-16T10:10:00Z');
            """)
            exec(db, """
            INSERT INTO sessions(session_id, source_id, project_path, first_event_at,
                                 last_event_at, event_count)
            VALUES
              ('ses_a',1,'/workspace/a','2026-07-16T10:00:00Z','2026-07-16T10:02:00Z',2),
              ('ses_b',1,'/workspace/b','2026-07-16T10:05:00Z','2026-07-16T10:05:00Z',1);
            """)
            exec(db, """
            INSERT INTO events(row_id, event_id, spec_version, session_id, occurred_at,
                               sequence, event_type, tool_name, exit_code, event_json,
                               content_hash, imported_at)
            VALUES
              (1,'evt_a1','aep/1','ses_a','2026-07-16T10:00:00Z',0,'tool.failed','shell',1,
               '{}',zeroblob(32),'2026-07-16T10:00:00Z'),
              (2,'evt_a2','aep/1','ses_a','2026-07-16T10:02:00Z',1,'tool.completed','shell',0,
               '{}',zeroblob(32),'2026-07-16T10:02:00Z'),
              (3,'evt_b1','aep/1','ses_b','2026-07-16T10:05:00Z',0,'tool.failed','shell',1,
               '{}',zeroblob(32),'2026-07-16T10:05:00Z');
            """)
            exec(db, "INSERT INTO events_search(event_row_id, searchable_text) VALUES (1,'cargo build'),(2,''),(3,'cargo build');")

            let package = fixturePackageJSON
            exec(db, """
            INSERT INTO mutation_candidates(mutation_id, source_finding_id, source_detector,
                equivalence_key, spec_version, semantic_version, state, package_json,
                content_hash, created_at, updated_at)
            VALUES
              ('mut_1','fnd_1','repeated_command_failure','eq1','mutation/0.1','0.1.0',
               'challenged','\(package)',zeroblob(32),'2026-07-16T11:00:00Z','2026-07-16T11:05:00Z'),
              ('mut_2','fnd_2','repeated_user_correction','eq2','mutation/0.1','0.1.0',
               'rejected','{}',zeroblob(32),'2026-07-16T11:10:00Z','2026-07-16T11:12:00Z');
            """)
            exec(db, "UPDATE mutation_candidates SET rejection_reason='trigger too broad' WHERE mutation_id='mut_2';")
            exec(db, """
            INSERT INTO mutation_evidence(mutation_id, event_id, role, ordinal) VALUES
              ('mut_1','evt_a1','support',0),
              ('mut_1','evt_b1','support',1),
              ('mut_1','evt_a2','counterexample',0);
            """)
            exec(db, """
            INSERT INTO mutation_transitions(transition_id, mutation_id, from_state, to_state,
                reason, metadata_json, occurred_at) VALUES
              (1,'mut_1',NULL,'candidate','generated from evidence','{}','2026-07-16T11:00:00Z'),
              (2,'mut_1','candidate','challenged','challenge checklist completed',
               '{"note":"ok"}','2026-07-16T11:05:00Z');
            """)
        }
    }

    // The subset of tables the app reads. Column definitions mirror the
    // authoritative migrations; unrelated constraints are relaxed for brevity.
    static func createSchema(_ db: OpaquePointer) {
        createFoundationSchema(db)
        exec(db, "CREATE TABLE event_conflicts(conflict_id INTEGER PRIMARY KEY, event_id TEXT);")
        exec(db, "CREATE TABLE event_signatures(event_row_id INTEGER PRIMARY KEY, signature TEXT);")
        exec(db, "CREATE TABLE mutation_candidates(mutation_id TEXT PRIMARY KEY, source_finding_id TEXT, source_detector TEXT, equivalence_key TEXT, spec_version TEXT, semantic_version TEXT, state TEXT, package_json TEXT, content_hash BLOB, challenge_json TEXT, rejection_reason TEXT, created_at TEXT, updated_at TEXT);")
        exec(db, "CREATE TABLE mutation_evidence(mutation_id TEXT, event_id TEXT, role TEXT, ordinal INTEGER);")
        exec(db, "CREATE TABLE mutation_transitions(transition_id INTEGER PRIMARY KEY, mutation_id TEXT, from_state TEXT, to_state TEXT, reason TEXT, metadata_json TEXT, occurred_at TEXT);")
        exec(db, "CREATE TABLE mutation_replays(replay_id TEXT PRIMARY KEY, mutation_id TEXT, scenario_set_hash TEXT, report_json TEXT, content_hash BLOB, passed INTEGER, created_at TEXT);")
        exec(db, "CREATE TABLE mutation_shadows(shadow_id TEXT PRIMARY KEY, mutation_id TEXT, observation_set_hash TEXT, report_json TEXT, content_hash BLOB, passed INTEGER, created_at TEXT);")
        exec(db, "CREATE TABLE mutation_installations(installation_id TEXT PRIMARY KEY, mutation_id TEXT, target TEXT, repository_root TEXT, relative_path TEXT, content_hash TEXT, permission_review_json TEXT, state TEXT, installed_at TEXT, uninstalled_at TEXT);")
    }

    /// Only the tables that existed in the earliest migrations (sources,
    /// sessions, events, and their search projection). Deliberately omits the
    /// mutation registry, conflict, and signature tables added by later
    /// migrations, so the reader's later-table guards can be exercised.
    static func createFoundationSchema(_ db: OpaquePointer) {
        exec(db, "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, description TEXT, checksum BLOB, applied_at TEXT);")
        exec(db, "CREATE TABLE sources(source_id INTEGER PRIMARY KEY, adapter TEXT, instance_key TEXT, display_name TEXT, first_seen_at TEXT, last_seen_at TEXT);")
        exec(db, "CREATE TABLE sessions(session_id TEXT PRIMARY KEY, source_id INTEGER, project_path TEXT, started_at TEXT, ended_at TEXT, first_event_at TEXT, last_event_at TEXT, event_count INTEGER, metadata_json TEXT DEFAULT '{}');")
        exec(db, "CREATE TABLE events(row_id INTEGER PRIMARY KEY, event_id TEXT UNIQUE, spec_version TEXT, session_id TEXT, occurred_at TEXT, sequence INTEGER, event_type TEXT, project_path TEXT, parent_event_id TEXT, tool_name TEXT, tool_input_text TEXT, exit_code INTEGER, event_json TEXT, content_hash BLOB, imported_at TEXT);")
        exec(db, "CREATE TABLE events_search(event_row_id INTEGER PRIMARY KEY, project_path TEXT, tool_name TEXT, tool_input_text TEXT, searchable_text TEXT DEFAULT '');")
    }

    /// A foundation-only (v2-era) database with one source, one session, and
    /// two events — but none of the later mutation/conflict/signature tables.
    static func foundationOnly() -> String {
        make(schemaVersion: 2) { db in
            createFoundationSchema(db)
            exec(db, """
            INSERT INTO schema_migrations(version, description, checksum, applied_at)
            VALUES (2,'source_cursors',zeroblob(32),'2026-07-16T00:00:00Z');
            """)
            exec(db, """
            INSERT INTO sources(source_id, adapter, instance_key, display_name,
                                first_seen_at, last_seen_at)
            VALUES (1,'generic-jsonl','demo','Demo',
                    '2026-07-16T10:00:00Z','2026-07-16T10:02:00Z');
            """)
            exec(db, """
            INSERT INTO sessions(session_id, source_id, project_path, first_event_at,
                                 last_event_at, event_count)
            VALUES ('ses_a',1,'/workspace/a','2026-07-16T10:00:00Z','2026-07-16T10:02:00Z',2);
            """)
            exec(db, """
            INSERT INTO events(row_id, event_id, spec_version, session_id, occurred_at,
                               sequence, event_type, tool_name, exit_code, event_json,
                               content_hash, imported_at)
            VALUES
              (1,'evt_a1','aep/1','ses_a','2026-07-16T10:00:00Z',0,'tool.failed','shell',1,
               '{}',zeroblob(32),'2026-07-16T10:00:00Z'),
              (2,'evt_a2','aep/1','ses_a','2026-07-16T10:02:00Z',1,'tool.completed','shell',0,
               '{}',zeroblob(32),'2026-07-16T10:02:00Z');
            """)
        }
    }

    static func exec(_ db: OpaquePointer, _ sql: String) {
        var error: UnsafeMutablePointer<CChar>?
        if sqlite3_exec(db, sql, nil, nil, &error) != SQLITE_OK {
            let message = error.map { String(cString: $0) } ?? "unknown"
            sqlite3_free(error)
            fatalError("fixture SQL failed: \(message)\nSQL: \(sql)")
        }
    }

    /// A valid Mutation Package v0.1 payload as raw JSON.
    static let unescapedFixturePackageJSON: String = """
    {"mutation_id":"mut_1","title":"Prevent repeated command failure: shell: cargo build",
     "version":"0.1.0","spec_version":"mutation/0.1",
     "source_detector":"repeated_command_failure","source_finding_id":"fnd_1",
     "hypothesis":{"statement":"cargo build fails repeatedly",
       "expected_result":"cargo build succeeds after preflight",
       "failure_cases":["transient failures"],
       "supporting_event_ids":["evt_a1","evt_b1"],
       "counterexample_event_ids":["evt_a2"]},
     "intervention":{"type":"agent_instruction","instruction":"Run codegen before build."},
     "triggers":[{"type":"tool_call","selector":"operation/v1|shell|cargo build|exit:1"}],
     "exclusions":["Skip when inputs differ."],
     "permissions":{"filesystem_read":[],"filesystem_write":[],"commands":[],
       "environment":[],"network":false}}
    """

    /// The same payload, single-quote-escaped for embedding in a SQL literal.
    static let fixturePackageJSON: String = unescapedFixturePackageJSON
        .replacingOccurrences(of: "\n", with: " ")
        .replacingOccurrences(of: "'", with: "''")
}

/// Deletes a fixture database (and its WAL/SHM siblings) when it goes out of use.
final class TempPath {
    let path: String
    init(_ path: String) { self.path = path }
    deinit {
        for suffix in ["", "-wal", "-shm"] {
            try? FileManager.default.removeItem(atPath: path + suffix)
        }
    }
}
