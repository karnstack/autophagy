//! Fixture-based tests for the generic AEP JSONL importer.

use std::io::Cursor;

use autophagy_core::{ImportDiagnosticCode, ImportOptions, ReindexOptions, import_jsonl, reindex};
use autophagy_store::{EventStore, RetrievalMatchKind, RetrievalQuery, StoreStats};

const MIXED: &str = include_str!("fixtures/mixed.jsonl");
const RETRIEVAL: &str = include_str!("../../../evals/fixtures/retrieval/deterministic.jsonl");

const BUILD_SIG: &str = "operation/v2|shell|cargo build";

fn hit_ids(hits: &[autophagy_store::RetrievalHit]) -> Vec<String> {
    hits.iter().map(|hit| hit.event_id.clone()).collect()
}

#[test]
fn reindex_heals_a_corpus_imported_without_indexing() {
    // Import the retrieval corpus WITHOUT indexing, exactly as history ingested
    // before signature indexing existed: canonical events land, but the exact
    // signature index is empty and tool input is not searchable.
    let mut store = EventStore::open_in_memory().expect("store");
    let options = ImportOptions::new("fixture:reindex");
    let summary = import_jsonl(Cursor::new(RETRIEVAL), Some(&mut store), &options).expect("import");
    assert_eq!(summary.inserted, 4);
    assert_eq!(store.signature_count().expect("signatures"), 0);
    assert!(
        store
            .retrieve(&RetrievalQuery {
                signature: Some(BUILD_SIG.to_owned()),
                limit: 10,
                ..RetrievalQuery::default()
            })
            .expect("exact")
            .is_empty(),
        "reimport cannot heal this; signatures must start empty"
    );

    // Reindex with the indexing gate rebuilds the exact-signature index from the
    // stored events without re-reading any transcript.
    let reindex_options = ReindexOptions {
        index_tool_input: true,
        ..ReindexOptions::default()
    };
    let first = reindex(&mut store, &reindex_options).expect("reindex");
    assert_eq!(first.events_scanned, 4);
    assert_eq!(first.search_rows_written, 4);
    assert_eq!(first.signatures_written, 4);
    assert_eq!(store.signature_count().expect("signatures"), 4);
    assert_eq!(
        hit_ids(
            &store
                .retrieve(&RetrievalQuery {
                    signature: Some(BUILD_SIG.to_owned()),
                    limit: 10,
                    ..RetrievalQuery::default()
                })
                .expect("exact after reindex")
        ),
        ["evt_r3", "evt_r2", "evt_r1"]
    );

    // Idempotent: a second rebuild produces identical counts and state.
    let second = reindex(&mut store, &reindex_options).expect("re-reindex");
    assert_eq!(second, first);
    assert_eq!(store.signature_count().expect("signatures"), 4);
}

#[test]
fn retrieval_corpus_recalls_exact_and_hybrid_matches_predictably() {
    let mut store = EventStore::open_in_memory().expect("store");
    let mut options = ImportOptions::new("fixture:retrieval");
    options.index_tool_input = true;
    options.index_metadata = vec!["note".to_owned()];
    let summary = import_jsonl(Cursor::new(RETRIEVAL), Some(&mut store), &options).expect("import");
    assert_eq!(summary.inserted, 4);

    // Exact normalized-signature lookup collapses `bash`, `exec_command`, and
    // whitespace variants into one signature and orders by recency.
    let exact = store
        .retrieve(&RetrievalQuery {
            signature: Some(BUILD_SIG.to_owned()),
            limit: 10,
            ..RetrievalQuery::default()
        })
        .expect("exact");
    assert_eq!(hit_ids(&exact), ["evt_r3", "evt_r2", "evt_r1"]);

    // Hybrid: an exact-signature-plus-text match leads, then remaining
    // exact-signature matches, then the full-text-only match.
    let hybrid = store
        .retrieve(&RetrievalQuery {
            text: Some("succeeded".to_owned()),
            signature: Some(BUILD_SIG.to_owned()),
            limit: 10,
            ..RetrievalQuery::default()
        })
        .expect("hybrid");
    assert_eq!(hit_ids(&hybrid), ["evt_r2", "evt_r3", "evt_r1", "evt_r4"]);
    assert_eq!(
        hybrid[0].explanation.match_kind,
        RetrievalMatchKind::SignatureAndFullText
    );
    assert_eq!(
        hybrid[3].explanation.match_kind,
        RetrievalMatchKind::FullText
    );
}

#[test]
fn signature_index_respects_the_tool_input_redaction_gate() {
    // Without explicit tool-input indexing approval, no signature is indexed,
    // so exact-signature lookup recalls nothing even though events are stored.
    let mut store = EventStore::open_in_memory().expect("store");
    let mut options = ImportOptions::new("fixture:retrieval-gated");
    options.index_metadata = vec!["note".to_owned()];
    let summary = import_jsonl(Cursor::new(RETRIEVAL), Some(&mut store), &options).expect("import");
    assert_eq!(summary.inserted, 4);
    assert!(
        store
            .retrieve(&RetrievalQuery {
                signature: Some(BUILD_SIG.to_owned()),
                limit: 10,
                ..RetrievalQuery::default()
            })
            .expect("gated retrieval")
            .is_empty()
    );
}

