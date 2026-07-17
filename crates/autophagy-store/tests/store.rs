//! Integration tests for transactional event storage and retrieval.

use std::collections::BTreeMap;

use autophagy_events::{
    Artifact, ArtifactKind, Event, EventId, EventKind, SessionId, SpecVersion, ToolCall,
};
use autophagy_store::{
    DeleteAllSummary, DeleteSummary, EventStore, InsertOutcome, InstallationRegistration,
    InstallationTransitionOutcome, MutationRegisterOutcome, MutationRegistration, PruneSummary,
    RebuildSummary, ReplayRegisterOutcome, ReplayRegistration, RetrievalMatchKind,
    RetrievalOutcome, RetrievalQuery, SearchProjection, ShadowRegisterOutcome, ShadowRegistration,
    SourceCursor, SourceIdentity, StoreError, StoreStats,
};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[test]
fn migrations_persist_and_reopen_cleanly() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let source = source("instance-a");
    let event = session_event(
        "evt_session-start",
        "ses_reopen",
        EventKind::SessionStarted,
        "2026-07-16T01:00:00Z",
        0,
    );

    {
        let mut store = EventStore::open(&database).expect("store should open");
        assert_eq!(store.schema_version().expect("schema version"), 2);
        assert!(matches!(
            store
                .insert_event(&source, &event, &SearchProjection::default())
                .expect("insert should succeed"),
            InsertOutcome::Inserted { .. }
        ));
    }

    let reopened = EventStore::open(&database).expect("store should reopen");
    assert_eq!(reopened.schema_version().expect("schema version"), 2);
    assert_eq!(
        reopened
            .get_event(event.event_id.as_str())
            .expect("event query"),
        Some(event)
    );
}

#[test]
fn source_cursors_round_trip_and_update() {
    let store = EventStore::open_in_memory().expect("store");
    let source = source("instance-cursor");
    assert_eq!(
        store
            .get_source_cursor(&source, "project/session.jsonl")
            .expect("missing cursor"),
        None
    );

    let mut cursor = SourceCursor {
        byte_offset: 120,
        line_number: 3,
        head_hash: [7; 32],
        state: json!({"pending": {"tool-1": "bash"}}),
    };
    store
        .save_source_cursor(&source, "project/session.jsonl", &cursor)
        .expect("save cursor");
    assert_eq!(
        store
            .get_source_cursor(&source, "project/session.jsonl")
            .expect("load cursor"),
        Some(cursor.clone())
    );

    cursor.byte_offset = 240;
    cursor.line_number = 6;
    store
        .save_source_cursor(&source, "project/session.jsonl", &cursor)
        .expect("update cursor");
    assert_eq!(
        store
            .get_source_cursor(&source, "project/session.jsonl")
            .expect("load updated cursor"),
        Some(cursor)
    );
    assert!(matches!(
        store.get_source_cursor(&source, "  "),
        Err(StoreError::InvalidCursorOrigin)
    ));
}

