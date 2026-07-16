//! Contract tests over an anonymized OpenCode-shaped storage tree.

use std::{
    fs,
    path::{Path, PathBuf},
};

use autophagy_adapter_opencode::{
    OpenCodeImportOptions, OpenCodeImportSummary, discover, import_opencode,
};
use autophagy_adapter_test_support::{ImportMetrics, verify_incremental_idempotency};
use autophagy_store::EventStore;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/storage")
}

fn staged_storage() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temp directory");
    let storage = directory.path().join("storage");
    fs::create_dir_all(&storage).expect("create storage");
    copy_tree(&fixture_root(), &storage);
    (directory, storage)
}

#[test]
fn discovery_selects_sessions_with_message_counts() {
    let (_guard, storage) = staged_storage();
    let plan = discover(&storage).expect("discover fixture");
    assert_eq!(plan.sessions.len(), 1);
    assert_eq!(plan.sessions[0].session_id, "ses_demo00000000000000000001");
    assert_eq!(plan.sessions[0].project_id, "prj_demo");
    assert_eq!(plan.sessions[0].message_count, 3);
}

#[test]
fn import_satisfies_shared_incremental_contract() {
    let (_guard, storage) = staged_storage();
    let mut store = EventStore::open_in_memory().expect("store");
    let options = OpenCodeImportOptions::new(storage.clone(), "fixture:opencode");

    let first = import_opencode(Some(&mut store), &options).expect("first import");
    assert_eq!(first.records_seen, 3);
    assert_eq!(first.parts_seen, 4);
    assert_eq!(first.events_emitted, 7);
    assert_eq!(first.inserted, 7);
    assert_eq!(first.unsupported, 1);
    assert_eq!(first.rejected, 0);
    let stored_after_first = store.stats().expect("stats").events;

    let second = import_opencode(Some(&mut store), &options).expect("second import");
    let stored_after_second = store.stats().expect("stats").events;
    verify_incremental_idempotency(
        metrics(&first),
        metrics(&second),
        stored_after_first,
        stored_after_second,
    )
    .expect("shared conformance");
}

#[test]
fn session_provenance_and_project_are_recorded() {
    let (_guard, storage) = staged_storage();
    let mut store = EventStore::open_in_memory().expect("store");
    let options = OpenCodeImportOptions::new(storage, "fixture:provenance");
    import_opencode(Some(&mut store), &options).expect("import");

    let sessions = store.list_sessions(10).expect("sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].adapter, "opencode");
    assert_eq!(sessions[0].project_path.as_deref(), Some("/work/demo"));
}

#[test]
fn realistic_ulid_identifiers_parse_and_order() {
    let directory = tempfile::tempdir().expect("temp directory");
    let storage = directory.path().join("storage");
    fs::create_dir_all(&storage).expect("create storage");
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ulid"),
        &storage,
    );
    let plan = discover(&storage).expect("discover");
    assert_eq!(plan.sessions.len(), 1);
    assert_eq!(plan.sessions[0].message_count, 2);

    let mut store = EventStore::open_in_memory().expect("store");
    let options = OpenCodeImportOptions::new(storage, "fixture:ulid");
    let summary = import_opencode(Some(&mut store), &options).expect("import");
    // Two messages (user + assistant), one prompt, one decision, one
    // tool.called + tool.completed, plus the session start.
    assert_eq!(summary.records_seen, 2);
    assert_eq!(summary.parts_seen, 3);
    assert_eq!(summary.events_emitted, 5);
    assert_eq!(summary.inserted, 5);
    assert_eq!(summary.rejected, 0);
}

