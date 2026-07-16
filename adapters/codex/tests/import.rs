//! Contract tests over anonymized Codex-shaped rollout fixtures.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use autophagy_adapter_codex::{CodexImportOptions, CodexImportSummary, discover, import_codex};
use autophagy_adapter_test_support::{ImportMetrics, verify_incremental_idempotency};
use autophagy_store::EventStore;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sessions")
}

#[test]
fn discovery_selects_only_sorted_rollout_jsonl() {
    let plan = discover(&fixture_root()).expect("discover fixture");
    assert_eq!(plan.files.len(), 1);
    assert!(
        plan.files[0]
            .relative_path
            .ends_with("rollout-2026-07-16T09-00-00-fixture.jsonl")
    );
}

#[test]
fn import_satisfies_shared_incremental_contract() {
    let directory = tempfile::tempdir().expect("temp directory");
    copy_tree(&fixture_root(), directory.path());
    let mut store = EventStore::open_in_memory().expect("store");
    let options = CodexImportOptions::new(directory.path().to_path_buf(), "fixture:codex");

    let first = import_codex(Some(&mut store), &options).expect("first import");
    assert_eq!(first.records_seen, 10);
    assert_eq!(first.events_emitted, 8);
    assert_eq!(first.inserted, 8);
    assert_eq!(first.rejected, 0);
    let stored_after_first = store.stats().expect("stats").events;

    let second = import_codex(Some(&mut store), &options).expect("second import");
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
fn partial_tail_is_deferred_until_newline() {
    let directory = tempfile::tempdir().expect("temp directory");
    copy_tree(&fixture_root(), directory.path());
    let rollout = find_rollout(directory.path());
    let mut store = EventStore::open_in_memory().expect("store");
    let options = CodexImportOptions::new(directory.path().to_path_buf(), "fixture:tail");
    import_codex(Some(&mut store), &options).expect("initial import");

    append(
        &rollout,
        "{\"timestamp\":\"2026-07-16T09:00:09Z\",\"type\":\"world_state\",\"payload\":{}",
    );
    let partial = import_codex(Some(&mut store), &options).expect("partial import");
    assert_eq!(partial.partial_tails, 1);
    assert_eq!(partial.records_seen, 0);
    append(&rollout, "}\n");
    let complete = import_codex(Some(&mut store), &options).expect("complete import");
    assert_eq!(complete.records_seen, 1);
    assert_eq!(complete.unsupported, 1);
}

fn metrics(summary: &CodexImportSummary) -> ImportMetrics {
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

fn find_rollout(root: &Path) -> PathBuf {
    discover(root)
        .expect("discover")
        .files
        .into_iter()
        .next()
        .expect("rollout")
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
