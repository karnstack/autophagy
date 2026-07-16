import AppKit
import AutophagyKit
import SwiftUI

/// The read-only menu-bar panel: connection state, quick stats, the most recent
/// candidates, and actions to open the main window, refresh, or quit. It reads
/// the snapshot the model assembles from cheap COUNT queries and refreshes on
/// open — there is no daemon and nothing is written or sent anywhere.
struct MenuBarView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openWindow) private var openWindow

    private var snapshot: MenuBarSnapshot { model.menuBar }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            connection
            Divider()
            if snapshot.isConnected {
                stats
                recent
            } else {
                Text("Open a database from the main window to see live stats.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Divider()
            actions
        }
        .padding(14)
        .frame(width: 320)
        .onAppear { model.refreshMenuBar() }
    }

    // MARK: - Connection

    private var connection: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Circle()
                    .fill(connectionColor)
                    .frame(width: 8, height: 8)
                Text(snapshot.isConnected ? "Connected" : "No database open")
                    .font(.headline)
            }
            if let name = snapshot.databaseFileName {
                Text(name)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            if let schema = snapshot.schema {
                Text(schema.summary)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var connectionColor: Color {
        guard let schema = snapshot.schema else { return .secondary }
        switch schema {
        case .supported: return .green
        case .olderReadable, .newerThanKnown: return .orange
        case .notAutophagy: return .red
        }
    }

    // MARK: - Stats

    private var stats: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 18) {
                MenuStat(label: "Sessions", value: snapshot.sessionCount)
                MenuStat(label: "Events", value: snapshot.eventCount)
                MenuStat(label: "Candidates", value: snapshot.candidateCount)
            }
            if !snapshot.stateCounts.isEmpty {
                VStack(alignment: .leading, spacing: 3) {
                    Text("BY STATE")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                    ForEach(snapshot.stateCounts) { entry in
                        HStack {
                            StateBadge(state: entry.state)
                            Spacer()
                            Text("\(entry.count)")
                                .font(.caption.monospacedDigit())
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }
        }
    }

    // MARK: - Recent candidates

    @ViewBuilder
    private var recent: some View {
        if !snapshot.recentCandidates.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text("RECENT CANDIDATES")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
                ForEach(snapshot.recentCandidates) { candidate in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(candidate.title)
                            .font(.callout)
                            .lineLimit(1)
                            .truncationMode(.tail)
                        HStack(spacing: 6) {
                            StateBadge(state: candidate.state)
                            Text(candidate.detector)
                                .font(.caption2)
                                .foregroundStyle(.tertiary)
                                .lineLimit(1)
                        }
                    }
                }
            }
        }
    }

    // MARK: - Actions

    private var actions: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                openMainWindow()
            } label: {
                Label("Open Autophagy", systemImage: "macwindow")
            }
            Button {
                model.reload()
            } label: {
                Label("Refresh", systemImage: "arrow.clockwise")
            }
            .disabled(!snapshot.isConnected)
            Divider()
            Button {
                NSApplication.shared.terminate(nil)
            } label: {
                Label("Quit Autophagy", systemImage: "power")
            }
        }
        .buttonStyle(.plain)
        .font(.callout)
    }

    private func openMainWindow() {
        NSApplication.shared.activate(ignoringOtherApps: true)
        openWindow(id: AutophagyApp.mainWindowID)
    }
}

/// A compact labelled count for the menu-bar stat row.
private struct MenuStat: View {
    let label: String
    let value: Int
    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text("\(value)")
                .font(.title3.bold().monospacedDigit())
            Text(label)
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
    }
}
