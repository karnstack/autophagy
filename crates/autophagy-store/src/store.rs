use std::{path::Path, time::Duration};

use autophagy_events::{Event, EventKind};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    DeleteSummary, InsertOutcome, SearchHit, SearchProjection, SessionSummary, SourceCursor,
    SourceIdentity, StoreError, StoreStats, migration, util,
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
        transaction.commit()?;

        Ok(DeleteSummary {
            session_deleted: true,
            events_deleted,
            artifacts_deleted: artifacts_before - artifacts_after,
        })
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
