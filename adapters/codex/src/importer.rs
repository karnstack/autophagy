use std::{
    fmt::Write as _,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use autophagy_events::Event;
use autophagy_redaction::{PrivacyError, PrivacyPolicy};
use autophagy_store::{
    EventStore, InsertOutcome, SearchProjection, SourceCursor, SourceIdentity, StoreError,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    ADAPTER_NAME,
    discovery::{CodexDiscoveryError, DiscoveryPlan, discover},
    normalize::{FileState, NormalizeContext, normalize_record},
};

/// Controls for one Codex rollout import.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexImportOptions {
    /// Codex sessions directory, or one explicit rollout file.
    pub input: PathBuf,
    /// Stable identity for this Codex installation.
    pub instance_key: String,
    /// Optional user-facing source label.
    pub display_name: Option<String>,
    /// Exact working directories to include; empty includes all.
    pub projects: Vec<String>,
    /// Glob patterns that exclude matching project or artifact paths.
    pub exclude_paths: Vec<String>,
    /// Persist prompt, assistant, and tool-result text in metadata.
    pub include_content: bool,
    /// Add tool input to the explicit search projection.
    pub index_tool_input: bool,
    /// Metadata keys approved for search indexing.
    pub index_metadata: Vec<String>,
    /// Preview without database writes or cursor reads.
    pub dry_run: bool,
    /// Maximum retained diagnostics.
    pub max_diagnostics: usize,
}

impl CodexImportOptions {
    /// Create conservative import defaults.
    #[must_use]
    pub fn new(input: PathBuf, instance_key: impl Into<String>) -> Self {
        Self {
            input,
            instance_key: instance_key.into(),
            display_name: None,
            projects: Vec::new(),
            exclude_paths: Vec::new(),
            include_content: false,
            index_tool_input: false,
            index_metadata: Vec::new(),
            dry_run: false,
            max_diagnostics: 100,
        }
    }
}

/// One bounded, source-addressed Codex diagnostic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CodexImportDiagnostic {
    /// Relative rollout path.
    pub file: String,
    /// One-based physical line.
    pub line: u64,
    /// Stable issue category.
    pub code: String,
    /// Human-readable detail.
    pub message: String,
}

/// Aggregate result and exact discovery plan for a Codex import.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CodexImportSummary {
    /// Exact metadata-only discovery result.
    pub discovery: DiscoveryPlan,
    /// Files opened after discovery.
    pub files_read: u64,
    /// Complete physical lines read.
    pub lines_read: u64,
    /// JSON records presented to normalization.
    pub records_seen: u64,
    /// Normalized AEP events.
    pub events_emitted: u64,
    /// Newly persisted events.
    pub inserted: u64,
    /// Existing identical events.
    pub duplicates: u64,
    /// Same-ID/different-content conflicts.
    pub conflicts: u64,
    /// Events excluded by project selection.
    pub skipped: u64,
    /// Events excluded by path privacy policy.
    pub privacy_skipped: u64,
    /// String fields changed by default secret redaction.
    pub redacted_fields: u64,
    /// Structurally valid unsupported records.
    pub unsupported: u64,
    /// Invalid records or supported-shape failures.
    pub rejected: u64,
    /// Cursors reset after replacement or truncation.
    pub cursor_resets: u64,
    /// Cursors saved after successful reads.
    pub cursors_advanced: u64,
    /// Incomplete trailing records deferred.
    pub partial_tails: u64,
    /// Retained diagnostics.
    pub diagnostics: Vec<CodexImportDiagnostic>,
    /// Diagnostics omitted after the configured bound.
    pub diagnostics_suppressed: u64,
    /// Whether this invocation performed no writes.
    pub dry_run: bool,
}

impl CodexImportSummary {
    /// Return whether operator attention is required.
    #[must_use]
    pub const fn has_issues(&self) -> bool {
        self.rejected > 0 || self.conflicts > 0
    }
}

