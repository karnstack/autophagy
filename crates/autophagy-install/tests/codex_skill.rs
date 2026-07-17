//! Reversible repo-scoped skill materialization tests (Codex and Claude Code).

use std::{fs, io::Cursor};

use autophagy_core::{ImportOptions, import_jsonl};
use autophagy_install::{
    InstallError, InstallTarget, materialize, plan_claude_code_skill, plan_codex_skill,
    unmaterialize,
};
use autophagy_mutations::{
    ADVISORY_EXCLUSION, GenerationOutcome, LEGACY_ADVISORY_UNTIL_REPLAY_EXCLUSION, MutationPackage,
    generate_candidates,
};
use autophagy_patterns::{DetectorConfig, detect};
use autophagy_store::EventStore;

const CORPUS: &str = include_str!("../../../evals/fixtures/findings/deterministic.jsonl");

#[test]
fn repo_skill_is_deterministic_non_overwriting_and_reversible() {
    let repository = tempfile::tempdir().expect("repository");
    fs::create_dir(repository.path().join(".git")).expect("git marker");
    let package = command_failure_package();
    let plan = plan_codex_skill(&package, repository.path()).expect("plan");
    assert!(plan.relative_path.starts_with(".agents/skills"));
    assert!(plan.content.contains("name: autophagy-"));
    assert!(plan.content.contains(&package.intervention.instruction));
    assert_eq!(
        plan,
        plan_codex_skill(&package, repository.path()).expect("stable plan")
    );

    let artifact = materialize(&plan).expect("materialize");
    assert!(plan.absolute_path().is_file());
    assert!(matches!(
        materialize(&plan),
        Err(InstallError::TargetExists(_))
    ));
    unmaterialize(&artifact).expect("uninstall");
    assert!(!plan.absolute_path().exists());
}

#[test]
fn uninstall_refuses_content_drift() {
    let repository = tempfile::tempdir().expect("repository");
    fs::create_dir(repository.path().join(".git")).expect("git marker");
    let package = command_failure_package();
    let plan = plan_codex_skill(&package, repository.path()).expect("plan");
    let artifact = materialize(&plan).expect("materialize");
    fs::write(plan.absolute_path(), "user changed this skill").expect("drift");
    assert!(matches!(
        unmaterialize(&artifact),
        Err(InstallError::ContentDrift { .. })
    ));
}

#[cfg(unix)]
#[test]
fn materializer_refuses_symlink_escape_before_creating_external_content() {
    use std::os::unix::fs::symlink;

    let repository = tempfile::tempdir().expect("repository");
    let outside = tempfile::tempdir().expect("outside");
    fs::create_dir(repository.path().join(".git")).expect("git marker");
    symlink(outside.path(), repository.path().join(".agents")).expect("symlink");
    let package = command_failure_package();
    let plan = plan_codex_skill(&package, repository.path()).expect("plan");
    assert!(matches!(
        materialize(&plan),
        Err(InstallError::TargetEscapesRepository(_))
    ));
    assert!(!outside.path().join("skills").exists());
}

#[test]
fn claude_code_skill_targets_claude_directory_with_evidence_footer() {
    let repository = tempfile::tempdir().expect("repository");
    fs::create_dir(repository.path().join(".git")).expect("git marker");
    let package = command_failure_package();

    let plan = plan_claude_code_skill(&package, repository.path()).expect("plan");
    assert_eq!(plan.target, InstallTarget::ClaudeCode);
    assert_eq!(plan.target.registry_id(), "claude_code_repo_skill");
    assert!(plan.relative_path.starts_with(".claude/skills"));
    assert!(plan.relative_path.ends_with("SKILL.md"));
    assert!(plan.content.contains("name: autophagy-"));
    assert!(plan.content.contains(&package.intervention.instruction));
    // Evidence footer cites exact event IDs, mutation ID, and version.
    assert!(plan.content.contains("## Evidence"));
    assert!(plan.content.contains(&package.mutation_id));
    for event_id in &package.hypothesis.supporting_event_ids {
        assert!(
            plan.content.contains(event_id),
            "footer must cite supporting event {event_id}"
        );
    }
    assert_eq!(
        plan,
        plan_claude_code_skill(&package, repository.path()).expect("stable plan")
    );

    // The Claude Code plan is a distinct target from the Codex plan for the
    // same package: different directory, distinct installation identity.
    let codex = plan_codex_skill(&package, repository.path()).expect("codex plan");
    assert_ne!(plan.relative_path, codex.relative_path);
    assert_ne!(plan.installation_id, codex.installation_id);
    assert_ne!(plan.content_hash, codex.content_hash);
    assert!(!codex.content.contains("## Evidence"));

    let artifact = materialize(&plan).expect("materialize");
    assert!(plan.absolute_path().is_file());
    assert!(
        repository
            .path()
            .join(".claude/skills")
            .join(&plan.skill_name)
            .join("SKILL.md")
            .is_file()
    );
    assert!(matches!(
        materialize(&plan),
        Err(InstallError::TargetExists(_))
    ));
    unmaterialize(&artifact).expect("uninstall");
    assert!(!plan.absolute_path().exists());

    // Reversible and idempotent: the identical skill can be reinstalled after
    // a clean uninstall, reproducing the exact deterministic bytes.
    let reinstalled = materialize(&plan).expect("reinstall");
    assert_eq!(reinstalled.content_hash, plan.content_hash);
    unmaterialize(&reinstalled).expect("second uninstall");
    assert!(!plan.absolute_path().exists());
}

