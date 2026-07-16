import AutophagyKit
import SwiftUI

/// Session list with an event-timeline drill-down.
struct SessionsView: View {
    @EnvironmentObject private var model: AppModel
    @State private var selection: SessionSummary.ID?
    @State private var deletionTarget: SessionSummary?

    private var selectedSession: SessionSummary? {
        model.sessions.first { $0.id == selection }
    }

    var body: some View {
        HSplitView {
            sessionList
                .frame(minWidth: 320, idealWidth: 380)
            timeline
                .frame(minWidth: 380)
        }
        .sheet(item: $deletionTarget) { session in
            DestructiveActionSheet(
                action: .deleteSession(sessionID: session.id),
                databasePath: model.overview?.databasePath ?? ""
            ) {
                model.reload()
                selection = nil
            }
        }
    }

    private var sessionList: some View {
        List(model.sessions, selection: $selection) { session in
            VStack(alignment: .leading, spacing: 4) {
                Text(session.projectPath ?? "(no project path)")
                    .font(.headline)
                    .lineLimit(1)
                    .truncationMode(.middle)
                HStack(spacing: 8) {
                    Badge(text: session.adapter, systemImage: "shippingbox")
                    Text(session.instanceKey)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                HStack(spacing: 8) {
                    Text("\(session.eventCount) events")
                    Text("·")
                    Text(timeRange(session))
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                Text(session.id)
                    .font(.caption2.monospaced())
                    .foregroundStyle(.tertiary)
                    .textSelection(.enabled)
            }
            .padding(.vertical, 2)
            .tag(session.id)
        }
        .overlay { if model.sessions.isEmpty { EmptyHint(text: "No sessions imported yet.") } }
    }

    @ViewBuilder
    private var timeline: some View {
        if let session = selectedSession {
            let events = model.events(inSession: session.id)
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .top) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Event timeline")
                            .font(.title3.bold())
                        Text("\(events.count) events · session \(session.id)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                    }
                    Spacer()
                    Button(role: .destructive) {
                        deletionTarget = session
                    } label: {
                        Label("Delete session…", systemImage: "trash")
                    }
                }
                .padding()
                Divider()
                Table(events) {
                    TableColumn("Seq") { event in
                        Text(event.sequence.map(String.init) ?? "—")
                            .font(.caption.monospaced())
                    }
                    .width(44)
                    TableColumn("Type") { event in
                        Text(event.eventType).font(.caption.monospaced())
                    }
                    TableColumn("Tool") { event in
                        Text(event.toolName ?? "—").font(.caption)
                    }
                    TableColumn("Exit") { event in
                        Text(event.exitCode.map(String.init) ?? "—")
                            .font(.caption.monospaced())
                            .foregroundStyle((event.exitCode ?? 0) != 0 ? .red : .secondary)
                    }
                    .width(44)
                    TableColumn("Occurred at") { event in
                        Text(event.occurredAt).font(.caption.monospaced())
                    }
                    TableColumn("Event ID") { event in
                        Text(event.id)
                            .font(.caption2.monospaced())
                            .textSelection(.enabled)
                    }
                }
            }
        } else {
            EmptyHint(text: "Select a session to see its event timeline.")
        }
    }

    private func timeRange(_ session: SessionSummary) -> String {
        session.firstEventAt == session.lastEventAt
            ? session.firstEventAt
            : "\(session.firstEventAt) → \(session.lastEventAt)"
    }
}

/// A small pill label.
struct Badge: View {
    let text: String
    var systemImage: String?

    var body: some View {
        Label {
            Text(text)
        } icon: {
            if let systemImage { Image(systemName: systemImage) }
        }
        .font(.caption2.weight(.medium))
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(.quaternary, in: Capsule())
    }
}

/// Centred placeholder text for empty panes.
struct EmptyHint: View {
    let text: String
    var body: some View {
        Text(text)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
