import Foundation

/// The schema version this build of the app was written to read.
///
/// Corresponds to the highest immutable migration in
/// `crates/autophagy-store/migrations` at the time of writing. The migrations
/// were squashed to a single v1 baseline before the first release (see ADR
/// 0012). A not-yet-adopted legacy database still carrying the development-time
/// v8 ledger classifies as `newerThanKnown` and reads read-only; the CLI adopts
/// it to v1 on first touch.
public let knownSchemaVersion = 1

/// How the opened database's schema relates to what this app understands.
public enum SchemaCompatibility: Equatable, Sendable {
    /// Exactly the schema version this build targets.
    case supported(version: Int)
    /// An older-but-readable schema; newer tables will simply be absent.
    case olderReadable(version: Int, known: Int)
    /// A newer schema than this build knows; unknown columns/tables are
    /// ignored and some data may not be shown.
    case newerThanKnown(version: Int, known: Int)
    /// The file opened, but does not look like an Autophagy database.
    case notAutophagy

    /// A short, user-facing sentence describing the state.
    public var summary: String {
        switch self {
        case let .supported(version):
            "Schema v\(version) — fully supported."
        case let .olderReadable(version, known):
            "Schema v\(version) is older than this app (v\(known)). Readable; some views may be empty."
        case let .newerThanKnown(version, known):
            "Schema v\(version) is newer than this app (v\(known)). Read-only and safe, but some data may not be shown."
        case .notAutophagy:
            "This file does not look like an Autophagy database."
        }
    }

    /// Whether the database is usable at all (i.e. it is an Autophagy database).
    public var isReadable: Bool {
        self != .notAutophagy
    }
}

/// The result of inspecting a database's schema on open.
public struct SchemaInfo: Equatable {
    /// `PRAGMA user_version`.
    public let userVersion: Int
    /// `max(version)` from `schema_migrations`, or 0 when the table is absent.
    public let migrationVersion: Int
    /// Derived compatibility verdict.
    public let compatibility: SchemaCompatibility

    public init(userVersion: Int, migrationVersion: Int, compatibility: SchemaCompatibility) {
        self.userVersion = userVersion
        self.migrationVersion = migrationVersion
        self.compatibility = compatibility
    }
}

/// A source: one adapter + instance-key pairing that produced sessions.
public struct SourceInfo: Equatable, Identifiable {
    public let id: Int
    public let adapter: String
    public let instanceKey: String
    public let displayName: String?
}

/// One observed session, summarised for the list view.
public struct SessionSummary: Equatable, Identifiable {
    public let id: String
    public let adapter: String
    public let instanceKey: String
    public let projectPath: String?
    public let firstEventAt: String
    public let lastEventAt: String
    public let eventCount: Int
}

/// A single event in a session's timeline.
public struct EventRow: Equatable, Identifiable {
    public let id: String
    public let eventType: String
    public let toolName: String?
    public let occurredAt: String
    public let sequence: Int?
    public let exitCode: Int?
    public let parentEventID: String?
}

/// A deterministic finding preserved inside a registered mutation candidate.
///
/// Findings are not persisted as their own rows; each candidate embeds the
/// Evidence Packet that produced it. This view surfaces that packet with its
/// exact supporting and counterexample event IDs.
public struct EvidencePacket: Equatable, Identifiable {
    public let id: String            // source_finding_id
    public let detector: String      // source_detector
    public let title: String
    public let statement: String?
    public let expectedResult: String?
    public let supportingEventIDs: [String]
    public let counterexampleEventIDs: [String]
}

/// A mutation candidate summarised for the registry list.
public struct MutationSummary: Equatable, Identifiable, Sendable {
    public let id: String
    public let title: String
    public let state: String
    public let detector: String
    public let semanticVersion: String
    public let specVersion: String
    public let createdAt: String
    public let updatedAt: String
}

/// One lifecycle transition (the audit log).
public struct LifecycleTransition: Equatable, Identifiable {
    public let id: Int
    public let fromState: String?
    public let toState: String
    public let reason: String
    public let occurredAt: String
    public let metadataJSON: String
}

/// An evidence link (support or counterexample) for a mutation.
public struct EvidenceLink: Equatable, Identifiable {
    public var id: String { "\(role):\(ordinal):\(eventID)" }
    public let eventID: String
    public let role: String
    public let ordinal: Int
    /// Whether the linked event still exists locally.
    public let eventPresent: Bool
}

/// A persisted replay evaluation record (read-only).
public struct ReplayRecord: Equatable, Identifiable {
    public let id: String
    public let scenarioSetHash: String
    public let passed: Bool
    public let createdAt: String
}

