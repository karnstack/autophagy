import Foundation
import SQLite3
import Testing
@testable import AutophagyKit

@Suite("Schema tolerance and read-only guarantee")
struct SchemaAndReadonlyTests {
    @Test func olderSchemaIsReadable() throws {
        let temp = TempPath(FixtureDatabase.make(schemaVersion: 3) { db in
            FixtureDatabase.createSchema(db)
            FixtureDatabase.exec(db, """
            INSERT INTO schema_migrations(version, description, checksum, applied_at)
            VALUES (3,'mutation_registry',zeroblob(32),'2026-07-16T00:00:00Z');
            """)
        })
        let reader = try DatabaseReader(path: temp.path)
        #expect(reader.schemaInfo().compatibility
            == .olderReadable(version: 3, known: knownSchemaVersion))
        // No sessions, but the call must not throw.
        #expect(reader.sessions().isEmpty)
    }

    @Test func newerSchemaIsReadableAndFlagged() throws {
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
        // A read-only + query_only connection must refuse writes.
        #expect(throws: SQLiteError.self) {
            try db.execute("DELETE FROM sessions;")
        }
    }
}