/// Discover, normalize, and incrementally import Codex rollout transcripts.
///
/// # Errors
/// Returns an error for invalid options, discovery or I/O failures, missing
/// writable storage, corrupt cursor state, or unrecoverable database failures.
#[allow(clippy::too_many_lines)]
pub fn import_codex(
    mut store: Option<&mut EventStore>,
    options: &CodexImportOptions,
) -> Result<CodexImportSummary, CodexImportError> {
    validate_options(options)?;
    let privacy = PrivacyPolicy::new(&options.exclude_paths)?;
    if !options.dry_run && store.is_none() {
        return Err(CodexImportError::MissingStore);
    }
    let discovery = discover(&options.input)?;
    let source = SourceIdentity {
        adapter: ADAPTER_NAME.to_owned(),
        instance_key: options.instance_key.clone(),
        display_name: options.display_name.clone(),
    };
    let mut summary = CodexImportSummary {
        discovery: discovery.clone(),
        files_read: 0,
        lines_read: 0,
        records_seen: 0,
        events_emitted: 0,
        inserted: 0,
        duplicates: 0,
        conflicts: 0,
        skipped: 0,
        privacy_skipped: 0,
        redacted_fields: 0,
        unsupported: 0,
        rejected: 0,
        cursor_resets: 0,
        cursors_advanced: 0,
        partial_tails: 0,
        diagnostics: Vec::new(),
        diagnostics_suppressed: 0,
        dry_run: options.dry_run,
    };
    let scope = import_scope(&options.projects, &options.exclude_paths);

    for discovered in &discovery.files {
        summary.files_read += 1;
        let origin = format!("{scope}:{}", discovered.relative_path);
        let mut state = FileState::default();
        let mut offset = 0_u64;
        let mut line_number = 0_u64;
        if !options.dry_run {
            if let Some(cursor) = store
                .as_deref()
                .ok_or(CodexImportError::MissingStore)?
                .get_source_cursor(&source, &origin)?
            {
                if cursor.byte_offset <= discovered.size_bytes
                    && prefix_hash(&discovered.path, cursor.byte_offset)? == cursor.head_hash
                {
                    offset = cursor.byte_offset;
                    line_number = cursor.line_number;
                    state = serde_json::from_value(cursor.state).map_err(|source| {
                        CodexImportError::CursorState {
                            file: discovered.relative_path.clone(),
                            source,
                        }
                    })?;
                } else {
                    summary.cursor_resets += 1;
                }
            }
        }

        let mut reader = BufReader::new(File::open(&discovered.path)?);
        reader.seek(SeekFrom::Start(offset))?;
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            let bytes = reader.read_until(b'\n', &mut buffer)?;
            if bytes == 0 {
                break;
            }
            if buffer.last() != Some(&b'\n') {
                summary.partial_tails += 1;
                break;
            }
            offset += u64::try_from(bytes).map_err(|_| CodexImportError::PositionOverflow)?;
            line_number += 1;
            summary.lines_read += 1;
            let record_bytes = buffer.strip_suffix(b"\n").unwrap_or(&buffer);
            let record_bytes = record_bytes.strip_suffix(b"\r").unwrap_or(record_bytes);
            if record_bytes.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            summary.records_seen += 1;
            let record: Value = match serde_json::from_slice(record_bytes) {
                Ok(value) => value,
                Err(error) => {
                    reject(
                        &mut summary,
                        options,
                        &discovered.relative_path,
                        line_number,
                        "invalid_json",
                        error.to_string(),
                    );
                    continue;
                }
            };
            let context = NormalizeContext {
                relative_path: &discovered.relative_path,
                line: line_number,
                include_content: options.include_content,
            };
            let outcome = match normalize_record(&record, &mut state, &context) {
                Ok(value) => value,
                Err(message) => {
                    reject(
                        &mut summary,
                        options,
                        &discovered.relative_path,
                        line_number,
                        "unsupported_shape",
                        message,
                    );
                    continue;
                }
            };
            if !outcome.supported {
                summary.unsupported += 1;
            }
            summary.events_emitted += u64::try_from(outcome.events.len())
                .map_err(|_| CodexImportError::PositionOverflow)?;
            for event in outcome.events {
                if !project_selected(&event, &options.projects) {
                    summary.skipped += 1;
                    continue;
                }
                let outcome = privacy.apply(&event);
                let Some(event) = outcome.event else {
                    summary.privacy_skipped += 1;
                    continue;
                };
                summary.redacted_fields += outcome.redacted_fields;
                if options.dry_run {
                    continue;
                }
                let projection = search_projection(&event, options);
                match store
                    .as_deref_mut()
                    .ok_or(CodexImportError::MissingStore)?
                    .insert_event(&source, &event, &projection)?
                {
                    InsertOutcome::Inserted { .. } => summary.inserted += 1,
                    InsertOutcome::Duplicate { .. } => summary.duplicates += 1,
                    InsertOutcome::ConflictQuarantined { .. } => summary.conflicts += 1,
                }
            }
        }
        if !options.dry_run {
            let cursor = SourceCursor {
                byte_offset: offset,
                line_number,
                head_hash: prefix_hash(&discovered.path, offset)?,
                state: serde_json::to_value(&state)?,
            };
            store
                .as_deref()
                .ok_or(CodexImportError::MissingStore)?
                .save_source_cursor(&source, &origin, &cursor)?;
            summary.cursors_advanced += 1;
        }
    }
    Ok(summary)
}

