use std::{collections::BTreeSet, path::Path, time::Duration};

use autophagy_events::{Event, EventKind};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use time::OffsetDateTime;

use crate::{
    DeleteAllSummary, DeleteSummary, InsertOutcome, MutationDetails, MutationRecord,
    MutationRegisterOutcome, MutationRegistration, MutationReplayRecord, MutationTransition,
    MutationTransitionOutcome, PruneSummary, ReplayRegisterOutcome, ReplayRegistration, SearchHit,
    SearchProjection, SessionSummary, SourceCursor, SourceIdentity, StoreError, StoreStats,
    migration, util,
};

/// Transactional owner of one local Autophagy `SQLite` database.
pub struct EventStore {
    connection: Connection,
}

impl EventStore {
    /// Open or create a store at a filesystem path and apply pending migrations.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot be configured or a migration
    /// cannot be verified and applied.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    /// Open a temporary in-memory store, primarily for tests and previews.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot be configured or migrated.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    /// Return the highest immutable migration applied to this database.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the migration table cannot be queried.
    pub fn schema_version(&self) -> Result<i64, StoreError> {
        Ok(self.connection.query_row(
            "SELECT coalesce(max(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?)
    }

    /// Load an adapter's durable cursor for one source origin.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for invalid source identity, blank origin,
    /// corrupt persisted state, or a database failure.
    pub fn get_source_cursor(
        &self,
        source: &SourceIdentity,
        origin: &str,
    ) -> Result<Option<SourceCursor>, StoreError> {
        validate_source(source)?;
        validate_cursor_origin(origin)?;
        let stored = self
            .connection
            .query_row(
                "SELECT byte_offset, line_number, head_hash, state_json
                 FROM source_cursors
                 WHERE adapter = ?1 AND instance_key = ?2 AND origin = ?3",
                params![source.adapter, source.instance_key, origin],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((byte_offset, line_number, head_hash, state_json)) = stored else {
            return Ok(None);
        };
        let head_hash: [u8; 32] = head_hash
            .try_into()
            .map_err(|_| StoreError::CorruptCursor { field: "head_hash" })?;
        Ok(Some(SourceCursor {
            byte_offset: u64::try_from(byte_offset).map_err(|_| StoreError::CorruptCursor {
                field: "byte_offset",
            })?,
            line_number: u64::try_from(line_number).map_err(|_| StoreError::CorruptCursor {
                field: "line_number",
            })?,
            head_hash,
            state: serde_json::from_str(&state_json)?,
        }))
    }

    /// Atomically create or replace an adapter's durable cursor.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for invalid identity or cursor values,
    /// serialization failure, or a database failure.
    pub fn save_source_cursor(
        &self,
        source: &SourceIdentity,
        origin: &str,
        cursor: &SourceCursor,
    ) -> Result<(), StoreError> {
        validate_source(source)?;
        validate_cursor_origin(origin)?;
        let byte_offset =
            i64::try_from(cursor.byte_offset).map_err(|_| StoreError::CursorOutOfRange {
                field: "byte_offset",
                value: cursor.byte_offset,
            })?;
        let line_number =
            i64::try_from(cursor.line_number).map_err(|_| StoreError::CursorOutOfRange {
                field: "line_number",
                value: cursor.line_number,
            })?;
        let state_json = serde_json::to_string(&cursor.state)?;
        let updated_at = util::now_timestamp()?;
        self.connection.execute(
            "INSERT INTO source_cursors(
                adapter, instance_key, origin, byte_offset, line_number,
                head_hash, state_json, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(adapter, instance_key, origin) DO UPDATE SET
                byte_offset = excluded.byte_offset,
                line_number = excluded.line_number,
                head_hash = excluded.head_hash,
                state_json = excluded.state_json,
                updated_at = excluded.updated_at",
            params![
                source.adapter,
                source.instance_key,
                origin,
                byte_offset,
                line_number,
                cursor.head_hash.as_slice(),
                state_json,
                updated_at,
            ],
        )?;
        Ok(())
    }

    /// Atomically validate and persist one normalized event.
    ///
    /// Identical event IDs and content hashes are no-ops. Reusing an event ID
    /// with different content commits an audit quarantine record without
    /// changing the canonical event.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when validation, serialization, provenance, or a
    /// transactional `SQLite` operation fails.
    pub fn insert_event(
        &mut self,
        source: &SourceIdentity,
        event: &Event,
        search: &SearchProjection,
    ) -> Result<InsertOutcome, StoreError> {
        event.validate()?;
        validate_source(source)?;
        if event.source != source.adapter {
            return Err(StoreError::SourceMismatch {
                event_source: event.source.clone(),
                adapter: source.adapter.clone(),
            });
        }

        let sequence = event
            .sequence
            .map(|value| {
                i64::try_from(value).map_err(|_| StoreError::SequenceOutOfRange { sequence: value })
            })
            .transpose()?;
        let event_json = serde_json::to_string(event)?;
        let content_hash = util::sha256(event_json.as_bytes());
        let occurred_at = util::canonical_timestamp(event.timestamp)?;
        let imported_at = util::now_timestamp()?;
        let tool_input_text = persisted_tool_input(event)?;

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Some((row_id, existing_hash)) = transaction
            .query_row(
                "SELECT row_id, content_hash FROM events WHERE event_id = ?1",
                [event.event_id.as_str()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?
        {
            if existing_hash.as_slice() == content_hash {
                return Ok(InsertOutcome::Duplicate { row_id });
            }
            return quarantine_conflict(
                transaction,
                source,
                event,
                &event_json,
                &existing_hash,
                &content_hash,
                &imported_at,
            );
        }

        let source_id = upsert_source(&transaction, source, &occurred_at)?;
        ensure_session(&transaction, source_id, event, &occurred_at)?;
        ensure_sequence_available(&transaction, event, sequence)?;

        let tool_name = event.tool.as_ref().map(|tool| tool.name.as_str());
        let exit_code = event.tool.as_ref().and_then(|tool| tool.exit_code);
        transaction.execute(
            "INSERT INTO events(
                event_id, spec_version, session_id, occurred_at, sequence,
                event_type, project_path, parent_event_id, tool_name,
                tool_input_text, exit_code, event_json, content_hash, imported_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
             )",
            params![
                event.event_id.as_str(),
                event.spec_version.as_str(),
                event.session_id.as_str(),
                occurred_at,
                sequence,
                event.kind.as_str(),
                event.project.as_deref(),
                event
                    .parent_event_id
                    .as_ref()
                    .map(autophagy_events::EventId::as_str),
                tool_name,
                tool_input_text,
                exit_code,
                event_json,
                content_hash.as_slice(),
                imported_at,
            ],
        )?;
        let row_id = transaction.last_insert_rowid();

        transaction.execute(
            "INSERT INTO events_search(
                event_row_id, project_path, tool_name, tool_input_text, searchable_text
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                row_id,
                event.project.as_deref(),
                tool_name,
                search.tool_input_text.as_deref(),
                search.searchable_text.as_deref().unwrap_or_default(),
            ],
        )?;

        insert_artifacts(&transaction, row_id, event)?;
        update_session_rollup(&transaction, event, &occurred_at)?;
        transaction.commit()?;

        Ok(InsertOutcome::Inserted { row_id })
    }

    /// Load and revalidate one canonical event by its evidence ID.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` fails or persisted JSON is invalid.
    pub fn get_event(&self, event_id: &str) -> Result<Option<Event>, StoreError> {
        let event_json = self
            .connection
            .query_row(
                "SELECT event_json FROM events WHERE event_id = ?1",
                [event_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        event_json
            .map(|json| Event::from_json_str(&json).map_err(StoreError::from))
            .transpose()
    }

    /// Return one session and its source provenance.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot execute the query.
    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionSummary>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT
                    sessions.session_id,
                    sources.adapter,
                    sources.instance_key,
                    sessions.project_path,
                    sessions.started_at,
                    sessions.ended_at,
                    sessions.first_event_at,
                    sessions.last_event_at,
                    sessions.event_count
                 FROM sessions
                 JOIN sources USING (source_id)
                 WHERE sessions.session_id = ?1",
                [session_id],
                |row| {
                    Ok(SessionSummary {
                        session_id: row.get(0)?,
                        adapter: row.get(1)?,
                        instance_key: row.get(2)?,
                        project_path: row.get(3)?,
                        started_at: row.get(4)?,
                        ended_at: row.get(5)?,
                        first_event_at: row.get(6)?,
                        last_event_at: row.get(7)?,
                        event_count: row.get(8)?,
                    })
                },
            )
            .optional()?)
    }

    /// List the most recently active sessions with their source provenance.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot execute the query.
    pub fn list_sessions(&self, limit: u32) -> Result<Vec<SessionSummary>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "SELECT
                sessions.session_id,
                sources.adapter,
                sources.instance_key,
                sessions.project_path,
                sessions.started_at,
                sessions.ended_at,
                sessions.first_event_at,
                sessions.last_event_at,
                sessions.event_count
             FROM sessions
             JOIN sources USING (source_id)
             ORDER BY sessions.last_event_at DESC, sessions.session_id
             LIMIT ?1",
        )?;
        let rows = statement.query_map([i64::from(limit)], |row| {
            Ok(SessionSummary {
                session_id: row.get(0)?,
                adapter: row.get(1)?,
                instance_key: row.get(2)?,
                project_path: row.get(3)?,
                started_at: row.get(4)?,
                ended_at: row.get(5)?,
                first_event_at: row.get(6)?,
                last_event_at: row.get(7)?,
                event_count: row.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Return canonical events in deterministic evidence order.
    ///
    /// An exact project path limits the result when supplied. This deliberately
    /// returns validated AEP envelopes rather than exposing `SQLite` rows to
    /// detector crates.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` fails or persisted event JSON no
    /// longer satisfies the AEP contract.
    pub fn list_events_for_detection(
        &self,
        project: Option<&str>,
    ) -> Result<Vec<Event>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT event_json
             FROM events
             WHERE (?1 IS NULL OR project_path = ?1)
             ORDER BY occurred_at, session_id, coalesce(sequence, 9223372036854775807), row_id",
        )?;
        let rows = statement.query_map([project], |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            let json = row?;
            Event::from_json_str(&json).map_err(StoreError::from)
        })
        .collect()
    }

    /// Search the explicit redaction-approved FTS5 projection.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for blank or invalid FTS5 queries and `SQLite`
    /// failures.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>, StoreError> {
        if query.trim().is_empty() {
            return Err(StoreError::EmptySearchQuery);
        }
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut statement = self.connection.prepare(
            "SELECT
                events.event_id,
                bm25(events_fts),
                snippet(events_fts, -1, '[', ']', ' … ', 16)
             FROM events_fts
             JOIN events ON events.row_id = events_fts.rowid
             WHERE events_fts MATCH ?1
             ORDER BY bm25(events_fts), events.occurred_at, events.row_id
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![query, i64::from(limit)], |row| {
            Ok(SearchHit {
                event_id: row.get(0)?,
                rank: row.get(1)?,
                snippet: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Return diagnostic row counts without exposing the `SQLite` connection.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot execute the query.
    pub fn stats(&self) -> Result<StoreStats, StoreError> {
        Ok(self.connection.query_row(
            "SELECT
                (SELECT count(*) FROM sources),
                (SELECT count(*) FROM sessions),
                (SELECT count(*) FROM events),
                (SELECT count(*) FROM artifacts),
                (SELECT count(*) FROM event_conflicts)",
            [],
            |row| {
                Ok(StoreStats {
                    sources: row.get(0)?,
                    sessions: row.get(1)?,
                    events: row.get(2)?,
                    artifacts: row.get(3)?,
                    conflicts: row.get(4)?,
                })
            },
        )?)
    }

    /// Register one immutable candidate and its exact evidence links.
    ///
    /// Identical packages are idempotent. A different package under the same
    /// ID is rejected, while an equivalent trigger/intervention returns the
    /// existing candidate without writing a duplicate.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for conflicting content, missing evidence,
    /// serialization, or transaction failures.
    pub fn register_mutation(
        &mut self,
        registration: &MutationRegistration,
    ) -> Result<MutationRegisterOutcome, StoreError> {
        let package_json = serde_json::to_string(&registration.package)?;
        let content_hash = util::sha256(package_json.as_bytes());
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing_hash) = transaction
            .query_row(
                "SELECT content_hash FROM mutation_candidates WHERE mutation_id = ?1",
                [&registration.mutation_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?
        {
            if existing_hash.as_slice() == content_hash {
                return Ok(MutationRegisterOutcome::Duplicate {
                    mutation_id: registration.mutation_id.clone(),
                });
            }
            return Err(StoreError::MutationContentConflict {
                mutation_id: registration.mutation_id.clone(),
            });
        }
        if let Some(existing_mutation_id) = transaction
            .query_row(
                "SELECT mutation_id FROM mutation_candidates
                 WHERE equivalence_key = ?1 OR source_finding_id = ?2",
                params![registration.equivalence_key, registration.source_finding_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return Ok(MutationRegisterOutcome::EquivalentExisting {
                mutation_id: registration.mutation_id.clone(),
                existing_mutation_id,
            });
        }
        let now = util::now_timestamp()?;
        transaction.execute(
            "INSERT INTO mutation_candidates(
                mutation_id, source_finding_id, source_detector, equivalence_key,
                spec_version, semantic_version, state, package_json, content_hash,
                created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'candidate', ?7, ?8, ?9, ?9)",
            params![
                registration.mutation_id,
                registration.source_finding_id,
                registration.source_detector,
                registration.equivalence_key,
                registration.spec_version,
                registration.semantic_version,
                package_json,
                content_hash.as_slice(),
                now,
            ],
        )?;
        transaction.execute(
            "INSERT INTO mutation_transitions(
                mutation_id, from_state, to_state, reason, metadata_json, occurred_at
             ) VALUES (?1, NULL, 'candidate', 'generated from evidence', '{}', ?2)",
            params![registration.mutation_id, now],
        )?;
        insert_mutation_evidence(
            &transaction,
            &registration.mutation_id,
            "support",
            &registration.supporting_event_ids,
        )?;
        insert_mutation_evidence(
            &transaction,
            &registration.mutation_id,
            "counterexample",
            &registration.counterexample_event_ids,
        )?;
        transaction.commit()?;
        Ok(MutationRegisterOutcome::Inserted {
            mutation_id: registration.mutation_id.clone(),
        })
    }

    /// List candidates in most-recently-updated order.
    ///
    /// # Errors
    /// Returns [`StoreError`] for database or persisted JSON failures.
    pub fn list_mutations(&self) -> Result<Vec<MutationRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT mutation_id, source_finding_id, source_detector, equivalence_key,
                    spec_version, semantic_version, state, package_json,
                    challenge_json, rejection_reason, created_at, updated_at
             FROM mutation_candidates
             ORDER BY updated_at DESC, mutation_id",
        )?;
        let rows = statement.query_map([], raw_mutation_record)?;
        rows.map(|row| mutation_record(row?)).collect()
    }

    /// Return one candidate and its complete lifecycle audit.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the candidate is missing or data is invalid.
    pub fn get_mutation(&self, mutation_id: &str) -> Result<MutationDetails, StoreError> {
        let raw = self
            .connection
            .query_row(
                "SELECT mutation_id, source_finding_id, source_detector, equivalence_key,
                        spec_version, semantic_version, state, package_json,
                        challenge_json, rejection_reason, created_at, updated_at
                 FROM mutation_candidates WHERE mutation_id = ?1",
                [mutation_id],
                raw_mutation_record,
            )
            .optional()?
            .ok_or_else(|| StoreError::MutationNotFound {
                mutation_id: mutation_id.to_owned(),
            })?;
        let mutation = mutation_record(raw)?;
        let mut statement = self.connection.prepare(
            "SELECT transition_id, mutation_id, from_state, to_state, reason,
                    metadata_json, occurred_at
             FROM mutation_transitions
             WHERE mutation_id = ?1
             ORDER BY occurred_at, transition_id",
        )?;
        let rows = statement.query_map([mutation_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        let transitions = rows
            .map(|row| {
                let (
                    transition_id,
                    mutation_id,
                    from_state,
                    to_state,
                    reason,
                    metadata,
                    occurred_at,
                ) = row?;
                Ok(MutationTransition {
                    transition_id,
                    mutation_id,
                    from_state,
                    to_state,
                    reason,
                    metadata: serde_json::from_str(&metadata)?,
                    occurred_at,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let mut statement = self.connection.prepare(
            "SELECT replay_id, mutation_id, scenario_set_hash, report_json, passed, created_at
             FROM mutation_replays
             WHERE mutation_id = ?1
             ORDER BY created_at, replay_id",
        )?;
        let rows = statement.query_map([mutation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let replays = rows
            .map(|row| {
                let (replay_id, mutation_id, scenario_set_hash, report, passed, created_at) = row?;
                Ok(MutationReplayRecord {
                    replay_id,
                    mutation_id,
                    scenario_set_hash,
                    report: serde_json::from_str(&report)?,
                    passed,
                    created_at,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(MutationDetails {
            mutation,
            transitions,
            replays,
        })
    }

    /// Persist one immutable replay report and advance a challenged candidate only on pass.
    ///
    /// Failed reports remain auditable without changing lifecycle state. Identical
    /// report registration is a no-op.
    ///
    /// # Errors
    /// Returns [`StoreError`] for inconsistent report metadata, content
    /// conflicts, invalid lifecycle state, or database failures.
    #[allow(clippy::too_many_lines)]
    pub fn register_replay(
        &mut self,
        registration: &ReplayRegistration,
    ) -> Result<ReplayRegisterOutcome, StoreError> {
        if !replay_report_matches_registration(registration) {
            return Err(StoreError::InvalidReplayRegistration);
        }
        let report_json = serde_json::to_string(&registration.report)?;
        let content_hash = util::sha256(report_json.as_bytes());
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing_hash) = transaction
            .query_row(
                "SELECT content_hash FROM mutation_replays WHERE replay_id = ?1",
                [&registration.replay_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?
        {
            if existing_hash.as_slice() != content_hash {
                return Err(StoreError::ReplayContentConflict {
                    replay_id: registration.replay_id.clone(),
                });
            }
            let mutation_state = transaction.query_row(
                "SELECT state FROM mutation_candidates WHERE mutation_id = ?1",
                [&registration.mutation_id],
                |row| row.get::<_, String>(0),
            )?;
            return Ok(ReplayRegisterOutcome::Duplicate {
                replay_id: registration.replay_id.clone(),
                mutation_state,
            });
        }
        let from_state = transaction
            .query_row(
                "SELECT state FROM mutation_candidates WHERE mutation_id = ?1",
                [&registration.mutation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::MutationNotFound {
                mutation_id: registration.mutation_id.clone(),
            })?;
        if from_state != "challenged" {
            return Err(StoreError::MutationStateTransition {
                mutation_id: registration.mutation_id.clone(),
                from_state,
                to_state: "replay_passed",
            });
        }
        let now = util::now_timestamp()?;
        transaction.execute(
            "INSERT INTO mutation_replays(
                replay_id, mutation_id, scenario_set_hash, report_json,
                content_hash, passed, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                registration.replay_id,
                registration.mutation_id,
                registration.scenario_set_hash,
                report_json,
                content_hash.as_slice(),
                registration.passed,
                now,
            ],
        )?;
        for (ordinal, event_id) in registration.source_event_ids.iter().enumerate() {
            let stored_ordinal = i64::try_from(ordinal)
                .map_err(|_| StoreError::ReplayEvidenceOrdinalOutOfRange { ordinal })?;
            transaction.execute(
                "INSERT INTO mutation_replay_evidence(replay_id, event_id, ordinal)
                 VALUES (?1, ?2, ?3)",
                params![registration.replay_id, event_id, stored_ordinal],
            )?;
        }
        let mutation_state = if registration.passed {
            transaction.execute(
                "UPDATE mutation_candidates
                 SET state = 'replay_passed', updated_at = ?2
                 WHERE mutation_id = ?1",
                params![registration.mutation_id, now],
            )?;
            let metadata = serde_json::to_string(&serde_json::json!({
                "replay_id": registration.replay_id,
                "scenario_set_hash": registration.scenario_set_hash,
            }))?;
            transaction.execute(
                "INSERT INTO mutation_transitions(
                    mutation_id, from_state, to_state, reason, metadata_json, occurred_at
                 ) VALUES (?1, 'challenged', 'replay_passed',
                           'deterministic replay thresholds passed', ?2, ?3)",
                params![registration.mutation_id, metadata, now],
            )?;
            "replay_passed"
        } else {
            "challenged"
        };
        transaction.commit()?;
        Ok(ReplayRegisterOutcome::Inserted {
            replay_id: registration.replay_id.clone(),
            advanced: registration.passed,
            mutation_state: mutation_state.to_owned(),
        })
    }

    /// Mark a candidate challenged after the caller validates every checklist item.
    ///
    /// # Errors
    /// Returns [`StoreError`] for a missing candidate, invalid transition, or
    /// database/serialization failure.
    pub fn challenge_mutation(
        &mut self,
        mutation_id: &str,
        assessment: &serde_json::Value,
    ) -> Result<MutationTransitionOutcome, StoreError> {
        self.transition_mutation(
            mutation_id,
            "challenged",
            "challenge checklist completed",
            assessment,
        )
    }

    /// Reject a candidate or challenged mutation with an auditable reason.
    ///
    /// # Errors
    /// Returns [`StoreError`] for blank reasons, a missing candidate, invalid
    /// transition, or database failure.
    pub fn reject_mutation(
        &mut self,
        mutation_id: &str,
        reason: &str,
    ) -> Result<MutationTransitionOutcome, StoreError> {
        if reason.trim().is_empty() {
            return Err(StoreError::InvalidMutationReason);
        }
        self.transition_mutation(mutation_id, "rejected", reason, &serde_json::json!({}))
    }

    fn transition_mutation(
        &mut self,
        mutation_id: &str,
        to_state: &'static str,
        reason: &str,
        metadata: &serde_json::Value,
    ) -> Result<MutationTransitionOutcome, StoreError> {
        let metadata_json = serde_json::to_string(metadata)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let from_state = transaction
            .query_row(
                "SELECT state FROM mutation_candidates WHERE mutation_id = ?1",
                [mutation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::MutationNotFound {
                mutation_id: mutation_id.to_owned(),
            })?;
        if from_state == to_state {
            return Ok(MutationTransitionOutcome {
                mutation_id: mutation_id.to_owned(),
                from_state: from_state.clone(),
                to_state: from_state,
                changed: false,
            });
        }
        let allowed = matches!(
            (from_state.as_str(), to_state),
            ("candidate", "challenged")
                | ("candidate" | "challenged" | "replay_passed", "rejected")
        );
        if !allowed {
            return Err(StoreError::MutationStateTransition {
                mutation_id: mutation_id.to_owned(),
                from_state,
                to_state,
            });
        }
        let now = util::now_timestamp()?;
        let (challenge_json, rejection_reason) = if to_state == "challenged" {
            (Some(metadata_json.as_str()), None)
        } else {
            (None, Some(reason))
        };
        transaction.execute(
            "UPDATE mutation_candidates
             SET state = ?2, challenge_json = coalesce(?3, challenge_json),
                 rejection_reason = ?4, updated_at = ?5
             WHERE mutation_id = ?1",
            params![mutation_id, to_state, challenge_json, rejection_reason, now],
        )?;
        transaction.execute(
            "INSERT INTO mutation_transitions(
                mutation_id, from_state, to_state, reason, metadata_json, occurred_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                mutation_id,
                from_state,
                to_state,
                reason,
                metadata_json,
                now
            ],
        )?;
        transaction.commit()?;
        Ok(MutationTransitionOutcome {
            mutation_id: mutation_id.to_owned(),
            from_state,
            to_state: to_state.to_owned(),
            changed: true,
        })
    }

    /// Delete a session, its events, search rows, conflicts, and orphaned artifacts.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the deletion transaction fails.
    pub fn delete_session(&mut self, session_id: &str) -> Result<DeleteSummary, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let session_exists = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = ?1)",
            [session_id],
            |row| row.get::<_, bool>(0),
        )?;
        if !session_exists {
            return Ok(DeleteSummary::default());
        }

        let events_deleted = transaction.query_row(
            "SELECT count(*) FROM events WHERE session_id = ?1",
            [session_id],
            |row| row.get::<_, i64>(0),
        )?;
        let artifacts_before =
            transaction.query_row("SELECT count(*) FROM artifacts", [], |row| {
                row.get::<_, i64>(0)
            })?;
        let mutations_before = count_mutations(&transaction)?;

        transaction.execute("DELETE FROM sessions WHERE session_id = ?1", [session_id])?;
        transaction.execute(
            "DELETE FROM artifacts
             WHERE NOT EXISTS (
                SELECT 1 FROM event_artifacts
                WHERE event_artifacts.artifact_id = artifacts.artifact_id
             )",
            [],
        )?;
        let artifacts_after =
            transaction.query_row("SELECT count(*) FROM artifacts", [], |row| {
                row.get::<_, i64>(0)
            })?;
        let mutations_after = count_mutations(&transaction)?;
        transaction.commit()?;

        Ok(DeleteSummary {
            session_deleted: true,
            events_deleted,
            artifacts_deleted: artifacts_before - artifacts_after,
            mutations_deleted: mutations_before - mutations_after,
        })
    }

    /// Delete every local event, source, cursor, conflict, and artifact.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the deletion transaction fails.
    pub fn delete_all(&mut self) -> Result<DeleteAllSummary, StoreError> {
        let before = self.stats()?;
        let cursors_deleted =
            self.connection
                .query_row("SELECT count(*) FROM source_cursors", [], |row| row.get(0))?;
        let mutations_deleted =
            self.connection
                .query_row("SELECT count(*) FROM mutation_candidates", [], |row| {
                    row.get(0)
                })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM imports", [])?;
        transaction.execute("DELETE FROM source_cursors", [])?;
        transaction.execute("DELETE FROM mutation_candidates", [])?;
        transaction.execute("DELETE FROM sessions", [])?;
        transaction.execute("DELETE FROM artifacts", [])?;
        transaction.execute("DELETE FROM sources", [])?;
        transaction.commit()?;
        Ok(DeleteAllSummary {
            sources_deleted: before.sources,
            sessions_deleted: before.sessions,
            events_deleted: before.events,
            artifacts_deleted: before.artifacts,
            conflicts_deleted: before.conflicts,
            cursors_deleted,
            mutations_deleted,
        })
    }

    /// Delete sessions whose last event is strictly older than a cutoff.
    ///
    /// Dry runs execute the same transaction and roll it back, so reported
    /// artifact counts match a real prune. An exact project limits selection.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when timestamp formatting or the retention
    /// transaction fails.
    pub fn prune_before(
        &mut self,
        cutoff: OffsetDateTime,
        project: Option<&str>,
        dry_run: bool,
    ) -> Result<PruneSummary, StoreError> {
        let cutoff = util::canonical_timestamp(cutoff)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sessions_deleted = transaction.query_row(
            "SELECT count(*) FROM sessions
             WHERE last_event_at < ?1 AND (?2 IS NULL OR project_path = ?2)",
            params![cutoff, project],
            |row| row.get::<_, i64>(0),
        )?;
        let events_deleted = transaction.query_row(
            "SELECT count(*) FROM events
             WHERE session_id IN (
               SELECT session_id FROM sessions
               WHERE last_event_at < ?1 AND (?2 IS NULL OR project_path = ?2)
             )",
            params![cutoff, project],
            |row| row.get::<_, i64>(0),
        )?;
        let artifacts_before =
            transaction.query_row("SELECT count(*) FROM artifacts", [], |row| {
                row.get::<_, i64>(0)
            })?;
        let mutations_before = count_mutations(&transaction)?;
        transaction.execute(
            "DELETE FROM sessions
             WHERE last_event_at < ?1 AND (?2 IS NULL OR project_path = ?2)",
            params![cutoff, project],
        )?;
        transaction.execute(
            "DELETE FROM artifacts
             WHERE NOT EXISTS (
               SELECT 1 FROM event_artifacts
               WHERE event_artifacts.artifact_id = artifacts.artifact_id
             )",
            [],
        )?;
        let artifacts_after =
            transaction.query_row("SELECT count(*) FROM artifacts", [], |row| {
                row.get::<_, i64>(0)
            })?;
        let mutations_after = count_mutations(&transaction)?;
        let summary = PruneSummary {
            sessions_deleted,
            events_deleted,
            artifacts_deleted: artifacts_before - artifacts_after,
            mutations_deleted: mutations_before - mutations_after,
            dry_run,
        };
        if dry_run {
            transaction.rollback()?;
        } else {
            transaction.commit()?;
        }
        Ok(summary)
    }

    fn from_connection(mut connection: Connection) -> Result<Self, StoreError> {
        configure(&connection)?;
        migration::apply(&mut connection)?;
        Ok(Self { connection })
    }
}

fn configure(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "secure_delete", true)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

struct RawMutationRecord {
    mutation_id: String,
    source_finding_id: String,
    source_detector: String,
    equivalence_key: String,
    spec_version: String,
    semantic_version: String,
    state: String,
    package_json: String,
    challenge_json: Option<String>,
    rejection_reason: Option<String>,
    created_at: String,
    updated_at: String,
}

fn raw_mutation_record(row: &rusqlite::Row<'_>) -> Result<RawMutationRecord, rusqlite::Error> {
    Ok(RawMutationRecord {
        mutation_id: row.get(0)?,
        source_finding_id: row.get(1)?,
        source_detector: row.get(2)?,
        equivalence_key: row.get(3)?,
        spec_version: row.get(4)?,
        semantic_version: row.get(5)?,
        state: row.get(6)?,
        package_json: row.get(7)?,
        challenge_json: row.get(8)?,
        rejection_reason: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn mutation_record(raw: RawMutationRecord) -> Result<MutationRecord, StoreError> {
    Ok(MutationRecord {
        mutation_id: raw.mutation_id,
        source_finding_id: raw.source_finding_id,
        source_detector: raw.source_detector,
        equivalence_key: raw.equivalence_key,
        spec_version: raw.spec_version,
        semantic_version: raw.semantic_version,
        state: raw.state,
        package: serde_json::from_str(&raw.package_json)?,
        challenge: raw
            .challenge_json
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        rejection_reason: raw.rejection_reason,
        created_at: raw.created_at,
        updated_at: raw.updated_at,
    })
}

fn insert_mutation_evidence(
    transaction: &Transaction<'_>,
    mutation_id: &str,
    role: &str,
    event_ids: &[String],
) -> Result<(), StoreError> {
    for (ordinal, event_id) in event_ids.iter().enumerate() {
        let stored_ordinal = i64::try_from(ordinal)
            .map_err(|_| StoreError::MutationEvidenceOrdinalOutOfRange { ordinal })?;
        transaction.execute(
            "INSERT INTO mutation_evidence(mutation_id, event_id, role, ordinal)
             VALUES (?1, ?2, ?3, ?4)",
            params![mutation_id, event_id, role, stored_ordinal],
        )?;
    }
    Ok(())
}

fn count_mutations(transaction: &Transaction<'_>) -> Result<i64, rusqlite::Error> {
    transaction.query_row("SELECT count(*) FROM mutation_candidates", [], |row| {
        row.get(0)
    })
}

fn replay_report_matches_registration(registration: &ReplayRegistration) -> bool {
    if registration
        .report
        .get("replay_id")
        .and_then(serde_json::Value::as_str)
        != Some(&registration.replay_id)
        || registration
            .report
            .get("mutation_id")
            .and_then(serde_json::Value::as_str)
            != Some(&registration.mutation_id)
        || registration
            .report
            .get("scenario_set_hash")
            .and_then(serde_json::Value::as_str)
            != Some(&registration.scenario_set_hash)
        || registration
            .report
            .get("passed")
            .and_then(serde_json::Value::as_bool)
            != Some(registration.passed)
    {
        return false;
    }
    let Some(results) = registration
        .report
        .get("results")
        .and_then(serde_json::Value::as_array)
    else {
        return false;
    };
    let mut report_event_values = Vec::new();
    for result in results {
        let Some(event_ids) = result
            .get("source_event_ids")
            .and_then(serde_json::Value::as_array)
        else {
            return false;
        };
        for event_id in event_ids {
            let Some(event_id) = event_id.as_str() else {
                return false;
            };
            report_event_values.push(event_id);
        }
    }
    let report_event_ids = report_event_values.iter().copied().collect::<BTreeSet<_>>();
    let registered_event_ids = registration
        .source_event_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    !report_event_ids.is_empty()
        && report_event_ids.len() == report_event_values.len()
        && report_event_ids.len() == registration.source_event_ids.len()
        && report_event_ids == registered_event_ids
}

fn validate_source(source: &SourceIdentity) -> Result<(), StoreError> {
    for (field, value) in [
        ("adapter", source.adapter.as_str()),
        ("instance_key", source.instance_key.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(StoreError::InvalidSource { field });
        }
    }
    if source
        .display_name
        .as_ref()
        .is_some_and(|name| name.trim().is_empty())
    {
        return Err(StoreError::InvalidSource {
            field: "display_name",
        });
    }
    Ok(())
}

fn validate_cursor_origin(origin: &str) -> Result<(), StoreError> {
    if origin.trim().is_empty() {
        Err(StoreError::InvalidCursorOrigin)
    } else {
        Ok(())
    }
}

fn persisted_tool_input(event: &Event) -> Result<Option<String>, serde_json::Error> {
    event
        .tool
        .as_ref()
        .and_then(|tool| tool.input.as_ref())
        .map(|input| match input {
            serde_json::Value::String(value) => Ok(value.clone()),
            value => serde_json::to_string(value),
        })
        .transpose()
}

fn quarantine_conflict(
    transaction: Transaction<'_>,
    source: &SourceIdentity,
    event: &Event,
    event_json: &str,
    existing_hash: &[u8],
    conflicting_hash: &[u8; 32],
    observed_at: &str,
) -> Result<InsertOutcome, StoreError> {
    transaction.execute(
        "INSERT INTO event_conflicts(
            event_id, existing_content_hash, conflicting_content_hash,
            conflicting_event_json, source_adapter, source_instance_key,
            first_seen_at, last_seen_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT(event_id, conflicting_content_hash) DO UPDATE SET
            last_seen_at = excluded.last_seen_at,
            observation_count = event_conflicts.observation_count + 1",
        params![
            event.event_id.as_str(),
            existing_hash,
            conflicting_hash.as_slice(),
            event_json,
            source.adapter,
            source.instance_key,
            observed_at,
        ],
    )?;
    let (conflict_id, observation_count) = transaction.query_row(
        "SELECT conflict_id, observation_count
         FROM event_conflicts
         WHERE event_id = ?1 AND conflicting_content_hash = ?2",
        params![event.event_id.as_str(), conflicting_hash.as_slice()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    transaction.commit()?;
    Ok(InsertOutcome::ConflictQuarantined {
        conflict_id,
        observation_count,
    })
}

fn upsert_source(
    transaction: &Transaction<'_>,
    source: &SourceIdentity,
    occurred_at: &str,
) -> Result<i64, rusqlite::Error> {
    transaction.execute(
        "INSERT INTO sources(
            adapter, instance_key, display_name, first_seen_at, last_seen_at
         ) VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(adapter, instance_key) DO UPDATE SET
            display_name = coalesce(excluded.display_name, sources.display_name),
            first_seen_at = min(sources.first_seen_at, excluded.first_seen_at),
            last_seen_at = max(sources.last_seen_at, excluded.last_seen_at)",
        params![
            source.adapter,
            source.instance_key,
            source.display_name,
            occurred_at
        ],
    )?;
    transaction.query_row(
        "SELECT source_id FROM sources WHERE adapter = ?1 AND instance_key = ?2",
        params![source.adapter, source.instance_key],
        |row| row.get(0),
    )
}

fn ensure_session(
    transaction: &Transaction<'_>,
    source_id: i64,
    event: &Event,
    occurred_at: &str,
) -> Result<(), StoreError> {
    if let Some(existing_source_id) = transaction
        .query_row(
            "SELECT source_id FROM sessions WHERE session_id = ?1",
            [event.session_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        if existing_source_id != source_id {
            return Err(StoreError::SessionSourceConflict {
                session_id: event.session_id.to_string(),
            });
        }
        return Ok(());
    }

    let started_at = (event.kind == EventKind::SessionStarted).then_some(occurred_at);
    let ended_at = (event.kind == EventKind::SessionEnded).then_some(occurred_at);
    transaction.execute(
        "INSERT INTO sessions(
            session_id, source_id, project_path, started_at, ended_at,
            first_event_at, last_event_at, event_count
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 0)",
        params![
            event.session_id.as_str(),
            source_id,
            event.project.as_deref(),
            started_at,
            ended_at,
            occurred_at,
        ],
    )?;
    Ok(())
}

fn ensure_sequence_available(
    transaction: &Transaction<'_>,
    event: &Event,
    sequence: Option<i64>,
) -> Result<(), StoreError> {
    let Some(sequence) = sequence else {
        return Ok(());
    };
    if let Some(existing_event_id) = transaction
        .query_row(
            "SELECT event_id FROM events WHERE session_id = ?1 AND sequence = ?2",
            params![event.session_id.as_str(), sequence],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        return Err(StoreError::SessionSequenceConflict {
            session_id: event.session_id.to_string(),
            sequence,
            existing_event_id,
        });
    }
    Ok(())
}

fn insert_artifacts(
    transaction: &Transaction<'_>,
    event_row_id: i64,
    event: &Event,
) -> Result<(), StoreError> {
    for (ordinal, artifact) in event.artifacts.iter().enumerate() {
        let ordinal = i64::try_from(ordinal)
            .map_err(|_| StoreError::ArtifactOrdinalOutOfRange { ordinal })?;
        let metadata_json = serde_json::to_string(&artifact.metadata)?;
        transaction.execute(
            "INSERT OR IGNORE INTO artifacts(
                artifact_type, path, uri, digest, metadata_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                artifact.kind.as_str(),
                artifact.path.as_deref(),
                artifact.uri.as_deref(),
                artifact.digest.as_deref(),
                metadata_json,
            ],
        )?;
        let artifact_id = transaction.query_row(
            "SELECT artifact_id FROM artifacts
             WHERE artifact_type = ?1
               AND path IS ?2
               AND uri IS ?3
               AND digest IS ?4",
            params![
                artifact.kind.as_str(),
                artifact.path.as_deref(),
                artifact.uri.as_deref(),
                artifact.digest.as_deref(),
            ],
            |row| row.get::<_, i64>(0),
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO event_artifacts(event_row_id, artifact_id, ordinal)
             VALUES (?1, ?2, ?3)",
            params![event_row_id, artifact_id, ordinal],
        )?;
    }
    Ok(())
}

fn update_session_rollup(
    transaction: &Transaction<'_>,
    event: &Event,
    occurred_at: &str,
) -> Result<(), rusqlite::Error> {
    let started_at = (event.kind == EventKind::SessionStarted).then_some(occurred_at);
    let ended_at = (event.kind == EventKind::SessionEnded).then_some(occurred_at);
    transaction.execute(
        "UPDATE sessions SET
            project_path = coalesce(project_path, ?2),
            started_at = CASE
                WHEN ?3 IS NULL THEN started_at
                WHEN started_at IS NULL THEN ?3
                ELSE min(started_at, ?3)
            END,
            ended_at = CASE
                WHEN ?4 IS NULL THEN ended_at
                WHEN ended_at IS NULL THEN ?4
                ELSE max(ended_at, ?4)
            END,
            first_event_at = min(first_event_at, ?5),
            last_event_at = max(last_event_at, ?5),
            event_count = event_count + 1
         WHERE session_id = ?1",
        params![
            event.session_id.as_str(),
            event.project.as_deref(),
            started_at,
            ended_at,
            occurred_at,
        ],
    )?;
    Ok(())
}
