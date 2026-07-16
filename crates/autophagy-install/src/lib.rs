//! Explicit, reversible out-of-database filesystem artifacts.
//!
//! Materializers write one repo-scoped skill for a supported coding agent:
//! Codex under `.agents/skills` or Claude Code under `.claude/skills`. Every
//! target follows the same lifecycle discipline: it never overwrites an
//! existing file and uninstall refuses content drift.
//!
//! The [`supervisor`] module extends the same discipline to platform supervisor
//! units (launchd/systemd) that run `autophagy watch` in the background (ADR
//! 0008). Both are explicit, reversible, and refuse to clobber files Autophagy
//! did not author.

pub mod supervisor;

pub use supervisor::{
    MANAGED_MARKER, SupervisorConfig, SupervisorError, SupervisorKind, SupervisorPlan, WATCH_LABEL,
    is_managed, plan_supervisor, remove_supervisor, write_supervisor,
};

use std::{
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use autophagy_mutations::MutationPackage;
use sha2::{Digest, Sha256};

/// Supported repo-scoped skill installation targets.
///
/// Both variants share the same planning, materialization, and rollback logic;
/// they differ only in the repository-relative skill directory, the persisted
/// registry identifier, and the coding agent named in the rendered guidance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallTarget {
    /// Codex repo-scoped skill under `.agents/skills`.
    Codex,
    /// Claude Code repo-scoped skill under `.claude/skills`.
    ClaudeCode,
}

impl InstallTarget {
    /// Stable registry identifier persisted in the installation audit.
    #[must_use]
    pub fn registry_id(self) -> &'static str {
        match self {
            Self::Codex => "codex_repo_skill",
            Self::ClaudeCode => "claude_code_repo_skill",
        }
    }

    /// Recover a target from its persisted registry identifier.
    #[must_use]
    pub fn from_registry_id(registry_id: &str) -> Option<Self> {
        match registry_id {
            "codex_repo_skill" => Some(Self::Codex),
            "claude_code_repo_skill" => Some(Self::ClaudeCode),
            _ => None,
        }
    }

    /// Repository-relative directory components holding installed skills.
    fn skill_root(self) -> [&'static str; 2] {
        match self {
            Self::Codex => [".agents", "skills"],
            Self::ClaudeCode => [".claude", "skills"],
        }
    }

    /// Human-facing coding agent name used in rendered guidance.
    fn agent_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
        }
    }
}

/// Exact filesystem plan for one repo-scoped skill.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillPlan {
    /// Stable installation identity for this mutation and repository.
    pub installation_id: String,
    /// Installed mutation identity.
    pub mutation_id: String,
    /// Coding agent this skill targets.
    pub target: InstallTarget,
    /// Stable skill name.
    pub skill_name: String,
    /// Canonical repository root.
    pub repository_root: PathBuf,
    /// Path relative to the repository root.
    pub relative_path: PathBuf,
    /// Complete deterministic `SKILL.md` body.
    pub content: String,
    /// SHA-256 of the exact installed bytes.
    pub content_hash: String,
}

/// Backwards-compatible alias for the original Codex-only plan name.
pub type CodexSkillPlan = SkillPlan;

impl SkillPlan {
    /// Absolute installation path.
    #[must_use]
    pub fn absolute_path(&self) -> PathBuf {
        self.repository_root.join(&self.relative_path)
    }
}

/// Materialized artifact needed for audit and rollback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledArtifact {
    /// Installed mutation identity.
    pub mutation_id: String,
    /// Canonical repository root.
    pub repository_root: PathBuf,
    /// Installed path relative to the repository root.
    pub relative_path: PathBuf,
    /// SHA-256 expected during uninstall.
    pub content_hash: String,
}

