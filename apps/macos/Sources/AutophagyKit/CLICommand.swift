import Foundation

/// A destructive operation the app can only perform by delegating to the CLI.
///
/// The app holds **no write path of its own**. Deletion is expressed as an
/// exact `autophagy` command that is shown to the user before it runs, and is
/// executed via `Process` only after explicit multi-step confirmation. If the
/// CLI binary cannot be found, the command is displayed for the user to run.
public enum DestructiveAction: Equatable, Sendable {
    /// Delete a single session and everything that cascades from it.
    case deleteSession(sessionID: String)
    /// Delete all local data.
    case deleteAll

    /// A short human label.
    public var title: String {
        switch self {
        case .deleteSession: "Delete session"
        case .deleteAll: "Delete all local data"
        }
    }

    /// A one-line description of the consequence.
    public var consequence: String {
        switch self {
        case let .deleteSession(id):
            "Permanently removes session \(id) and its events, evidence, and any "
                + "mutation candidates that cite them. This cannot be undone."
        case .deleteAll:
            "Permanently removes every session, event, source, import record, and "
                + "mutation candidate in this database. This cannot be undone."
        }
    }

    /// The confirmation phrase the user must type for `delete all`, mirroring
    /// the CLI's own `--confirm delete-all` guard.
    public var confirmationPhrase: String? {
        switch self {
        case .deleteSession: nil
        case .deleteAll: "delete-all"
        }
    }
}

/// Builds and (optionally) runs CLI commands for destructive actions.
public struct CLICommand: Equatable, Sendable {
    /// The resolved executable name or path (`autophagy` when only on `PATH`).
    public let executable: String
    /// The full argument vector, executable first — the exact command shown.
    public let arguments: [String]
    /// Whether an `autophagy` binary was actually located on disk.
    public let executableFound: Bool

    /// Build the command for `action` against `databasePath`.
    public init(action: DestructiveAction, databasePath: String, locator: CLILocator = .init()) {
        let resolved = locator.locate()
        executable = resolved ?? "autophagy"
        executableFound = resolved != nil

        var args = [executable, "--database", databasePath]
        switch action {
        case let .deleteSession(sessionID):
            args += ["delete", "session", sessionID]
        case .deleteAll:
            args += ["delete", "all", "--confirm", "delete-all"]
        }
        arguments = args
    }

    /// The command rendered as a copy-pasteable shell line.
    public var displayString: String {
        arguments.map(Self.shellQuote).joined(separator: " ")
    }

    private static func shellQuote(_ argument: String) -> String {
        if argument.allSatisfy({ $0.isLetter || $0.isNumber || "-_./:".contains($0) }) {
            return argument
        }
        return "'" + argument.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}

/// Locates the `autophagy` CLI binary without shelling out.
public struct CLILocator {
    private let fileManager: FileManager
    private let pathVariable: String
    private let extraCandidates: [String]

    public init(
        fileManager: FileManager = .default,
        pathVariable: String = ProcessInfo.processInfo.environment["PATH"] ?? "",
        extraCandidates: [String] = [
            "/usr/local/bin/autophagy",
            "/opt/homebrew/bin/autophagy"
        ]
    ) {
        self.fileManager = fileManager
        self.pathVariable = pathVariable
        self.extraCandidates = extraCandidates
    }

    /// The first executable `autophagy` found on `PATH` or in a common location.
    public func locate() -> String? {
        let fromPath = pathVariable
            .split(separator: ":", omittingEmptySubsequences: true)
            .map { URL(fileURLWithPath: String($0)).appendingPathComponent("autophagy").path }
        for candidate in fromPath + extraCandidates where isExecutable(candidate) {
            return candidate
        }
        return nil
    }

    private func isExecutable(_ path: String) -> Bool {
        fileManager.isExecutableFile(atPath: path)
    }
}