#[test]
fn claude_code_uninstall_refuses_content_drift() {
    let repository = tempfile::tempdir().expect("repository");
    fs::create_dir(repository.path().join(".git")).expect("git marker");
    let package = command_failure_package();
    let plan = plan_claude_code_skill(&package, repository.path()).expect("plan");
    let artifact = materialize(&plan).expect("materialize");
    fs::write(plan.absolute_path(), "user changed this skill").expect("drift");
    assert!(matches!(
        unmaterialize(&artifact),
        Err(InstallError::ContentDrift { .. })
    ));
}

#[cfg(unix)]
#[test]
fn claude_code_materializer_refuses_symlink_escape() {
    use std::os::unix::fs::symlink;

    let repository = tempfile::tempdir().expect("repository");
    let outside = tempfile::tempdir().expect("outside");
    fs::create_dir(repository.path().join(".git")).expect("git marker");
    symlink(outside.path(), repository.path().join(".claude")).expect("symlink");
    let package = command_failure_package();
    let plan = plan_claude_code_skill(&package, repository.path()).expect("plan");
    assert!(matches!(
        materialize(&plan),
        Err(InstallError::TargetEscapesRepository(_))
    ));
    assert!(!outside.path().join("skills").exists());
}

#[test]
fn install_targets_round_trip_registry_identifiers() {
    assert_eq!(
        InstallTarget::from_registry_id("codex_repo_skill"),
        Some(InstallTarget::Codex)
    );
    assert_eq!(
        InstallTarget::from_registry_id("claude_code_repo_skill"),
        Some(InstallTarget::ClaudeCode)
    );
    assert_eq!(InstallTarget::from_registry_id("vscode_repo_skill"), None);
}

#[test]
fn generated_skill_has_no_self_contradicting_exclusion() {
    let repository = tempfile::tempdir().expect("repository");
    fs::create_dir(repository.path().join(".git")).expect("git marker");
    let package = command_failure_package();

    // A freshly generated candidate already uses the corrected template
    // phrasing, so the installed SKILL.md must not carry the old claim that
    // this instruction is merely "advisory until replay and shadow evaluation
    // pass" — installation only happens after both already passed, and the
    // file says so two lines below the exclusion list.
    let plan = plan_claude_code_skill(&package, repository.path()).expect("plan");
    assert!(
        !plan
            .content
            .contains(LEGACY_ADVISORY_UNTIL_REPLAY_EXCLUSION),
        "freshly generated SKILL.md must not carry the stale pipeline-stage exclusion: {}",
        plan.content
    );
    assert!(
        plan.content.contains(ADVISORY_EXCLUSION),
        "freshly generated SKILL.md should render the corrected advisory exclusion"
    );

    let codex_plan = plan_codex_skill(&package, repository.path()).expect("codex plan");
    assert!(
        !codex_plan
            .content
            .contains(LEGACY_ADVISORY_UNTIL_REPLAY_EXCLUSION)
    );
    assert!(codex_plan.content.contains(ADVISORY_EXCLUSION));
}

#[test]
fn installer_tolerates_legacy_advisory_exclusion_phrasing() {
    let repository = tempfile::tempdir().expect("repository");
    fs::create_dir(repository.path().join(".git")).expect("git marker");

    // Registered mutation packages are immutable and audit-logged, so a
    // package minted before the template fix can still carry the old exact
    // phrasing forever. The installer must render it sanely at
    // materialization time without rewriting the stored package.
    let mut package = command_failure_package();
    let other_exclusion =
        "Do not intervene when equivalent inputs already succeeded in the current context."
            .to_owned();
    package.exclusions = vec![
        LEGACY_ADVISORY_UNTIL_REPLAY_EXCLUSION.to_owned(),
        other_exclusion.clone(),
    ];

    let plan = plan_claude_code_skill(&package, repository.path()).expect("plan");
    assert!(
        !plan
            .content
            .contains(LEGACY_ADVISORY_UNTIL_REPLAY_EXCLUSION),
        "installer must not render the stale pipeline-stage claim for legacy packages: {}",
        plan.content
    );
    assert!(
        plan.content.contains(ADVISORY_EXCLUSION),
        "installer should substitute the corrected advisory phrasing"
    );
    // Every other exclusion is kept verbatim.
    assert!(plan.content.contains(&other_exclusion));
}

fn command_failure_package() -> MutationPackage {
    let mut store = EventStore::open_in_memory().expect("store");
    import_jsonl(
        Cursor::new(CORPUS),
        Some(&mut store),
        &ImportOptions::new("fixture:install"),
    )
    .expect("import");
    let findings = detect(
        &store.list_events_for_detection(None).expect("events"),
        DetectorConfig::default(),
    );
    generate_candidates(&findings)
        .into_iter()
        .find_map(|outcome| match outcome {
            GenerationOutcome::Candidate { package }
                if package.mutation_id
                    == "mut_6b51ef819f54c0275db19b15907b0b23c39598241c912828bb64cd5bf824a0ee" =>
            {
                Some(*package)
            }
            _ => None,
        })
        .expect("command failure package")
}
