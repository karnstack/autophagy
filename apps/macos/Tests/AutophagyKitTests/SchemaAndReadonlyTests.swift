import Foundation
import SQLite3
import Testing
@testable import AutophagyKit

@Suite("Schema tolerance and read-only guarantee")
struct SchemaAndReadonlyTests {
    @Test func notYetAdoptedLegacyV8IsReadableAndFlagged() throws {
        // Before the CLI adopts it, the one legacy database still carries the
        // full development-time v8 ledger. This build knows only the squashed v1
        // baseline, so it classifies the database as newer-than-known and reads
        // it read-only; the CLI rewrites the ledger to v1 on first touch.
        let temp = TempPath(FixtureDatabase.make(schemaVersion: 8) { db in
            FixtureDatabase.createSchema(db)
            FixtureDatabase.exec(db, """
            INSERT INTO schema_migrations(version, description, checksum, applied_at) VALUES
              (1,'initial event store',zeroblob(32),'2026-07-16T00:00:00Z'),
              (8,'accept mutation/0.2 provenance packages',zeroblob(32),'2026-07-16T00:00:00Z');
            """)
        })
        let reader = try DatabaseReader(path: temp.path)
        #expect(reader.schemaInfo().compatibility
            == .newerThanKnown(version: 8, known: knownSchemaVersion))
        // No sessions, but the call must not throw.
        #expect(reader.sessions().isEmpty)
    }

    @Test func farNewerSchemaIsReadableAndFlagged() throws {
        let temp = TempPath(FixtureDatabase.make(schemaVersion: 99) { db in
            FixtureDatabase.createSchema(db)
            FixtureDatabase.exec(db, """
            INSERT INTO schema_migrations(version, description, checksum, applied_at)
            VALUES (99,'future',zeroblob(32),'2026-07-16T00:00:00Z');
            """)
        })
        let reader = try DatabaseReader(path: temp.path)
        #expect(reader.schemaInfo().compatibility
            == .newerThanKnown(version: 99, known: knownSchemaVersion))
    }

    @Test func missingTablesDegradeToEmpty() throws {
        // A database carrying only the foundational tables (sources, sessions,
        // events, search) but none of the mutation, conflict, or signature
        // tables. Its ledger reports a version this build does not know, so it is
        // flagged newer-than-known; the reader must still return the absent
        // tables as empty rather than throwing.
        let temp = TempPath(FixtureDatabase.foundationOnly())
        let reader = try DatabaseReader(path: temp.path)

        #expect(reader.isAutophagyDatabase())
        #expect(reader.schemaInfo().compatibility
            == .newerThanKnown(version: 2, known: knownSchemaVersion))

        // Foundational reads still work.
        #expect(reader.sessions().map(\.id) == ["ses_a"])
        #expect(reader.events(inSession: "ses_a").count == 2)

        // Later-migration reads degrade to empty, not a throw.
        #expect(reader.mutations().isEmpty)
        #expect(reader.evidencePackets().isEmpty)
        #expect(reader.mutationDetail(id: "mut_1") == nil)

        // Overview tolerates the absent tables.
        let overview = reader.overview()
        #expect(overview.sessionCount == 1)
        #expect(overview.eventCount == 2)
        #expect(overview.mutationCount == 0)
        #expect(overview.conflictCount == 0)
        #expect(!overview.hasSignatureIndex)
    }

    @Test func missingColumnInExistingTableDegradesToEmpty() throws {
        // mutation_candidates exists but predates the semantic_version column
        // the reader selects. The query fails internally and the reader yields
        // an empty registry rather than propagating the error.
        let temp = TempPath(FixtureDatabase.make(schemaVersion: 3) { db in
            FixtureDatabase.createFoundationSchema(db)
            FixtureDatabase.exec(db, """
            INSERT INTO schema_migrations(version, description, checksum, applied_at)
            VALUES (3,'mutation_registry',zeroblob(32),'2026-07-16T00:00:00Z');
            """)
            // Note: no semantic_version / created_at / package_json columns.
            FixtureDatabase.exec(db, "CREATE TABLE mutation_candidates(mutation_id TEXT PRIMARY KEY, state TEXT);")
            FixtureDatabase.exec(db, "INSERT INTO mutation_candidates(mutation_id, state) VALUES ('mut_1','candidate');")
        })
        let reader = try DatabaseReader(path: temp.path)
        #expect(reader.mutations().isEmpty)
        #expect(reader.mutationDetail(id: "mut_1") == nil)
    }

    @Test func checkpointedWALWithoutSidecarsIsReadable() throws {
        // The engine leaves the database in WAL mode but removes the -wal and
        // -shm sidecars on a clean close. A read-only connection cannot create
        // the shared-memory index WAL needs, so without a fallback every query
        // fails and the file looks empty. The reader must still read it.
        let path = FixtureDatabase.populated()
        let temp = TempPath(path)
        for suffix in ["-wal", "-shm"] {
            try? FileManager.default.removeItem(atPath: path + suffix)
        }
        #expect(!FileManager.default.fileExists(atPath: path + "-wal"))

        let reader = try DatabaseReader(path: temp.path)
        #expect(reader.isAutophagyDatabase())
        #expect(reader.schemaInfo().compatibility == .supported(version: 1))
        #expect(reader.sessions().count == 2)
        #expect(reader.mutations().count == 2)
    }

    @Test func refreshSurfacesRowsWrittenAfterOpen() throws {
        // A cleanly checkpointed database is opened as a frozen immutable
        // snapshot. The reader must advance past that snapshot on refresh so a
        // viewer sees rows the engine wrote after the reader was created.
        let path = FixtureDatabase.populated()
        let temp = TempPath(path)
        for suffix in ["-wal", "-shm"] {
            try? FileManager.default.removeItem(atPath: path + suffix)
        }

        let reader = try DatabaseReader(path: temp.path)
        #expect(reader.sessions().count == 2)

        // A separate *writable* connection — the engine's role, never the app's
        // — adds a session, checkpoints, and leaves the file cleanly closed.
        var writer: OpaquePointer?
        #expect(sqlite3_open(path, &writer) == SQLITE_OK)
        FixtureDatabase.exec(writer!, """
        INSERT INTO sources(source_id, adapter, instance_key, display_name,
                            first_seen_at, last_seen_at)
        VALUES (2,'generic-jsonl','demo2','Demo2',
                '2026-07-16T12:00:00Z','2026-07-16T12:00:00Z');
        """)
        FixtureDatabase.exec(writer!, """
        INSERT INTO sessions(session_id, source_id, project_path, first_event_at,
                             last_event_at, event_count)
        VALUES ('ses_c',2,'/workspace/c','2026-07-16T12:00:00Z',
                '2026-07-16T12:00:00Z',0);
        """)
        FixtureDatabase.exec(writer!, "PRAGMA wal_checkpoint(TRUNCATE);")
        sqlite3_close(writer)
        for suffix in ["-wal", "-shm"] {
            try? FileManager.default.removeItem(atPath: path + suffix)
        }

        // The frozen snapshot still reads its original state until re-opened.
        reader.refresh()
        #expect(reader.sessions().count == 3)
    }

    @Test func nonAutophagyDatabaseIsRejectedNotCrashed() throws {
        let temp = TempPath(FixtureDatabase.make(schemaVersion: 0) { db in
            FixtureDatabase.exec(db, "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT);")
            FixtureDatabase.exec(db, "INSERT INTO notes(body) VALUES ('hello');")
        })
        let reader = try DatabaseReader(path: temp.path)
        #expect(!reader.isAutophagyDatabase())
        #expect(reader.schemaInfo().compatibility == .notAutophagy)
        #expect(reader.sessions().isEmpty)
    }

    @Test func openingMissingFileThrows() {
        #expect(throws: (any Error).self) {
            _ = try DatabaseReader(path: "/nonexistent/nope.db")
        }
    }

    @Test func connectionIsQueryOnly() throws {
        let temp = TempPath(FixtureDatabase.populated())
        let db = try Database(readonlyPath: temp.path)
        // The only public SQL path is `query`; a write attempted through it must
        // be refused by the read-only + query_only connection. (`execute` is
        // private, so no write API is reachable at all.)
        #expect(throws: SQLiteError.self) {
            _ = try db.query("DELETE FROM sessions;") { _ in 0 }
        }
    }
}