/// A persisted shadow evaluation record (read-only).
public struct ShadowRecord: Equatable, Identifiable {
    public let id: String
    public let observationSetHash: String
    public let passed: Bool
    public let createdAt: String
}

/// A persisted filesystem installation record (read-only).
public struct InstallationRecord: Equatable, Identifiable {
    public let id: String
    public let target: String
    public let repositoryRoot: String
    public let relativePath: String
    public let state: String
    public let installedAt: String
    public let uninstalledAt: String?

    public init(
        id: String,
        target: String,
        repositoryRoot: String,
        relativePath: String,
        state: String,
        installedAt: String,
        uninstalledAt: String?
    ) {
        self.id = id
        self.target = target
        self.repositoryRoot = repositoryRoot
        self.relativePath = relativePath
        self.state = state
        self.installedAt = installedAt
        self.uninstalledAt = uninstalledAt
    }

    /// A friendly name for the install target: the stored value is one of
    /// `codex_repo_skill` (`.agents/skills/…`) or `claude_code_repo_skill`
    /// (`.claude/skills/…`). Unknown values pass through verbatim so a newer
    /// target added by the engine still displays honestly.
    public var targetDisplayName: String {
        switch target {
        case "codex_repo_skill": "Codex repo skill"
        case "claude_code_repo_skill": "Claude Code repo skill"
        default: target
        }
    }
}

/// The full detail for one mutation candidate.
public struct MutationDetail: Equatable {
    public let summary: MutationSummary
    public let package: MutationPackage?
    public let rawPackageJSON: String
    public let evidence: [EvidenceLink]
    public let transitions: [LifecycleTransition]
    public let replays: [ReplayRecord]
    public let shadows: [ShadowRecord]
    public let installation: InstallationRecord?
    public let rejectionReason: String?
}

/// A count of mutation candidates in one lifecycle state.
public struct MutationStateCount: Equatable, Identifiable, Sendable {
    public let state: String
    public let count: Int

    public var id: String { state }

    public init(state: String, count: Int) {
        self.state = state
        self.count = count
    }
}

/// The always-available menu-bar summary of a database.
///
/// This is a pure value assembled from cheap read-only queries. It is computed
/// off the reader (or `disconnected` when no database is open) so the menu-bar
/// UI stays a thin projection and the state-assembly logic is unit-testable
/// without a UI.
public struct MenuBarSnapshot: Equatable, Sendable {
    /// Whether a database is currently open.
    public let isConnected: Bool
    /// The open database's file name, for a compact menu label.
    public let databaseFileName: String?
    /// The open database's absolute path.
    public let databasePath: String?
    /// The schema compatibility verdict, when connected.
    public let schema: SchemaCompatibility?
    public let sessionCount: Int
    public let eventCount: Int
    /// Total registered mutation candidates.
    public let candidateCount: Int
    /// Candidate counts broken down by lifecycle state, state-ascending.
    public let stateCounts: [MutationStateCount]
    /// The most recent candidates (newest first), capped to a small limit.
    public let recentCandidates: [MutationSummary]

    public init(
        isConnected: Bool,
        databaseFileName: String?,
        databasePath: String?,
        schema: SchemaCompatibility?,
        sessionCount: Int,
        eventCount: Int,
        candidateCount: Int,
        stateCounts: [MutationStateCount],
        recentCandidates: [MutationSummary]
    ) {
        self.isConnected = isConnected
        self.databaseFileName = databaseFileName
        self.databasePath = databasePath
        self.schema = schema
        self.sessionCount = sessionCount
        self.eventCount = eventCount
        self.candidateCount = candidateCount
        self.stateCounts = stateCounts
        self.recentCandidates = recentCandidates
    }

    /// The state shown before any database is open.
    public static let disconnected = MenuBarSnapshot(
        isConnected: false,
        databaseFileName: nil,
        databasePath: nil,
        schema: nil,
        sessionCount: 0,
        eventCount: 0,
        candidateCount: 0,
        stateCounts: [],
        recentCandidates: []
    )
}

/// High-level counts and posture for the privacy view.
public struct DatabaseOverview: Equatable {
    public let databasePath: String
    public let schema: SchemaInfo
    public let sourceCount: Int
    public let sessionCount: Int
    public let eventCount: Int
    public let mutationCount: Int
    public let conflictCount: Int
    public let hasFTS: Bool
    public let hasSignatureIndex: Bool
    public let indexedFreeTextRows: Int
}
