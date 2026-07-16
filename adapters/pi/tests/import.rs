//! Contract tests over anonymized Pi-shaped session fixtures.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use autophagy_adapter_pi::{PiImportOptions, PiImportSummary, discover, import_pi};
use autophagy_adapter_test_support::{ImportMetrics, verify_incremental_idempotency};
use autophagy_store::EventStore;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sessions")
}

#[test]
fn discovery_selects_only_sorted_session_jsonl() {
    let plan = discover(&fixture_root()).expect("discover fixture");
    assert_eq!(plan.files.len(), 1);
    assert_eq!(
        Path::new(&plan.files[0].relative_path)
            .extension()
            .and_then(|value| value.to_str()),
        Some("jsonl")
    );
    assert!(plan.files[0].relative_path.starts_with("project-demo/"));
}

#[test]
fn import_satisfies_shared_incremental_contract() {
    let directory = tempfile::tempdir().expect("temp directory");
    copy_tree(&fixture_root(), directory.path());
    let mut store = EventStore::open_in_memory().expect("store");
    let options = PiImportOptions::new(directory.path().to_path_buf(), "fixture:pi");

    let first = import_pi(Some(&mut store), &options).expect("first import");
    assert_eq!(first.records_seen, 11);
    assert_eq!(first.events_emitted, 12);
    assert_eq!(first.inserted, 12);
    assert_eq!(first.unsupported, 3);
    assert_eq!(first.rejected, 0);
    let stored_after_first = store.stats().expect("stats").events;

    let second = import_pi(Some(&mut store), &options).expect("second import");
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
fn tool_failure_and_success_map_to_distinct_kinds() {
    let directory = tempfile::tempdir().expect("temp directory");
    copy_tree(&fixture_root(), directory.path());
    let mut store = EventStore::open_in_memory().expect("store");
    let options = PiImportOptions::new(directory.path().to_path_buf(), "fixture:kinds");
    import_pi(Some(&mut store), &options).expect("import");

    let sessions = store.list_sessions(10).expect("sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].adapter, "pi");
    assert_eq!(sessions[0].project_path.as_deref(), Some("/work/demo"));
}

#[test]
fn partial_tail_is_deferred_until_newline() {
    let directory = tempfile::tempdir().expect("temp directory");
    copy_tree(&fixture_root(), directory.path());
    let session = find_session(directory.path());
    let mut store = EventStore::open_in_memory().expect("store");
    let options = PiImportOptions::new(directory.path().to_path_buf(), "fixture:tail");
    import_pi(Some(&mut store), &options).expect("initial import");

    append(
        &session,
        "{\"timestamp\":\"2026-07-16T09:00:11.000Z\",\"type\":\"model_change\",\"id\":\"rec-late\",\"provider\":\"p\",\"modelId\":\"m\"",
    );
    let partial = import_pi(Some(&mut store), &options).expect("partial import");
    assert_eq!(partial.partial_tails, 1);
    assert_eq!(partial.records_seen, 0);
    append(&session, "}\n");
    let complete = import_pi(Some(&mut store), &options).expect("complete import");
    assert_eq!(complete.records_seen, 1);
    assert_eq!(complete.unsupported, 1);
}

#[test]
fn unknown_message_role_is_unsupported_not_rejected() {
    let directory = tempfile::tempdir().expect("temp directory");
    copy_tree(&fixture_root(), directory.path());
    let session = find_session(directory.path());
    let mut store = EventStore::open_in_memory().expect("store");
    let options = PiImportOptions::new(directory.path().to_path_buf(), "fixture:role");
    let baseline = import_pi(Some(&mut store), &options).expect("baseline import");

    append(
        &session,
        "{\"type\":\"message\",\"id\":\"rec-sys\",\"parentId\":\"rec-custom\",\"timestamp\":\"2026-07-16T09:00:12.000Z\",\"message\":{\"role\":\"system\",\"content\":[{\"type\":\"text\",\"text\":\"system notice\"}],\"timestamp\":1784192412000}}\n",
    );
    let after = import_pi(Some(&mut store), &options).expect("second import");
    assert_eq!(after.records_seen, 1);
    assert_eq!(after.events_emitted, 0);
    assert_eq!(after.unsupported, 1);
    assert_eq!(after.rejected, 0);
    // The unknown role adds no diagnostics: it is a counted skip, not an error.
    assert!(after.diagnostics.is_empty());
    assert_eq!(baseline.rejected, 0);
}

fn metrics(summary: &PiImportSummary) -> ImportMetrics {
    ImportMetrics {
        discovered_files: summary
            .discovery
            .files
            .len()
            .try_into()
            .expect("file count"),
        records_seen: summary.records_seen,
        events_emitted: summary.events_emitted,
        inserted: summary.inserted,
        duplicates: summary.duplicates,
        conflicts: summary.conflicts,
        rejected: summary.rejected,
    }
}

fn append(path: &Path, value: &str) {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open append");
    file.write_all(value.as_bytes()).expect("append fixture");
}

fn find_session(root: &Path) -> PathBuf {
    discover(root)
        .expect("discover")
        .files
        .into_iter()
        .next()
        .expect("session")
        .path
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
