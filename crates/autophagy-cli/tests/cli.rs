//! End-to-end tests for the user-facing command line.

use std::{fs, path::Path, process::Command};

use serde_json::Value;

const VALID_JSONL: &str = concat!(
    "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_cli_start\",",
    "\"session_id\":\"ses_cli\",\"timestamp\":\"2026-07-16T09:00:00Z\",",
    "\"sequence\":0,\"source\":\"generic-jsonl\",\"type\":\"session.started\",",
    "\"project\":\"/repo/cli\"}\n",
    "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_cli_failure\",",
    "\"session_id\":\"ses_cli\",\"timestamp\":\"2026-07-16T09:01:00Z\",",
    "\"sequence\":1,\"source\":\"generic-jsonl\",\"type\":\"tool.failed\",",
    "\"project\":\"/repo/cli\",\"tool\":{\"name\":\"bash\",\"exit_code\":1},",
    "\"metadata\":{\"search\":\"generated client stale\"}}\n"
);

#[test]
fn import_sessions_search_and_reimport_work_end_to_end() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = directory.path().join("events.jsonl");
    let database = directory.path().join("autophagy.db");
    fs::write(&input, VALID_JSONL).expect("write fixture");

    let imported = run_json(
        &database,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:cli",
            "--index-metadata",
            "search",
        ],
    );
    assert_eq!(imported["command"], "import");
    assert_eq!(imported["result"]["inserted"], 2);
    assert_eq!(imported["result"]["rejected"], 0);

    let sessions = run_json(&database, ["sessions"]);
    assert_eq!(sessions["result"].as_array().expect("sessions").len(), 1);
    assert_eq!(sessions["result"][0]["session_id"], "ses_cli");
    assert_eq!(sessions["result"][0]["event_count"], 2);

    let search = run_json(&database, ["search", "generated"]);
    assert_eq!(search["result"].as_array().expect("hits").len(), 1);
    assert_eq!(search["result"][0]["event_id"], "evt_cli_failure");

    let duplicate = run_json(
        &database,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:cli",
        ],
    );
    assert_eq!(duplicate["result"]["inserted"], 0);
    assert_eq!(duplicate["result"]["duplicates"], 2);
}

#[test]
fn dry_run_with_bad_records_returns_attention_exit_without_creating_database() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = directory.path().join("invalid.jsonl");
    let database = directory.path().join("must-not-exist.db");
    fs::write(&input, "{not json}\n").expect("write fixture");

    let output = command(&database)
        .args([
            "--output",
            "json",
            "import",
            input.to_str().expect("UTF-8 path"),
            "--dry-run",
        ])
        .output()
        .expect("run command");
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON output");
    assert_eq!(report["result"]["rejected"], 1);
    assert!(!database.exists());
}

#[test]
fn claude_code_history_imports_and_reimports_incrementally() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/claude-code/tests/fixtures/projects");

    let imported = run_json(
        &database,
        [
            "import",
            fixture.to_str().expect("UTF-8 path"),
            "--adapter",
            "claude-code",
        ],
    );
    assert_eq!(imported["result"]["inserted"], 8);
    assert_eq!(
        imported["result"]["discovery"]["files"]
            .as_array()
            .expect("files")
            .len(),
        1
    );

    let repeated = run_json(
        &database,
        [
            "import",
            fixture.to_str().expect("UTF-8 path"),
            "--adapter",
            "claude-code",
        ],
    );
    assert_eq!(repeated["result"]["records_seen"], 0);
    assert_eq!(repeated["result"]["inserted"], 0);
}

fn run_json<const N: usize>(database: &Path, args: [&str; N]) -> Value {
    let output = command(database)
        .args(["--output", "json"])
        .args(args)
        .output()
        .expect("run command");
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON output")
}

fn command(database: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_autophagy"));
    command.args(["--database", database.to_str().expect("UTF-8 path")]);
    command
}
