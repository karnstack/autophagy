import AutophagyKit
import SwiftUI

/// Deterministic findings (Evidence Packets) preserved inside each candidate,
/// with their exact supporting and counterexample event IDs.
struct PatternsView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Group {
            if model.patterns.isEmpty {
                EmptyHint(text: "No deterministic findings are registered yet.\n"
                    + "Findings appear here once `autophagy mutations propose` records them.")
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        Text("Each finding is the evidence packet preserved inside a "
                            + "registered mutation candidate. Event IDs are exact links.")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                        ForEach(model.patterns) { packet in
                            PatternCard(packet: packet)
                        }
                    }
                    .padding()
                }
            }
        }
    }
}

private struct PatternCard: View {
    let packet: EvidencePacket

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(packet.title).font(.headline)
                Spacer()
                Badge(text: packet.detector, systemImage: "function")
            }
            Text(packet.id)
                .font(.caption2.monospaced())
                .foregroundStyle(.tertiary)
                .textSelection(.enabled)

            if let statement = packet.statement {
                LabeledText(label: "Hypothesis", text: statement)
            }
            if let expected = packet.expectedResult {
                LabeledText(label: "Expected result", text: expected)
            }

            EventIDList(
                label: "Supporting evidence",
                ids: packet.supportingEventIDs,
                tint: .green
            )
            EventIDList(
                label: "Counterexamples",
                ids: packet.counterexampleEventIDs,
                tint: .orange
            )
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 10))
    }
}

/// A label above a selectable multi-line body.
struct LabeledText: View {
    let label: String
    let text: String
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label.uppercased())
                .font(.caption2.weight(.semibold))
                .foregroundStyle(.secondary)
            Text(text)
                .font(.callout)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// A labelled, wrapping list of exact event IDs.
struct EventIDList: View {
    let label: String
    let ids: [String]
    var tint: Color = .secondary

    var body: some View {
        if !ids.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text("\(label.uppercased()) (\(ids.count))")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
                FlowLayout(spacing: 6) {
                    ForEach(ids, id: \.self) { id in
                        Text(id)
                            .font(.caption2.monospaced())
                            .padding(.horizontal, 6)
                            .padding(.vertical, 3)
                            .background(tint.opacity(0.15), in: RoundedRectangle(cornerRadius: 5))
                            .textSelection(.enabled)
                    }
                }
            }
        }
    }
}
