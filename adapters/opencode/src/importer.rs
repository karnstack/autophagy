use std::{fmt::Write as _, fs, path::PathBuf};

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
    discovery::{DiscoveredSession, DiscoveryPlan, OpenCodeDiscoveryError, discover},
    normalize::{NormalizeContext, SessionState, normalize_message, session_started_event},
};

/// Controls for one `OpenCode` storage import.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenCodeImportOptions {
    /// `OpenCode` `storage/` root directory.
    pub input: PathBuf,
    /// Stable identity for this `OpenCode` installation.
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

impl OpenCodeImportOptions {
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

/// One bounded, source-addressed `OpenCode` diagnostic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OpenCodeImportDiagnostic {
    /// Relative path of the offending file.
    pub file: String,
    /// Stable issue category.
    pub code: String,
    /// Human-readable detail.
    pub message: String,
}

/// Aggregate result and exact discovery plan for an `OpenCode` import.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OpenCodeImportSummary {
    /// Exact metadata-only discovery result.
    pub discovery: DiscoveryPlan,
    /// Sessions opened after discovery.
    pub sessions_read: u64,
    /// Message records presented to normalization.
    pub records_seen: u64,
    /// Part files read across all messages.
    pub parts_seen: u64,
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
    /// Structurally valid unsupported message records.
    pub unsupported: u64,
    /// Invalid records or supported-shape failures.
    pub rejected: u64,
    /// Cursors reset after unreadable state.
    pub cursor_resets: u64,
    /// Cursors saved after successful reads.
    pub cursors_advanced: u64,
    /// Retained diagnostics.
    pub diagnostics: Vec<OpenCodeImportDiagnostic>,
    /// Diagnostics omitted after the configured bound.
    pub diagnostics_suppressed: u64,
    /// Whether this invocation performed no writes.
    pub dry_run: bool,
}

impl OpenCodeImportSummary {
    /// Return whether operator attention is required.
    #[must_use]
    pub const fn has_issues(&self) -> bool {
        self.rejected > 0 || self.conflicts > 0
    }
}

/// Discover, normalize, and incrementally import `OpenCode` session storage.
///
/// # Errors
/// Returns an error for invalid options, discovery or I/O failures, missing
/// writable storage, corrupt cursor state, or unrecoverable database failures.
#[allow(clippy::too_many_lines)]
pub fn import_opencode(
    mut store: Option<&mut EventStore>,
    options: &OpenCodeImportOptions,
) -> Result<OpenCodeImportSummary, OpenCodeImportError> {
    validate_options(options)?;
    let privacy = PrivacyPolicy::new(&options.exclude_paths)?;
    if !options.dry_run && store.is_none() {
        return Err(OpenCodeImportError::MissingStore);
    }
    let discovery = discover(&options.input)?;
    let source = SourceIdentity {
        adapter: ADAPTER_NAME.to_owned(),
        instance_key: options.instance_key.clone(),
        display_name: options.display_name.clone(),
    };
    let mut summary = OpenCodeImportSummary {
        discovery: discovery.clone(),
        sessions_read: 0,
        records_seen: 0,
        parts_seen: 0,
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
        diagnostics: Vec::new(),
        diagnostics_suppressed: 0,
        dry_run: options.dry_run,
    };
    let scope = import_scope(&options.projects, &options.exclude_paths);

    for session in &discovery.sessions {
        summary.sessions_read += 1;
        let origin = format!("{scope}:{}/{}", session.project_id, session.session_id);
        let mut state = SessionState::default();
        if !options.dry_run {
            if let Some(cursor) = store
                .as_deref()
                .ok_or(OpenCodeImportError::MissingStore)?
                .get_source_cursor(&source, &origin)?
            {
                match serde_json::from_value::<SessionState>(cursor.state) {
                    Ok(resumed) => state = resumed,
                    Err(_) => summary.cursor_resets += 1,
                }
            }
        }

        import_session(
            store.as_deref_mut(),
            &source,
            &privacy,
            options,
            &discovery.root,
            session,
            &mut state,
            &mut summary,
        )?;

        if !options.dry_run {
            let cursor = SourceCursor {
                byte_offset: 0,
                line_number: state.next_sequence,
                head_hash: session_head_hash(&session.session_id),
                state: serde_json::to_value(&state)?,
            };
            store
                .as_deref()
                .ok_or(OpenCodeImportError::MissingStore)?
                .save_source_cursor(&source, &origin, &cursor)?;
            summary.cursors_advanced += 1;
        }
    }
    Ok(summary)
}

