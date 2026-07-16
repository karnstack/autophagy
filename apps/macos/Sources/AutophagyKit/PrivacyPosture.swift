import Foundation

/// Static, honest explanations of the database's privacy posture.
///
/// These describe guarantees enforced by the engine at ingestion time. The app
/// only reads the database, so it restates rather than measures these; counts
/// that can be measured live in ``DatabaseOverview``.
public enum PrivacyPosture {
    /// One explanatory bullet with a title and body.
    public struct Note: Equatable, Identifiable, Sendable {
        public var id: String { title }
        public let title: String
        public let body: String
    }

    public static let notes: [Note] = [
        Note(
            title: "Redaction at ingestion",
            body: "Secret redaction runs before any row is written. Deterministic rules "
                + "replace recognised keys, tokens, bearer credentials, and secret-like "
                + "assignments with [REDACTED]. A zero-redaction count is not proof that "
                + "content is safe."
        ),
        Note(
            title: "Search projection is deliberate",
            body: "Raw event JSON and raw tool input are never indexed blindly. Full-text "
                + "search covers only a redaction-approved projection (project paths, tool "
                + "names, and explicitly approved free text). The indexed-free-text count "
                + "shown above reflects how many events opted into that projection."
        ),
        Note(
            title: "Local-first and offline",
            body: "This database lives only on this machine. The app opens it strictly "
                + "read-only and never sends its contents anywhere."
        ),
        Note(
            title: "Retention and forgetting",
            body: "Raw history is retained by policy and can be pruned by age; learned "
                + "behaviour decays unless reused. Deletion cascades through events, FTS, "
                + "conflicts, and any mutation candidate that cites removed evidence."
        ),
        Note(
            title: "Deletion is CLI-mediated",
            body: "This app cannot write the database. Destructive actions are delegated "
                + "to the autophagy CLI after explicit confirmation, so every change to "
                + "your data goes through the same audited, reversible engine path."
        )
    ]
}
