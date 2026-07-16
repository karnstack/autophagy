use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use directories::BaseDirs;
use serde::Serialize;

/// Whether a transcript belongs to a primary Claude Code session or subagent.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// Top-level session transcript.
    Main,
    /// Nested `agent-*.jsonl` subagent transcript.
    Subagent,
}

/// Metadata-only description of one transcript selected for import.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiscoveredSession {
    /// Absolute source path.
    pub path: PathBuf,
    /// Stable slash-separated path relative to the discovery root.
    pub relative_path: String,
    /// File size observed during discovery.
    pub size_bytes: u64,
    /// Transcript category.
    pub kind: SessionKind,
}

/// Discovery controls. Discovery never opens transcript contents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryOptions {
    /// Claude Code projects directory, or one explicit JSONL file.
    pub input: PathBuf,
    /// Include nested subagent transcripts.
    pub include_subagents: bool,
}

/// Exact set of source files an import will consider.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiscoveryPlan {
    /// Canonical input root or explicit file.
    pub root: PathBuf,
    /// Sorted, deterministic transcript list.
    pub files: Vec<DiscoveredSession>,
}

/// Resolve `${CLAUDE_CONFIG_DIR:-~/.claude}/projects`.
///
/// # Errors
/// Returns an error if no home directory can be determined.
pub fn default_projects_root() -> Result<PathBuf, DiscoveryError> {
    if let Some(config) = env::var_os("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(config).join("projects"));
    }
    let base = BaseDirs::new().ok_or(DiscoveryError::HomeUnavailable)?;
    Ok(base.home_dir().join(".claude/projects"))
}

/// Discover selected Claude Code transcript files without reading their contents.
///
/// # Errors
/// Returns an error for an inaccessible input or filesystem traversal failure.
pub fn discover(options: &DiscoveryOptions) -> Result<DiscoveryPlan, DiscoveryError> {
    let root = fs::canonicalize(&options.input)?;
    let metadata = fs::metadata(&root)?;
    let mut files = Vec::new();
    if metadata.is_file() {
        add_file(&root, &root, options.include_subagents, &mut files)?;
    } else if metadata.is_dir() {
        walk(&root, &root, options.include_subagents, &mut files)?;
    } else {
        return Err(DiscoveryError::UnsupportedInput(root));
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(DiscoveryPlan { root, files })
}

fn walk(
    root: &Path,
    directory: &Path,
    include_subagents: bool,
    files: &mut Vec<DiscoveredSession>,
) -> Result<(), DiscoveryError> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            walk(root, &entry.path(), include_subagents, files)?;
        } else if file_type.is_file() {
            add_file(root, &entry.path(), include_subagents, files)?;
        }
    }
    Ok(())
}

fn add_file(
    root: &Path,
    path: &Path,
    include_subagents: bool,
    files: &mut Vec<DiscoveredSession>,
) -> Result<(), DiscoveryError> {
    if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
        return Ok(());
    }
    let is_subagent = path
        .file_stem()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.starts_with("agent-"));
    if is_subagent && !include_subagents {
        return Ok(());
    }
    let relative = if root.is_file() {
        path.file_name().map(PathBuf::from).unwrap_or_default()
    } else {
        path.strip_prefix(root).unwrap_or(path).to_path_buf()
    };
    files.push(DiscoveredSession {
        path: path.to_path_buf(),
        relative_path: relative.to_string_lossy().replace('\\', "/"),
        size_bytes: fs::metadata(path)?.len(),
        kind: if is_subagent {
            SessionKind::Subagent
        } else {
            SessionKind::Main
        },
    });
    Ok(())
}

/// Failure while resolving or enumerating Claude Code history.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// Filesystem operation failed.
    #[error("could not discover Claude Code transcripts: {0}")]
    Io(#[from] io::Error),
    /// Platform home directory was unavailable.
    #[error("could not determine the home directory for Claude Code history")]
    HomeUnavailable,
    /// Input was neither a regular file nor directory.
    #[error("Claude Code input is not a regular file or directory: {}", .0.display())]
    UnsupportedInput(PathBuf),
}
