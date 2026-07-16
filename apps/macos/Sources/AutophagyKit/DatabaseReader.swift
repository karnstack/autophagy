import Foundation

/// Read-only, schema-tolerant access to an Autophagy database.
///
/// Every method degrades gracefully: a table that does not exist in an older
/// (or unexpectedly shaped) schema yields empty results rather than an error,
/// so the UI never crashes on a database it only partly understands.
public final class DatabaseReader {
    private let db: Database

    /// The absolute path this reader opened.
    public let path: String

    /// Open `path` read-only.
    ///
    /// - Throws: ``SQLiteError`` if the file cannot be opened.
    public init(path: String) throws {
        self.path = path
        db = try Database(readonlyPath: path)
    }

    // MARK: - Schema

    /// Inspect the schema version and derive a compatibility verdict.
    public func schemaInfo() -> SchemaInfo {
        let userVersion = (try? db.queryScalarInt("PRAGMA user_version;")) ?? 0

        guard isAutophagyDatabase() else {
            return SchemaInfo(
                userVersion: userVersion,
                migrationVersion: 0,
                compatibility: .notAutophagy
            )
        }

        let migrationVersion = (try? db.queryScalarInt(
            "SELECT coalesce(max(version), 0) FROM schema_migrations;"
        )) ?? 0
        // Prefer the migration table as the authoritative version; fall back to
        // the pragma if the table is somehow empty.
        let effective = migrationVersion > 0 ? migrationVersion : userVersion

        let compatibility: SchemaCompatibility
        if effective == knownSchemaVersion {
            compatibility = .supported(version: effective)
        } else if effective < knownSchemaVersion {
            compatibility = .olderReadable(version: effective, known: knownSchemaVersion)
        } else {
            compatibility = .newerThanKnown(version: effective, known: knownSchemaVersion)
        }

        return SchemaInfo(
            userVersion: userVersion,
            migrationVersion: migrationVersion,
            compatibility: compatibility
        )
    }

    /// A database is recognised as Autophagy when it carries the migration
    /// ledger and the two foundational tables present since the first migration.
    public func isAutophagyDatabase() -> Bool {
        db.objectExists("schema_migrations")
            && db.objectExists("sessions")
            && db.objectExists("events")
    }

    // MARK: - Overview / privacy

    /// Gather the counts and posture flags shown in the privacy view.
    public func overview() -> DatabaseOverview {
        DatabaseOverview(
            databasePath: path,
            schema: schemaInfo(),
            sourceCount: count("sources"),
            sessionCount: count("sessions"),
            eventCount: count("events"),
            mutationCount: count("mutation_candidates"),
            conflictCount: count("event_conflicts"),
            hasFTS: db.objectExists("events_fts"),
            hasSignatureIndex: db.objectExists("event_signatures"),
            indexedFreeTextRows: indexedFreeTextRows()
        )
    }

    private func count(_ table: String) -> Int {
        guard db.objectExists(table) else { return 0 }
        return (try? db.queryScalarInt("SELECT count(*) FROM \(table);")) ?? 0
    }

    /// Number of events whose free text was approved into the search projection.
    private func indexedFreeTextRows() -> Int {
        guard db.objectExists("events_search") else { return 0 }
        return (try? db.queryScalarInt(
            "SELECT count(*) FROM events_search WHERE coalesce(searchable_text, '') <> '';"
        )) ?? 0
    }

    // MARK: - Sessions

    /// All sessions, most recently active first.
    public func sessions() -> [SessionSummary] {
        guard db.objectExists("sessions"), db.objectExists("sources") else { return [] }
        let sql = """
        SELECT s.session_id, src.adapter, src.instance_key, s.project_path,
               s.first_event_at, s.last_event_at, s.event_count
        FROM sessions s
        JOIN sources src ON src.source_id = s.source_id
        ORDER BY s.last_event_at DESC, s.session_id ASC;
        """
        return (try? db.query(sql) { row in
            SessionSummary(
                id: row.string(0) ?? "",
                adapter: row.string(1) ?? "",
                instanceKey: row.string(2) ?? "",
                projectPath: row.string(3),
                firstEventAt: row.string(4) ?? "",
                lastEventAt: row.string(5) ?? "",
                eventCount: row.int(6) ?? 0
            )
        }) ?? []
    }

