import AutophagyKit
import SwiftUI

/// Candidate registry with lifecycle state, evidence lineage, and audit log.
struct MutationsView: View {
    @EnvironmentObject private var model: AppModel
    @State private var selection: MutationSummary.ID?

    private var detail: MutationDetail? {
        selection.flatMap { model.mutationDetail(id: $0) }
    }

    var body: some View {
        HSplitView {
            List(model.mutations, selection: $selection) { mutation in
                VStack(alignment: .leading, spacing: 4) {
                    Text(mutation.title).font(.headline).lineLimit(2)
                    HStack(spacing: 8) {
                        StateBadge(state: mutation.state)
                        Text("v\(mutation.semanticVersion)")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    Text(mutation.detector)
                        .font(.caption2).foregroundStyle(.tertiary)
                }
                .padding(.vertical, 2)
                .tag(mutation.id)
            }
            .frame(minWidth: 300, idealWidth: 340)
            .overlay { if model.mutations.isEmpty {
                EmptyHint(text: "No mutation candidates registered yet.")
            } }

            Group {
                if let detail {
                    MutationDetailView(detail: detail)
                } else {
                    EmptyHint(text: "Select a candidate to inspect its lifecycle and evidence.")
                }
            }
            .frame(minWidth: 420)
        }
    }
}

private struct MutationDetailView: View {
    let detail: MutationDetail
    @EnvironmentObject private var model: AppModel
    @State private var showRawPackage = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                header
                if let package = detail.package {
                    interventionSection(package)
                    permissionsSection(package)
                }
                evidenceSection
                auditSection
                evaluationSection
                installationSection
            }
            .padding()
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(detail.summary.title).font(.title2.bold())
            HStack(spacing: 8) {
                StateBadge(state: detail.summary.state)
                Badge(text: detail.summary.detector, systemImage: "function")
                Text("spec \(detail.summary.specVersion)")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Text(detail.summary.id)
                .font(.caption2.monospaced())
                .foregroundStyle(.tertiary)
                .textSelection(.enabled)
            if let reason = detail.rejectionReason, !reason.isEmpty {
                LabeledText(label: "Rejection reason", text: reason)
                    .foregroundStyle(.red)
            }
        }
    }

    private func interventionSection(_ package: MutationPackage) -> some View {
        SectionCard(title: "Intervention", systemImage: "text.badge.checkmark") {
            LabeledText(label: "Type", text: package.intervention.type)
            LabeledText(label: "Instruction", text: package.intervention.instruction)
            if let statement = package.hypothesis.statement {
                LabeledText(label: "Hypothesis", text: statement)
            }
            if let expected = package.hypothesis.expectedResult {
                LabeledText(label: "Expected result", text: expected)
            }
            if !package.exclusions.isEmpty {
                BulletList(label: "Exclusions", items: package.exclusions)
            }
            DisclosureGroup("Raw package JSON", isExpanded: $showRawPackage) {
                Text(detail.rawPackageJSON)
                    .font(.caption2.monospaced())
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .font(.caption)
        }
    }

    private func permissionsSection(_ package: MutationPackage) -> some View {
        SectionCard(title: "Permissions", systemImage: "lock") {
            if package.permissions.isZeroPermission {
                Label("Zero permissions — no filesystem, commands, environment, or network.",
                      systemImage: "checkmark.shield")
                    .font(.callout)
                    .foregroundStyle(.green)
            } else {
                Text("network: \(package.permissions.network ? "true" : "false")")
                    .font(.callout.monospaced())
            }
        }
    }

    private var evidenceSection: some View {
        SectionCard(title: "Evidence lineage", systemImage: "link") {
            let support = detail.evidence.filter { $0.role == "support" }
            let counter = detail.evidence.filter { $0.role == "counterexample" }
            EvidenceLinkList(label: "Supporting", links: support, tint: .green)
            EvidenceLinkList(label: "Counterexamples", links: counter, tint: .orange)
            if detail.evidence.contains(where: { !$0.eventPresent }) {
                Text("Some cited events are no longer present locally.")
                    .font(.caption).foregroundStyle(.red)
            }
        }
    }

    private var auditSection: some View {
        SectionCard(title: "Lifecycle audit", systemImage: "clock.arrow.circlepath") {
            if detail.transitions.isEmpty {
                Text("No transitions recorded.").font(.caption).foregroundStyle(.secondary)
            } else {
                ForEach(detail.transitions) { transition in
                    HStack(alignment: .top, spacing: 8) {
                        Text(transitionArrow(transition))
                            .font(.caption.monospaced())
                            .foregroundStyle(.secondary)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(transition.reason).font(.callout)
                            Text(transition.occurredAt)
                                .font(.caption2.monospaced())
                                .foregroundStyle(.tertiary)
                        }
                    }
                    .padding(.vertical, 2)
                }
            }
        }
    }

    @ViewBuilder
    private var evaluationSection: some View {
        if !detail.replays.isEmpty || !detail.shadows.isEmpty {
            SectionCard(title: "Evaluation records", systemImage: "chart.bar.doc.horizontal") {
                ForEach(detail.replays) { replay in
                    EvaluationRow(kind: "Replay", passed: replay.passed,
                                  hash: replay.scenarioSetHash, at: replay.createdAt)
                }
                ForEach(detail.shadows) { shadow in
                    EvaluationRow(kind: "Shadow", passed: shadow.passed,
                                  hash: shadow.observationSetHash, at: shadow.createdAt)
                }
            }
        }
    }

    @ViewBuilder
    private var installationSection: some View {
        if let install = detail.installation {
            SectionCard(title: "Installation", systemImage: "square.and.arrow.down") {
                LabeledText(label: "State", text: install.state)
                LabeledText(label: "Repository root", text: install.repositoryRoot)
                LabeledText(label: "Relative path", text: install.relativePath)
                LabeledText(label: "Installed at", text: install.installedAt)
                if let uninstalled = install.uninstalledAt {
                    LabeledText(label: "Uninstalled at", text: uninstalled)
                }
            }
        }
    }

    private func transitionArrow(_ transition: LifecycleTransition) -> String {
        "\(transition.fromState ?? "∅") → \(transition.toState)"
    }
}

