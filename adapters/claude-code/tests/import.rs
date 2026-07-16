//! Contract tests over anonymized Claude Code-shaped transcript fixtures.

use std::{
    fs,
    path::{Path, PathBuf},
};

use autophagy_adapter_claude_code::{
    ClaudeImportOptions, DiscoveryOptions, SessionKind, discover, import_claude_code,
};
use autophagy_adapter_test_support::{ImportMetrics, verify_incremental_idempotency};
use autophagy_store::EventStore;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/projects")
}

#[test]
fn discovery_is_sorted_and_subagents_are_opt_in() {
    let root = fixture_root();
    let main = discover(&DiscoveryOptions {
        input: root.clone(),
        include_subagents: false,
    })
    .expect("discover main");
    assert_eq!(main.files.len(), 1);
    assert_eq!(main.files[0].kind, SessionKind::Main);

    let all = discover(&DiscoveryOptions {
        input: root,
        include_subagents: true,
    })
    .expect("discover all");
    assert_eq!(all.files.len(), 2);
    assert_eq!(all.files[1].kind, SessionKind::Subagent);
    assert!(all.files[0].relative_path < all.files[1].relative_path);
}

#[test]
fn import_is_structural_incremental_and_defers_partial_tails() {
    let directory = tempfile::tempdir().expect("temp directory");
    copy_tree(&fixture_root(), directory.path());
    let mut store = EventStore::open_in_memory().expect("store");
    let mut options = ClaudeImportOptions::new(directory.path().to_path_buf(), "fixture:claude");

    let first = import_claude_code(Some(&mut store), &options).expect("first import");
    assert_eq!(first.records_seen, 7);
    assert_eq!(first.events_emitted, 8);
    assert_eq!(first.inserted, 8);
    assert_eq!(first.unsupported, 1);
    assert_eq!(first.rejected, 0);
    let stored_after_first = store.stats().expect("stats").events;
    assert_eq!(stored_after_first, 8);

    let second = import_claude_code(Some(&mut store), &options).expect("second import");
    assert_eq!(second.records_seen, 0);
    assert_eq!(second.inserted, 0);
    let stored_after_second = store.stats().expect("stats").events;
    verify_incremental_idempotency(
        metrics(&first),
        metrics(&second),
        stored_after_first,
        stored_after_second,
    )
    .expect("shared adapter conformance");

    let transcript = directory
        .path()
        .join("-workspace-demo/11111111-1111-4111-8111-111111111111.jsonl");
    append(
        &transcript,
        "{\"type\":\"queue-operation\",\"uuid\":\"partial\"}",
    );
    let partial = import_claude_code(Some(&mut store), &options).expect("partial import");
    assert_eq!(partial.partial_tails, 1);
    assert_eq!(partial.records_seen, 0);

    append(&transcript, "\n");
    let completed = import_claude_code(Some(&mut store), &options).expect("completed import");
    assert_eq!(completed.records_seen, 1);
    assert_eq!(completed.unsupported, 1);

    options.dry_run = true;
    let preview = import_claude_code(None, &options).expect("preview");
    assert_eq!(preview.discovery.files.len(), 1);
    assert_eq!(preview.inserted, 0);
    assert!(!preview.discovery.files[0].relative_path.is_empty());
}

#[test]
fn pending_tool_state_survives_between_appends() {
    let directory = tempfile::tempdir().expect("temp directory");
    let transcript = directory.path().join("session.jsonl");
    fs::write(&transcript, "{\"type\":\"assistant\",\"uuid\":\"call\",\"sessionId\":\"session\",\"timestamp\":\"2026-07-16T09:00:00Z\",\"cwd\":\"/repo\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Bash\",\"input\":{\"command\":\"false\"}}]}}\n").expect("write call");
    let mut store = EventStore::open_in_memory().expect("store");
    let options = ClaudeImportOptions::new(transcript.clone(), "fixture:pending");
    let first = import_claude_code(Some(&mut store), &options).expect("call import");
    assert_eq!(first.inserted, 2);

    append(
        &transcript,
        "{\"type\":\"user\",\"uuid\":\"result\",\"sessionId\":\"session\",\"timestamp\":\"2026-07-16T09:00:01Z\",\"cwd\":\"/repo\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"t1\",\"is_error\":true,\"content\":\"Exit code 7\"}]}}\n",
    );
    let second = import_claude_code(Some(&mut store), &options).expect("result import");
    assert_eq!(second.inserted, 1);
    assert_eq!(second.rejected, 0);
    assert_eq!(store.stats().expect("stats").events, 3);
}

#[test]
fn orphaned_tool_results_are_skipped_without_guessing() {
    let directory = tempfile::tempdir().expect("temp directory");
    let transcript = directory.path().join("session.jsonl");
    fs::write(&transcript, "{\"type\":\"user\",\"uuid\":\"orphan\",\"timestamp\":\"2026-07-16T09:00:01Z\",\"cwd\":\"/repo\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"missing\",\"content\":\"result\"}]}}\n").expect("write orphan");
    let mut store = EventStore::open_in_memory().expect("store");
    let options = ClaudeImportOptions::new(transcript, "fixture:orphan");
    let summary = import_claude_code(Some(&mut store), &options).expect("orphan import");
    assert_eq!(summary.rejected, 0);
    assert_eq!(summary.inserted, 1);
}

#[test]
fn changing_path_exclusions_uses_an_independent_cursor_scope() {
    let directory = tempfile::tempdir().expect("temp directory");
    copy_tree(&fixture_root(), directory.path());
    let mut store = EventStore::open_in_memory().expect("store");
    let mut options =
        ClaudeImportOptions::new(directory.path().to_path_buf(), "fixture:policy-scope");
    options.exclude_paths = vec!["/workspace/**".to_owned()];
    let excluded = import_claude_code(Some(&mut store), &options).expect("excluded import");
    assert_eq!(excluded.inserted, 0);
    assert_eq!(excluded.privacy_skipped, 8);

    options.exclude_paths.clear();
    let included = import_claude_code(Some(&mut store), &options).expect("included import");
    assert_eq!(included.records_seen, 7);
    assert_eq!(included.inserted, 8);
}

fn append(path: &Path, value: &str) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open append");
    file.write_all(value.as_bytes()).expect("append fixture");
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

fn metrics(summary: &autophagy_adapter_claude_code::ClaudeImportSummary) -> ImportMetrics {
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