    /// The ordered event timeline for one session.
    public func events(inSession sessionID: String) -> [EventRow] {
        guard db.objectExists("events") else { return [] }
        let sql = """
        SELECT event_id, event_type, tool_name, occurred_at, sequence, exit_code, parent_event_id
        FROM events
        WHERE session_id = ?1
        ORDER BY occurred_at ASC, sequence ASC, row_id ASC;
        """
        return (try? db.query(sql, text: [sessionID]) { row in
            EventRow(
                id: row.string(0) ?? "",
                eventType: row.string(1) ?? "",
                toolName: row.string(2),
                occurredAt: row.string(3) ?? "",
                sequence: row.int(4),
                exitCode: row.int(5),
                parentEventID: row.string(6)
            )
        }) ?? []
    }

    // MARK: - Patterns (evidence packets embedded in candidates)

    /// The deterministic findings preserved inside each registered candidate,
    /// with their exact supporting and counterexample event IDs.
    public func evidencePackets() -> [EvidencePacket] {
        guard db.objectExists("mutation_candidates") else { return [] }
        let sql = """
        SELECT mutation_id, source_finding_id, source_detector, package_json
        FROM mutation_candidates
        ORDER BY source_detector ASC, source_finding_id ASC;
        """
        let rows = (try? db.query(sql) { row in
            (
                findingID: row.string(1) ?? "",
                detector: row.string(2) ?? "",
                json: row.string(3) ?? "{}"
            )
        }) ?? []

        return rows.map { row in
            let package = MutationPackage.decode(from: row.json)
            return EvidencePacket(
                id: row.findingID,
                detector: row.detector,
                title: package?.title ?? row.findingID,
                statement: package?.hypothesis.statement,
                expectedResult: package?.hypothesis.expectedResult,
                supportingEventIDs: package?.hypothesis.supportingEventIDs ?? [],
                counterexampleEventIDs: package?.hypothesis.counterexampleEventIDs ?? []
            )
        }
    }

    // MARK: - Mutations

    /// The mutation candidate registry, newest first.
    public func mutations() -> [MutationSummary] {
        guard db.objectExists("mutation_candidates") else { return [] }
        let sql = """
        SELECT mutation_id, source_detector, state, semantic_version, spec_version,
               created_at, updated_at, package_json
        FROM mutation_candidates
        ORDER BY created_at DESC, mutation_id ASC;
        """
        return (try? db.query(sql) { row in
            let package = MutationPackage.decode(from: row.string(7) ?? "{}")
            return MutationSummary(
                id: row.string(0) ?? "",
                title: package?.title ?? (row.string(0) ?? ""),
                state: row.string(2) ?? "",
                detector: row.string(1) ?? "",
                semanticVersion: row.string(3) ?? "",
                specVersion: row.string(4) ?? "",
                createdAt: row.string(5) ?? "",
                updatedAt: row.string(6) ?? ""
            )
        }) ?? []
    }

    /// Full detail for one candidate: package, evidence lineage, audit log, and
    /// any replay/shadow/installation records.
    public func mutationDetail(id: String) -> MutationDetail? {
        guard db.objectExists("mutation_candidates") else { return nil }
        let sql = """
        SELECT mutation_id, source_detector, state, semantic_version, spec_version,
               created_at, updated_at, package_json, rejection_reason
        FROM mutation_candidates
        WHERE mutation_id = ?1;
        """
        let head = (try? db.query(sql, text: [id]) { row in
            (
                summary: MutationSummary(
                    id: row.string(0) ?? "",
                    title: "",
                    state: row.string(2) ?? "",
                    detector: row.string(1) ?? "",
                    semanticVersion: row.string(3) ?? "",
                    specVersion: row.string(4) ?? "",
                    createdAt: row.string(5) ?? "",
                    updatedAt: row.string(6) ?? ""
                ),
                json: row.string(7) ?? "{}",
                rejection: row.string(8)
            )
        })?.first

        guard let head else { return nil }
        let package = MutationPackage.decode(from: head.json)
        let summary = MutationSummary(
            id: head.summary.id,
            title: package?.title ?? head.summary.id,
            state: head.summary.state,
            detector: head.summary.detector,
            semanticVersion: head.summary.semanticVersion,
            specVersion: head.summary.specVersion,
            createdAt: head.summary.createdAt,
            updatedAt: head.summary.updatedAt
        )

        return MutationDetail(
            summary: summary,
            package: package,
            rawPackageJSON: prettyJSON(head.json),
            evidence: evidenceLinks(mutationID: id),
            transitions: transitions(mutationID: id),
            replays: replays(mutationID: id),
            shadows: shadows(mutationID: id),
            installation: installation(mutationID: id),
            rejectionReason: head.rejection
        )
    }

