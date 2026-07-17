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

/// A fixture whose failing tool call carries `tool.input`, so indexing builds a
/// signature and a single-occurrence finding qualifies at the lowest thresholds.
const INDEXABLE_JSONL: &str = concat!(
    "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_ix_start\",",
    "\"session_id\":\"ses_ix\",\"timestamp\":\"2026-07-16T10:00:00Z\",",
    "\"sequence\":0,\"source\":\"generic-jsonl\",\"type\":\"session.started\",",
    "\"project\":\"/workspace/demo\"}\n",
    "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_ix_failure\",",
    "\"session_id\":\"ses_ix\",\"timestamp\":\"2026-07-16T10:01:00Z\",",
    "\"sequence\":1,\"source\":\"generic-jsonl\",\"type\":\"tool.failed\",",
    "\"project\":\"/workspace/demo\",\"tool\":{\"name\":\"bash\",",
    "\"input\":\"cargo test\",\"exit_code\":1},",
    "\"metadata\":{\"summary\":\"schema changed; generated client was stale\"}}\n",
    "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_ix_end\",",
    "\"session_id\":\"ses_ix\",\"timestamp\":\"2026-07-16T10:02:00Z\",",
    "\"sequence\":2,\"source\":\"generic-jsonl\",\"type\":\"session.ended\",",
    "\"project\":\"/workspace/demo\"}\n"
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
    let findings = patterns["result"]["findings"].as_array().expect("findings");
    assert_eq!(findings.len(), 2);
    assert!(
        findings
            .iter()
            .all(|finding| finding["evidence"].as_array().expect("evidence").len() == 3)
    );
    assert_eq!(patterns["result"]["events_scanned"], 11);
    assert_eq!(patterns["result"]["sessions_scanned"], 10);
    // Diagnostics accompany findings so a scan is never silent.
    assert!(
        patterns["result"]["candidate_signatures"]
            .as_u64()
            .expect("candidates")
            >= 2
    );
    assert!(patterns["result"]["observations"].is_array());

    let digest = run_json(&database, ["digest"]);
    assert_eq!(digest["result"]["spec_version"], "digest/0.1");
    assert_eq!(digest["result"]["events_scanned"], 11);
    assert_eq!(digest["result"]["sessions_scanned"], 10);
    assert_eq!(digest["result"]["model_used"], false);
    assert_eq!(digest["result"]["network_used"], false);
    assert!(
        digest["result"]["candidate_signatures"]
            .as_u64()
            .expect("candidates")
            >= 2
    );
    assert!(digest["result"]["observations"].is_array());
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
    // `--scenarios` still works as a hidden alias for the canonical `--suite`.
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
    let unreviewed_stderr = String::from_utf8_lossy(&unreviewed.stderr);
    // The error names the unreviewed scenarios and the exact remedy, pointing at
    // the suite file the user must edit.
    assert!(
        unreviewed_stderr.contains("unreviewed scenario"),
        "missing scenario count: {unreviewed_stderr}"
    );
    assert!(
        unreviewed_stderr.contains("rps_"),
        "missing scenario ids: {unreviewed_stderr}"
    );
    assert!(
        unreviewed_stderr
            .contains("set counterfactual_outcome to \"expected_result\" or \"contradiction\""),
        "missing remedy: {unreviewed_stderr}"
    );
    assert!(
        unreviewed_stderr.contains(review_draft.to_str().expect("UTF-8 path")),
        "missing suite path: {unreviewed_stderr}"
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
    assert_eq!(preview_install["result"]["target"], "codex_repo_skill");
    assert!(!install_repository.join(".agents").exists());

    // The `--target claude-code` selector plans a `.claude/skills` skill and
    // reports the Claude Code target without writing anything on a dry run.
    let claude_preview = run_json(
        &database,
        [
            "mutations",
            "install",
            &failure_id,
            "--repository",
            install_repository.to_str().expect("UTF-8 path"),
            "--target",
            "claude-code",
            "--confirm-permissions",
            "repo-skill-write",
            "--dry-run",
        ],
    );
    assert_eq!(claude_preview["result"]["target"], "claude_code_repo_skill");
    assert!(
        claude_preview["result"]["relative_path"]
            .as_str()
            .expect("relative path")
            .starts_with(".claude/skills/")
    );
    assert_eq!(claude_preview["result"]["materialized"], false);
    assert!(!install_repository.join(".claude").exists());

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

/// `propose`/`synthesize` regenerate the exact same deterministic candidate
/// for evidence that was already registered in an earlier pass. Once that
/// mutation moves past `candidate` (rejected, shadow-evaluated, retired, ...)
/// re-running `propose`/`synthesize` must display its ACTUAL current state,
/// never the stale, generation-time `candidate` label.
#[test]
fn propose_and_synthesize_display_current_mutation_state_not_stale_candidate() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/findings/deterministic.jsonl");
    run_json(&database, ["import", fixture.to_str().expect("UTF-8 path")]);

    let proposed = run_json(&database, ["mutations", "propose"]);
    let generated = proposed["result"]["generated"]
        .as_array()
        .expect("generated");
    assert_eq!(generated.len(), 2);
    let mutation_id = generated[0]["package"]["mutation_id"]
        .as_str()
        .expect("mutation id")
        .to_owned();
    // Freshly generated and just registered: both fixture rows are still
    // literally `candidate`.
    assert_eq!(
        proposed["result"]["current_states"][mutation_id.as_str()],
        "candidate"
    );

    // Move the mutation past `candidate` with an auditable rejection.
    let rejected = command(&database)
        .args([
            "mutations",
            "reject",
            &mutation_id,
            "--reason",
            "superseded by manual review",
        ])
        .output()
        .expect("reject");
    assert!(
        rejected.status.success(),
        "reject failed: {}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    let registry = run_json(&database, ["mutations", "list"]);
    let stored_state = registry["result"]
        .as_array()
        .expect("registry")
        .iter()
        .find(|mutation| mutation["mutation_id"] == mutation_id)
        .expect("mutation still registered")["state"]
        .clone();
    assert_eq!(stored_state, "rejected");

    // Re-running `propose` deterministically re-derives the identical
    // candidate for the same evidence (same mutation ID, same content), so
    // registration collapses to a no-op `duplicate` outcome. The JSON
    // `current_states` map must reflect the mutation's real state, not the
    // package's generation-time classification.
    let reproposed = run_json(&database, ["mutations", "propose"]);
    assert_eq!(
        reproposed["result"]["current_states"][mutation_id.as_str()],
        "rejected"
    );

    // `mutations synthesize` (deterministic provider) must show the same fix.
    let synthesized = run_json(&database, ["mutations", "synthesize"]);
    assert_eq!(
        synthesized["result"]["current_states"][mutation_id.as_str()],
        "rejected"
    );

    // The human-readable text row must print the real state too — never the
    // hardcoded `candidate` literal the audit found.
    let text_output = command(&database)
        .args(["mutations", "propose"])
        .output()
        .expect("run propose text");
    assert!(text_output.status.success());
    let stdout = String::from_utf8_lossy(&text_output.stdout);
    let row = stdout
        .lines()
        .find(|line| line.starts_with(&mutation_id))
        .unwrap_or_else(|| panic!("no row for {mutation_id} in:\n{stdout}"));
    assert!(
        row.ends_with("\trejected"),
        "row must show the actual current state, not a stale 'candidate': {row}"
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
    let recovery = patterns["result"]["findings"]
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

#[test]
#[allow(clippy::too_many_lines)]
fn claude_code_install_and_uninstall_round_trip_through_cli() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/findings/deterministic.jsonl");
    run_json(&database, ["import", fixture.to_str().expect("UTF-8 path")]);
    run_json(&database, ["mutations", "propose"]);
    let registry = run_json(&database, ["mutations", "list"]);
    let failure_id = registry["result"]
        .as_array()
        .expect("registry")
        .iter()
        .find(|mutation| mutation["source_detector"] == "repeated_command_failure")
        .expect("failure mutation")["mutation_id"]
        .as_str()
        .expect("mutation ID")
        .to_owned();

    run_json(
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
        ],
    );
    let passing_replay = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/replay/command-preflight-pass.json");
    run_json(
        &database,
        [
            "mutations",
            "replay",
            &failure_id,
            "--scenarios",
            passing_replay.to_str().expect("UTF-8 path"),
        ],
    );
    let passing_shadow = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/shadow/command-preflight-pass.json");
    run_json(
        &database,
        [
            "mutations",
            "shadow",
            &failure_id,
            "--observations",
            passing_shadow.to_str().expect("UTF-8 path"),
        ],
    );

    let repository = directory.path().join("claude-target");
    fs::create_dir(&repository).expect("repository");
    fs::create_dir(repository.join(".git")).expect("git marker");

    // Real (non-dry-run) install materializes the Claude Code skill.
    let installed = run_json(
        &database,
        [
            "mutations",
            "install",
            &failure_id,
            "--repository",
            repository.to_str().expect("UTF-8 path"),
            "--target",
            "claude-code",
            "--confirm-permissions",
            "repo-skill-write",
        ],
    );
    assert_eq!(installed["result"]["target"], "claude_code_repo_skill");
    assert_eq!(installed["result"]["materialized"], true);
    assert_eq!(
        installed["result"]["transition"]["mutation_state"],
        "active"
    );
    let installed_path = repository.join(
        installed["result"]["relative_path"]
            .as_str()
            .expect("relative path"),
    );
    assert!(installed_path.is_file());
    let body = fs::read_to_string(&installed_path).expect("installed skill");
    assert!(body.contains("## Evidence"));
    assert!(body.contains(&failure_id));

    // Uninstall (no --target flag) reconstructs the materializer from the
    // stored audit target and reverses cleanly.
    let uninstalled = run_json(&database, ["mutations", "uninstall", &failure_id]);
    assert_eq!(uninstalled["result"]["mutation_state"], "retired");
    assert_eq!(uninstalled["result"]["installation_state"], "uninstalled");
    assert!(!installed_path.exists());

    // The retired installation audit retains the Claude Code target.
    let shown = run_json(&database, ["mutations", "show", &failure_id]);
    assert_eq!(
        shown["result"]["installations"][0]["target"],
        "claude_code_repo_skill"
    );
    assert_eq!(shown["result"]["installations"][0]["state"], "uninstalled");
}

#[test]
fn reindex_rebuilds_search_from_history_imported_without_indexing() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/claude-code/tests/fixtures/projects");

    // Import native history WITHOUT indexing: canonical events land, but the
    // exact-signature index is empty and tool input is not searchable.
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
    assert!(
        run_json(&database, ["search", "check"])["result"]
            .as_array()
            .expect("hits")
            .is_empty(),
        "commands must not be searchable before reindex"
    );

    // Reindexing without the gate rebuilds only the baseline projection.
    let baseline = run_json(&database, ["reindex"]);
    assert_eq!(baseline["result"]["events_scanned"], 8);
    assert_eq!(baseline["result"]["search_rows_written"], 8);
    assert_eq!(baseline["result"]["signatures_written"], 0);

    // Reindexing with the gate makes commands searchable and populates the
    // exact-signature index — something reimport can never do.
    let rebuilt = run_json(&database, ["reindex", "--index-tool-input"]);
    assert_eq!(rebuilt["result"]["events_scanned"], 8);
    assert!(
        rebuilt["result"]["signatures_written"]
            .as_u64()
            .expect("signatures")
            > 0
    );
    assert!(
        !run_json(&database, ["search", "check"])["result"]
            .as_array()
            .expect("hits")
            .is_empty(),
        "commands must be searchable after reindex --index-tool-input"
    );

    // Idempotent: a second identical rebuild yields identical counts.
    let again = run_json(&database, ["reindex", "--index-tool-input"]);
    assert_eq!(
        again["result"]["events_scanned"],
        rebuilt["result"]["events_scanned"]
    );
    assert_eq!(
        again["result"]["signatures_written"],
        rebuilt["result"]["signatures_written"]
    );
    assert_eq!(
        again["result"]["search_rows_written"],
        rebuilt["result"]["search_rows_written"]
    );
}

#[test]
fn reindex_exclude_path_drops_matching_projects_from_the_search_index() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let input = directory.path().join("two-projects.jsonl");
    fs::write(
        &input,
        concat!(
            "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_keep\",\"session_id\":\"ses_keep\",",
            "\"timestamp\":\"2026-07-16T09:00:00Z\",\"source\":\"generic-jsonl\",\"type\":\"tool.called\",",
            "\"project\":\"/repo/keep\",\"tool\":{\"name\":\"shell\",\"input\":{\"command\":\"keepneedle build\"}}}\n",
            "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_drop\",\"session_id\":\"ses_drop\",",
            "\"timestamp\":\"2026-07-16T09:01:00Z\",\"source\":\"generic-jsonl\",\"type\":\"tool.called\",",
            "\"project\":\"/repo/drop\",\"tool\":{\"name\":\"shell\",\"input\":{\"command\":\"dropneedle deploy\"}}}\n"
        ),
    )
    .expect("two-project fixture");

    // Import both projects with indexing so both are searchable to begin with.
    let imported = run_json(
        &database,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--index-tool-input",
        ],
    );
    assert_eq!(imported["result"]["inserted"], 2);
    assert_eq!(
        run_json(&database, ["search", "keepneedle"])["result"]
            .as_array()
            .expect("hits")
            .len(),
        1
    );
    assert_eq!(
        run_json(&database, ["search", "dropneedle"])["result"]
            .as_array()
            .expect("hits")
            .len(),
        1
    );

    // Reindex excluding one project: its rows must leave the index entirely,
    // including the structural project path and tool name, while the kept
    // project stays searchable.
    let rebuilt = run_json(
        &database,
        ["reindex", "--index-tool-input", "--exclude-path", "**/drop"],
    );
    assert_eq!(rebuilt["result"]["events_scanned"], 2);
    assert_eq!(rebuilt["result"]["search_rows_written"], 1);
    assert_eq!(rebuilt["result"]["signatures_written"], 1);

    assert_eq!(
        run_json(&database, ["search", "keepneedle"])["result"]
            .as_array()
            .expect("hits")
            .len(),
        1,
        "the included project must remain searchable"
    );
    assert!(
        run_json(&database, ["search", "dropneedle"])["result"]
            .as_array()
            .expect("hits")
            .is_empty(),
        "the excluded project must not be searchable after reindex"
    );
    // Even the excluded project's path must not surface via free text.
    assert!(
        run_json(&database, ["search", "drop"])["result"]
            .as_array()
            .expect("hits")
            .is_empty(),
        "the excluded project path must not stay in the index"
    );
}