#[allow(clippy::too_many_arguments)]
fn import_session(
    mut store: Option<&mut EventStore>,
    source: &SourceIdentity,
    privacy: &PrivacyPolicy,
    options: &OpenCodeImportOptions,
    root: &std::path::Path,
    session: &DiscoveredSession,
    state: &mut SessionState,
    summary: &mut OpenCodeImportSummary,
) -> Result<(), OpenCodeImportError> {
    let context = NormalizeContext {
        session_id: &session.session_id,
        project_id: &session.project_id,
        include_content: options.include_content,
    };

    // Session start (emitted once, tracked by cursor state).
    let info_path = root.join(&session.relative_path);
    match read_json(&info_path) {
        Ok(info) => match session_started_event(&info, state, &context) {
            Ok(Some(event)) => {
                summary.events_emitted += 1;
                persist(
                    store.as_deref_mut(),
                    source,
                    privacy,
                    options,
                    &event,
                    summary,
                )?;
            }
            Ok(None) => {}
            Err(message) => reject(
                summary,
                options,
                &session.relative_path,
                "unsupported_shape",
                message,
            ),
        },
        Err(message) => reject(
            summary,
            options,
            &session.relative_path,
            "invalid_json",
            message,
        ),
    }

    // New messages, ordered by ascending identifier.
    let message_dir = root.join("message").join(&session.session_id);
    let mut message_stems = json_stems(&message_dir)?;
    message_stems.sort();
    let mut new_high_water = state.high_water.clone();
    for stem in message_stems {
        if state
            .high_water
            .as_deref()
            .is_some_and(|water| stem.as_str() <= water)
        {
            continue;
        }
        new_high_water = Some(match new_high_water {
            Some(current) if current >= stem => current,
            _ => stem.clone(),
        });
        summary.records_seen += 1;
        let message_rel = format!("message/{}/{stem}.json", session.session_id);
        let message_path = message_dir.join(format!("{stem}.json"));
        let info = match read_json(&message_path) {
            Ok(value) => value,
            Err(message) => {
                reject(summary, options, &message_rel, "invalid_json", message);
                continue;
            }
        };
        let parts = read_parts(root, &stem, summary, options)?;
        let outcome = match normalize_message(&info, &parts, state, &context) {
            Ok(value) => value,
            Err(message) => {
                reject(summary, options, &message_rel, "unsupported_shape", message);
                continue;
            }
        };
        if !outcome.supported {
            summary.unsupported += 1;
        }
        summary.events_emitted += u64::try_from(outcome.events.len())
            .map_err(|_| OpenCodeImportError::PositionOverflow)?;
        for event in &outcome.events {
            persist(
                store.as_deref_mut(),
                source,
                privacy,
                options,
                event,
                summary,
            )?;
        }
    }
    state.high_water = new_high_water;
    Ok(())
}

fn read_parts(
    root: &std::path::Path,
    message_stem: &str,
    summary: &mut OpenCodeImportSummary,
    options: &OpenCodeImportOptions,
) -> Result<Vec<Value>, OpenCodeImportError> {
    let part_dir = root.join("part").join(message_stem);
    let mut part_stems = json_stems(&part_dir)?;
    part_stems.sort();
    let mut parts = Vec::new();
    for stem in part_stems {
        summary.parts_seen += 1;
        let part_path = part_dir.join(format!("{stem}.json"));
        match read_json(&part_path) {
            Ok(value) => parts.push(value),
            Err(message) => {
                let part_rel = format!("part/{message_stem}/{stem}.json");
                reject(summary, options, &part_rel, "invalid_json", message);
            }
        }
    }
    Ok(parts)
}

