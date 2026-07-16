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
    assert_eq!(sessions["result"][0]["instance_key"], "fixture:cli");

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

#[test]
fn codex_rollouts_import_and_reimport_incrementally() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/codex/tests/fixtures/sessions");

    let imported = run_json(
        &database,
        [
            "import",
            fixture.to_str().expect("UTF-8 path"),
            "--adapter",
            "codex",
        ],
    );
    assert_eq!(imported["result"]["inserted"], 8);
    assert_eq!(imported["result"]["rejected"], 0);

    let repeated = run_json(
        &database,
        [
            "import",
            fixture.to_str().expect("UTF-8 path"),
            "--adapter",
            "codex",
        ],
    );
    assert_eq!(repeated["result"]["records_seen"], 0);
    assert_eq!(repeated["result"]["inserted"], 0);

    let claude_fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/claude-code/tests/fixtures/projects");
    let claude = run_json(
        &database,
        [
            "import",
            claude_fixture.to_str().expect("UTF-8 path"),
            "--adapter",
            "claude-code",
        ],
    );
    assert_eq!(claude["result"]["inserted"], 8);
    let sessions = run_json(&database, ["sessions"]);
    let sessions = sessions["result"].as_array().expect("sessions");
    assert_eq!(sessions.len(), 2);
    assert!(sessions.iter().any(|session| session["adapter"] == "codex"));
    assert!(
        sessions
            .iter()
            .any(|session| session["adapter"] == "claude-code")
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn milestone_demo_digests_exports_deletes_and_prunes_offline() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/findings/deterministic.jsonl");
    let imported = run_json(&database, ["import", fixture.to_str().expect("UTF-8 path")]);
    assert_eq!(imported["result"]["inserted"], 11);

    let patterns = run_json(&database, ["patterns"]);
    let findings = patterns["result"].as_array().expect("findings");
    assert_eq!(findings.len(), 2);
    assert!(
        findings
            .iter()
            .all(|finding| finding["evidence"].as_array().expect("evidence").len() == 3)
    );

    let digest = run_json(&database, ["digest"]);
    assert_eq!(digest["result"]["spec_version"], "digest/0.1");
    assert_eq!(digest["result"]["events_scanned"], 11);
    assert_eq!(digest["result"]["model_used"], false);
    assert_eq!(digest["result"]["network_used"], false);
    assert_eq!(
        digest["result"]["findings"]
            .as_array()
            .expect("findings")
            .len(),
        2
    );

    let mutations = run_json(&database, ["mutations", "propose"]);
    let candidates = mutations["result"]["generated"]
        .as_array()
        .expect("candidates");
    assert_eq!(candidates.len(), 2);
    assert_eq!(
        mutations["result"]["registrations"]
            .as_array()
            .expect("registrations")
            .len(),
        2
    );
    for outcome in candidates {
        assert_eq!(outcome["status"], "candidate");
        assert_eq!(outcome["package"]["state"], "candidate");
        assert_eq!(outcome["package"]["permissions"]["network"], false);
        assert!(
            outcome["package"]["permissions"]["commands"]
                .as_array()
                .expect("commands")
                .is_empty()
        );
    }

    let repeated = run_json(&database, ["mutations", "propose"]);
    assert!(
        repeated["result"]["registrations"]
            .as_array()
            .expect("registrations")
            .iter()
            .all(|registration| registration["status"] == "duplicate")
    );

    let registry = run_json(&database, ["mutations", "list"]);
    let registry = registry["result"].as_array().expect("registry");
    assert_eq!(registry.len(), 2);
    let failure_id = registry
        .iter()
        .find(|mutation| mutation["source_detector"] == "repeated_command_failure")
        .expect("failure mutation")["mutation_id"]
        .as_str()
        .expect("mutation ID")
        .to_owned();
    let correction_id = registry
        .iter()
        .find(|mutation| mutation["source_detector"] == "repeated_user_correction")
        .expect("correction mutation")["mutation_id"]
        .as_str()
        .expect("mutation ID")
        .to_owned();

    let review_draft = directory.path().join("command-preflight-draft.json");
    let extracted = run_json(
        &database,
        [
            "mutations",
            "replay-draft",
            &failure_id,
            "--suite",
            review_draft.to_str().expect("UTF-8 path"),
            "--context-events",
            "1",
        ],
    );
    assert_eq!(extracted["command"], "mutations_replay_draft");
    assert_eq!(extracted["result"]["scenarios"], 4);
    assert_eq!(extracted["result"]["intervention_scenarios"], 3);
    assert_eq!(extracted["result"]["no_op_scenarios"], 1);
    assert_eq!(extracted["result"]["unreviewed_scenarios"], 3);
    let written_draft: Value =
        serde_json::from_slice(&fs::read(&review_draft).expect("read review draft"))
            .expect("review draft JSON");
    assert_eq!(written_draft, extracted["result"]["draft"]);
    assert_eq!(
        written_draft["scenarios"]
            .as_array()
            .expect("draft scenarios")
            .iter()
            .filter(|scenario| scenario["counterfactual_outcome"] == "unknown")
            .count(),
        3
    );
    let unreviewed = command(&database)
        .args([
            "mutations",
            "replay",
            &failure_id,
            "--scenarios",
            review_draft.to_str().expect("UTF-8 path"),
        ])
        .output()
        .expect("unreviewed replay");
    assert!(!unreviewed.status.success());
    assert!(
        String::from_utf8_lossy(&unreviewed.stderr).contains("unreviewed counterfactual outcomes")
    );

    let incomplete = command(&database)
        .args([
            "mutations",
            "challenge",
            &failure_id,
            "--check",
            "coincidence-considered",
        ])
        .output()
        .expect("incomplete challenge");
    assert!(!incomplete.status.success());
    assert!(String::from_utf8_lossy(&incomplete.stderr).contains("missing checks"));

    let challenged = run_json(
        &database,
        [
            "mutations",
            "challenge",
            &failure_id,
            "--check",
            "coincidence-considered",
            "--check",
            "sessions-comparable",
            "--check",
            "trigger-observable",
            "--check",
            "legitimate-uses-bounded",
            "--check",
            "equivalent-searched",
            "--check",
            "counterexamples-reviewed",
            "--note",
            "reviewed against the fixture",
        ],
    );
    assert_eq!(challenged["result"]["to_state"], "challenged");
    assert_eq!(challenged["result"]["changed"], true);

    let shown = run_json(&database, ["mutations", "show", &failure_id]);
    assert_eq!(shown["result"]["mutation"]["state"], "challenged");
    assert_eq!(
        shown["result"]["transitions"]
            .as_array()
            .expect("transitions")
            .len(),
        2
    );

    let passing_suite = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/replay/command-preflight-pass.json");
    let mut missing_evidence: Value =
        serde_json::from_slice(&fs::read(&passing_suite).expect("read passing replay fixture"))
            .expect("passing replay fixture JSON");
    missing_evidence["scenarios"][0]["source_event_ids"][0] =
        Value::String("evt_missing_replay_evidence".to_owned());
    let missing_suite = directory.path().join("missing-replay-evidence.json");
    fs::write(
        &missing_suite,
        serde_json::to_vec(&missing_evidence).expect("missing fixture JSON"),
    )
    .expect("write missing fixture");
    let missing = command(&database)
        .args([
            "mutations",
            "replay",
            &failure_id,
            "--scenarios",
            missing_suite.to_str().expect("UTF-8 path"),
        ])
        .output()
        .expect("missing evidence replay");
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("not in the local evidence store"));

    let failing_suite = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/replay/command-preflight-fail.json");
    let failed_replay = command(&database)
        .args([
            "--output",
            "json",
            "mutations",
            "replay",
            &failure_id,
            "--scenarios",
            failing_suite.to_str().expect("UTF-8 path"),
        ])
        .output()
        .expect("failed replay");
    assert_eq!(failed_replay.status.code(), Some(2));
    let failed_replay: Value =
        serde_json::from_slice(&failed_replay.stdout).expect("failed replay JSON");
    assert_eq!(failed_replay["result"]["evaluation"]["passed"], false);
    assert_eq!(
        failed_replay["result"]["registration"]["mutation_state"],
        "challenged"
    );

    let passed_replay = run_json(
        &database,
        [
            "mutations",
            "replay",
            &failure_id,
            "--scenarios",
            passing_suite.to_str().expect("UTF-8 path"),
        ],
    );
    assert_eq!(passed_replay["result"]["evaluation"]["passed"], true);
    assert_eq!(
        passed_replay["result"]["registration"]["mutation_state"],
        "replay_passed"
    );
    assert_eq!(
        passed_replay["result"]["evaluation"]["mutation_executed"],
        false
    );
    let duplicate_replay = run_json(
        &database,
        [
            "mutations",
            "replay",
            &failure_id,
            "--scenarios",
            passing_suite.to_str().expect("UTF-8 path"),
        ],
    );
    assert_eq!(
        duplicate_replay["result"]["registration"]["status"],
        "duplicate"
    );

    let replayed = run_json(&database, ["mutations", "show", &failure_id]);
    assert_eq!(replayed["result"]["mutation"]["state"], "replay_passed");
    assert_eq!(
        replayed["result"]["transitions"]
            .as_array()
            .expect("transitions")
            .len(),
        3
    );
    assert_eq!(
        replayed["result"]["replays"]
            .as_array()
            .expect("replays")
            .len(),
        2
    );

    let failing_shadow = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/shadow/command-preflight-fail.json");
    let failed_shadow = command(&database)
        .args([
            "--output",
            "json",
            "mutations",
            "shadow",
            &failure_id,
            "--observations",
            failing_shadow.to_str().expect("UTF-8 path"),
        ])
        .output()
        .expect("failed shadow");
    assert_eq!(failed_shadow.status.code(), Some(2));
    let failed_shadow: Value =
        serde_json::from_slice(&failed_shadow.stdout).expect("failed shadow JSON");
    assert_eq!(failed_shadow["result"]["evaluation"]["passed"], false);
    assert_eq!(
        failed_shadow["result"]["registration"]["mutation_state"],
        "replay_passed"
    );

    let passing_shadow = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/shadow/command-preflight-pass.json");
    let passed_shadow = run_json(
        &database,
        [
            "mutations",
            "shadow",
            &failure_id,
            "--observations",
            passing_shadow.to_str().expect("UTF-8 path"),
        ],
    );
    assert_eq!(passed_shadow["result"]["evaluation"]["passed"], true);
    assert_eq!(
        passed_shadow["result"]["evaluation"]["mutation_applied"],
        false
    );
    assert_eq!(
        passed_shadow["result"]["registration"]["mutation_state"],
        "shadow_passed"
    );

    let install_repository = directory.path().join("install-target");
    fs::create_dir(&install_repository).expect("install target");
    fs::create_dir(install_repository.join(".git")).expect("git marker");
    let refused_install = command(&database)
        .args([
            "mutations",
            "install",
            &failure_id,
            "--repository",
            install_repository.to_str().expect("UTF-8 path"),
            "--confirm-permissions",
            "nope",
        ])
        .output()
        .expect("refused install");
    assert!(!refused_install.status.success());
    assert!(!install_repository.join(".agents").exists());

    let preview_install = run_json(
        &database,
        [
            "mutations",
            "install",
            &failure_id,
            "--repository",
            install_repository.to_str().expect("UTF-8 path"),
            "--confirm-permissions",
            "repo-skill-write",
            "--dry-run",
        ],
    );
    assert_eq!(preview_install["result"]["dry_run"], true);
    assert_eq!(preview_install["result"]["materialized"], false);
    assert!(!install_repository.join(".agents").exists());

    let installed = run_json(
        &database,
        [
            "mutations",
            "install",
            &failure_id,
            "--repository",
            install_repository.to_str().expect("UTF-8 path"),
            "--confirm-permissions",
            "repo-skill-write",
        ],
    );
    assert_eq!(installed["result"]["materialized"], true);
    assert_eq!(
        installed["result"]["transition"]["mutation_state"],
        "active"
    );
    let installed_path = install_repository.join(
        installed["result"]["relative_path"]
            .as_str()
            .expect("relative path"),
    );
    assert!(installed_path.is_file());
    assert!(
        fs::read_to_string(&installed_path)
            .expect("installed skill")
            .contains("Mutation: `mut_")
    );

    let uninstalled = run_json(&database, ["mutations", "uninstall", &failure_id]);
    assert_eq!(uninstalled["result"]["mutation_state"], "retired");
    assert_eq!(uninstalled["result"]["installation_state"], "uninstalled");
    assert!(!installed_path.exists());

    let retired = run_json(&database, ["mutations", "show", &failure_id]);
    assert_eq!(retired["result"]["mutation"]["state"], "retired");
    assert_eq!(
        retired["result"]["shadows"]
            .as_array()
            .expect("shadows")
            .len(),
        2
    );
    assert_eq!(
        retired["result"]["installations"][0]["state"],
        "uninstalled"
    );

    let rejected = run_json(
        &database,
        [
            "mutations",
            "reject",
            &correction_id,
            "--reason",
            "rule is too broad",
        ],
    );
    assert_eq!(rejected["result"]["to_state"], "rejected");

    let exported = command(&database).arg("export").output().expect("export");
    assert!(exported.status.success());
    let lines = String::from_utf8(exported.stdout).expect("UTF-8 export");
    assert_eq!(lines.lines().count(), 11);
    for line in lines.lines() {
        autophagy_events::Event::from_json_str(line).expect("valid exported AEP event");
    }

    let deleted = run_json(&database, ["delete", "session", "ses_failure_1"]);
    assert_eq!(deleted["result"]["session_deleted"], true);
    assert_eq!(deleted["result"]["events_deleted"], 1);
    assert_eq!(deleted["result"]["mutations_deleted"], 1);
    assert_eq!(
        run_json(&database, ["mutations", "list"])["result"]
            .as_array()
            .expect("registry")
            .len(),
        1
    );

    let preview = run_json(&database, ["prune", "--older-than-days", "0", "--dry-run"]);
    assert_eq!(preview["result"]["events_deleted"], 10);
    assert_eq!(preview["result"]["mutations_deleted"], 1);
    assert_eq!(preview["result"]["dry_run"], true);
    assert_eq!(
        run_json(&database, ["sessions"])["result"]
            .as_array()
            .expect("sessions")
            .len(),
        9
    );

    let pruned = run_json(&database, ["prune", "--older-than-days", "0"]);
    assert_eq!(pruned["result"]["events_deleted"], 10);
    assert_eq!(pruned["result"]["mutations_deleted"], 1);
    assert_eq!(pruned["result"]["dry_run"], false);
    assert!(
        run_json(&database, ["sessions"])["result"]
            .as_array()
            .expect("sessions")
            .is_empty()
    );
    assert!(
        run_json(&database, ["mutations", "list"])["result"]
            .as_array()
            .expect("registry")
            .is_empty()
    );
}

