import Foundation
import SQLite3
import Testing
@testable import AutophagyKit

@Suite("Database reader")
struct DatabaseReaderTests {
    @Test func recognisesPopulatedDatabase() throws {
        let temp = TempPath(FixtureDatabase.populated())
        let reader = try DatabaseReader(path: temp.path)
        #expect(reader.isAutophagyDatabase())
        #expect(reader.schemaInfo().compatibility == .supported(version: 8))
    }

    @Test func readsSessionsWithSourceMetadata() throws {
        let temp = TempPath(FixtureDatabase.populated())
        let reader = try DatabaseReader(path: temp.path)
        let sessions = reader.sessions()
        #expect(sessions.count == 2)
        // Ordered by last_event_at DESC -> ses_b first.
        #expect(sessions.first?.id == "ses_b")
        let a = try #require(sessions.first { $0.id == "ses_a" })
        #expect(a.adapter == "generic-jsonl")
        #expect(a.instanceKey == "demo")
        #expect(a.projectPath == "/workspace/a")
        #expect(a.eventCount == 2)
    }

    @Test func readsOrderedEventTimelineWithExactIDs() throws {
        let temp = TempPath(FixtureDatabase.populated())
        let reader = try DatabaseReader(path: temp.path)
        let events = reader.events(inSession: "ses_a")
        #expect(events.map(\.id) == ["evt_a1", "evt_a2"])
        #expect(events.first?.eventType == "tool.failed")
        #expect(events.first?.exitCode == 1)
        #expect(events.first?.toolName == "shell")
    }

    @Test func evidencePacketsExposeExactEventIDs() throws {
        let temp = TempPath(FixtureDatabase.populated())
        let reader = try DatabaseReader(path: temp.path)
        let packets = reader.evidencePackets()
        let packet = try #require(packets.first { $0.id == "fnd_1" })
        #expect(packet.detector == "repeated_command_failure")
        #expect(packet.supportingEventIDs == ["evt_a1", "evt_b1"])
        #expect(packet.counterexampleEventIDs == ["evt_a2"])
    }

    @Test func mutationDetailCarriesLineageAuditAndPackage() throws {
        let temp = TempPath(FixtureDatabase.populated())
        let reader = try DatabaseReader(path: temp.path)
        let detail = try #require(reader.mutationDetail(id: "mut_1"))
        #expect(detail.summary.state == "challenged")
        #expect(detail.package?.intervention.type == "agent_instruction")
        #expect(detail.package?.permissions.isZeroPermission == true)

        let support = detail.evidence.filter { $0.role == "support" }
        #expect(support.map(\.eventID) == ["evt_a1", "evt_b1"])
        let allPresent = support.allSatisfy(\.eventPresent)
        #expect(allPresent)

        #expect(detail.transitions.map(\.toState) == ["candidate", "challenged"])
        #expect(detail.rawPackageJSON.contains("mutation_id"))
    }

    @Test func rejectedMutationExposesReason() throws {
        let temp = TempPath(FixtureDatabase.populated())
        let reader = try DatabaseReader(path: temp.path)
        let detail = try #require(reader.mutationDetail(id: "mut_2"))
        #expect(detail.summary.state == "rejected")
        #expect(detail.rejectionReason == "trigger too broad")
    }

    @Test func overviewCounts() throws {
        let temp = TempPath(FixtureDatabase.populated())
        let reader = try DatabaseReader(path: temp.path)
        let overview = reader.overview()
        #expect(overview.sessionCount == 2)
        #expect(overview.eventCount == 3)
        #expect(overview.mutationCount == 2)
        #expect(overview.sourceCount == 1)
        #expect(overview.hasSignatureIndex)
        #expect(overview.indexedFreeTextRows == 2) // two rows have non-empty text
    }

    @Test func missingEventPresentsAsAbsentLink() throws {
        // Delete a cited event directly in the fixture to simulate an orphan.
        let path = FixtureDatabase.populated()
        let temp = TempPath(path)
        var db: OpaquePointer?
        #expect(sqlite3_open(path, &db) == SQLITE_OK)
        FixtureDatabase.exec(db!, "DELETE FROM events WHERE event_id='evt_b1';")
        sqlite3_close(db)

        let reader = try DatabaseReader(path: temp.path)
        let detail = try #require(reader.mutationDetail(id: "mut_1"))
        let orphan = try #require(detail.evidence.first { $0.eventID == "evt_b1" })
        #expect(!orphan.eventPresent)
    }
}
