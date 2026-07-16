import Foundation

/// A decoded Mutation Package v0.1 (the immutable candidate payload).
///
/// Only the fields the app displays are modelled; unknown fields are ignored so
/// a newer package format still decodes what it can.
public struct MutationPackage: Equatable, Codable {
    public struct Hypothesis: Equatable, Codable {
        public let statement: String?
        public let expectedResult: String?
        public let failureCases: [String]
        public let supportingEventIDs: [String]
        public let counterexampleEventIDs: [String]

        enum CodingKeys: String, CodingKey {
            case statement
            case expectedResult = "expected_result"
            case failureCases = "failure_cases"
            case supportingEventIDs = "supporting_event_ids"
            case counterexampleEventIDs = "counterexample_event_ids"
        }

        public init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            statement = try container.decodeIfPresent(String.self, forKey: .statement)
            expectedResult = try container.decodeIfPresent(String.self, forKey: .expectedResult)
            failureCases = try container.decodeIfPresent([String].self, forKey: .failureCases) ?? []
            supportingEventIDs = try container
                .decodeIfPresent([String].self, forKey: .supportingEventIDs) ?? []
            counterexampleEventIDs = try container
                .decodeIfPresent([String].self, forKey: .counterexampleEventIDs) ?? []
        }
    }

    public struct Intervention: Equatable, Codable {
        public let type: String
        public let instruction: String
    }

    /// The v0.2 model-synthesis provenance block.
    ///
    /// Present only when a package was enriched by a local model provider. It
    /// records model identity — never an endpoint, API key, prompt, or raw
    /// payload — so surfacing it here leaks nothing beyond who produced the
    /// reviewable content. A package that omits provenance is a plain v0.1
    /// package; its presence is exactly what distinguishes v0.2.
    public struct Provenance: Equatable, Codable {
        public let provider: String
        public let modelName: String
        public let modelRevision: String
        public let modelDigest: String?
        public let manifestSpecVersion: String

        enum CodingKeys: String, CodingKey {
            case provider
            case modelName = "model_name"
            case modelRevision = "model_revision"
            case modelDigest = "model_digest"
            case manifestSpecVersion = "manifest_spec_version"
        }
    }

    public struct Trigger: Equatable, Codable {
        public let type: String
        public let selector: String
    }

    public struct Permissions: Equatable, Codable {
        public let filesystemRead: [String]
        public let filesystemWrite: [String]
        public let commands: [String]
        public let environment: [String]
        public let network: Bool

        enum CodingKeys: String, CodingKey {
            case filesystemRead = "filesystem_read"
            case filesystemWrite = "filesystem_write"
            case commands
            case environment
            case network
        }

        public init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            filesystemRead = try container
                .decodeIfPresent([String].self, forKey: .filesystemRead) ?? []
            filesystemWrite = try container
                .decodeIfPresent([String].self, forKey: .filesystemWrite) ?? []
            commands = try container.decodeIfPresent([String].self, forKey: .commands) ?? []
            environment = try container.decodeIfPresent([String].self, forKey: .environment) ?? []
            network = try container.decodeIfPresent(Bool.self, forKey: .network) ?? false
        }

        /// Whether this package requests no permissions at all.
        public var isZeroPermission: Bool {
            filesystemRead.isEmpty && filesystemWrite.isEmpty
                && commands.isEmpty && environment.isEmpty && !network
        }
    }

    public let mutationID: String
    public let title: String
    public let version: String
    public let specVersion: String
    public let sourceDetector: String
    public let sourceFindingID: String
    public let hypothesis: Hypothesis
    public let intervention: Intervention
    public let triggers: [Trigger]
    public let exclusions: [String]
    public let permissions: Permissions
    /// The model-synthesis provenance block, present only for v0.2 packages
    /// that were enriched by a local model provider.
    public let provenance: Provenance?

    enum CodingKeys: String, CodingKey {
        case mutationID = "mutation_id"
        case title
        case version
        case specVersion = "spec_version"
        case sourceDetector = "source_detector"
        case sourceFindingID = "source_finding_id"
        case hypothesis
        case intervention
        case triggers
        case exclusions
        case permissions
        case provenance
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        mutationID = try container.decode(String.self, forKey: .mutationID)
        title = try container.decodeIfPresent(String.self, forKey: .title) ?? mutationID
        version = try container.decodeIfPresent(String.self, forKey: .version) ?? "unknown"
        specVersion = try container.decodeIfPresent(String.self, forKey: .specVersion) ?? "unknown"
        sourceDetector = try container.decodeIfPresent(String.self, forKey: .sourceDetector) ?? ""
        sourceFindingID = try container.decodeIfPresent(String.self, forKey: .sourceFindingID) ?? ""
        hypothesis = try container.decode(Hypothesis.self, forKey: .hypothesis)
        intervention = try container.decode(Intervention.self, forKey: .intervention)
        triggers = try container.decodeIfPresent([Trigger].self, forKey: .triggers) ?? []
        exclusions = try container.decodeIfPresent([String].self, forKey: .exclusions) ?? []
        permissions = try container.decode(Permissions.self, forKey: .permissions)
        provenance = try container.decodeIfPresent(Provenance.self, forKey: .provenance)
    }

    /// Decode a package from its stored JSON text, or `nil` if it cannot be
    /// decoded (e.g. a newer format missing required fields).
    public static func decode(from json: String) -> MutationPackage? {
        guard let data = json.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(MutationPackage.self, from: data)
    }
}
