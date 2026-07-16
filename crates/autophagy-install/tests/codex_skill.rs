//! Reversible repo-scoped Codex skill materialization tests.

use std::{fs, io::Cursor};

use autophagy_core::{ImportOptions, import_jsonl};
use autophagy_install::{InstallError, materialize, plan_codex_skill, unmaterialize};
use autophagy_mutations::{GenerationOutcome, MutationPackage, generate_candidates};
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
                    == "mut_d6b7a340eb2fb6f18bee4a20932b9c954adb4975f3ea8136bf0bd264b3ec431c" =>
            {
                Some(*package)
            }
            _ => None,
        })
        .expect("command failure package")
}