#[test]
fn setup_non_interactive_imports_runs_digest_and_leaves_monitoring_off() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let config_dir = directory.path().join("claude-config");
    let project_dir = config_dir.join("projects").join("-workspace-demo");
    fs::create_dir_all(&project_dir).expect("create projects dir");
    fs::write(project_dir.join("session.jsonl"), CLAUDE_TRANSCRIPT).expect("write transcript");
    let database = directory.path().join("setup.db");

    let output = command(&database)
        .args(["--output", "json"])
        .args([
            "setup",
            "--adapter",
            "claude-code",
            "--index-tool-input",
            "--yes",
        ])
        .env("CLAUDE_CONFIG_DIR", &config_dir)
        .output()
        .expect("run setup");
    assert!(
        output.status.success(),
        "setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("setup JSON");
    assert_eq!(report["command"], "setup");
    assert_eq!(report["result"]["index_tool_input"], true);
    assert_eq!(report["result"]["monitor_installed"], false);

    let adapter = report["result"]["adapters"]
        .as_array()
        .expect("adapters")
        .iter()
        .find(|entry| entry["adapter"] == "claude-code")
        .expect("claude-code adapter entry");
    assert_eq!(adapter["present"], true);
    assert!(adapter["inserted"].as_u64().expect("inserted") > 0);
    assert!(
        report["result"]["digest_events_scanned"]
            .as_u64()
            .expect("scanned")
            > 0
    );
    // The digest report carries #28's diagnostics; the observation count is
    // always present so a zero-finding scan is never a silent nothing.
    assert!(report["result"]["digest_observations"].is_u64());

    // Setup wired the indexing gate, so search returns hits immediately.
    assert!(
        !run_json(&database, ["search", "check"])["result"]
            .as_array()
            .expect("hits")
            .is_empty()
    );
}

