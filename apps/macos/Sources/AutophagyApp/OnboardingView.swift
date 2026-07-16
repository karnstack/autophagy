import AppKit
import AutophagyKit
import SwiftUI

/// First-run flow: find the default database or let the user pick one.
struct OnboardingView: View {
    @EnvironmentObject private var model: AppModel

    private var defaultExists: Bool {
        DatabaseLocator.defaultDatabaseExists()
    }

    var body: some View {
        VStack(spacing: 24) {
            VStack(spacing: 8) {
                Image(systemName: "cube.transparent")
                    .font(.system(size: 48))
                    .foregroundStyle(.secondary)
                Text("Autophagy")
                    .font(.largeTitle.bold())
                Text("A strictly read-only window into your local database.")
                    .foregroundStyle(.secondary)
            }

            VStack(alignment: .leading, spacing: 16) {
                LabeledContent("Default location") {
                    Text(model.defaultDatabasePath)
                        .font(.callout.monospaced())
                        .textSelection(.enabled)
                        .foregroundStyle(.secondary)
                }

                HStack(spacing: 12) {
                    Button {
                        model.open(path: model.defaultDatabasePath)
                    } label: {
                        Label("Open default database", systemImage: "internaldrive")
                    }
                    .disabled(!defaultExists)

                    Button {
                        pickDatabase()
                    } label: {
                        Label("Choose a .db file…", systemImage: "folder")
                    }
                }

                if !defaultExists {
                    Text("No database found at the default location. Run "
                        + "`autophagy import …` first, or choose a file.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(20)
            .frame(maxWidth: 560)
            .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 12))

            if let error = model.openError {
                Text(error)
                    .font(.callout)
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
                    .multilineTextAlignment(.leading)
                    .frame(maxWidth: 560, alignment: .leading)
            }

            Text("The app opens the database read-only and never writes to it.")
                .font(.footnote)
                .foregroundStyle(.tertiary)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func pickDatabase() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        panel.message = "Choose an Autophagy database file"
        if panel.runModal() == .OK, let url = panel.url {
            model.open(path: url.path)
        }
    }
}