/// Build a deterministic repo-scoped skill plan without writing files.
///
/// # Errors
/// Returns [`InstallError`] when the package is invalid or the target is not an
/// existing directory.
pub fn plan_skill(
    package: &MutationPackage,
    repository_root: &Path,
    target: InstallTarget,
) -> Result<SkillPlan, InstallError> {
    package
        .validate()
        .map_err(|error| InstallError::InvalidPackage(error.to_string()))?;
    if !repository_root.is_dir() || !repository_root.join(".git").exists() {
        return Err(InstallError::InvalidRepositoryRoot(
            repository_root.to_path_buf(),
        ));
    }
    let repository_root = fs::canonicalize(repository_root)?;
    let suffix = package
        .mutation_id
        .strip_prefix("mut_")
        .unwrap_or(&package.mutation_id)
        .chars()
        .take(12)
        .collect::<String>();
    let skill_name = format!("autophagy-{suffix}");
    let mut relative_path = PathBuf::new();
    for component in target.skill_root() {
        relative_path.push(component);
    }
    relative_path.push(&skill_name);
    relative_path.push("SKILL.md");
    let content = render_skill(package, &skill_name, target);
    let content_hash = sha256_hex(content.as_bytes());
    let installation_id = format!(
        "ins_{}",
        sha256_hex(
            format!(
                "install/v1\0{}\0{}\0{}",
                package.mutation_id,
                repository_root.display(),
                relative_path.display()
            )
            .as_bytes()
        )
    );
    Ok(SkillPlan {
        installation_id,
        mutation_id: package.mutation_id.clone(),
        target,
        skill_name,
        repository_root,
        relative_path,
        content,
        content_hash,
    })
}

/// Build a deterministic repo-scoped Codex skill plan without writing files.
///
/// # Errors
/// Returns [`InstallError`] when the package is invalid or the target is not an
/// existing directory.
pub fn plan_codex_skill(
    package: &MutationPackage,
    repository_root: &Path,
) -> Result<SkillPlan, InstallError> {
    plan_skill(package, repository_root, InstallTarget::Codex)
}

/// Build a deterministic repo-scoped Claude Code skill plan without writing files.
///
/// # Errors
/// Returns [`InstallError`] when the package is invalid or the target is not an
/// existing directory.
pub fn plan_claude_code_skill(
    package: &MutationPackage,
    repository_root: &Path,
) -> Result<SkillPlan, InstallError> {
    plan_skill(package, repository_root, InstallTarget::ClaudeCode)
}

/// Create exactly one planned `SKILL.md` without overwriting existing content.
///
/// # Errors
/// Returns [`InstallError`] for an existing target or filesystem failure.
pub fn materialize(plan: &SkillPlan) -> Result<InstalledArtifact, InstallError> {
    let root = fs::canonicalize(&plan.repository_root)?;
    let skill_directory = create_scoped_skill_directory(&root, plan.target, &plan.skill_name)?;
    let path = skill_directory.join("SKILL.md");
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(InstallError::TargetExists(path));
        }
        Err(error) => return Err(error.into()),
    };
    if let Err(error) = file
        .write_all(plan.content.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(error.into());
    }
    Ok(InstalledArtifact {
        mutation_id: plan.mutation_id.clone(),
        repository_root: plan.repository_root.clone(),
        relative_path: plan.relative_path.clone(),
        content_hash: plan.content_hash.clone(),
    })
}

fn create_scoped_skill_directory(
    root: &Path,
    target: InstallTarget,
    skill_name: &str,
) -> Result<PathBuf, InstallError> {
    let mut current = root.to_path_buf();
    let [first, second] = target.skill_root();
    for component in [first, second, skill_name] {
        let next = current.join(component);
        match fs::create_dir(&next) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let canonical = fs::canonicalize(&next)?;
        if !canonical.starts_with(root) {
            return Err(InstallError::TargetEscapesRepository(canonical));
        }
        current = canonical;
    }
    Ok(current)
}

