use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use directories::BaseDirs;
use serde::Serialize;

/// Metadata-only description of one Codex rollout.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiscoveredRollout {
    /// Absolute rollout path.
    pub path: PathBuf,
    /// Slash-separated path relative to the discovery root.
    pub relative_path: String,
    /// File size observed during discovery.
    pub size_bytes: u64,
}

/// Exact, deterministically sorted discovery result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiscoveryPlan {
    /// Canonical input root or explicit rollout file.
    pub root: PathBuf,
    /// Selected rollout files.
    pub files: Vec<DiscoveredRollout>,
}

/// Resolve `${CODEX_HOME:-~/.codex}/sessions`.
///
/// # Errors
/// Returns an error when no home directory can be determined.
pub fn default_sessions_root() -> Result<PathBuf, CodexDiscoveryError> {
    if let Some(home) = env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home).join("sessions"));
    }
    let base = BaseDirs::new().ok_or(CodexDiscoveryError::HomeUnavailable)?;
    Ok(base.home_dir().join(".codex/sessions"))
}

/// Discover rollout JSONL files without opening their contents.
///
/// # Errors
/// Returns an error for inaccessible or unsupported inputs.
pub fn discover(input: &Path) -> Result<DiscoveryPlan, CodexDiscoveryError> {
    let root = fs::canonicalize(input)?;
    let metadata = fs::metadata(&root)?;
    let mut files = Vec::new();
    if metadata.is_file() {
        add_file(&root, &root, &mut files)?;
    } else if metadata.is_dir() {
        walk(&root, &root, &mut files)?;
    } else {
        return Err(CodexDiscoveryError::UnsupportedInput(root));
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(DiscoveryPlan { root, files })
}

fn walk(
    root: &Path,
    directory: &Path,
    files: &mut Vec<DiscoveredRollout>,
) -> Result<(), CodexDiscoveryError> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk(root, &entry.path(), files)?;
        } else if kind.is_file() {
            add_file(root, &entry.path(), files)?;
        }
    }
    Ok(())
}

fn add_file(
    root: &Path,
    path: &Path,
    files: &mut Vec<DiscoveredRollout>,
) -> Result<(), CodexDiscoveryError> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !name.starts_with("rollout-")
        || path.extension().and_then(|value| value.to_str()) != Some("jsonl")
    {
        return Ok(());
    }
    let relative = if root.is_file() {
        path.file_name().map(PathBuf::from).unwrap_or_default()
    } else {
        path.strip_prefix(root).unwrap_or(path).to_path_buf()
    };
    files.push(DiscoveredRollout {
        path: path.to_path_buf(),
        relative_path: relative.to_string_lossy().replace('\\', "/"),
        size_bytes: fs::metadata(path)?.len(),
    });
    Ok(())
}

/// Codex rollout discovery failure.
#[derive(Debug, thiserror::Error)]
pub enum CodexDiscoveryError {
    /// Filesystem operation failed.
    #[error("could not discover Codex rollouts: {0}")]
    Io(#[from] io::Error),
    /// Platform home directory is unavailable.
    #[error("could not determine the home directory for Codex sessions")]
    HomeUnavailable,
    /// Input was neither a regular file nor directory.
    #[error("Codex input is not a regular file or directory: {}", .0.display())]
    UnsupportedInput(PathBuf),
}
