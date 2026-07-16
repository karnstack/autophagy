import AutophagyKit
import SwiftUI

/// The main navigation shell shown once a database is open.
struct MainView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        NavigationSplitView {
            List(Section.allCases, selection: $model.selectedSection) { section in
                Label(section.rawValue, systemImage: section.systemImage)
                    .tag(section)
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 200)
            .safeAreaInset(edge: .bottom) { schemaFooter }
        } detail: {
            detail
                .navigationTitle(model.selectedSection.rawValue)
        }
    }

    @ViewBuilder
    private var detail: some View {
        switch model.selectedSection {
        case .sessions: SessionsView()
        case .patterns: PatternsView()
        case .mutations: MutationsView()
        case .privacy: PrivacyView()
        }
    }

    @ViewBuilder
    private var schemaFooter: some View {
        if let overview = model.overview {
            VStack(alignment: .leading, spacing: 6) {
                Divider()
                SchemaBadge(compatibility: overview.schema.compatibility)
                Text(URL(fileURLWithPath: overview.databasePath).lastPathComponent)
                    .font(.caption2.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Button("Switch database…") { model.closeDatabase() }
                    .buttonStyle(.link)
                    .font(.caption)
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 10)
        }
    }
}

/// A colour-coded badge for schema compatibility.
struct SchemaBadge: View {
    let compatibility: SchemaCompatibility

    private var color: Color {
        switch compatibility {
        case .supported: .green
        case .olderReadable, .newerThanKnown: .orange
        case .notAutophagy: .red
        }
    }

    var body: some View {
        HStack(spacing: 6) {
            Circle().fill(color).frame(width: 8, height: 8)
            Text(compatibility.summary)
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