#[test]
fn setup_digest_surfaces_scan_stats_and_observations_when_nothing_qualifies() {
    // A single session with two identical command failures crosses no finding
    // threshold (needs three occurrences across two sessions), so the digest
    // must fall back to scan stats plus the near-threshold observation rather
    // than printing nothing — surfaced through the shared digest renderer.
    let directory = tempfile::tempdir().expect("temporary directory");
    let config_dir = directory.path().join("claude-config");
    let project_dir = config_dir.join("projects").join("-workspace-demo");
    fs::create_dir_all(&project_dir).expect("create projects dir");
    fs::write(project_dir.join("session.jsonl"), CLAUDE_NEAR_THRESHOLD).expect("write transcript");
    let database = directory.path().join("setup.db");

    let output = command(&database)
        .args([
            "setup",
            "--adapter",
            "claude-code",
            "--index-tool-input",
            "--yes",
        ])
        .env("CLAUDE_CONFIG_DIR", &config_dir)
        .output()
        .expect("run setup");
    assert!(
        output.status.success(),
        "setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    assert!(
        stdout.contains("candidate signatures") && stdout.contains("0 findings"),
        "digest scan stats missing from setup output:\n{stdout}"
    );
    assert!(
        stdout.contains("near-threshold observations"),
        "zero-finding digest must surface observations:\n{stdout}"
    );
    assert!(
        stdout.contains("needs 3+ occurrences, saw 2"),
        "each observation must name the missed gate in plain language:\n{stdout}"
    );
    assert!(
        !stdout.contains("bps"),
        "the digest must speak in percentages, not basis points:\n{stdout}"
    );
}

#[test]
fn setup_model_backend_writes_a_loadable_manifest_and_config() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let config_dir = directory.path().join("claude-config");
    let project_dir = config_dir.join("projects").join("-workspace-demo");
    fs::create_dir_all(&project_dir).expect("create projects dir");
    fs::write(project_dir.join("session.jsonl"), CLAUDE_TRANSCRIPT).expect("write transcript");
    let database = directory.path().join("setup.db");

    let output = command(&database)
        .args([
            "setup",
            "--adapter",
            "claude-code",
            "--index-tool-input",
            "--model-backend",
            "claude-cli",
            "--yes",
        ])
        .env("CLAUDE_CONFIG_DIR", &config_dir)
        .output()
        .expect("run setup");
    assert!(
        output.status.success(),
        "setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    assert!(
        stdout.contains("--allow-remote-endpoint"),
        "cloud backends must state the consent requirement:\n{stdout}"
    );
    assert!(
        stdout.contains("mutations synthesize --provider claude-cli"),
        "setup must print the exact synthesize command:\n{stdout}"
    );

    // The manifest is written next to the config file and satisfies the
    // versioned manifest contract.
    let manifest_path = directory.path().join("synthesis-manifest.json");
    let manifest = autophagy_synthesis::ModelManifest::from_path(&manifest_path)
        .expect("manifest must load through the versioned contract");
    assert!(manifest.declares(autophagy_synthesis::Capability::MutationSynthesis));

    // The chosen backend is persisted so `mutations synthesize` inherits it.
    let config = fs::read_to_string(directory.path().join("config.toml")).expect("config");
    assert!(config.contains("provider = \"claude-cli\""), "{config}");
    assert!(config.contains("synthesis-manifest.json"), "{config}");

    // Default backend (`none`) leaves synthesis config untouched.
    let second = tempfile::tempdir().expect("temporary directory");
    let second_db = second.path().join("setup.db");
    let output = command(&second_db)
        .args(["setup", "--yes"])
        .output()
        .expect("run setup");
    assert!(output.status.success());
    let config = fs::read_to_string(second.path().join("config.toml")).expect("config");
    assert!(!config.contains("[synthesis]"), "{config}");
    assert!(!second.path().join("synthesis-manifest.json").exists());
}

#[test]
fn setup_without_terminal_or_flags_points_at_the_non_interactive_flags() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("setup.db");

    // No terminal (test harness stdin is not a TTY) and no `--yes`: setup must
    // refuse with guidance rather than hang waiting for input.
    let output = command(&database).arg("setup").output().expect("run setup");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--yes"),
        "stderr should name the flags: {stderr}"
    );
    assert!(
        !database.exists(),
        "a refused setup must not create a database"
    );
}

const CLAUDE_TRANSCRIPT: &str = concat!(
    "{\"type\":\"user\",\"uuid\":\"r1\",\"sessionId\":\"11111111-1111-4111-8111-111111111111\",",
    "\"timestamp\":\"2026-07-16T08:00:00Z\",\"cwd\":\"/workspace/demo\",",
    "\"message\":{\"role\":\"user\",\"content\":\"Please inspect the build.\"}}\n",
    "{\"type\":\"assistant\",\"uuid\":\"r2\",\"sessionId\":\"11111111-1111-4111-8111-111111111111\",",
    "\"timestamp\":\"2026-07-16T08:00:01Z\",\"cwd\":\"/workspace/demo\",",
    "\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"I will run the build.\"},",
    "{\"type\":\"tool_use\",\"id\":\"tool-1\",\"name\":\"Bash\",\"input\":{\"command\":\"mise run check\"}}]}}\n",
    "{\"type\":\"user\",\"uuid\":\"r3\",\"sessionId\":\"11111111-1111-4111-8111-111111111111\",",
    "\"timestamp\":\"2026-07-16T08:00:02Z\",\"cwd\":\"/workspace/demo\",",
    "\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"tool-1\",",
    "\"is_error\":true,\"content\":\"Exit code 1\\nfixture failure\"}]}}\n"
);