// MARK: - Small building blocks

struct StateBadge: View {
    let state: String
    private var color: Color {
        switch state {
        case "active", "shadow_passed", "replay_passed": .green
        case "rejected", "retired": .red
        case "challenged": .blue
        default: .secondary
        }
    }
    var body: some View {
        Text(state)
            .font(.caption2.weight(.semibold))
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(color.opacity(0.18), in: Capsule())
            .foregroundStyle(color)
    }
}

struct SectionCard<Content: View>: View {
    let title: String
    let systemImage: String
    @ViewBuilder let content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label(title, systemImage: systemImage).font(.headline)
            content
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 10))
    }
}

struct BulletList: View {
    let label: String
    let items: [String]
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label.uppercased())
                .font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            ForEach(items, id: \.self) { item in
                Text("• \(item)").font(.callout).fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

struct EvidenceLinkList: View {
    let label: String
    let links: [EvidenceLink]
    var tint: Color
    var body: some View {
        if !links.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text("\(label.uppercased()) (\(links.count))")
                    .font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                FlowLayout(spacing: 6) {
                    ForEach(links) { link in
                        Text(link.eventID)
                            .font(.caption2.monospaced())
                            .padding(.horizontal, 6).padding(.vertical, 3)
                            .background((link.eventPresent ? tint : Color.red).opacity(0.15),
                                        in: RoundedRectangle(cornerRadius: 5))
                            .textSelection(.enabled)
                    }
                }
            }
        }
    }
}

struct EvaluationRow: View {
    let kind: String
    let passed: Bool
    let hash: String
    let at: String
    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: passed ? "checkmark.circle" : "xmark.circle")
                .foregroundStyle(passed ? .green : .red)
            Text(kind).font(.callout.weight(.medium))
            Text(hash).font(.caption2.monospaced()).foregroundStyle(.tertiary)
                .lineLimit(1).truncationMode(.middle)
            Spacer()
            Text(at).font(.caption2.monospaced()).foregroundStyle(.secondary)
        }
    }
}