#[test]
fn streams_selected_records_and_reports_bounded_diagnostics() {
    let mut store = EventStore::open_in_memory().expect("store");
    let mut options = ImportOptions::new("fixture:mixed");
    options.projects = vec!["/repo/a".to_owned()];
    options.index_metadata = vec!["search".to_owned()];
    options.max_diagnostics = 1;

    let summary = import_jsonl(Cursor::new(MIXED), Some(&mut store), &options).expect("import");
    assert_eq!(summary.lines_read, 5);
    assert_eq!(summary.events_seen, 5);
    assert_eq!(summary.validated, 2);
    assert_eq!(summary.inserted, 2);
    assert_eq!(summary.duplicates, 0);
    assert_eq!(summary.conflicts, 0);
    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.rejected, 2);
    assert_eq!(summary.diagnostics.len(), 1);
    assert_eq!(summary.diagnostics[0].line, 3);
    assert_eq!(
        summary.diagnostics[0].code,
        ImportDiagnosticCode::InvalidJson
    );
    assert_eq!(summary.diagnostics_suppressed, 1);
    assert!(summary.has_issues());

    assert_eq!(store.search("generated", 10).expect("search").len(), 1);
    assert!(
        store
            .search("\"secret-tool-token\"", 10)
            .expect("private search")
            .is_empty()
    );
    assert_eq!(
        store.stats().expect("stats"),
        StoreStats {
            sources: 1,
            sessions: 1,
            events: 2,
            artifacts: 0,
            conflicts: 0,
        }
    );

    let duplicate =
        import_jsonl(Cursor::new(MIXED), Some(&mut store), &options).expect("duplicate import");
    assert_eq!(duplicate.inserted, 0);
    assert_eq!(duplicate.duplicates, 2);
    assert_eq!(store.stats().expect("stats").events, 2);
}

#[test]
fn tool_input_indexing_requires_explicit_opt_in() {
    let input = concat!(
        "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_tool\",",
        "\"session_id\":\"ses_tool\",\"timestamp\":\"2026-07-16T07:00:00Z\",",
        "\"source\":\"generic-jsonl\",\"type\":\"tool.failed\",",
        "\"tool\":{\"name\":\"bash\",\"input\":\"approved search phrase\",\"exit_code\":1}}\n"
    );
    let mut store = EventStore::open_in_memory().expect("store");
    let mut options = ImportOptions::new("fixture:tool");
    options.index_tool_input = true;

    let summary = import_jsonl(Cursor::new(input), Some(&mut store), &options).expect("import");
    assert_eq!(summary.inserted, 1);
    assert_eq!(store.search("approved", 10).expect("search").len(), 1);
}

#[test]
fn privacy_policy_redacts_secrets_and_excludes_paths_before_storage() {
    let input = concat!(
        "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_secret\",",
        "\"session_id\":\"ses_secret\",\"timestamp\":\"2026-07-16T09:00:00Z\",",
        "\"source\":\"generic-jsonl\",\"type\":\"tool.called\",",
        "\"project\":\"/repo/public\",\"tool\":{\"name\":\"shell\",",
        "\"input\":{\"command\":\"API_KEY=abcdefgh12345678\"}}}\n",
        "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_private\",",
        "\"session_id\":\"ses_private\",\"timestamp\":\"2026-07-16T09:01:00Z\",",
        "\"source\":\"generic-jsonl\",\"type\":\"session.started\",",
        "\"project\":\"/repo/private/client\"}\n"
    );
    let mut store = EventStore::open_in_memory().expect("store");
    let mut options = ImportOptions::new("fixture:privacy");
    options.exclude_paths = vec!["**/private/**".to_owned()];
    let summary = import_jsonl(Cursor::new(input), Some(&mut store), &options).expect("import");
    assert_eq!(summary.inserted, 1);
    assert_eq!(summary.privacy_skipped, 1);
    assert_eq!(summary.redacted_fields, 1);
    let stored = store
        .get_event("evt_secret")
        .expect("query")
        .expect("event");
    let encoded = serde_json::to_string(&stored).expect("JSON");
    assert!(encoded.contains("[REDACTED]"));
    assert!(!encoded.contains("abcdefgh12345678"));
    assert!(store.get_event("evt_private").expect("query").is_none());
}

#[test]
fn dry_run_validates_without_a_store_or_writes() {
    let mut options = ImportOptions::new("fixture:dry-run");
    options.dry_run = true;

    let summary = import_jsonl(Cursor::new(MIXED), None, &options).expect("dry run");
    assert!(summary.dry_run);
    assert_eq!(summary.validated, 3);
    assert_eq!(summary.inserted, 0);
    assert_eq!(summary.skipped, 0);
    assert_eq!(summary.rejected, 2);
}

#[test]
fn same_session_sequence_is_a_record_diagnostic_not_a_fatal_error() {
    let input = concat!(
        "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_one\",",
        "\"session_id\":\"ses_sequence\",\"timestamp\":\"2026-07-16T08:00:00Z\",",
        "\"sequence\":0,\"source\":\"generic-jsonl\",\"type\":\"session.started\"}\n",
        "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_two\",",
        "\"session_id\":\"ses_sequence\",\"timestamp\":\"2026-07-16T08:01:00Z\",",
        "\"sequence\":0,\"source\":\"generic-jsonl\",\"type\":\"session.ended\"}\n",
        "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_three\",",
        "\"session_id\":\"ses_sequence\",\"timestamp\":\"2026-07-16T08:02:00Z\",",
        "\"sequence\":1,\"source\":\"generic-jsonl\",\"type\":\"session.ended\"}\n"
    );
    let mut store = EventStore::open_in_memory().expect("store");
    let summary = import_jsonl(
        Cursor::new(input),
        Some(&mut store),
        &ImportOptions::new("fixture:sequence"),
    )
    .expect("import should continue");

    assert_eq!(summary.inserted, 2);
    assert_eq!(summary.rejected, 1);
    assert_eq!(summary.diagnostics[0].line, 2);
    assert_eq!(
        summary.diagnostics[0].code,
        ImportDiagnosticCode::StoreRejected
    );
}