/// Two identical command failures in one session: enough to be a recurring
/// candidate signature, but below the finding thresholds, so the digest reports
/// it as a near-threshold observation.
const CLAUDE_NEAR_THRESHOLD: &str = concat!(
    "{\"type\":\"assistant\",\"uuid\":\"n1\",\"sessionId\":\"22222222-2222-4222-8222-222222222222\",",
    "\"timestamp\":\"2026-07-16T08:00:00Z\",\"cwd\":\"/workspace/demo\",",
    "\"message\":{\"role\":\"assistant\",\"content\":[",
    "{\"type\":\"tool_use\",\"id\":\"n-tool-1\",\"name\":\"Bash\",\"input\":{\"command\":\"mise run check\"}}]}}\n",
    "{\"type\":\"user\",\"uuid\":\"n2\",\"sessionId\":\"22222222-2222-4222-8222-222222222222\",",
    "\"timestamp\":\"2026-07-16T08:00:01Z\",\"cwd\":\"/workspace/demo\",",
    "\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"n-tool-1\",",
    "\"is_error\":true,\"content\":\"Exit code 1\\nboom\"}]}}\n",
    "{\"type\":\"assistant\",\"uuid\":\"n3\",\"sessionId\":\"22222222-2222-4222-8222-222222222222\",",
    "\"timestamp\":\"2026-07-16T08:00:02Z\",\"cwd\":\"/workspace/demo\",",
    "\"message\":{\"role\":\"assistant\",\"content\":[",
    "{\"type\":\"tool_use\",\"id\":\"n-tool-2\",\"name\":\"Bash\",\"input\":{\"command\":\"mise run check\"}}]}}\n",
    "{\"type\":\"user\",\"uuid\":\"n4\",\"sessionId\":\"22222222-2222-4222-8222-222222222222\",",
    "\"timestamp\":\"2026-07-16T08:00:03Z\",\"cwd\":\"/workspace/demo\",",
    "\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"n-tool-2\",",
    "\"is_error\":true,\"content\":\"Exit code 1\\nboom\"}]}}\n"
);

/// One watch cycle imports the fixture; a second cycle inserts nothing because
/// the source cursor has already consumed the transcript. Uses `CLAUDE_CONFIG_DIR`
/// so the real `~/.claude` is never touched.
#[test]
fn watch_once_imports_incrementally() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let config_dir = directory.path().join("claude-config");
    let project_dir = config_dir.join("projects").join("-workspace-demo");
    fs::create_dir_all(&project_dir).expect("create projects dir");
    fs::write(project_dir.join("session.jsonl"), CLAUDE_TRANSCRIPT).expect("write transcript");
    let database = directory.path().join("watch.db");

    let first = run_watch_summary(&database, &config_dir);
    assert_eq!(first["cycles"], 1);
    assert_eq!(first["failures"], 0);
    let first_inserted = first["inserted"].as_u64().expect("inserted count");
    assert!(first_inserted > 0, "first cycle should insert events");

    let second = run_watch_summary(&database, &config_dir);
    assert_eq!(second["cycles"], 1);
    assert_eq!(
        second["inserted"], 0,
        "second cycle is incremental (no re-insert)"
    );
}

#[test]
fn config_set_get_unset_round_trip_and_validation() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");

    let set = run_json(&database, ["config", "set", "detect.min_occurrences", "7"]);
    assert_eq!(set["result"]["action"], "set");

    let got = run_json(&database, ["config", "get", "detect.min_occurrences"]);
    assert_eq!(got["result"]["value"], "7");
    assert_eq!(got["result"]["source"], "config");

    // The value is visible in the effective listing with a config source.
    let list = run_json(&database, ["config", "list"]);
    let entry = list["result"]["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["key"] == "detect.min_occurrences")
        .expect("key present");
    assert_eq!(entry["value"], "7");
    assert_eq!(entry["source"], "config");

    let unset = run_json(&database, ["config", "unset", "detect.min_occurrences"]);
    assert_eq!(unset["result"]["removed"], true);

    // After unset the effective value reverts to the built-in default.
    let reverted = run_json(&database, ["config", "get", "detect.min_occurrences"]);
    assert_eq!(reverted["result"]["value"], "3");
    assert_eq!(reverted["result"]["source"], "default");

    // A wrong-typed value is a precise error and does not write.
    let bad = command(&database)
        .args(["config", "set", "detect.min_occurrences", "lots"])
        .output()
        .expect("run");
    assert!(!bad.status.success());
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("detect.min_occurrences"),
        "error names the offending key"
    );

    // An unknown key is rejected.
    let unknown = command(&database)
        .args(["config", "get", "nonsense.key"])
        .output()
        .expect("run");
    assert!(!unknown.status.success());
}

#[test]
fn digest_precedence_is_default_then_config_then_flag() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = directory.path().join("events.jsonl");
    let database = directory.path().join("autophagy.db");
    fs::write(&input, INDEXABLE_JSONL).expect("write fixture");
    run_json(
        &database,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:cli",
            "--index-tool-input",
        ],
    );

    // Default thresholds: the two-event fixture yields no findings.
    let default = run_json(&database, ["digest"]);
    assert_eq!(
        default["result"]["findings"].as_array().expect("arr").len(),
        0
    );

    // Config lowers the thresholds, so a finding qualifies without any flag.
    run_json(&database, ["config", "set", "detect.min_occurrences", "1"]);
    run_json(&database, ["config", "set", "detect.min_sessions", "1"]);
    let configured = run_json(&database, ["digest"]);
    assert_eq!(
        configured["result"]["findings"]
            .as_array()
            .expect("arr")
            .len(),
        1,
        "config thresholds take effect"
    );

    // An explicit flag overrides the config file.
    let overridden = run_json(&database, ["digest", "--min-occurrences", "999"]);
    assert_eq!(
        overridden["result"]["findings"]
            .as_array()
            .expect("arr")
            .len(),
        0,
        "explicit flag wins over config"
    );
}

#[test]
fn import_honors_config_index_tool_input() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = directory.path().join("events.jsonl");
    let database = directory.path().join("autophagy.db");
    fs::write(&input, INDEXABLE_JSONL).expect("write fixture");

    // With indexing enabled only in config (no flag), import builds signatures.
    run_json(
        &database,
        ["config", "set", "import.index_tool_input", "true"],
    );
    run_json(
        &database,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:cli",
        ],
    );
    let status = run_json(&database, ["status"]);
    assert_eq!(status["result"]["index"]["tool_input_indexed"], true);
    assert!(
        status["result"]["index"]["signatures"]
            .as_u64()
            .expect("sigs")
            >= 1,
        "config enabled tool-input indexing"
    );
}

#[test]
fn watch_honors_config_interval_with_flag_override() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let empty_claude = directory.path().join("empty-claude");
    fs::create_dir_all(&empty_claude).expect("mkdir");

    run_json(
        &database,
        ["config", "set", "watch.interval_seconds", "123"],
    );

    // No --interval flag: the configured interval is used.
    let configured = run_watch_summary(&database, &empty_claude);
    assert_eq!(configured["interval_secs"], 123);

    // An explicit --interval overrides the config file.
    let output = command(&database)
        .args(["--output", "json"])
        .args([
            "watch",
            "--adapter",
            "claude-code",
            "--once",
            "--interval",
            "5",
        ])
        .env("CLAUDE_CONFIG_DIR", &empty_claude)
        .output()
        .expect("run watch");
    let stdout = String::from_utf8(output.stdout).expect("UTF-8");
    let last = stdout
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .expect("summary line");
    let overridden: Value = serde_json::from_str(last).expect("summary JSON");
    assert_eq!(overridden["interval_secs"], 5);
}