fn persist(
    store: Option<&mut EventStore>,
    source: &SourceIdentity,
    privacy: &PrivacyPolicy,
    options: &OpenCodeImportOptions,
    event: &Event,
    summary: &mut OpenCodeImportSummary,
) -> Result<(), OpenCodeImportError> {
    if !project_selected(event, &options.projects) {
        summary.skipped += 1;
        return Ok(());
    }
    let outcome = privacy.apply(event);
    let Some(event) = outcome.event else {
        summary.privacy_skipped += 1;
        return Ok(());
    };
    summary.redacted_fields += outcome.redacted_fields;
    if options.dry_run {
        return Ok(());
    }
    let projection = search_projection(&event, options);
    match store
        .ok_or(OpenCodeImportError::MissingStore)?
        .insert_event(source, &event, &projection)?
    {
        InsertOutcome::Inserted { .. } => summary.inserted += 1,
        InsertOutcome::Duplicate { .. } => summary.duplicates += 1,
        InsertOutcome::ConflictQuarantined { .. } => summary.conflicts += 1,
    }
    Ok(())
}

fn json_stems(directory: &std::path::Path) -> Result<Vec<String>, OpenCodeImportError> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut stems = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("json")
        {
            if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
                stems.push(stem.to_owned());
            }
        }
    }
    Ok(stems)
}

fn read_json(path: &std::path::Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn session_head_hash(session_id: &str) -> [u8; 32] {
    Sha256::digest(session_id.as_bytes()).into()
}

fn validate_options(options: &OpenCodeImportOptions) -> Result<(), OpenCodeImportError> {
    if options.instance_key.trim().is_empty() {
        return Err(OpenCodeImportError::InvalidOptions(
            "instance_key must not be blank".to_owned(),
        ));
    }
    if options
        .display_name
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(OpenCodeImportError::InvalidOptions(
            "display_name must not be blank".to_owned(),
        ));
    }
    if options.projects.iter().any(|value| value.trim().is_empty()) {
        return Err(OpenCodeImportError::InvalidOptions(
            "project selections must not be blank".to_owned(),
        ));
    }
    if options
        .index_metadata
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err(OpenCodeImportError::InvalidOptions(
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

fn project_selected(event: &Event, projects: &[String]) -> bool {
    projects.is_empty()
        || event
            .project
            .as_ref()
            .is_some_and(|project| projects.contains(project))
}

fn search_projection(event: &Event, options: &OpenCodeImportOptions) -> SearchProjection {
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
        signature: options
            .index_tool_input
            .then(|| autophagy_events::signature::normalize_operation(event))
            .flatten()
            .map(|operation| operation.operation_key()),
    }
}

fn value_as_text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

fn reject(
    summary: &mut OpenCodeImportSummary,
    options: &OpenCodeImportOptions,
    file: &str,
    code: &str,
    message: String,
) {
    summary.rejected += 1;
    if summary.diagnostics.len() < options.max_diagnostics {
        summary.diagnostics.push(OpenCodeImportDiagnostic {
            file: file.to_owned(),
            code: code.to_owned(),
            message,
        });
    } else {
        summary.diagnostics_suppressed += 1;
    }
}

/// Fatal `OpenCode` adapter failure.
#[derive(Debug, thiserror::Error)]
pub enum OpenCodeImportError {
    /// Option validation failed.
    #[error("invalid OpenCode import option: {0}")]
    InvalidOptions(String),
    /// Metadata-only discovery failed.
    #[error(transparent)]
    Discovery(#[from] OpenCodeDiscoveryError),
    /// Storage I/O failed.
    #[error("could not read OpenCode storage: {0}")]
    Io(#[from] std::io::Error),
    /// Database operation failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Privacy policy could not be compiled.
    #[error(transparent)]
    Privacy(#[from] PrivacyError),
    /// Cursor JSON could not be encoded.
    #[error("could not serialize OpenCode cursor state: {0}")]
    Json(#[from] serde_json::Error),
    /// Non-preview imports need a store.
    #[error("a writable event store is required unless dry_run is enabled")]
    MissingStore,
    /// Source position exceeded supported integer ranges.
    #[error("OpenCode source position exceeds supported integer range")]
    PositionOverflow,
}
