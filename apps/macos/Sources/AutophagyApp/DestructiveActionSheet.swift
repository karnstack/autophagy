import AppKit
import AutophagyKit
import SwiftUI

/// A multi-step confirmation for a destructive action.
///
/// The app never writes the database. This sheet shows the exact `autophagy`
/// command, requires explicit confirmation (and a typed phrase for delete-all),
/// then either runs the installed CLI via `Process` or — when no binary is
/// found — leaves the command for the user to run themselves.
struct DestructiveActionSheet: View {
    let action: DestructiveAction
    let databasePath: String
    /// Called after a successful CLI run so the caller can reload or dismiss.
    let onCompleted: () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var typedPhrase = ""
    @State private var isRunning = false
    @State private var result: RunResult?

    private var command: CLICommand {
        CLICommand(action: action, databasePath: databasePath)
    }

    private var phraseSatisfied: Bool {
        guard let required = action.confirmationPhrase else { return true }
        return typedPhrase == required
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Label(action.title, systemImage: "exclamationmark.triangle.fill")
                .font(.title2.bold())
                .foregroundStyle(.red)

            Text(action.consequence)
                .fixedSize(horizontal: false, vertical: true)

            VStack(alignment: .leading, spacing: 6) {
                Text("EXACT COMMAND")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
                Text(command.displayString)
                    .font(.callout.monospaced())
                    .textSelection(.enabled)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
                HStack(spacing: 6) {
                    Image(systemName: command.executableFound
                        ? "checkmark.seal" : "questionmark.circle")
                    Text(command.executableFound
                        ? "CLI found at \(command.executable)"
                        : "No autophagy CLI found on PATH — copy the command and run it yourself.")
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }

            if let phrase = action.confirmationPhrase {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Type \(phrase) to confirm")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    TextField(phrase, text: $typedPhrase)
                        .textFieldStyle(.roundedBorder)
                        .font(.callout.monospaced())
                        .disableAutocorrection(true)
                }
            }

            if let result {
                resultView(result)
            }

            Spacer(minLength: 0)

            HStack {
                Button("Copy command") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(command.displayString, forType: .string)
                }
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                Button(role: .destructive) {
                    run()
                } label: {
                    if isRunning { ProgressView().controlSize(.small) } else { Text("Run command") }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!command.executableFound || !phraseSatisfied || isRunning)
            }
        }
        .padding(24)
        .frame(width: 560)
    }

    @ViewBuilder
    private func resultView(_ result: RunResult) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Label(
                result.exitCode == 0 ? "Command succeeded" : "Command failed (exit \(result.exitCode))",
                systemImage: result.exitCode == 0 ? "checkmark.circle" : "xmark.circle"
            )
            .foregroundStyle(result.exitCode == 0 ? .green : .red)
            if !result.output.isEmpty {
                ScrollView {
                    Text(result.output)
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 120)
                .padding(8)
                .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 6))
            }
        }
    }

    private func run() {
        isRunning = true
        let outcome = CLIExecutor.run(command)
        isRunning = false
        result = outcome
        if outcome.exitCode == 0 {
            onCompleted()
        }
    }
}

/// The captured result of a CLI invocation.
struct RunResult {
    let exitCode: Int32
    let output: String
}

/// Runs a resolved ``CLICommand`` and captures its combined output.
enum CLIExecutor {
    static func run(_ command: CLICommand) -> RunResult {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: command.executable)
        process.arguments = Array(command.arguments.dropFirst())
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        do {
            try process.run()
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            return RunResult(
                exitCode: process.terminationStatus,
                output: String(data: data, encoding: .utf8) ?? ""
            )
        } catch {
            return RunResult(exitCode: -1, output: "Failed to launch: \(error)")
        }
    }
}