#[test]
fn status_reports_counts_and_shape_against_a_fixture_database() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = directory.path().join("events.jsonl");
    let database = directory.path().join("autophagy.db");
    fs::write(&input, INDEXABLE_JSONL).expect("write fixture");
    run_json(
        &database,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:cli",
            "--index-tool-input",
        ],
    );

    let status = run_json(&database, ["status"]);
    let result = &status["result"];
    assert_eq!(result["database"]["events"], 3);
    assert_eq!(result["database"]["sessions"], 1);
    assert!(
        result["database"]["schema_version"]
            .as_i64()
            .expect("schema")
            > 0
    );
    assert!(result["database"]["size_bytes"].as_u64().is_some());
    assert_eq!(result["index"]["tool_input_indexed"], true);
    assert_eq!(result["detector"]["min_occurrences"], 3);
    // The generic source appears in the per-adapter activity breakdown.
    let adapters = result["adapters"].as_array().expect("adapters");
    assert!(adapters.iter().any(|adapter| adapter["events"] == 3));
    assert_eq!(result["config_present"], false);
}

#[test]
fn status_works_against_an_empty_database_and_no_config() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let status = run_json(&database, ["status"]);
    let result = &status["result"];
    assert_eq!(result["database"]["events"], 0);
    assert_eq!(result["config_present"], false);
    assert!(result["adapters"].as_array().expect("adapters").is_empty());
    // Findings are opt-in and omitted by default (a full detection pass).
    assert!(result["findings"].is_null(), "findings omitted by default");

    // `--with-findings` runs the detection pass and reports the count.
    let with = run_json(&database, ["status", "--with-findings"]);
    assert_eq!(with["result"]["findings"], 0);
}

#[test]
fn rerunnable_setup_enables_indexing_and_reindexes_in_place() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let input = directory.path().join("events.jsonl");
    let database = directory.path().join("autophagy.db");
    let empty_claude = directory.path().join("empty-claude");
    fs::create_dir_all(&empty_claude).expect("mkdir");
    fs::write(&input, INDEXABLE_JSONL).expect("write fixture");

    // Seed events without indexing, and a config that has indexing off.
    run_json(
        &database,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:cli",
        ],
    );
    run_json(
        &database,
        ["config", "set", "import.index_tool_input", "false"],
    );

    // Re-run setup non-interactively, newly enabling indexing. No adapter root
    // exists (empty CLAUDE_CONFIG_DIR), so setup imports nothing but must still
    // heal the existing events in place through reindex.
    let output = command(&database)
        .args(["--output", "json"])
        .args([
            "setup",
            "--yes",
            "--index-tool-input",
            "--adapter",
            "claude-code",
        ])
        .env("CLAUDE_CONFIG_DIR", &empty_claude)
        .output()
        .expect("run setup");
    assert!(
        output.status.success(),
        "setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    let result = &report["result"];
    assert_eq!(result["index_tool_input"], true);
    assert_eq!(result["config_written"], true);
    assert!(
        result["reindex"].is_object(),
        "index newly enabled must trigger reindex"
    );
    assert!(
        result["changed"]
            .as_array()
            .expect("changed")
            .iter()
            .any(|change| change
                .as_str()
                .is_some_and(|c| c.contains("index_tool_input"))),
        "the change is reported"
    );

    // The heal is observable: signatures now exist.
    let status = run_json(&database, ["status"]);
    assert_eq!(status["result"]["index"]["tool_input_indexed"], true);
}

/// Run `autophagy watch --adapter claude-code --once` and return the final
/// summary object (the last JSON line the command prints).
fn run_watch_summary(database: &Path, claude_config_dir: &Path) -> Value {
    let output = command(database)
        .args(["--output", "json"])
        .args(["watch", "--adapter", "claude-code", "--once"])
        .env("CLAUDE_CONFIG_DIR", claude_config_dir)
        .output()
        .expect("run watch");
    assert!(
        output.status.success(),
        "watch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    let last = stdout
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .expect("at least one summary line");
    serde_json::from_str(last).expect("summary JSON")
}

#[test]
fn detection_findings_are_cached_and_report_progress() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let indexable = directory.path().join("indexable.jsonl");
    let database = directory.path().join("autophagy.db");
    fs::write(&indexable, INDEXABLE_JSONL).expect("write fixture");

    run_json(
        &database,
        [
            "import",
            indexable.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:cache",
            "--index-tool-input",
        ],
    );

    // Run `patterns`, returning the parsed report and captured stderr so we can
    // observe whether a fresh detection pass actually ran (it prints a progress
    // line to stderr) versus being served from the cache (silent).
    let run = |args: &[&str]| -> (Value, String) {
        let output = command(&database)
            .args(["--output", "json"])
            .args(args)
            .output()
            .expect("run command");
        assert!(
            output.status.success(),
            "command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        (
            serde_json::from_slice(&output.stdout).expect("JSON output"),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };

    // First pass computes fresh and reports progress.
    let (first, first_stderr) = run(&["patterns"]);
    assert!(
        first_stderr.contains("digesting"),
        "first pass should report detection progress: {first_stderr}"
    );

    // A second identical pass is a cache hit: byte-for-byte identical findings
    // (exact evidence IDs included) and no fresh detection pass.
    let (second, second_stderr) = run(&["patterns"]);
    assert_eq!(
        first["result"], second["result"],
        "cache hit must return identical findings"
    );
    assert!(
        !second_stderr.contains("digesting"),
        "an unchanged corpus at unchanged thresholds must not recompute: {second_stderr}"
    );

    // Changing thresholds invalidates the key, forcing a fresh pass.
    let (_, retuned_stderr) = run(&["patterns", "--min-occurrences", "1"]);
    assert!(
        retuned_stderr.contains("digesting"),
        "a threshold change must invalidate the cache: {retuned_stderr}"
    );

    // --recompute forces a fresh pass even on an otherwise-cached key.
    let (_, forced_stderr) = run(&["patterns", "--recompute"]);
    assert!(
        forced_stderr.contains("digesting"),
        "--recompute must always run a fresh pass: {forced_stderr}"
    );

    // A new import changes the corpus and invalidates the cache.
    let more = directory.path().join("more.jsonl");
    fs::write(&more, VALID_JSONL).expect("write second fixture");
    run_json(
        &database,
        [
            "import",
            more.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:cache-2",
        ],
    );
    let (_, after_import_stderr) = run(&["patterns"]);
    assert!(
        after_import_stderr.contains("digesting"),
        "a new import must invalidate the cache: {after_import_stderr}"
    );
    // The corpus is stable again, so the next identical pass is a hit.
    let (_, repeat_stderr) = run(&["patterns"]);
    assert!(
        !repeat_stderr.contains("digesting"),
        "the cache should be warm again after the invalidating import: {repeat_stderr}"
    );
}

/// A read-only command must report the database's real footprint — the store
/// opens in WAL mode, so a naive read of the main file right after migrations
/// undercounts a brand-new database (the ~4 KiB header, not the migrated
/// schema). `status` sums the WAL sidecars so the number matches disk, and marks
/// a database it just created as new rather than pre-existing.
#[test]
fn status_reports_post_migration_size_for_a_new_database() {
    let directory = tempfile::tempdir().expect("temporary directory");

    let db_json = directory.path().join("fresh-json.db");
    assert!(!db_json.exists(), "precondition: database absent");
    let status = run_json(&db_json, ["status"]);
    let size = status["result"]["database"]["size_bytes"]
        .as_u64()
        .expect("size_bytes present");
    assert!(
        size > 100_000,
        "post-migration size must reflect the real footprint, got {size} bytes"
    );
    assert_eq!(
        status["result"]["database"]["exists"], false,
        "a database created by this very command reports as new, not pre-existing"
    );

    let db_text = directory.path().join("fresh-text.db");
    let output = command(&db_text)
        .arg("status")
        .output()
        .expect("run status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("created new database"),
        "text status must say the database was just created: {stdout}"
    );
}

/// On an empty database the search line points at `setup` (there is nothing to
/// reindex yet); once events exist but are unindexed, the reindex hint is the
/// correct one and returns.
#[test]
fn status_search_hint_points_at_setup_before_import_and_reindex_after() {
    let directory = tempfile::tempdir().expect("temporary directory");

    let fresh = directory.path().join("fresh.db");
    let output = command(&fresh).arg("status").output().expect("run status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("autophagy setup"),
        "an empty database's search line must point at setup: {stdout}"
    );
    assert!(
        !stdout.contains("reindex"),
        "an empty database must not suggest reindex — nothing to rebuild: {stdout}"
    );

    // Events imported without indexing: reindex is now the right and only fix.
    let imported = directory.path().join("imported.db");
    let input = directory.path().join("events.jsonl");
    fs::write(&input, VALID_JSONL).expect("write fixture");
    run_json(
        &imported,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:cli",
        ],
    );
    let output = command(&imported)
        .arg("status")
        .output()
        .expect("run status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("reindex --index-tool-input"),
        "an imported-but-unindexed database keeps the reindex hint: {stdout}"
    );
}