/// Remove an installed skill only when its bytes still match the audit hash.
///
/// # Errors
/// Returns [`InstallError`] when the file is missing, has drifted, escapes its
/// repository root, or cannot be removed.
pub fn unmaterialize(artifact: &InstalledArtifact) -> Result<(), InstallError> {
    let root = fs::canonicalize(&artifact.repository_root)?;
    let path = root.join(&artifact.relative_path);
    let canonical_path = fs::canonicalize(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            InstallError::TargetMissing(path.clone())
        } else {
            InstallError::Io(error)
        }
    })?;
    if !canonical_path.starts_with(&root) {
        return Err(InstallError::TargetEscapesRepository(canonical_path));
    }
    let actual_hash = sha256_hex(&fs::read(&canonical_path)?);
    if actual_hash != artifact.content_hash {
        return Err(InstallError::ContentDrift {
            path: canonical_path,
            expected_hash: artifact.content_hash.clone(),
            actual_hash,
        });
    }
    fs::remove_file(&canonical_path)?;
    if let Some(skill_directory) = canonical_path.parent() {
        let _ = fs::remove_dir(skill_directory);
    }
    Ok(())
}

fn render_skill(package: &MutationPackage, skill_name: &str, target: InstallTarget) -> String {
    let title = package
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let description = yaml_double_quoted(&format!(
        "Autophagy-reviewed guardrail: {title}. Use when a matching trigger is observed."
    ));
    let mut rendered =
        format!("---\nname: {skill_name}\ndescription: {description}\n---\n\n# {title}\n\n");
    writeln!(rendered, "Mutation: `{}`", package.mutation_id).expect("String write");
    writeln!(rendered, "Version: `{}`\n", package.version).expect("String write");
    rendered.push_str("## When to use\n\nUse this instruction only when one of these exact versioned selectors matches:\n\n");
    for trigger in &package.triggers {
        writeln!(rendered, "- `{}`", trigger.selector).expect("String write");
    }
    rendered.push_str("\n## Instruction\n\n");
    rendered.push_str(&package.intervention.instruction);
    rendered.push_str("\n\n## Do not use\n\n");
    for exclusion in &package.exclusions {
        writeln!(rendered, "- {exclusion}").expect("String write");
    }
    rendered.push_str("\nThis skill was installed only after challenge, replay, shadow evaluation, and explicit user approval.\n");
    if target == InstallTarget::ClaudeCode {
        rendered.push_str(&render_evidence_footer(package, target));
    }
    rendered
}

fn render_evidence_footer(package: &MutationPackage, target: InstallTarget) -> String {
    let mut footer = String::from("\n## Evidence\n\n");
    writeln!(
        footer,
        "Installed for {} from Autophagy mutation `{}` (version `{}`), finding `{}`.",
        target.agent_name(),
        package.mutation_id,
        package.version,
        package.source_finding_id
    )
    .expect("String write");
    footer.push_str("\nSupporting events:\n\n");
    for event_id in &package.hypothesis.supporting_event_ids {
        writeln!(footer, "- `{event_id}`").expect("String write");
    }
    if !package.hypothesis.counterexample_event_ids.is_empty() {
        footer.push_str("\nCounterexample events:\n\n");
        for event_id in &package.hypothesis.counterexample_event_ids {
            writeln!(footer, "- `{event_id}`").expect("String write");
        }
    }
    footer
}

fn yaml_double_quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

/// Error produced by repo-scoped skill planning or materialization.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// The mutation package failed its semantic contract.
    #[error("mutation package is not installable: {0}")]
    InvalidPackage(String),
    /// The requested repository root is not an existing directory.
    #[error("repository root '{}' is not an existing Git repository root", .0.display())]
    InvalidRepositoryRoot(PathBuf),
    /// The target already exists and will not be overwritten.
    #[error("installation target '{}' already exists", .0.display())]
    TargetExists(PathBuf),
    /// The audited target is missing during rollback.
    #[error("installation target '{}' is missing", .0.display())]
    TargetMissing(PathBuf),
    /// Canonicalization revealed a target outside the approved repository.
    #[error("installation target '{}' escapes the repository root", .0.display())]
    TargetEscapesRepository(PathBuf),
    /// Installed bytes no longer match the audit hash.
    #[error(
        "installation target '{}' changed after install (expected {expected_hash}, got {actual_hash})",
        path.display()
    )]
    ContentDrift {
        /// Drifted path.
        path: PathBuf,
        /// Audited installation hash.
        expected_hash: String,
        /// Current file hash.
        actual_hash: String,
    },
    /// Filesystem operation failed.
    #[error("installation filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}
