import AppKit
import AutophagyKit
import SwiftUI

/// Honest, read-only summary of what the database contains, where it lives, and
/// its privacy posture — plus the CLI-mediated delete-all action.
struct PrivacyView: View {
    @EnvironmentObject private var model: AppModel
    @State private var showDeleteAll = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                if let overview = model.overview {
                    locationSection(overview)
                    contentsSection(overview)
                }
                postureSection
                dangerZone
            }
            .padding()
        }
        .sheet(isPresented: $showDeleteAll) {
            DestructiveActionSheet(
                action: .deleteAll,
                databasePath: model.overview?.databasePath ?? ""
            ) {
                model.reload()
            }
        }
    }

    private func locationSection(_ overview: DatabaseOverview) -> some View {
        SectionCard(title: "On disk", systemImage: "externaldrive") {
            LabeledText(label: "Database path", text: overview.databasePath)
            SchemaBadge(compatibility: overview.schema.compatibility)
            HStack(spacing: 16) {
                Text("user_version: \(overview.schema.userVersion)")
                Text("schema_migrations max: \(overview.schema.migrationVersion)")
            }
            .font(.caption.monospaced())
            .foregroundStyle(.secondary)
            Button("Reveal in Finder") {
                NSWorkspace.shared.activateFileViewerSelecting(
                    [URL(fileURLWithPath: overview.databasePath)]
                )
            }
            .buttonStyle(.link)
        }
    }

    private func contentsSection(_ overview: DatabaseOverview) -> some View {
        SectionCard(title: "What it contains", systemImage: "tablecells") {
            FlowLayout(spacing: 10) {
                CountTile(label: "Sources", value: overview.sourceCount)
                CountTile(label: "Sessions", value: overview.sessionCount)
                CountTile(label: "Events", value: overview.eventCount)
                CountTile(label: "Mutations", value: overview.mutationCount)
                CountTile(label: "Conflicts", value: overview.conflictCount)
                CountTile(label: "Indexed free-text", value: overview.indexedFreeTextRows)
            }
            HStack(spacing: 16) {
                Label(overview.hasFTS ? "FTS index present" : "No FTS index",
                      systemImage: overview.hasFTS ? "magnifyingglass" : "magnifyingglass.circle")
                Label(overview.hasSignatureIndex ? "Signature index present" : "No signature index",
                      systemImage: "number")
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
    }

    private var postureSection: some View {
        SectionCard(title: "Privacy posture", systemImage: "hand.raised") {
            ForEach(PrivacyPosture.notes) { note in
                VStack(alignment: .leading, spacing: 2) {
                    Text(note.title).font(.callout.weight(.semibold))
                    Text(note.body)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.vertical, 2)
            }
        }
    }

    private var dangerZone: some View {
        SectionCard(title: "Danger zone", systemImage: "exclamationmark.triangle") {
            Text("This app is read-only and cannot modify the database. Deletion is "
                + "delegated to the autophagy CLI after explicit confirmation.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Button(role: .destructive) {
                showDeleteAll = true
            } label: {
                Label("Delete all local data…", systemImage: "trash")
            }
        }
    }
}

struct CountTile: View {
    let label: String
    let value: Int
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("\(value)").font(.title2.bold().monospacedDigit())
            Text(label).font(.caption).foregroundStyle(.secondary)
        }
        .padding(12)
        .frame(minWidth: 96, alignment: .leading)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
    }
}