/// Empty `sessions` and `search` output guides the new user in text, while the
/// JSON shapes stay stable empty arrays so machine consumers get no prose.
#[test]
fn empty_sessions_and_search_guide_the_user_but_keep_json_arrays() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("empty.db");

    // sessions: guidance instead of a bare header row; JSON is an empty array.
    let output = command(&database)
        .arg("sessions")
        .output()
        .expect("run sessions");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("no sessions imported yet"),
        "empty sessions prints guidance, not a bare header: {stdout}"
    );
    assert!(
        !stdout.contains("SESSION\tSOURCE"),
        "no bare tab-separated header row on empty sessions: {stdout}"
    );
    let sessions_json = run_json(&database, ["sessions"]);
    assert_eq!(
        sessions_json["result"]
            .as_array()
            .expect("sessions array")
            .len(),
        0,
        "sessions JSON stays an empty array"
    );

    // search on an empty database explains nothing was imported; JSON is [].
    let output = command(&database)
        .args(["search", "anything"])
        .output()
        .expect("run search");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("no events imported yet"),
        "search against an empty database explains why it is empty: {stdout}"
    );
    let search_json = run_json(&database, ["search", "anything"]);
    assert!(
        search_json["result"]
            .as_array()
            .expect("search array")
            .is_empty(),
        "search JSON result stays an empty array"
    );

    // Once events exist, a genuine miss reads as a miss — not "nothing imported".
    let input = directory.path().join("events.jsonl");
    fs::write(&input, VALID_JSONL).expect("write fixture");
    run_json(
        &database,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:cli",
        ],
    );
    let output = command(&database)
        .args(["search", "zzznomatch"])
        .output()
        .expect("run search");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("no retrieval matches"),
        "a real miss on a populated database still says so: {stdout}"
    );
    assert!(
        !stdout.contains("no events imported"),
        "a populated database must not claim nothing was imported: {stdout}"
    );
}

/// `setup` must warn, before writing, that it saves settings to the GLOBAL
/// config file when the database was pointed elsewhere and no
/// `AUTOPHAGY_CONFIG_DIR` isolation is in effect — otherwise a throwaway
/// `--database /tmp/…` run silently rewrites the user's real global config.
///
/// The "global" location is faked by overriding `HOME` (and `XDG_DATA_HOME` for
/// Linux) to a throwaway directory, so the resolved config path lands under the
/// temp home and never the developer's real one, while `AUTOPHAGY_CONFIG_DIR`
/// stays unset so the notice condition (config is global) genuinely holds.
#[test]
fn setup_warns_before_writing_global_config_for_explicit_database() {
    let home = tempfile::tempdir().expect("temporary home");
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let database = workspace.path().join("throwaway.db");
    let empty_claude = workspace.path().join("empty-claude");
    fs::create_dir_all(&empty_claude).expect("mkdir");

    let output = Command::new(env!("CARGO_BIN_EXE_autophagy"))
        .args(["--database", database.to_str().expect("UTF-8 path")])
        .args(["setup", "--yes", "--adapter", "claude-code"])
        // Fake the global config location under a throwaway home and leave
        // AUTOPHAGY_CONFIG_DIR unset so setup treats the config as global.
        .env_remove("AUTOPHAGY_CONFIG_DIR")
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join("xdg"))
        // No real adapter history under the throwaway home: import nothing.
        .env("CLAUDE_CONFIG_DIR", &empty_claude)
        .output()
        .expect("run setup");
    assert!(
        output.status.success(),
        "setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("settings are saved globally to"),
        "the global-config notice must appear on stderr: {stderr}"
    );
    assert!(
        stderr.contains("AUTOPHAGY_CONFIG_DIR"),
        "the notice must name the isolation escape hatch: {stderr}"
    );
    assert!(
        stderr.contains(home.path().to_str().expect("UTF-8 home")),
        "the config path in the notice must resolve under the throwaway home, \
         never the real one: {stderr}"
    );
}

/// The mirror of the above: when `AUTOPHAGY_CONFIG_DIR` isolates the run (as the
/// test harness always does), the global-config notice must stay silent.
#[test]
fn setup_omits_global_notice_when_config_dir_is_isolated() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let empty_claude = directory.path().join("empty-claude");
    fs::create_dir_all(&empty_claude).expect("mkdir");

    // command() sets AUTOPHAGY_CONFIG_DIR to the database's temporary directory,
    // so the config is isolated and the notice must not fire.
    let output = command(&database)
        .args(["--output", "json"])
        .args(["setup", "--yes", "--adapter", "claude-code"])
        .env("CLAUDE_CONFIG_DIR", &empty_claude)
        .output()
        .expect("run setup");
    assert!(
        output.status.success(),
        "setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("saved globally"),
        "isolated runs must not print the global-config notice: {stderr}"
    );
}

