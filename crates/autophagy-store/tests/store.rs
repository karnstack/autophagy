//! Integration tests for transactional event storage and retrieval.

use std::collections::BTreeMap;

use autophagy_events::{
    Artifact, ArtifactKind, Event, EventId, EventKind, SessionId, SpecVersion, ToolCall,
};
use autophagy_store::{
    DeleteAllSummary, DeleteSummary, EventStore, InsertOutcome, MutationRegisterOutcome,
    MutationRegistration, PruneSummary, SearchProjection, SourceCursor, SourceIdentity, StoreError,
    StoreStats,
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
        assert_eq!(store.schema_version().expect("schema version"), 3);
        assert!(matches!(
            store
                .insert_event(&source, &event, &SearchProjection::default())
                .expect("insert should succeed"),
            InsertOutcome::Inserted { .. }
        ));
    }

    let reopened = EventStore::open(&database).expect("store should reopen");
    assert_eq!(reopened.schema_version().expect("schema version"), 3);
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
    let rejected = store
        .reject_mutation("mut_registry", "counterexample risk remains")
        .expect("reject");
    assert!(rejected.changed);
    assert_eq!(rejected.from_state, "challenged");
    assert!(
        !store
            .reject_mutation("mut_registry", "same decision")
            .expect("repeat")
            .changed
    );
    assert!(matches!(
        store.challenge_mutation("mut_registry", &assessment),
        Err(StoreError::MutationStateTransition { .. })
    ));
    let details = store
        .get_mutation("mut_registry")
        .expect("rejected details");
    assert_eq!(details.mutation.state, "rejected");
    assert_eq!(
        details.mutation.rejection_reason.as_deref(),
        Some("counterexample risk remains")
    );
    assert_eq!(details.transitions.len(), 3);

    let deleted = store
        .delete_session("ses_mutation-support-a")
        .expect("delete evidence");
    assert_eq!(deleted.mutations_deleted, 1);
    assert!(matches!(
        store.get_mutation("mut_registry"),
        Err(StoreError::MutationNotFound { .. })
    ));
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
