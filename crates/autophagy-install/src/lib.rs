//! Explicit, reversible mutation installation targets.
//!
//! The initial materializer writes one repo-scoped Codex skill under
//! `.agents/skills`. It never overwrites an existing file and uninstall refuses
//! content drift.

use std::{
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use autophagy_mutations::MutationPackage;
use sha2::{Digest, Sha256};

/// Exact filesystem plan for one repo-scoped Codex skill.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexSkillPlan {
    /// Stable installation identity for this mutation and repository.
    pub installation_id: String,
    /// Installed mutation identity.
    pub mutation_id: String,
    /// Stable Codex skill name.
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

impl CodexSkillPlan {
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

/// Build a deterministic repo-scoped Codex skill plan without writing files.
///
/// # Errors
/// Returns [`InstallError`] when the package is invalid or the target is not an
/// existing directory.
pub fn plan_codex_skill(
    package: &MutationPackage,
    repository_root: &Path,
) -> Result<CodexSkillPlan, InstallError> {
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
    let relative_path = PathBuf::from(".agents")
        .join("skills")
        .join(&skill_name)
        .join("SKILL.md");
    let content = render_skill(package, &skill_name);
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
    Ok(CodexSkillPlan {
        installation_id,
        mutation_id: package.mutation_id.clone(),
        skill_name,
        repository_root,
        relative_path,
        content,
        content_hash,
    })
}

/// Create exactly one planned `SKILL.md` without overwriting existing content.
///
/// # Errors
/// Returns [`InstallError`] for an existing target or filesystem failure.
pub fn materialize(plan: &CodexSkillPlan) -> Result<InstalledArtifact, InstallError> {
    let root = fs::canonicalize(&plan.repository_root)?;
    let skill_directory = create_scoped_skill_directory(&root, &plan.skill_name)?;
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

fn create_scoped_skill_directory(root: &Path, skill_name: &str) -> Result<PathBuf, InstallError> {
    let mut current = root.to_path_buf();
    for component in [".agents", "skills", skill_name] {
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

fn render_skill(package: &MutationPackage, skill_name: &str) -> String {
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
    rendered
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

/// Error produced by Codex skill planning or materialization.
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