#[test]
fn deterministic_synthesis_needs_no_manifest() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/findings/deterministic.jsonl");
    run_json(&database, ["import", fixture.to_str().expect("UTF-8 path")]);

    // The built-in `deterministic` provider synthesizes with no --manifest and
    // no config manifest path: it falls back to its reference manifest.
    let synthesized = run_json(&database, ["mutations", "synthesize"]);
    assert_eq!(synthesized["command"], "mutations_synthesize");
    assert_eq!(synthesized["result"]["provider"], "deterministic");
    assert_eq!(synthesized["result"]["model_used"], false);
    assert_eq!(synthesized["result"]["network_used"], false);
    assert_eq!(
        synthesized["result"]["manifest_path"],
        "builtin://deterministic"
    );
    assert!(
        !synthesized["result"]["registrations"]
            .as_array()
            .expect("registrations")
            .is_empty()
    );

    // A non-deterministic provider still requires a manifest, and the error now
    // points at a complete example manifest.
    let missing = command(&database)
        .args(["mutations", "synthesize", "--provider", "ollama"])
        .output()
        .expect("missing manifest");
    assert!(!missing.status.success());
    let missing_stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(
        missing_stderr.contains("needs a manifest"),
        "missing manifest hint: {missing_stderr}"
    );
    assert!(
        missing_stderr.contains("docs/specs/synthesis/0.3/manifest/valid/claude_cli.json"),
        "missing example pointer: {missing_stderr}"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn mutation_pipeline_ergonomics_are_smooth_end_to_end() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("autophagy.db");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/findings/deterministic.jsonl");
    run_json(&database, ["import", fixture.to_str().expect("UTF-8 path")]);

    // propose (text mode) prints a next-step challenge hint to stderr.
    let proposed = command(&database)
        .args(["mutations", "propose"])
        .output()
        .expect("propose");
    assert!(proposed.status.success());
    let proposed_stderr = String::from_utf8_lossy(&proposed.stderr);
    assert!(
        proposed_stderr.contains("next: autophagy mutations challenge"),
        "missing propose hint: {proposed_stderr}"
    );

    // The same propose under JSON output keeps stdout a clean report and prints
    // no hint at all.
    let proposed_json = command(&database)
        .args(["--output", "json", "mutations", "propose"])
        .output()
        .expect("propose json");
    assert!(proposed_json.status.success());
    assert!(
        !String::from_utf8_lossy(&proposed_json.stderr).contains("next:"),
        "JSON stdout must not carry a next-step hint on stderr"
    );
    serde_json::from_slice::<Value>(&proposed_json.stdout).expect("clean JSON stdout");

    let registry = run_json(&database, ["mutations", "list"]);
    let failure_id = registry["result"]
        .as_array()
        .expect("registry")
        .iter()
        .find(|mutation| mutation["source_detector"] == "repeated_command_failure")
        .expect("failure mutation")["mutation_id"]
        .as_str()
        .expect("mutation ID")
        .to_owned();

    // challenge (text mode) prints a next-step replay-draft hint.
    let challenged = command(&database)
        .args([
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
        ])
        .output()
        .expect("challenge");
    assert!(challenged.status.success());
    assert!(
        String::from_utf8_lossy(&challenged.stderr)
            .contains("next: autophagy mutations replay-draft"),
        "missing challenge hint"
    );

    // replay accepts the canonical --suite name; on pass it hints shadow-draft.
    let passing_replay = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/replay/command-preflight-pass.json");
    let replayed = command(&database)
        .args([
            "mutations",
            "replay",
            &failure_id,
            "--suite",
            passing_replay.to_str().expect("UTF-8 path"),
        ])
        .output()
        .expect("replay --suite");
    assert!(replayed.status.success(), "replay --suite must succeed");
    assert!(
        String::from_utf8_lossy(&replayed.stderr)
            .contains("next: autophagy mutations shadow-draft"),
        "missing replay hint"
    );

    // shadow-draft exports a schema-valid, deterministic Shadow Suite draft.
    let shadow_suite = directory.path().join("shadow-suite.json");
    let drafted = run_json(
        &database,
        [
            "mutations",
            "shadow-draft",
            &failure_id,
            "--suite",
            shadow_suite.to_str().expect("UTF-8 path"),
            "--context-events",
            "1",
        ],
    );
    assert_eq!(drafted["command"], "mutations_shadow_draft");
    assert!(
        drafted["result"]["observations"]
            .as_u64()
            .expect("observation count")
            >= 1
    );
    let written_draft: Value =
        serde_json::from_slice(&fs::read(&shadow_suite).expect("read shadow draft"))
            .expect("shadow draft JSON");
    assert_eq!(written_draft, drafted["result"]["draft"]);
    assert_eq!(written_draft["spec_version"], "shadow-suite/0.1");
    assert_eq!(written_draft["mutation_id"], failure_id.as_str());
    assert!(
        written_draft["observations"]
            .as_array()
            .expect("observations")
            .iter()
            .all(|observation| observation["observation_id"]
                .as_str()
                .expect("observation id")
                .starts_with("shd_"))
    );

    // Re-drafting without --force refuses to clobber the existing file.
    let refused = command(&database)
        .args([
            "mutations",
            "shadow-draft",
            &failure_id,
            "--suite",
            shadow_suite.to_str().expect("UTF-8 path"),
        ])
        .output()
        .expect("shadow-draft no force");
    assert!(
        !refused.status.success(),
        "shadow-draft must not overwrite without --force"
    );

    // With --force it overwrites and the content is byte-for-byte deterministic.
    let redrafted = run_json(
        &database,
        [
            "mutations",
            "shadow-draft",
            &failure_id,
            "--suite",
            shadow_suite.to_str().expect("UTF-8 path"),
            "--context-events",
            "1",
            "--force",
        ],
    );
    assert_eq!(redrafted["result"]["draft"], drafted["result"]["draft"]);

    // shadow accepts the canonical --suite name; on pass it hints install.
    let passing_shadow = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/shadow/command-preflight-pass.json");
    let shadowed = command(&database)
        .args([
            "mutations",
            "shadow",
            &failure_id,
            "--suite",
            passing_shadow.to_str().expect("UTF-8 path"),
        ])
        .output()
        .expect("shadow --suite");
    assert!(shadowed.status.success(), "shadow --suite must pass");
    assert!(
        String::from_utf8_lossy(&shadowed.stderr).contains("next: autophagy mutations install"),
        "missing shadow install hint"
    );
}

/// One failing session whose `tool.input` is a command object with a long,
/// abbreviatable event id — exercises snippet cleaning and short-id rendering.
const COMMAND_JSONL: &str = concat!(
    "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_generic_1111aaaa2222bbbb\",",
    "\"session_id\":\"ses_generic_9999cccc8888dddd\",\"timestamp\":\"2026-07-16T10:00:00Z\",",
    "\"sequence\":0,\"source\":\"generic-jsonl\",\"type\":\"session.started\",",
    "\"project\":\"/workspace/demo\"}\n",
    "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_generic_3333eeee4444ffff\",",
    "\"session_id\":\"ses_generic_9999cccc8888dddd\",\"timestamp\":\"2026-07-16T10:01:00Z\",",
    "\"sequence\":1,\"source\":\"generic-jsonl\",\"type\":\"tool.failed\",",
    "\"project\":\"/workspace/demo\",\"tool\":{\"name\":\"bash\",",
    "\"input\":{\"command\":\"mise exec -- cargo test -p autophagy-store\"},\"exit_code\":1}}\n"
);

/// A single session with two distinct one-off command failures: candidate
/// signatures exist, but none recurs, so the digest must print its summary line
/// instead of listing one-occurrence giants.
const NONRECURRING_JSONL: &str = concat!(
    "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_nr_start\",",
    "\"session_id\":\"ses_nr\",\"timestamp\":\"2026-07-16T10:00:00Z\",",
    "\"sequence\":0,\"source\":\"generic-jsonl\",\"type\":\"session.started\",",
    "\"project\":\"/workspace/demo\"}\n",
    "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_nr_a\",",
    "\"session_id\":\"ses_nr\",\"timestamp\":\"2026-07-16T10:01:00Z\",",
    "\"sequence\":1,\"source\":\"generic-jsonl\",\"type\":\"tool.failed\",",
    "\"project\":\"/workspace/demo\",\"tool\":{\"name\":\"bash\",",
    "\"input\":{\"command\":\"cargo build --release\"},\"exit_code\":1}}\n",
    "{\"spec_version\":\"aep/0.1\",\"event_id\":\"evt_nr_b\",",
    "\"session_id\":\"ses_nr\",\"timestamp\":\"2026-07-16T10:02:00Z\",",
    "\"sequence\":2,\"source\":\"generic-jsonl\",\"type\":\"tool.failed\",",
    "\"project\":\"/workspace/demo\",\"tool\":{\"name\":\"bash\",",
    "\"input\":{\"command\":\"npm run lint\"},\"exit_code\":1}}\n"
);