fn validate_options(options: &CodexImportOptions) -> Result<(), CodexImportError> {
    if options.instance_key.trim().is_empty() {
        return Err(CodexImportError::InvalidOptions(
            "instance_key must not be blank".to_owned(),
        ));
    }
    if options
        .display_name
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(CodexImportError::InvalidOptions(
            "display_name must not be blank".to_owned(),
        ));
    }
    if options.projects.iter().any(|value| value.trim().is_empty()) {
        return Err(CodexImportError::InvalidOptions(
            "project selections must not be blank".to_owned(),
        ));
    }
    if options
        .index_metadata
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err(CodexImportError::InvalidOptions(
            "metadata keys must not be blank".to_owned(),
        ));
    }
    Ok(())
}

fn import_scope(projects: &[String], exclude_paths: &[String]) -> String {
    let mut selected = projects.to_vec();
    selected.sort();
    let mut excluded = exclude_paths.to_vec();
    excluded.sort();
    let scope = format!(
        "privacy/v1\0projects\0{}\0exclusions\0{}",
        selected.join("\0"),
        excluded.join("\0")
    );
    let digest = Sha256::digest(scope.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn prefix_hash(path: &Path, consumed: u64) -> Result<[u8; 32], CodexImportError> {
    let mut file = File::open(path)?;
    let limit = consumed.min(4096);
    let mut bytes =
        vec![0; usize::try_from(limit).map_err(|_| CodexImportError::PositionOverflow)?];
    file.read_exact(&mut bytes)?;
    Ok(Sha256::digest(bytes).into())
}

fn project_selected(event: &Event, projects: &[String]) -> bool {
    projects.is_empty()
        || event
            .project
            .as_ref()
            .is_some_and(|project| projects.contains(project))
}

fn search_projection(event: &Event, options: &CodexImportOptions) -> SearchProjection {
    let tool_input_text = options
        .index_tool_input
        .then(|| event.tool.as_ref()?.input.as_ref().map(value_as_text))
        .flatten();
    let searchable_text = options
        .index_metadata
        .iter()
        .filter_map(|key| event.metadata.get(key))
        .map(value_as_text)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    SearchProjection {
        tool_input_text,
        searchable_text: (!searchable_text.is_empty()).then_some(searchable_text),
    }
}

fn value_as_text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

fn reject(
    summary: &mut CodexImportSummary,
    options: &CodexImportOptions,
    file: &str,
    line: u64,
    code: &str,
    message: String,
) {
    summary.rejected += 1;
    if summary.diagnostics.len() < options.max_diagnostics {
        summary.diagnostics.push(CodexImportDiagnostic {
            file: file.to_owned(),
            line,
            code: code.to_owned(),
            message,
        });
    } else {
        summary.diagnostics_suppressed += 1;
    }
}

/// Fatal Codex adapter failure.
#[derive(Debug, thiserror::Error)]
pub enum CodexImportError {
    /// Option validation failed.
    #[error("invalid Codex import option: {0}")]
    InvalidOptions(String),
    /// Metadata-only discovery failed.
    #[error(transparent)]
    Discovery(#[from] CodexDiscoveryError),
    /// Rollout I/O failed.
    #[error("could not read Codex rollout: {0}")]
    Io(#[from] std::io::Error),
    /// Database operation failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Privacy policy could not be compiled.
    #[error(transparent)]
    Privacy(#[from] PrivacyError),
    /// Cursor JSON could not be encoded.
    #[error("could not serialize Codex cursor state: {0}")]
    Json(#[from] serde_json::Error),
    /// Persisted cursor state is invalid.
    #[error("invalid cursor state for {file}: {source}")]
    CursorState {
        /// Relative rollout path.
        file: String,
        /// JSON shape error.
        source: serde_json::Error,
    },
    /// Non-preview imports need a store.
    #[error("a writable event store is required unless dry_run is enabled")]
    MissingStore,
    /// Source position exceeded supported integer ranges.
    #[error("Codex source position exceeds supported integer range")]
    PositionOverflow,
}
