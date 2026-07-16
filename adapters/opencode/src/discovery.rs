use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use directories::BaseDirs;
use serde::Serialize;

/// Metadata-only description of one `OpenCode` session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiscoveredSession {
    /// Native `OpenCode` session identifier (the info filename stem).
    pub session_id: String,
    /// Native `OpenCode` project identifier (the info file's parent directory).
    pub project_id: String,
    /// Slash-separated info-file path relative to the discovery root.
    pub relative_path: String,
    /// Number of message files observed for this session.
    pub message_count: u64,
    /// Session info file size observed during discovery.
    pub size_bytes: u64,
}

/// Exact, deterministically sorted discovery result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiscoveryPlan {
    /// Canonical storage root.
    pub root: PathBuf,
    /// Selected sessions.
    pub sessions: Vec<DiscoveredSession>,
}

/// Resolve `${XDG_DATA_HOME:-~/.local/share}/opencode/storage`.
///
/// # Errors
/// Returns an error when no home directory can be determined.
pub fn default_storage_root() -> Result<PathBuf, OpenCodeDiscoveryError> {
    if let Some(data) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(data).join("opencode/storage"));
    }
    let base = BaseDirs::new().ok_or(OpenCodeDiscoveryError::HomeUnavailable)?;
    Ok(base.home_dir().join(".local/share/opencode/storage"))
}

/// Discover `OpenCode` sessions under a `storage/` root without reading contents.
///
/// Session info files live at `session/<projectID>/<sessionID>.json`.
///
/// # Errors
/// Returns an error for an inaccessible or unsupported input.
pub fn discover(root: &Path) -> Result<DiscoveryPlan, OpenCodeDiscoveryError> {
    let root = fs::canonicalize(root)?;
    let metadata = fs::metadata(&root)?;
    if !metadata.is_dir() {
        return Err(OpenCodeDiscoveryError::UnsupportedInput(root));
    }
    let session_root = root.join("session");
    let mut sessions = Vec::new();
    if session_root.is_dir() {
        for project_entry in sorted_dir(&session_root)? {
            if !project_entry.file_type()?.is_dir() {
                continue;
            }
            let project_dir = project_entry.path();
            let project_id = file_name(&project_dir);
            for info_entry in sorted_dir(&project_dir)? {
                let info_path = info_entry.path();
                if !info_entry.file_type()?.is_file()
                    || info_path.extension().and_then(|value| value.to_str()) != Some("json")
                {
                    continue;
                }
                let session_id = file_stem(&info_path);
                let message_count = count_messages(&root, &session_id)?;
                sessions.push(DiscoveredSession {
                    session_id,
                    project_id: project_id.clone(),
                    relative_path: relative(&root, &info_path),
                    message_count,
                    size_bytes: fs::metadata(&info_path)?.len(),
                });
            }
        }
    }
    sessions.sort_by(|left, right| {
        left.project_id
            .cmp(&right.project_id)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    Ok(DiscoveryPlan { root, sessions })
}

fn count_messages(root: &Path, session_id: &str) -> Result<u64, OpenCodeDiscoveryError> {
    let message_dir = root.join("message").join(session_id);
    if !message_dir.is_dir() {
        return Ok(0);
    }
    let mut count = 0_u64;
    for entry in fs::read_dir(&message_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry.path().extension().and_then(|value| value.to_str()) == Some("json")
        {
            count += 1;
        }
    }
    Ok(count)
}

fn sorted_dir(directory: &Path) -> Result<Vec<fs::DirEntry>, OpenCodeDiscoveryError> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_owned()
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_owned()
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// `OpenCode` discovery failure.
#[derive(Debug, thiserror::Error)]
pub enum OpenCodeDiscoveryError {
    /// Filesystem operation failed.
    #[error("could not discover OpenCode sessions: {0}")]
    Io(#[from] io::Error),
    /// Platform home directory is unavailable.
    #[error("could not determine the home directory for OpenCode storage")]
    HomeUnavailable,
    /// Input was not a storage directory.
    #[error("OpenCode input is not a storage directory: {}", .0.display())]
    UnsupportedInput(PathBuf),
}