/// Search text rows must be scannable — abbreviated event id, percentage score,
/// and a cleaned command snippet — while `--output json` keeps the full event
/// id, the raw snippet, and the basis-points ranking fields byte-stable.
#[test]
fn search_text_humanizes_rows_while_json_keeps_full_fields() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("search.db");
    let input = directory.path().join("events.jsonl");
    fs::write(&input, COMMAND_JSONL).expect("write fixture");
    run_json(
        &database,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:search",
            "--index-tool-input",
        ],
    );

    let output = command(&database)
        .args(["search", "cargo"])
        .output()
        .expect("run search");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    assert!(
        stdout.contains("evt_generic_3333eeee…"),
        "search rows must abbreviate the event id:\n{stdout}"
    );
    assert!(
        !stdout.contains("evt_generic_3333eeee4444ffff"),
        "the full event id must not appear in text rows:\n{stdout}"
    );
    assert!(
        stdout.contains("mise exec -- ") && stdout.contains("cargo"),
        "the snippet must show the cleaned command text:\n{stdout}"
    );
    assert!(
        !stdout.contains("{\"command\""),
        "the raw JSON framing must be stripped from the snippet:\n{stdout}"
    );
    assert!(
        stdout.contains('%') && !stdout.contains("bps"),
        "the rank score must read as a percentage, not basis points:\n{stdout}"
    );

    let search = run_json(&database, ["search", "cargo"]);
    let hit = &search["result"][0];
    assert_eq!(
        hit["event_id"], "evt_generic_3333eeee4444ffff",
        "JSON keeps the full event id"
    );
    assert!(
        hit["snippet"]
            .as_str()
            .expect("snippet")
            .contains("\"command\""),
        "JSON keeps the raw snippet unchanged: {hit}"
    );
    assert!(
        hit["explanation"]["rank_score_bps"].is_number(),
        "JSON keeps the spec-versioned basis-points field: {hit}"
    );
}

/// Session text rows must align without raw tabs, abbreviate the session id, and
/// print a compact timestamp; JSON keeps the full session id and RFC3339 time.
#[test]
fn sessions_text_aligns_and_compacts_while_json_stays_stable() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("sessions.db");
    let input = directory.path().join("events.jsonl");
    fs::write(&input, COMMAND_JSONL).expect("write fixture");
    run_json(
        &database,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:sessions",
        ],
    );

    let output = command(&database)
        .arg("sessions")
        .output()
        .expect("run sessions");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    assert!(
        !stdout.contains('\t'),
        "aligned columns must not use raw tabs:\n{stdout}"
    );
    assert!(
        stdout.contains("ses_generic_9999cccc…"),
        "the session id must be abbreviated:\n{stdout}"
    );
    assert!(
        !stdout.contains("2026-07-16T10:01:00Z"),
        "the timestamp must be compact (relative or short-absolute), not raw RFC3339:\n{stdout}"
    );

    let sessions = run_json(&database, ["sessions"]);
    assert_eq!(
        sessions["result"][0]["session_id"], "ses_generic_9999cccc8888dddd",
        "JSON keeps the full session id"
    );
    assert_eq!(
        sessions["result"][0]["last_event_at"], "2026-07-16T10:01:00Z",
        "JSON keeps the full RFC3339 timestamp"
    );
}

/// A zero-finding digest with no recurring candidate prints a single summary
/// line rather than a wall of one-occurrence signatures.
#[test]
fn digest_summarizes_when_no_candidate_recurs() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("digest.db");
    let input = directory.path().join("events.jsonl");
    fs::write(&input, NONRECURRING_JSONL).expect("write fixture");
    run_json(
        &database,
        [
            "import",
            input.to_str().expect("UTF-8 path"),
            "--instance-key",
            "fixture:digest",
            "--index-tool-input",
        ],
    );

    let output = command(&database)
        .arg("digest")
        .output()
        .expect("run digest");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    assert!(
        stdout.contains("none recurring — nothing near threshold"),
        "a non-recurring scan must print the summary line:\n{stdout}"
    );
    assert!(
        !stdout.contains("near-threshold observations"),
        "one-occurrence giants must not be listed:\n{stdout}"
    );
}

/// `mutations show` text must render the lesson — hypothesis statement,
/// intervention instruction, and promotion gates as percentages — not just a
/// header. JSON keeps the full package byte-stable.
#[test]
fn mutations_show_text_renders_the_lesson() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("show.db");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/findings/deterministic.jsonl");
    run_json(&database, ["import", fixture.to_str().expect("UTF-8 path")]);
    run_json(&database, ["mutations", "propose"]);

    let registry = run_json(&database, ["mutations", "list"]);
    let record = registry["result"]
        .as_array()
        .expect("registry")
        .iter()
        .find(|mutation| mutation["source_detector"] == "repeated_command_failure")
        .expect("failure mutation");
    let mutation_id = record["mutation_id"].as_str().expect("id").to_owned();

    // Pull the authoritative lesson text from the JSON package.
    let shown = run_json(&database, ["mutations", "show", &mutation_id]);
    let statement = shown["result"]["mutation"]["package"]["hypothesis"]["statement"]
        .as_str()
        .expect("statement")
        .to_owned();
    let instruction = shown["result"]["mutation"]["package"]["intervention"]["instruction"]
        .as_str()
        .expect("instruction")
        .to_owned();

    let output = command(&database)
        .args(["mutations", "show", &mutation_id])
        .output()
        .expect("run show");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    assert!(
        stdout.contains("hypothesis") && stdout.contains(&statement),
        "show must render the hypothesis statement:\n{stdout}"
    );
    assert!(
        stdout.contains("intervention") && stdout.contains(&instruction),
        "show must render the intervention instruction:\n{stdout}"
    );
    assert!(
        stdout.contains("gates") && stdout.contains('%') && !stdout.contains("bps"),
        "promotion gates must read as percentages:\n{stdout}"
    );
}

/// A unique `mut_` prefix resolves to the full id, and the next-step hint echoes
/// the short id the user typed rather than the resolved 64-hex identity.
#[test]
fn short_mutation_id_prefix_resolves_and_hint_echoes_it() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("shortid.db");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/fixtures/findings/deterministic.jsonl");
    run_json(&database, ["import", fixture.to_str().expect("UTF-8 path")]);
    run_json(&database, ["mutations", "propose"]);

    let registry = run_json(&database, ["mutations", "list"]);
    let record = registry["result"]
        .as_array()
        .expect("registry")
        .iter()
        .find(|mutation| mutation["source_detector"] == "repeated_command_failure")
        .expect("failure mutation");
    let mutation_id = record["mutation_id"].as_str().expect("id").to_owned();
    let prefix: String = mutation_id.chars().take("mut_".len() + 10).collect();
    assert_ne!(prefix, mutation_id, "prefix must be a genuine abbreviation");

    // A unique short prefix resolves to the same package the full id shows.
    let shown = run_json(&database, ["mutations", "show", &prefix]);
    assert_eq!(shown["result"]["mutation"]["mutation_id"], mutation_id);

    // The next-step hint echoes the short prefix the user supplied.
    let challenged = command(&database)
        .args([
            "mutations",
            "challenge",
            &prefix,
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
        ])
        .output()
        .expect("challenge");
    assert!(challenged.status.success());
    let stderr = String::from_utf8_lossy(&challenged.stderr);
    assert!(
        stderr.contains(&format!("replay-draft {prefix} ")),
        "the hint must echo the short id the user typed:\n{stderr}"
    );

    // An unknown prefix still surfaces the standard not-found error.
    let missing = command(&database)
        .args(["mutations", "show", "mut_ffffffffffff"])
        .output()
        .expect("show missing");
    assert!(
        !missing.status.success(),
        "an unknown id must fail, not silently resolve"
    );
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
    // Isolate configuration per test: pin the config directory to the database's
    // own temporary directory so tests never read the developer's real config.
    if let Some(parent) = database.parent() {
        command.env("AUTOPHAGY_CONFIG_DIR", parent);
    }
    command
}