    private func evidenceLinks(mutationID: String) -> [EvidenceLink] {
        guard db.objectExists("mutation_evidence") else { return [] }
        // LEFT JOIN so a link whose event was deleted still reports as absent
        // (cascade deletes normally prevent this, but we display honestly).
        let sql = """
        SELECT me.event_id, me.role, me.ordinal, (e.event_id IS NOT NULL)
        FROM mutation_evidence me
        LEFT JOIN events e ON e.event_id = me.event_id
        WHERE me.mutation_id = ?1
        ORDER BY me.role ASC, me.ordinal ASC;
        """
        return (try? db.query(sql, text: [mutationID]) { row in
            EvidenceLink(
                eventID: row.string(0) ?? "",
                role: row.string(1) ?? "",
                ordinal: row.int(2) ?? 0,
                eventPresent: (row.int(3) ?? 0) != 0
            )
        }) ?? []
    }

    private func transitions(mutationID: String) -> [LifecycleTransition] {
        guard db.objectExists("mutation_transitions") else { return [] }
        let sql = """
        SELECT transition_id, from_state, to_state, reason, occurred_at, metadata_json
        FROM mutation_transitions
        WHERE mutation_id = ?1
        ORDER BY transition_id ASC;
        """
        return (try? db.query(sql, text: [mutationID]) { row in
            LifecycleTransition(
                id: row.int(0) ?? 0,
                fromState: row.string(1),
                toState: row.string(2) ?? "",
                reason: row.string(3) ?? "",
                occurredAt: row.string(4) ?? "",
                metadataJSON: row.string(5) ?? "{}"
            )
        }) ?? []
    }

    private func replays(mutationID: String) -> [ReplayRecord] {
        guard db.objectExists("mutation_replays") else { return [] }
        let sql = """
        SELECT replay_id, scenario_set_hash, passed, created_at
        FROM mutation_replays
        WHERE mutation_id = ?1
        ORDER BY created_at ASC;
        """
        return (try? db.query(sql, text: [mutationID]) { row in
            ReplayRecord(
                id: row.string(0) ?? "",
                scenarioSetHash: row.string(1) ?? "",
                passed: (row.int(2) ?? 0) != 0,
                createdAt: row.string(3) ?? ""
            )
        }) ?? []
    }

    private func shadows(mutationID: String) -> [ShadowRecord] {
        guard db.objectExists("mutation_shadows") else { return [] }
        let sql = """
        SELECT shadow_id, observation_set_hash, passed, created_at
        FROM mutation_shadows
        WHERE mutation_id = ?1
        ORDER BY created_at ASC;
        """
        return (try? db.query(sql, text: [mutationID]) { row in
            ShadowRecord(
                id: row.string(0) ?? "",
                observationSetHash: row.string(1) ?? "",
                passed: (row.int(2) ?? 0) != 0,
                createdAt: row.string(3) ?? ""
            )
        }) ?? []
    }

    private func installation(mutationID: String) -> InstallationRecord? {
        guard db.objectExists("mutation_installations") else { return nil }
        let sql = """
        SELECT installation_id, target, repository_root, relative_path, state,
               installed_at, uninstalled_at
        FROM mutation_installations
        WHERE mutation_id = ?1
        LIMIT 1;
        """
        return (try? db.query(sql, text: [mutationID]) { row in
            InstallationRecord(
                id: row.string(0) ?? "",
                target: row.string(1) ?? "",
                repositoryRoot: row.string(2) ?? "",
                relativePath: row.string(3) ?? "",
                state: row.string(4) ?? "",
                installedAt: row.string(5) ?? "",
                uninstalledAt: row.string(6)
            )
        })?.first
    }

    private func prettyJSON(_ json: String) -> String {
        guard let data = json.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data),
              let pretty = try? JSONSerialization.data(
                  withJSONObject: object,
                  options: [.prettyPrinted, .sortedKeys]
              ),
              let text = String(data: pretty, encoding: .utf8)
        else { return json }
        return text
    }
}
