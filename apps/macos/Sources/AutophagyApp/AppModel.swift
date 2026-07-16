import AutophagyKit
import Foundation
import SwiftUI

/// The primary navigation destinations.
enum Section: String, CaseIterable, Identifiable {
    case sessions = "Sessions"
    case patterns = "Patterns"
    case mutations = "Mutations"
    case privacy = "Privacy"

    var id: String { rawValue }

    var systemImage: String {
        switch self {
        case .sessions: "list.bullet.rectangle"
        case .patterns: "waveform.path.ecg"
        case .mutations: "arrow.triangle.branch"
        case .privacy: "lock.shield"
        }
    }
}

/// Observable application state. All database reads run synchronously on the
/// main actor: the database is local and the app is read-only, so there is no
/// long-running work to move off the main thread.
@MainActor
final class AppModel: ObservableObject {
    /// The currently open database, or `nil` before onboarding completes.
    @Published private(set) var reader: DatabaseReader?
    @Published private(set) var overview: DatabaseOverview?
    @Published private(set) var openError: String?
    @Published var selectedSection: Section = .sessions

    // Loaded content for the open database.
    @Published private(set) var sessions: [SessionSummary] = []
    @Published private(set) var patterns: [EvidencePacket] = []
    @Published private(set) var mutations: [MutationSummary] = []

    /// The always-available menu-bar summary, refreshed alongside content.
    @Published private(set) var menuBar: MenuBarSnapshot = .disconnected

    private let selection = DatabaseSelection()

    /// The path that should be opened on launch, if any.
    var startupPath: String? { selection.resolveStartupPath() }

    /// The CLI's default database location, for display in onboarding.
    var defaultDatabasePath: String { DatabaseLocator.defaultDatabaseURL().path }

    /// Whether a database is currently open.
    var hasOpenDatabase: Bool { reader != nil }

    /// Attempt to open `path`, validate it, and load content.
    ///
    /// A file that opens but is not an Autophagy database is rejected without
    /// being remembered, so onboarding can show a clear message.
    @discardableResult
    func open(path: String, remember: Bool = true) -> Bool {
        openError = nil
        do {
            let reader = try DatabaseReader(path: path)
            guard reader.isAutophagyDatabase() else {
                openError = "\(path)\n\nThis file is not an Autophagy database "
                    + "(no migration ledger or session tables)."
                return false
            }
            self.reader = reader
            overview = reader.overview()
            if remember {
                selection.selectedPath = path
            }
            reload()
            return true
        } catch {
            openError = "\(path)\n\n\(error)"
            return false
        }
    }

    /// Re-run all queries against the open database.
    ///
    /// Re-opens the connection first so the read reflects the current on-disk
    /// state: a cleanly checkpointed database is opened as a frozen `immutable`
    /// snapshot, so without re-opening a reload would never surface rows written
    /// since the reader was created (e.g. after a CLI import).
    func reload() {
        guard let reader else { return }
        reader.refresh()
        let currentOverview = reader.overview()
        overview = currentOverview
        sessions = reader.sessions()
        patterns = reader.evidencePackets()
        mutations = reader.mutations()
        menuBar = reader.menuBarSnapshot(overview: currentOverview)
    }

    /// Refresh only the menu-bar summary. Cheap enough to call on menu open
    /// without reloading every view's content. Re-opens the connection so the
    /// summary advances past the initial snapshot.
    func refreshMenuBar() {
        guard let reader else {
            menuBar = .disconnected
            return
        }
        reader.refresh()
        menuBar = reader.menuBarSnapshot()
    }

    /// Return to onboarding and forget the remembered choice.
    func closeDatabase() {
        selection.selectedPath = nil
        reader = nil
        overview = nil
        sessions = []
        patterns = []
        mutations = []
        menuBar = .disconnected
    }

    /// Load the ordered event timeline for a session.
    func events(inSession id: String) -> [EventRow] {
        reader?.events(inSession: id) ?? []
    }

    /// Load full detail for one mutation.
    func mutationDetail(id: String) -> MutationDetail? {
        reader?.mutationDetail(id: id)
    }

    /// Open the default database at launch if the app has a remembered/present
    /// choice. Safe to call once from `.onAppear`.
    func openStartupDatabaseIfAvailable() {
        guard reader == nil, let path = startupPath else { return }
        open(path: path, remember: false)
    }
}
