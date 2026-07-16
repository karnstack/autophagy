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
