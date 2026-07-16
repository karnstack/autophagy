import Foundation
import Testing
@testable import AutophagyKit

@Suite("Supporting types")
struct SupportingTypesTests {
    // MARK: - DatabaseLocator

    @Test func defaultDatabasePathMatchesCLILayout() {
        let home = URL(fileURLWithPath: "/Users/example")
        let url = DatabaseLocator.defaultDatabaseURL(home: home)
        #expect(url.path
            == "/Users/example/Library/Application Support/sh.autophagy.Autophagy/autophagy.db")
    }

    // MARK: - CLICommand

    @Test func deleteSessionCommand() {
        let command = CLICommand(
            action: .deleteSession(sessionID: "ses_a"),
            databasePath: "/tmp/demo.db",
            locator: CLILocator(pathVariable: "", extraCandidates: [])
        )
        #expect(!command.executableFound)
        #expect(command.arguments
            == ["autophagy", "--database", "/tmp/demo.db", "delete", "session", "ses_a"])
        #expect(command.displayString == "autophagy --database /tmp/demo.db delete session ses_a")
    }

    @Test func deleteAllCommandCarriesConfirmGuard() {
        let command = CLICommand(
            action: .deleteAll,
            databasePath: "/tmp/demo.db",
            locator: CLILocator(pathVariable: "", extraCandidates: [])
        )
        #expect(Array(command.arguments.suffix(4)) == ["delete", "all", "--confirm", "delete-all"])
        #expect(DestructiveAction.deleteAll.confirmationPhrase == "delete-all")
        #expect(DestructiveAction.deleteSession(sessionID: "x").confirmationPhrase == nil)
    }

    @Test func displayStringQuotesPathsWithSpaces() {
        let command = CLICommand(
            action: .deleteAll,
            databasePath: "/tmp/my db.db",
            locator: CLILocator(pathVariable: "", extraCandidates: [])
        )
        #expect(command.displayString.contains("'/tmp/my db.db'"))
    }

    @Test func locatorFindsExecutableCandidate() throws {
        // Create a fake executable and confirm the locator resolves it.
        let dir = NSTemporaryDirectory() + "cli-\(UUID().uuidString)"
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(atPath: dir) }
        let bin = dir + "/autophagy"
        FileManager.default.createFile(
            atPath: bin,
            contents: Data("#!/bin/sh\n".utf8),
            attributes: [.posixPermissions: 0o755]
        )
        let locator = CLILocator(pathVariable: dir, extraCandidates: [])
        #expect(locator.locate() == bin)
    }

    // MARK: - MutationPackage

    @Test func packageDecodesZeroPermission() throws {
        let package = try #require(
            MutationPackage.decode(from: FixtureDatabase.unescapedFixturePackageJSON)
        )
        #expect(package.title == "Prevent repeated command failure: shell: cargo build")
        #expect(package.permissions.isZeroPermission)
        #expect(package.hypothesis.supportingEventIDs == ["evt_a1", "evt_b1"])
    }

    @Test func packageDecodeReturnsNilForGarbage() {
        #expect(MutationPackage.decode(from: "{ not json") == nil)
    }

    @Test func v01PackageHasNoProvenance() throws {
        let package = try #require(
            MutationPackage.decode(from: FixtureDatabase.unescapedFixturePackageJSON)
        )
        #expect(package.provenance == nil)
    }

    @Test func v02PackageDecodesProvenance() throws {
        let package = try #require(
            MutationPackage.decode(from: FixtureDatabase.modelSynthesizedPackageJSON)
        )
        let provenance = try #require(package.provenance)
        #expect(provenance.provider == "ollama")
        #expect(provenance.modelName == "qwen3-coder:30b")
        #expect(provenance.modelRevision == "30b-a3b")
        #expect(provenance.manifestSpecVersion == "synthesis-manifest/0.2")
        #expect(provenance.modelDigest?.hasPrefix("sha256:") == true)
    }

    // MARK: - InstallationRecord target display

    @Test func installationTargetDisplayNames() {
        func record(_ target: String) -> InstallationRecord {
            InstallationRecord(
                id: "ins", target: target, repositoryRoot: "/r", relativePath: "p",
                state: "installed", installedAt: "t", uninstalledAt: nil
            )
        }
        #expect(record("codex_repo_skill").targetDisplayName == "Codex repo skill")
        #expect(record("claude_code_repo_skill").targetDisplayName == "Claude Code repo skill")
        // Unknown targets pass through verbatim.
        #expect(record("future_target").targetDisplayName == "future_target")
    }

    // MARK: - AppSettings

    @Test func appSettingsMenuBarOnlyDefaultsFalseAndPersists() throws {
        let defaults = try ephemeralDefaults()
        let settings = AppSettings(defaults: defaults)
        #expect(settings.menuBarOnly == false)
        settings.menuBarOnly = true
        #expect(AppSettings(defaults: defaults).menuBarOnly == true)
    }

    // MARK: - DatabaseSelection

    @Test func startupPathPrefersRememberedExistingFile() throws {
        let selection = DatabaseSelection(defaults: try ephemeralDefaults())
        selection.selectedPath = "/remembered.db"
        let resolved = selection.resolveStartupPath(
            defaultExists: true,
            fileExists: { $0 == "/remembered.db" }
        )
        #expect(resolved == "/remembered.db")
    }

    @Test func startupPathFallsBackToDefaultWhenRememberedMissing() throws {
        let selection = DatabaseSelection(defaults: try ephemeralDefaults())
        selection.selectedPath = "/gone.db"
        let resolved = selection.resolveStartupPath(
            defaultExists: true,
            fileExists: { _ in false }
        )
        #expect(resolved == DatabaseLocator.defaultDatabaseURL().path)
    }

    @Test func startupPathNilWhenNothingAvailable() throws {
        let selection = DatabaseSelection(defaults: try ephemeralDefaults())
        let resolved = selection.resolveStartupPath(
            defaultExists: false,
            fileExists: { _ in false }
        )
        #expect(resolved == nil)
    }

    private func ephemeralDefaults() throws -> UserDefaults {
        let suite = "test-\(UUID().uuidString)"
        return try #require(UserDefaults(suiteName: suite))
    }
}