#[test]
fn recovery_motif_is_detected_and_registered_end_to_end() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/findings/recovery-motif.jsonl");
    let imported = run_json(&database, ["import", fixture.to_str().expect("UTF-8 path")]);
    assert_eq!(imported["result"]["inserted"], 11);

    let patterns = run_json(&database, ["patterns"]);
    let recovery = patterns["result"]
        .as_array()
        .expect("patterns")
        .iter()
        .find(|finding| finding["detector"] == "repeated_successful_recovery")
        .expect("recovery finding");
    assert_eq!(recovery["score"]["occurrences"], 3);
    assert_eq!(recovery["evidence"].as_array().expect("evidence").len(), 9);
    assert_eq!(
        recovery["counterexamples"]
            .as_array()
            .expect("counterexamples")
            .len(),
        2
    );

    let proposed = run_json(&database, ["mutations", "propose"]);
    let recovery = proposed["result"]["generated"]
        .as_array()
        .expect("generated")
        .iter()
        .find(|outcome| outcome["package"]["source_detector"] == "repeated_successful_recovery")
        .expect("recovery candidate");
    assert_eq!(recovery["status"], "candidate");
    assert!(
        recovery["package"]["intervention"]["instruction"]
            .as_str()
            .expect("instruction")
            .contains("mise run codegen")
    );
    assert_eq!(recovery["package"]["permissions"]["network"], false);
    assert!(
        proposed["result"]["registrations"]
            .as_array()
            .expect("registrations")
            .iter()
            .any(|registration| registration["status"] == "inserted")
    );
}