#[test]
fn insertion_rolls_up_sessions_and_indexes_only_approved_text() {
    let mut store = EventStore::open_in_memory().expect("store should open");
    let source = source("instance-a");
    let mut failure = tool_failure("evt_failure", "ses_rollup", "2026-07-16T01:10:00+00:00", 1);
    failure
        .metadata
        .insert("private".to_owned(), json!("supersecretneedle"));
    failure.artifacts.push(file_artifact("src/schema.graphql"));
    let approved = SearchProjection {
        tool_input_text: Some("pytest translation".to_owned()),
        searchable_text: Some("generated client was stale".to_owned()),
        signature: None,
    };

    store
        .insert_event(&source, &failure, &approved)
        .expect("failure insert");
    store
        .insert_event(
            &source,
            &session_event(
                "evt_start",
                "ses_rollup",
                EventKind::SessionStarted,
                "2026-07-16T01:00:00-00:00",
                0,
            ),
            &SearchProjection::default(),
        )
        .expect("start insert");
    store
        .insert_event(
            &source,
            &session_event(
                "evt_end",
                "ses_rollup",
                EventKind::SessionEnded,
                "2026-07-16T01:20:00Z",
                2,
            ),
            &SearchProjection::default(),
        )
        .expect("end insert");

    let session = store
        .get_session("ses_rollup")
        .expect("session query")
        .expect("session should exist");
    assert_eq!(session.event_count, 3);
    assert_eq!(session.first_event_at, "2026-07-16T01:00:00Z");
    assert_eq!(session.last_event_at, "2026-07-16T01:20:00Z");
    assert_eq!(session.started_at.as_deref(), Some("2026-07-16T01:00:00Z"));
    assert_eq!(session.ended_at.as_deref(), Some("2026-07-16T01:20:00Z"));
    assert_eq!(session.project_path.as_deref(), Some("/workspace/project"));
    assert_eq!(store.list_sessions(1).expect("session list"), vec![session]);
    assert!(
        store
            .list_sessions(0)
            .expect("empty session list")
            .is_empty()
    );

    let hits = store.search("generated", 10).expect("approved search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].event_id, "evt_failure");
    assert!(hits[0].snippet.contains("[generated]"));
    assert_eq!(store.search("pytest", 10).expect("tool search").len(), 1);
    assert!(
        store
            .search("supersecretneedle", 10)
            .expect("private metadata search")
            .is_empty()
    );
    assert!(
        store
            .search("\"private-command-token\"", 10)
            .expect("raw tool input search")
            .is_empty()
    );
    assert_eq!(
        store.stats().expect("stats"),
        StoreStats {
            sources: 1,
            sessions: 1,
            events: 3,
            artifacts: 1,
            conflicts: 0,
        }
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn rebuild_search_projection_is_gated_idempotent_and_ignores_removed_events() {
    let mut store = EventStore::open_in_memory().expect("store should open");
    let source = source("instance-a");

    // Import three events with NO approved projection, exactly as history
    // ingested before signature indexing existed would look: canonical rows
    // present, but the derived search index empty.
    store
        .insert_event(
            &source,
            &session_event(
                "evt_start",
                "ses_reindex",
                EventKind::SessionStarted,
                "2026-07-16T01:00:00Z",
                0,
            ),
            &SearchProjection::default(),
        )
        .expect("start insert");
    store
        .insert_event(
            &source,
            &tool_failure("evt_fail", "ses_reindex", "2026-07-16T01:10:00Z", 1),
            &SearchProjection::default(),
        )
        .expect("failure insert");
    // A quarantined conflict must never be resurrected into the rebuilt index:
    // reuse an existing event ID with different content.
    let mut conflict = tool_failure("evt_fail", "ses_reindex", "2026-07-16T01:10:00Z", 1);
    conflict
        .metadata
        .insert("changed".to_owned(), Value::Bool(true));
    store
        .insert_event(&source, &conflict, &SearchProjection::default())
        .expect("conflict quarantined");

    assert_eq!(store.signature_count().expect("signatures"), 0);
    assert!(
        store
            .search("command", 10)
            .expect("pre-rebuild search")
            .is_empty(),
        "raw tool input must not be searchable before rebuild"
    );

    // Gated OFF: rebuilding with empty projections writes one baseline row per
    // event and no signatures.
    let disabled = store
        .rebuild_search_projection(|_event| Some(SearchProjection::default()))
        .expect("gated-off rebuild");
    assert_eq!(
        disabled,
        RebuildSummary {
            events_scanned: 2,
            search_rows_written: 2,
            signatures_written: 0,
        },
        "quarantined conflict has no canonical row, so it is not scanned"
    );
    assert_eq!(store.signature_count().expect("signatures"), 0);

    // Gated ON: project the redaction-approved tool input and signature. This
    // mirrors what `autophagy reindex --index-tool-input` derives per event.
    let project = |event: &Event| {
        let tool_input_text = event
            .tool
            .as_ref()
            .and_then(|tool| tool.input.as_ref())
            .map(|input| {
                input
                    .as_str()
                    .map_or_else(|| input.to_string(), str::to_owned)
            });
        Some(SearchProjection {
            tool_input_text,
            searchable_text: None,
            signature: autophagy_events::signature::normalize_operation(event)
                .map(|operation| operation.operation_key()),
        })
    };
    let enabled = store
        .rebuild_search_projection(project)
        .expect("gated-on rebuild");
    assert_eq!(
        enabled,
        RebuildSummary {
            events_scanned: 2,
            search_rows_written: 2,
            signatures_written: 1,
        }
    );
    assert_eq!(store.signature_count().expect("signatures"), 1);
    let hits = store.search("command", 10).expect("post-rebuild search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].event_id, "evt_fail");

    // Idempotent: an identical rebuild yields identical counts and state.
    let again = store
        .rebuild_search_projection(project)
        .expect("re-rebuild");
    assert_eq!(again, enabled);
    assert_eq!(store.signature_count().expect("signatures"), 1);
    assert_eq!(store.search("command", 10).expect("stable search").len(), 1);

    // Canonical rows, sessions, and quarantine are untouched by the rebuild.
    assert_eq!(
        store.stats().expect("stats"),
        StoreStats {
            sources: 1,
            sessions: 1,
            events: 2,
            artifacts: 0,
            conflicts: 1,
        }
    );

    // Deleting a session cascades its projection rows; a rebuild then never
    // re-creates them because the canonical events are gone.
    store.delete_session("ses_reindex").expect("delete session");
    let after_delete = store
        .rebuild_search_projection(project)
        .expect("rebuild after delete");
    assert_eq!(after_delete, RebuildSummary::default());
    assert_eq!(store.signature_count().expect("signatures"), 0);
}

#[test]
fn duplicate_is_a_noop_and_conflicting_content_is_quarantined() {
    let mut store = EventStore::open_in_memory().expect("store should open");
    let source = source("instance-a");
    let original = tool_failure("evt_stable", "ses_idempotent", "2026-07-16T02:00:00Z", 0);
    let inserted = store
        .insert_event(
            &source,
            &original,
            &SearchProjection {
                tool_input_text: None,
                searchable_text: Some("first projection".to_owned()),
                signature: None,
            },
        )
        .expect("first insert");
    let row_id = match inserted {
        InsertOutcome::Inserted { row_id } => row_id,
        outcome => panic!("unexpected first outcome: {outcome:?}"),
    };

    assert_eq!(
        store
            .insert_event(
                &source,
                &original,
                &SearchProjection {
                    tool_input_text: None,
                    searchable_text: Some("must not replace projection".to_owned()),
                    signature: None,
                },
            )
            .expect("duplicate insert"),
        InsertOutcome::Duplicate { row_id }
    );
    assert!(
        store
            .search("replace", 10)
            .expect("duplicate projection search")
            .is_empty()
    );

    let mut conflict = original.clone();
    conflict
        .metadata
        .insert("changed".to_owned(), Value::Bool(true));
    let first_conflict = store
        .insert_event(&source, &conflict, &SearchProjection::default())
        .expect("conflict should be quarantined");
    let conflict_id = match first_conflict {
        InsertOutcome::ConflictQuarantined {
            conflict_id,
            observation_count: 1,
        } => conflict_id,
        outcome => panic!("unexpected conflict outcome: {outcome:?}"),
    };
    assert_eq!(
        store
            .insert_event(&source, &conflict, &SearchProjection::default())
            .expect("repeated conflict"),
        InsertOutcome::ConflictQuarantined {
            conflict_id,
            observation_count: 2,
        }
    );
    assert_eq!(
        store
            .get_event(original.event_id.as_str())
            .expect("canonical query"),
        Some(original)
    );
    assert_eq!(
        store.stats().expect("stats"),
        StoreStats {
            sources: 1,
            sessions: 1,
            events: 1,
            artifacts: 0,
            conflicts: 1,
        }
    );
}

#[test]
fn provenance_and_sequence_conflicts_roll_back_atomically() {
    let mut store = EventStore::open_in_memory().expect("store should open");
    let first_source = source("instance-a");
    let first = session_event(
        "evt_first",
        "ses_provenance",
        EventKind::SessionStarted,
        "2026-07-16T03:00:00Z",
        0,
    );
    store
        .insert_event(&first_source, &first, &SearchProjection::default())
        .expect("first insert");

    let second_source = source("instance-b");
    let source_conflict = session_event(
        "evt_second",
        "ses_provenance",
        EventKind::SessionEnded,
        "2026-07-16T03:10:00Z",
        1,
    );
    assert!(matches!(
        store.insert_event(
            &second_source,
            &source_conflict,
            &SearchProjection::default()
        ),
        Err(StoreError::SessionSourceConflict { .. })
    ));

    let sequence_conflict = session_event(
        "evt_third",
        "ses_provenance",
        EventKind::SessionEnded,
        "2026-07-16T03:20:00Z",
        0,
    );
    assert!(matches!(
        store.insert_event(
            &first_source,
            &sequence_conflict,
            &SearchProjection::default()
        ),
        Err(StoreError::SessionSequenceConflict { .. })
    ));
    assert_eq!(
        store.stats().expect("stats"),
        StoreStats {
            sources: 1,
            sessions: 1,
            events: 1,
            artifacts: 0,
            conflicts: 0,
        }
    );
    assert_eq!(
        store
            .get_session("ses_provenance")
            .expect("session query")
            .expect("session")
            .event_count,
        1
    );

    let wrong_adapter = SourceIdentity::new("claude-code", "instance-c");
    assert!(matches!(
        store.insert_event(&wrong_adapter, &first, &SearchProjection::default()),
        Err(StoreError::SourceMismatch { .. })
    ));
}

#[test]
fn deleting_sessions_cascades_search_and_only_orphaned_artifacts() {
    let mut store = EventStore::open_in_memory().expect("store should open");
    let source = source("instance-a");
    let shared = file_artifact("src/shared.rs");
    let mut first = session_event(
        "evt_delete-one",
        "ses_delete-one",
        EventKind::FileChanged,
        "2026-07-16T04:00:00Z",
        0,
    );
    first.artifacts = vec![shared.clone(), file_artifact("src/only-one.rs")];
    let mut second = session_event(
        "evt_delete-two",
        "ses_delete-two",
        EventKind::FileChanged,
        "2026-07-16T04:10:00Z",
        0,
    );
    second.artifacts = vec![shared, file_artifact("src/only-two.rs")];

    store
        .insert_event(
            &source,
            &first,
            &SearchProjection {
                tool_input_text: None,
                searchable_text: Some("delete marker one".to_owned()),
                signature: None,
            },
        )
        .expect("first insert");
    store
        .insert_event(
            &source,
            &second,
            &SearchProjection {
                tool_input_text: None,
                searchable_text: Some("delete marker two".to_owned()),
                signature: None,
            },
        )
        .expect("second insert");
    assert_eq!(store.stats().expect("stats").artifacts, 3);

    let deleted = store
        .delete_session("ses_delete-one")
        .expect("first deletion");
    assert!(deleted.session_deleted);
    assert_eq!(deleted.events_deleted, 1);
    assert_eq!(deleted.artifacts_deleted, 1);
    assert!(store.search("one", 10).expect("deleted search").is_empty());
    assert_eq!(store.search("two", 10).expect("retained search").len(), 1);
    assert_eq!(store.stats().expect("stats").artifacts, 2);

    assert_eq!(
        store
            .delete_session("ses_missing")
            .expect("missing deletion"),
        DeleteSummary::default()
    );
    let deleted = store
        .delete_session("ses_delete-two")
        .expect("second deletion");
    assert_eq!(deleted.artifacts_deleted, 2);
    assert_eq!(
        store.stats().expect("stats"),
        StoreStats {
            sources: 1,
            ..StoreStats::default()
        }
    );
}

#[test]
fn invalid_events_are_rejected_before_storage() {
    let mut store = EventStore::open_in_memory().expect("store should open");
    let source = source("instance-a");
    let mut invalid = session_event(
        "evt_valid",
        "ses_invalid",
        EventKind::SessionStarted,
        "2026-07-16T05:00:00Z",
        0,
    );
    invalid.event_id = EventId::new("not-an-event-id");
    assert!(matches!(
        store.insert_event(&source, &invalid, &SearchProjection::default()),
        Err(StoreError::InvalidEvent(_))
    ));
    let mut oversized_sequence = session_event(
        "evt_oversized-sequence",
        "ses_invalid",
        EventKind::SessionStarted,
        "2026-07-16T05:00:00Z",
        0,
    );
    oversized_sequence.sequence = Some(u64::MAX);
    assert!(matches!(
        store.insert_event(&source, &oversized_sequence, &SearchProjection::default()),
        Err(StoreError::SequenceOutOfRange { .. })
    ));
    assert_eq!(store.stats().expect("stats"), StoreStats::default());
    assert!(matches!(
        store.search("   ", 10),
        Err(StoreError::EmptySearchQuery)
    ));
}

#[test]
fn retention_preview_rolls_back_and_delete_all_removes_local_state() {
    let mut store = EventStore::open_in_memory().expect("store");
    let source = source("instance-retention");
    let mut old = tool_failure(
        "evt_retention-old",
        "ses_retention-old",
        "2026-07-01T00:00:00Z",
        0,
    );
    old.artifacts.push(file_artifact("old.log"));
    store
        .insert_event(&source, &old, &SearchProjection::default())
        .expect("old event");
    store
        .insert_event(
            &source,
            &session_event(
                "evt_retention-new",
                "ses_retention-new",
                EventKind::SessionStarted,
                "2026-07-15T00:00:00Z",
                0,
            ),
            &SearchProjection::default(),
        )
        .expect("new event");
    store
        .save_source_cursor(
            &source,
            "retention.jsonl",
            &SourceCursor {
                byte_offset: 10,
                line_number: 1,
                head_hash: [1; 32],
                state: json!({}),
            },
        )
        .expect("cursor");
    let cutoff = OffsetDateTime::parse("2026-07-10T00:00:00Z", &Rfc3339).expect("cutoff");
    assert_eq!(
        store.prune_before(cutoff, None, true).expect("preview"),
        PruneSummary {
            sessions_deleted: 1,
            events_deleted: 1,
            artifacts_deleted: 1,
            mutations_deleted: 0,
            dry_run: true,
        }
    );
    assert_eq!(store.stats().expect("stats after preview").events, 2);
    assert_eq!(
        store.prune_before(cutoff, None, false).expect("prune"),
        PruneSummary {
            sessions_deleted: 1,
            events_deleted: 1,
            artifacts_deleted: 1,
            mutations_deleted: 0,
            dry_run: false,
        }
    );
    assert_eq!(store.stats().expect("stats after prune").events, 1);
    assert_eq!(
        store.delete_all().expect("delete all"),
        DeleteAllSummary {
            sources_deleted: 1,
            sessions_deleted: 1,
            events_deleted: 1,
            artifacts_deleted: 0,
            conflicts_deleted: 0,
            cursors_deleted: 1,
            mutations_deleted: 0,
        }
    );
    assert_eq!(store.stats().expect("empty stats"), StoreStats::default());
}

#[test]
#[allow(clippy::too_many_lines)]
fn mutation_registry_is_idempotent_audited_and_evidence_bound() {
    let mut store = EventStore::open_in_memory().expect("store");
    let source = source("instance-mutations");
    for (event_id, session_id, timestamp) in [
        (
            "evt_mutation-support-a",
            "ses_mutation-support-a",
            "2026-07-16T06:00:00Z",
        ),
        (
            "evt_mutation-support-b",
            "ses_mutation-support-b",
            "2026-07-16T06:01:00Z",
        ),
        (
            "evt_mutation-counter",
            "ses_mutation-counter",
            "2026-07-16T06:02:00Z",
        ),
        ("evt_replay-only", "ses_replay-only", "2026-07-16T06:03:00Z"),
    ] {
        store
            .insert_event(
                &source,
                &session_event(
                    event_id,
                    session_id,
                    EventKind::DecisionRecorded,
                    timestamp,
                    0,
                ),
                &SearchProjection::default(),
            )
            .expect("evidence event");
    }
    let registration = mutation_registration("mut_registry", "fnd_registry", "eqv_registry");
    assert_eq!(
        store.register_mutation(&registration).expect("register"),
        MutationRegisterOutcome::Inserted {
            mutation_id: "mut_registry".to_owned(),
        }
    );
    assert_eq!(
        store.register_mutation(&registration).expect("duplicate"),
        MutationRegisterOutcome::Duplicate {
            mutation_id: "mut_registry".to_owned(),
        }
    );
    let equivalent = mutation_registration("mut_equivalent", "fnd_equivalent", "eqv_registry");
    assert_eq!(
        store.register_mutation(&equivalent).expect("equivalent"),
        MutationRegisterOutcome::EquivalentExisting {
            mutation_id: "mut_equivalent".to_owned(),
            existing_mutation_id: "mut_registry".to_owned(),
        }
    );
    assert_eq!(store.list_mutations().expect("list").len(), 1);
    let initial = store.get_mutation("mut_registry").expect("details");
    assert_eq!(initial.mutation.state, "candidate");
    assert_eq!(initial.transitions.len(), 1);

    let assessment = json!({"checks":["sessions_comparable","trigger_observable"]});
    let challenged = store
        .challenge_mutation("mut_registry", &assessment)
        .expect("challenge");
    assert!(challenged.changed);
    assert_eq!(challenged.to_state, "challenged");
    assert!(
        !store
            .challenge_mutation("mut_registry", &assessment)
            .expect("repeat")
            .changed
    );
    let failed_replay = replay_registration("rpl_failed", "rsh_failed", false);
    let mut mismatched_evidence = failed_replay.clone();
    mismatched_evidence.source_event_ids = vec!["evt_mutation-support-a".to_owned()];
    assert!(matches!(
        store.register_replay(&mismatched_evidence),
        Err(StoreError::InvalidReplayRegistration)
    ));
    assert_eq!(
        store
            .register_replay(&failed_replay)
            .expect("failed replay record"),
        ReplayRegisterOutcome::Inserted {
            replay_id: "rpl_failed".to_owned(),
            advanced: false,
            mutation_state: "challenged".to_owned(),
        }
    );
    assert_eq!(
        store
            .register_replay(&failed_replay)
            .expect("duplicate replay"),
        ReplayRegisterOutcome::Duplicate {
            replay_id: "rpl_failed".to_owned(),
            mutation_state: "challenged".to_owned(),
        }
    );
    let passing_replay = replay_registration("rpl_passing", "rsh_passing", true);
    assert_eq!(
        store
            .register_replay(&passing_replay)
            .expect("passing replay"),
        ReplayRegisterOutcome::Inserted {
            replay_id: "rpl_passing".to_owned(),
            advanced: true,
            mutation_state: "replay_passed".to_owned(),
        }
    );
    let failed_shadow = shadow_registration("shr_failed", "shh_failed", false);
    assert_eq!(
        store
            .register_shadow(&failed_shadow)
            .expect("failed shadow"),
        ShadowRegisterOutcome::Inserted {
            shadow_id: "shr_failed".to_owned(),
            advanced: false,
            mutation_state: "replay_passed".to_owned(),
        }
    );
    let passing_shadow = shadow_registration("shr_passing", "shh_passing", true);
    assert_eq!(
        store
            .register_shadow(&passing_shadow)
            .expect("passing shadow"),
        ShadowRegisterOutcome::Inserted {
            shadow_id: "shr_passing".to_owned(),
            advanced: true,
            mutation_state: "shadow_passed".to_owned(),
        }
    );
    let installation = installation_registration();
    assert_eq!(
        store
            .register_installation(&installation)
            .expect("installation"),
        InstallationTransitionOutcome {
            installation_id: "ins_registry".to_owned(),
            mutation_state: "active".to_owned(),
            installation_state: "installed".to_owned(),
        }
    );
    assert_eq!(
        store
            .get_installation("mut_registry")
            .expect("installation audit")
            .relative_path,
        ".agents/skills/autophagy-registry/SKILL.md"
    );
    assert!(matches!(
        store.delete_session("ses_replay-only"),
        Err(StoreError::ActiveInstallationBlocksEvidenceDeletion { .. })
    ));
    assert!(matches!(
        store.prune_before(
            OffsetDateTime::parse("2027-01-01T00:00:00Z", &Rfc3339).expect("cutoff"),
            None,
            true,
        ),
        Err(StoreError::ActiveInstallationBlocksEvidenceDeletion { .. })
    ));
    assert!(matches!(
        store.delete_all(),
        Err(StoreError::ActiveInstallationBlocksEvidenceDeletion { .. })
    ));
    assert_eq!(
        store.record_uninstall("mut_registry").expect("uninstall"),
        InstallationTransitionOutcome {
            installation_id: "ins_registry".to_owned(),
            mutation_state: "retired".to_owned(),
            installation_state: "uninstalled".to_owned(),
        }
    );
    assert!(matches!(
        store.challenge_mutation("mut_registry", &assessment),
        Err(StoreError::MutationStateTransition { .. })
    ));
    let details = store.get_mutation("mut_registry").expect("retired details");
    assert_eq!(details.mutation.state, "retired");
    assert_eq!(details.transitions.len(), 6);
    assert_eq!(details.replays.len(), 2);
    assert_eq!(details.shadows.len(), 2);
    assert_eq!(details.installations.len(), 1);
    assert!(!details.replays[0].passed);
    assert!(details.replays[1].passed);

    let deleted = store
        .delete_session("ses_replay-only")
        .expect("delete replay evidence");
    assert_eq!(deleted.mutations_deleted, 1);
    assert!(matches!(
        store.get_mutation("mut_registry"),
        Err(StoreError::MutationNotFound { .. })
    ));
}

#[test]
fn claude_code_installation_registers_audits_and_reverses() {
    let mut store = EventStore::open_in_memory().expect("store");
    let source = source("instance-claude-code");
    for (event_id, session_id, timestamp) in [
        (
            "evt_mutation-support-a",
            "ses_mutation-support-a",
            "2026-07-16T06:00:00Z",
        ),
        (
            "evt_mutation-support-b",
            "ses_mutation-support-b",
            "2026-07-16T06:01:00Z",
        ),
        (
            "evt_mutation-counter",
            "ses_mutation-counter",
            "2026-07-16T06:02:00Z",
        ),
        ("evt_replay-only", "ses_replay-only", "2026-07-16T06:03:00Z"),
    ] {
        store
            .insert_event(
                &source,
                &session_event(
                    event_id,
                    session_id,
                    EventKind::DecisionRecorded,
                    timestamp,
                    0,
                ),
                &SearchProjection::default(),
            )
            .expect("evidence event");
    }
    store
        .register_mutation(&mutation_registration(
            "mut_registry",
            "fnd_registry",
            "eqv_registry",
        ))
        .expect("register");
    store
        .challenge_mutation("mut_registry", &json!({"checks":["sessions_comparable"]}))
        .expect("challenge");
    store
        .register_replay(&replay_registration("rpl_passing", "rsh_passing", true))
        .expect("replay");
    store
        .register_shadow(&shadow_registration("shr_passing", "shh_passing", true))
        .expect("shadow");

    // Unknown targets are refused by validation before any state change.
    let mut unknown = claude_code_installation_registration();
    unknown.target = "vscode_repo_skill".to_owned();
    assert!(matches!(
        store.register_installation(&unknown),
        Err(StoreError::InvalidInstallationRegistration)
    ));

    let installation = claude_code_installation_registration();
    assert_eq!(
        store
            .register_installation(&installation)
            .expect("claude code install"),
        InstallationTransitionOutcome {
            installation_id: "ins_claude".to_owned(),
            mutation_state: "active".to_owned(),
            installation_state: "installed".to_owned(),
        }
    );
    let audit = store.get_installation("mut_registry").expect("audit");
    assert_eq!(audit.target, "claude_code_repo_skill");
    assert_eq!(
        audit.relative_path,
        ".claude/skills/autophagy-registry/SKILL.md"
    );

    assert_eq!(
        store.record_uninstall("mut_registry").expect("uninstall"),
        InstallationTransitionOutcome {
            installation_id: "ins_claude".to_owned(),
            mutation_state: "retired".to_owned(),
            installation_state: "uninstalled".to_owned(),
        }
    );
    let details = store.get_mutation("mut_registry").expect("retired details");
    assert_eq!(details.mutation.state, "retired");
    assert_eq!(details.installations[0].state, "uninstalled");
    // The retirement transition reason is derived from the stored target, not
    // hardcoded to Codex.
    let retire = details
        .transitions
        .iter()
        .find(|transition| transition.to_state == "retired")
        .expect("retire transition");
    assert_eq!(retire.reason, "Claude Code repo skill uninstalled");
}

#[test]
fn detection_fingerprint_tracks_the_scanned_corpus() {
    let mut store = EventStore::open_in_memory().expect("store");
    let source = source("fingerprint");

    let empty = store
        .detection_fingerprint(None)
        .expect("empty fingerprint");
    assert_eq!(empty.event_count, 0);
    assert_eq!(empty.max_row_id, 0);
    assert_eq!(empty.session_count, 0);
    assert_eq!(empty.import_watermark, "");

    let start = session_event(
        "evt_fp_start",
        "ses_fp",
        EventKind::SessionStarted,
        "2026-07-16T10:00:00Z",
        0,
    );
    store
        .insert_event(&source, &start, &SearchProjection::default())
        .expect("insert start");
    let failure = tool_failure("evt_fp_fail", "ses_fp", "2026-07-16T10:01:00Z", 1);
    store
        .insert_event(&source, &failure, &SearchProjection::default())
        .expect("insert failure");

    let after = store.detection_fingerprint(None).expect("fingerprint");
    assert_eq!(after.event_count, 2, "both events counted");
    assert_eq!(after.max_row_id, 2, "highest row id advances");
    assert_eq!(after.session_count, 1, "one distinct session");
    assert!(
        !after.import_watermark.is_empty(),
        "import watermark set once events exist"
    );

    // The project filter scopes the fingerprint to matching events only.
    let scoped = store
        .detection_fingerprint(Some("/nonexistent"))
        .expect("scoped fingerprint");
    assert_eq!(scoped.event_count, 0);
}

#[test]
fn findings_cache_round_trips_and_collects_stale_generations() {
    let store = EventStore::open_in_memory().expect("store");
    let key_a = [1_u8; 32];
    let key_b = [2_u8; 32];
    let generation_one = [9_u8; 32];

    assert_eq!(
        store.cached_findings(&key_a).expect("initial miss"),
        None,
        "an unwritten key misses"
    );

    store
        .store_findings(&key_a, &generation_one, "{\"findings\":[]}")
        .expect("store a");
    store
        .store_findings(&key_b, &generation_one, "{\"findings\":[1]}")
        .expect("store b");
    assert_eq!(
        store.cached_findings(&key_a).expect("hit a").as_deref(),
        Some("{\"findings\":[]}")
    );
    assert_eq!(
        store.cached_findings(&key_b).expect("hit b").as_deref(),
        Some("{\"findings\":[1]}"),
        "concurrent current-generation entries coexist"
    );

    // Writing under a new generation collects every prior-generation entry.
    let key_c = [3_u8; 32];
    let generation_two = [8_u8; 32];
    store
        .store_findings(&key_c, &generation_two, "{\"findings\":[2]}")
        .expect("store c");
    assert_eq!(
        store.cached_findings(&key_a).expect("evicted a"),
        None,
        "stale-generation entries are collected"
    );
    assert_eq!(store.cached_findings(&key_b).expect("evicted b"), None);
    assert_eq!(
        store.cached_findings(&key_c).expect("hit c").as_deref(),
        Some("{\"findings\":[2]}")
    );
}

fn source(instance_key: &str) -> SourceIdentity {
    SourceIdentity::new("codex", instance_key).with_display_name("Codex")
}

fn mutation_registration(
    mutation_id: &str,
    source_finding_id: &str,
    equivalence_key: &str,
) -> MutationRegistration {
    MutationRegistration {
        mutation_id: mutation_id.to_owned(),
        source_finding_id: source_finding_id.to_owned(),
        source_detector: "repeated_user_correction".to_owned(),
        equivalence_key: equivalence_key.to_owned(),
        spec_version: "mutation/0.1".to_owned(),
        semantic_version: "0.1.0".to_owned(),
        package: json!({"mutation_id": mutation_id, "state": "candidate"}),
        supporting_event_ids: vec![
            "evt_mutation-support-a".to_owned(),
            "evt_mutation-support-b".to_owned(),
        ],
        counterexample_event_ids: vec!["evt_mutation-counter".to_owned()],
    }
}

fn replay_registration(
    replay_id: &str,
    scenario_set_hash: &str,
    passed: bool,
) -> ReplayRegistration {
    ReplayRegistration {
        replay_id: replay_id.to_owned(),
        mutation_id: "mut_registry".to_owned(),
        scenario_set_hash: scenario_set_hash.to_owned(),
        report: json!({
            "replay_id": replay_id,
            "mutation_id": "mut_registry",
            "scenario_set_hash": scenario_set_hash,
            "passed": passed,
            "results": [{"source_event_ids": ["evt_replay-only"]}],
        }),
        passed,
        source_event_ids: vec!["evt_replay-only".to_owned()],
    }
}

fn shadow_registration(
    shadow_id: &str,
    observation_set_hash: &str,
    passed: bool,
) -> ShadowRegistration {
    ShadowRegistration {
        shadow_id: shadow_id.to_owned(),
        mutation_id: "mut_registry".to_owned(),
        observation_set_hash: observation_set_hash.to_owned(),
        report: json!({
            "shadow_id": shadow_id,
            "mutation_id": "mut_registry",
            "observation_set_hash": observation_set_hash,
            "passed": passed,
            "results": [{"source_event_ids": ["evt_replay-only"]}],
        }),
        passed,
        source_event_ids: vec!["evt_replay-only".to_owned()],
    }
}

fn installation_registration() -> InstallationRegistration {
    InstallationRegistration {
        installation_id: "ins_registry".to_owned(),
        mutation_id: "mut_registry".to_owned(),
        target: "codex_repo_skill".to_owned(),
        repository_root: "/workspace/project".to_owned(),
        relative_path: ".agents/skills/autophagy-registry/SKILL.md".to_owned(),
        content_hash: "a".repeat(64),
        permission_review: json!({"confirmed":"repo-skill-write"}),
    }
}

fn claude_code_installation_registration() -> InstallationRegistration {
    InstallationRegistration {
        installation_id: "ins_claude".to_owned(),
        mutation_id: "mut_registry".to_owned(),
        target: "claude_code_repo_skill".to_owned(),
        repository_root: "/workspace/project".to_owned(),
        relative_path: ".claude/skills/autophagy-registry/SKILL.md".to_owned(),
        content_hash: "a".repeat(64),
        permission_review: json!({"confirmed":"repo-skill-write"}),
    }
}

fn session_event(
    event_id: &str,
    session_id: &str,
    kind: EventKind,
    timestamp: &str,
    sequence: u64,
) -> Event {
    Event {
        spec_version: SpecVersion::V0_1,
        event_id: EventId::new(event_id),
        session_id: SessionId::new(session_id),
        timestamp: OffsetDateTime::parse(timestamp, &Rfc3339).expect("valid timestamp"),
        sequence: Some(sequence),
        source: "codex".to_owned(),
        kind,
        project: Some("/workspace/project".to_owned()),
        parent_event_id: None,
        tool: None,
        artifacts: Vec::new(),
        metadata: BTreeMap::new(),
    }
}

fn tool_failure(event_id: &str, session_id: &str, timestamp: &str, sequence: u64) -> Event {
    let mut event = session_event(
        event_id,
        session_id,
        EventKind::ToolFailed,
        timestamp,
        sequence,
    );
    event.tool = Some(ToolCall {
        name: "bash".to_owned(),
        input: Some(Value::String("private-command-token".to_owned())),
        exit_code: Some(1),
        duration_ms: Some(100),
        metadata: BTreeMap::new(),
    });
    event
}

fn file_artifact(path: &str) -> Artifact {
    Artifact {
        kind: ArtifactKind::File,
        path: Some(path.to_owned()),
        uri: None,
        digest: None,
        metadata: BTreeMap::new(),
    }
}

const RETRIEVAL_SCHEMA: &str = include_str!("../../../docs/specs/retrieval/0.1/schema.json");
const RETRIEVAL_VALID: &[&str] = &[
    include_str!("../../../docs/specs/retrieval/0.1/valid/exact_signature.json"),
    include_str!("../../../docs/specs/retrieval/0.1/valid/hybrid_with_filters.json"),
];
const RETRIEVAL_INVALID: &[&str] = &[
    include_str!("../../../docs/specs/retrieval/0.1/invalid/unknown_match_kind.json"),
    include_str!("../../../docs/specs/retrieval/0.1/invalid/bad_spec_version.json"),
    include_str!("../../../docs/specs/retrieval/0.1/invalid/score_out_of_range.json"),
    include_str!("../../../docs/specs/retrieval/0.1/invalid/empty_signals.json"),
    include_str!("../../../docs/specs/retrieval/0.1/invalid/unknown_field.json"),
    include_str!("../../../docs/specs/retrieval/0.1/invalid/bad_signal_kind.json"),
];

#[test]
fn ranking_explanations_match_the_versioned_schema() {
    let schema: Value = serde_json::from_str(RETRIEVAL_SCHEMA).expect("schema JSON");
    assert_eq!(
        schema["properties"]["spec_version"]["const"],
        "retrieval/0.1"
    );
    let validator = jsonschema::validator_for(&schema).expect("compile schema");

    for fixture in RETRIEVAL_VALID {
        let instance: Value = serde_json::from_str(fixture).expect("valid fixture JSON");
        assert!(validator.is_valid(&instance), "schema rejected {fixture}");
    }
    for fixture in RETRIEVAL_INVALID {
        let instance: Value = serde_json::from_str(fixture).expect("invalid fixture JSON");
        assert!(!validator.is_valid(&instance), "schema accepted {fixture}");
    }

    // A ranking explanation produced by the store conforms to its own contract.
    let store = seed_retrieval_store();
    let hits = store
        .retrieve(&RetrievalQuery {
            text: Some("succeeded".to_owned()),
            signature: Some(BUILD_SIG.to_owned()),
            project: Some("/repo/alpha".to_owned()),
            outcome: Some(RetrievalOutcome::Success),
            limit: 10,
            ..RetrievalQuery::default()
        })
        .expect("retrieve");
    assert!(!hits.is_empty());
    for hit in &hits {
        let explanation = serde_json::to_value(&hit.explanation).expect("serialize explanation");
        assert!(
            validator.is_valid(&explanation),
            "store produced an explanation the schema rejects: {explanation}"
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn retrieval_event(
    event_id: &str,
    session_id: &str,
    kind: EventKind,
    command: &str,
    exit_code: Option<i64>,
    project: &str,
    timestamp: &str,
    sequence: u64,
) -> Event {
    let mut event = session_event(event_id, session_id, kind, timestamp, sequence);
    event.project = Some(project.to_owned());
    event.tool = Some(ToolCall {
        name: "bash".to_owned(),
        input: Some(Value::String(command.to_owned())),
        exit_code,
        duration_ms: Some(50),
        metadata: BTreeMap::new(),
    });
    event
}

fn retrieval_projection(searchable: &str, signature: &str) -> SearchProjection {
    SearchProjection {
        tool_input_text: None,
        searchable_text: Some(searchable.to_owned()),
        signature: Some(signature.to_owned()),
    }
}

fn hit_ids(hits: &[autophagy_store::RetrievalHit]) -> Vec<&str> {
    hits.iter().map(|hit| hit.event_id.as_str()).collect()
}

const BUILD_SIG: &str = "operation/v1|shell|cargo build";
const TEST_SIG: &str = "operation/v1|shell|npm test";

fn seed_retrieval_store() -> EventStore {
    let mut store = EventStore::open_in_memory().expect("store");
    let source = source("instance-retrieval");
    let events = [
        // Session A, project alpha: two runs of `cargo build`.
        (
            retrieval_event(
                "evt_a1",
                "ses_alpha",
                EventKind::ToolFailed,
                "cargo build",
                Some(1),
                "/repo/alpha",
                "2026-07-10T00:00:00Z",
                0,
            ),
            retrieval_projection("compile error occurred", BUILD_SIG),
        ),
        (
            retrieval_event(
                "evt_a2",
                "ses_alpha",
                EventKind::ToolCompleted,
                "cargo build",
                Some(0),
                "/repo/alpha",
                "2026-07-11T00:00:00Z",
                1,
            ),
            retrieval_projection("build succeeded cleanly", BUILD_SIG),
        ),
        // Session B, project beta: a `cargo build` failure and an `npm test`.
        (
            retrieval_event(
                "evt_b1",
                "ses_beta",
                EventKind::ToolFailed,
                "cargo build",
                Some(2),
                "/repo/beta",
                "2026-07-12T00:00:00Z",
                0,
            ),
            retrieval_projection("linker failed loudly", BUILD_SIG),
        ),
        (
            retrieval_event(
                "evt_b2",
                "ses_beta",
                EventKind::ToolCompleted,
                "npm test",
                Some(0),
                "/repo/beta",
                "2026-07-13T00:00:00Z",
                1,
            ),
            retrieval_projection("suite succeeded overall", TEST_SIG),
        ),
    ];
    for (event, projection) in &events {
        store
            .insert_event(&source, event, projection)
            .expect("insert retrieval event");
    }
    store
}

#[test]
fn exact_signature_lookup_orders_by_recency_then_id() {
    let store = seed_retrieval_store();
    let hits = store
        .retrieve(&RetrievalQuery {
            signature: Some(BUILD_SIG.to_owned()),
            limit: 10,
            ..RetrievalQuery::default()
        })
        .expect("signature lookup");
    // Newest matching event first; every hit is an exact-signature match.
    assert_eq!(hit_ids(&hits), ["evt_b1", "evt_a2", "evt_a1"]);
    assert!(hits.iter().all(|hit| {
        hit.explanation.match_kind == RetrievalMatchKind::ExactSignature
            && hit.explanation.rank_score_bps == 10_000
            && hit.explanation.spec_version == "retrieval/0.1"
    }));
}

#[test]
fn exact_signature_matches_outrank_full_text_only_matches() {
    let store = seed_retrieval_store();
    let hits = store
        .retrieve(&RetrievalQuery {
            text: Some("succeeded".to_owned()),
            signature: Some(BUILD_SIG.to_owned()),
            limit: 10,
            ..RetrievalQuery::default()
        })
        .expect("hybrid retrieval");
    // evt_a2 matches both the signature and the text (highest), then the
    // remaining exact-signature matches, then the full-text-only match evt_b2.
    assert_eq!(hit_ids(&hits), ["evt_a2", "evt_b1", "evt_a1", "evt_b2"]);
    assert_eq!(
        hits[0].explanation.match_kind,
        RetrievalMatchKind::SignatureAndFullText
    );
    assert_eq!(hits[0].explanation.rank_score_bps, 15_000);
    assert_eq!(
        hits[1].explanation.match_kind,
        RetrievalMatchKind::ExactSignature
    );
    assert_eq!(hits[1].explanation.rank_score_bps, 10_000);
    assert_eq!(hits[3].explanation.match_kind, RetrievalMatchKind::FullText);
    assert_eq!(hits[3].explanation.rank_score_bps, 5_000);
    // The exact-signature tier strictly outranks the full-text-only tier.
    assert!(hits[2].explanation.rank_score_bps > hits[3].explanation.rank_score_bps);
}

#[test]
fn dual_source_match_classified_when_outside_a_source_top_n() {
    // Three events share the signature; only the oldest also matches the text
    // query. With `limit` smaller than the signature candidate count, the
    // dual-matching event falls outside the newest-first signature top-`limit`,
    // so an independent per-source `LIMIT` would misclassify it as a full-text
    // only match (and even truncate it away). Union classification over the full
    // candidate sets must still label it `SignatureAndFullText` and rank it first.
    let mut store = EventStore::open_in_memory().expect("store");
    let source = source("instance-dual");
    let seeds = [
        (
            "evt_new1",
            "2026-07-15T00:00:00Z",
            0_u64,
            "alpha compile output",
        ),
        ("evt_new2", "2026-07-14T00:00:00Z", 1, "beta compile output"),
        (
            "evt_old",
            "2026-07-01T00:00:00Z",
            2,
            "widget assembled cleanly",
        ),
    ];
    for (event_id, timestamp, sequence, searchable) in seeds {
        store
            .insert_event(
                &source,
                &retrieval_event(
                    event_id,
                    "ses_dual",
                    EventKind::ToolCompleted,
                    "cargo build",
                    Some(0),
                    "/repo/dual",
                    timestamp,
                    sequence,
                ),
                &retrieval_projection(searchable, BUILD_SIG),
            )
            .expect("insert dual-source event");
    }

    let hits = store
        .retrieve(&RetrievalQuery {
            text: Some("widget".to_owned()),
            signature: Some(BUILD_SIG.to_owned()),
            limit: 2,
            ..RetrievalQuery::default()
        })
        .expect("dual-source retrieval");

    // The dual-matching event ranks first regardless of its recency position in
    // the signature source, and survives truncation to `limit`.
    assert_eq!(hit_ids(&hits), ["evt_old", "evt_new1"]);
    assert_eq!(
        hits[0].explanation.match_kind,
        RetrievalMatchKind::SignatureAndFullText
    );
    assert_eq!(hits[0].explanation.rank_score_bps, 15_000);
    assert_eq!(
        hits[1].explanation.match_kind,
        RetrievalMatchKind::ExactSignature
    );
}

#[test]
fn filters_include_and_exclude_deterministically() {
    let store = seed_retrieval_store();

    let by_project = store
        .retrieve(&RetrievalQuery {
            signature: Some(BUILD_SIG.to_owned()),
            project: Some("/repo/alpha".to_owned()),
            limit: 10,
            ..RetrievalQuery::default()
        })
        .expect("project filter");
    assert_eq!(hit_ids(&by_project), ["evt_a2", "evt_a1"]);
    assert!(by_project.iter().all(|hit| {
        hit.explanation
            .applied_filters
            .iter()
            .any(|filter| filter.value == "/repo/alpha")
    }));

    let failures = store
        .retrieve(&RetrievalQuery {
            signature: Some(BUILD_SIG.to_owned()),
            outcome: Some(RetrievalOutcome::Failure),
            limit: 10,
            ..RetrievalQuery::default()
        })
        .expect("outcome filter");
    assert_eq!(hit_ids(&failures), ["evt_b1", "evt_a1"]);

    let completed = store
        .retrieve(&RetrievalQuery {
            signature: Some(BUILD_SIG.to_owned()),
            event_kinds: vec!["tool.completed".to_owned()],
            limit: 10,
            ..RetrievalQuery::default()
        })
        .expect("event-kind filter");
    assert_eq!(hit_ids(&completed), ["evt_a2"]);

    let recent = store
        .retrieve(&RetrievalQuery {
            signature: Some(BUILD_SIG.to_owned()),
            since: Some(OffsetDateTime::parse("2026-07-11T00:00:00Z", &Rfc3339).expect("since")),
            limit: 10,
            ..RetrievalQuery::default()
        })
        .expect("recency filter");
    assert_eq!(hit_ids(&recent), ["evt_b1", "evt_a2"]);
}

#[test]
fn identical_timestamps_break_ties_by_event_id() {
    let mut store = EventStore::open_in_memory().expect("store");
    let source = source("instance-tie");
    for event_id in ["evt_tie_b", "evt_tie_a"] {
        store
            .insert_event(
                &source,
                &retrieval_event(
                    event_id,
                    "ses_tie",
                    EventKind::ToolFailed,
                    "cargo build",
                    Some(1),
                    "/repo/alpha",
                    "2026-07-14T00:00:00Z",
                    u64::from(event_id != "evt_tie_b"),
                ),
                &retrieval_projection("same instant", BUILD_SIG),
            )
            .expect("tie event");
    }
    let hits = store
        .retrieve(&RetrievalQuery {
            signature: Some(BUILD_SIG.to_owned()),
            limit: 10,
            ..RetrievalQuery::default()
        })
        .expect("tie retrieval");
    // Equal score and equal timestamp resolve by ascending event_id.
    assert_eq!(hit_ids(&hits), ["evt_tie_a", "evt_tie_b"]);
}

#[test]
fn empty_retrieval_query_is_rejected() {
    let store = seed_retrieval_store();
    assert!(matches!(
        store.retrieve(&RetrievalQuery {
            limit: 10,
            ..RetrievalQuery::default()
        }),
        Err(StoreError::EmptyRetrievalQuery)
    ));
}

#[test]
fn signature_index_preserves_idempotency_quarantine_and_deletion() {
    let mut store = seed_retrieval_store();
    let source = source("instance-retrieval");
    let query = RetrievalQuery {
        signature: Some(BUILD_SIG.to_owned()),
        limit: 10,
        ..RetrievalQuery::default()
    };
    assert_eq!(store.retrieve(&query).expect("baseline").len(), 3);

    // Reimporting an identical event is a no-op and does not duplicate the index.
    assert!(matches!(
        store
            .insert_event(
                &source,
                &retrieval_event(
                    "evt_a1",
                    "ses_alpha",
                    EventKind::ToolFailed,
                    "cargo build",
                    Some(1),
                    "/repo/alpha",
                    "2026-07-10T00:00:00Z",
                    0,
                ),
                &retrieval_projection("compile error occurred", BUILD_SIG),
            )
            .expect("duplicate insert"),
        InsertOutcome::Duplicate { .. }
    ));
    assert_eq!(store.retrieve(&query).expect("after duplicate").len(), 3);

    // A conflicting reuse of an event ID quarantines and never re-indexes.
    let mut conflicting = retrieval_event(
        "evt_a1",
        "ses_alpha",
        EventKind::ToolFailed,
        "cargo build --release",
        Some(9),
        "/repo/alpha",
        "2026-07-10T00:00:00Z",
        0,
    );
    conflicting
        .metadata
        .insert("drift".to_owned(), json!("conflicting"));
    assert!(matches!(
        store
            .insert_event(
                &source,
                &conflicting,
                &retrieval_projection("conflicting", "operation/v1|shell|cargo build --release"),
            )
            .expect("conflict insert"),
        InsertOutcome::ConflictQuarantined { .. }
    ));
    assert_eq!(
        hit_ids(&store.retrieve(&query).expect("after conflict")).len(),
        3
    );
    assert!(
        store
            .retrieve(&RetrievalQuery {
                signature: Some("operation/v1|shell|cargo build --release".to_owned()),
                limit: 10,
                ..RetrievalQuery::default()
            })
            .expect("conflict signature")
            .is_empty()
    );

    // Deleting a session cascades its signature index rows.
    store.delete_session("ses_alpha").expect("delete session");
    assert_eq!(
        hit_ids(&store.retrieve(&query).expect("after delete")),
        ["evt_b1"]
    );
}