#[test]
fn pending_tool_completes_across_import_cycles() {
    let directory = tempfile::tempdir().expect("temp directory");
    let storage = directory.path().join("storage");
    let session = "ses_defer000000000000000001";
    write_json(
        &storage.join(format!("session/prj_defer/{session}.json")),
        &format!(
            r#"{{"id":"{session}","projectID":"prj_defer","directory":"/work/defer","title":"t","version":"0.15.0","time":{{"created":1784192400000,"updated":1784192400000}}}}"#
        ),
    );
    let user = "msg_00000000000000000000000001";
    let assistant = "msg_00000000000000000000000002";
    write_json(
        &storage.join(format!("message/{session}/{user}.json")),
        &format!(
            r#"{{"id":"{user}","sessionID":"{session}","role":"user","time":{{"created":1784192401000}}}}"#
        ),
    );
    write_json(
        &storage.join(format!("part/{user}/prt_00000000000000000000000001.json")),
        &format!(
            r#"{{"id":"prt_00000000000000000000000001","sessionID":"{session}","messageID":"{user}","type":"text","text":"run the build"}}"#
        ),
    );
    write_json(
        &storage.join(format!("message/{session}/{assistant}.json")),
        &format!(
            r#"{{"id":"{assistant}","sessionID":"{session}","role":"assistant","time":{{"created":1784192402000}},"modelID":"m","providerID":"p","mode":"build","path":{{"cwd":"/work/defer","root":"/work/defer"}},"system":[],"cost":0,"tokens":{{"input":1,"output":1,"reasoning":0,"cache":{{"read":0,"write":0}}}}}}"#
        ),
    );
    let tool_part = storage.join(format!(
        "part/{assistant}/prt_00000000000000000000000002.json"
    ));
    write_json(
        &tool_part,
        &format!(
            r#"{{"id":"prt_00000000000000000000000002","sessionID":"{session}","messageID":"{assistant}","type":"tool","callID":"call_1","tool":"bash","state":{{"status":"running","input":{{"command":"cargo build"}},"time":{{"start":1784192403000}}}}}}"#
        ),
    );

    let mut store = EventStore::open_in_memory().expect("store");
    let options = OpenCodeImportOptions::new(storage, "fixture:defer");

    // Cycle 1: the running tool yields tool.called only; no terminal outcome.
    let first = import_opencode(Some(&mut store), &options).expect("first");
    assert_eq!(first.records_seen, 2);
    assert_eq!(first.inserted, 3); // session.started + prompt.submitted + tool.called
    assert_eq!(store.stats().expect("stats").events, 3);

    // Cycle 2: unchanged, but the deferred assistant message is re-read (the
    // cursor did not advance past it) and its events deduplicate.
    let second = import_opencode(Some(&mut store), &options).expect("second");
    assert_eq!(second.records_seen, 1);
    assert_eq!(second.inserted, 0);
    assert_eq!(second.duplicates, 1);
    assert_eq!(store.stats().expect("stats").events, 3);

    // The tool transitions to completed in the same part file.
    write_json(
        &tool_part,
        &format!(
            r#"{{"id":"prt_00000000000000000000000002","sessionID":"{session}","messageID":"{assistant}","type":"tool","callID":"call_1","tool":"bash","state":{{"status":"completed","input":{{"command":"cargo build"}},"output":"ok","title":"bash","metadata":{{}},"time":{{"start":1784192403000,"end":1784192404000}}}}}}"#
        ),
    );

    // Cycle 3: the completion event now lands.
    let third = import_opencode(Some(&mut store), &options).expect("third");
    assert_eq!(third.records_seen, 1);
    assert_eq!(third.inserted, 1); // tool.completed
    assert_eq!(store.stats().expect("stats").events, 4);

    // Cycle 4: now fully terminal, the cursor advances and reads nothing.
    let fourth = import_opencode(Some(&mut store), &options).expect("fourth");
    assert_eq!(fourth.records_seen, 0);
    assert_eq!(store.stats().expect("stats").events, 4);
}

fn write_json(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
    fs::write(path, content).expect("write fixture");
}

fn metrics(summary: &OpenCodeImportSummary) -> ImportMetrics {
    ImportMetrics {
        discovered_files: summary
            .discovery
            .sessions
            .len()
            .try_into()
            .expect("session count"),
        records_seen: summary.records_seen,
        events_emitted: summary.events_emitted,
        inserted: summary.inserted,
        duplicates: summary.duplicates,
        conflicts: summary.conflicts,
        rejected: summary.rejected,
    }
}

fn copy_tree(source: &Path, destination: &Path) {
    for entry in fs::read_dir(source)
        .expect("read fixture")
        .map(Result::unwrap)
    {
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            fs::create_dir_all(&target).expect("create fixture directory");
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy fixture");
        }
    }
}