#[test]
fn import_redacts_secrets_excludes_paths_and_requires_delete_confirmation() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let input = directory.path().join("privacy.jsonl");
    fs::write(&input, concat!(
        "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_cli_secret\",\"session_id\":\"ses_cli_secret\",\"timestamp\":\"2026-07-16T09:00:00Z\",\"source\":\"generic-jsonl\",\"type\":\"tool.called\",\"project\":\"/repo/public\",\"tool\":{\"name\":\"shell\",\"input\":{\"command\":\"API_KEY=abcdefgh12345678\"}}}\n",
        "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_cli_private\",\"session_id\":\"ses_cli_private\",\"timestamp\":\"2026-07-16T09:01:00Z\",\"source\":\"generic-jsonl\",\"type\":\"session.started\",\"project\":\"/repo/private/client\"}\n"
    )).expect("privacy fixture");
    let imported = run_json(
        &database,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--exclude-path",
            "**/private/**",
        ],
    );
    assert_eq!(imported["result"]["inserted"], 1);
    assert_eq!(imported["result"]["privacy_skipped"], 1);
    assert_eq!(imported["result"]["redacted_fields"], 1);
    let sessions = run_json(&database, ["sessions"]);
    assert!(
        sessions["result"][0]["instance_key"]
            .as_str()
            .expect("instance key")
            .starts_with("path:")
    );
    assert!(
        !sessions["result"][0]["instance_key"]
            .as_str()
            .expect("instance key")
            .contains(directory.path().to_str().expect("path"))
    );

    let exported = command(&database).arg("export").output().expect("export");
    let exported = String::from_utf8(exported.stdout).expect("UTF-8");
    assert!(exported.contains("[REDACTED]"));
    assert!(!exported.contains("abcdefgh12345678"));
    assert!(!exported.contains("evt_cli_private"));

    let refused = command(&database)
        .args(["delete", "all", "--confirm", "nope"])
        .output()
        .expect("refused delete");
    assert!(!refused.status.success());
    assert_eq!(
        run_json(&database, ["sessions"])["result"]
            .as_array()
            .expect("sessions")
            .len(),
        1
    );

    let deleted = run_json(&database, ["delete", "all", "--confirm", "delete-all"]);
    assert_eq!(deleted["result"]["events_deleted"], 1);
    assert_eq!(deleted["result"]["sessions_deleted"], 1);
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
