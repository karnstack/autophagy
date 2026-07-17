use std::io::BufRead;

use autophagy_events::{Event, EventParseError};
use autophagy_redaction::{PrivacyError, PrivacyPolicy};
use autophagy_store::{EventStore, InsertOutcome, SearchProjection, SourceIdentity, StoreError};
use serde::Serialize;

/// Configuration for one generic AEP JSONL import stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportOptions {
    /// Stable identity for the file, pipe, or producer being imported.
    pub instance_key: String,
    /// Optional user-facing source label.
    pub display_name: Option<String>,
    /// Exact project paths to include. An empty list includes every project.
    pub projects: Vec<String>,
    /// Glob patterns that exclude matching project or artifact paths.
    pub exclude_paths: Vec<String>,
    /// Whether already-redacted tool input may enter FTS5.
    pub index_tool_input: bool,
    /// Explicit metadata keys whose already-redacted values may enter FTS5.
    pub index_metadata: Vec<String>,
    /// Parse and filter without opening or mutating a database.
    pub dry_run: bool,
    /// Maximum number of diagnostics retained in memory and output.
    pub max_diagnostics: usize,
}

impl ImportOptions {
    /// Create conservative import options for a stable source instance.
    #[must_use]
    pub fn new(instance_key: impl Into<String>) -> Self {
        Self {
            instance_key: instance_key.into(),
            display_name: None,
            projects: Vec::new(),
            exclude_paths: Vec::new(),
            index_tool_input: false,
            index_metadata: Vec::new(),
            dry_run: false,
            max_diagnostics: 100,
        }
    }
}

/// Stable category for a rejected JSONL record.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportDiagnosticCode {
    /// The line was not structurally valid AEP JSON.
    InvalidJson,
    /// The decoded line violated an AEP semantic invariant.
    InvalidEvent,
    /// The event was valid AEP but conflicted with store-level invariants.
    StoreRejected,
}

impl ImportDiagnosticCode {
    /// Return the stable machine-readable code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid_json",
            Self::InvalidEvent => "invalid_event",
            Self::StoreRejected => "store_rejected",
        }
    }
}

/// One bounded, line-addressed import diagnostic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportDiagnostic {
    /// One-based source line number.
    pub line: u64,
    /// Stable diagnostic category.
    pub code: ImportDiagnosticCode,
    /// Human-readable parser, validation, or storage message.
    pub message: String,
}

/// Aggregate result of streaming one JSONL source.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ImportSummary {
    /// Physical lines read, including blank lines.
    pub lines_read: u64,
    /// Nonblank records presented to the AEP parser.
    pub events_seen: u64,
    /// Selected events that passed AEP validation.
    pub validated: u64,
    /// Newly persisted canonical events.
    pub inserted: u64,
    /// Existing identical events that caused no writes.
    pub duplicates: u64,
    /// Same-ID/different-content events retained in quarantine.
    pub conflicts: u64,
    /// Valid events excluded by project selection.
    pub skipped: u64,
    /// Selected events excluded by path privacy policy.
    pub privacy_skipped: u64,
    /// String fields changed by default secret redaction.
    pub redacted_fields: u64,
    /// Invalid or store-rejected records.
    pub rejected: u64,
    /// Retained line-addressed diagnostics.
    pub diagnostics: Vec<ImportDiagnostic>,
    /// Diagnostics omitted after reaching `max_diagnostics`.
    pub diagnostics_suppressed: u64,
    /// Whether this import intentionally performed no writes.
    pub dry_run: bool,
}

impl ImportSummary {
    /// Return whether operator attention is required.
    #[must_use]
    pub const fn has_issues(&self) -> bool {
        self.rejected > 0 || self.conflicts > 0
    }
}

/// Fatal error that prevents the importer from continuing safely.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The source stream could not be read as UTF-8 text.
    #[error("could not read JSONL source: {0}")]
    Io(#[from] std::io::Error),
    /// A database or migration operation failed.
    #[error("event store failed: {0}")]
    Store(#[from] StoreError),
    /// Privacy policy could not be compiled.
    #[error(transparent)]
    Privacy(#[from] PrivacyError),
    /// Non-dry imports require a writable store.
    #[error("a writable event store is required unless dry_run is enabled")]
    MissingStore,
    /// An importer option was empty or internally inconsistent.
    #[error("invalid import option: {0}")]
    InvalidOptions(String),
}

/// Stream AEP JSONL records into the local event store.
///
/// Blank lines are ignored. AEP and record-level storage failures become
/// bounded diagnostics so later lines can still be processed. Infrastructure
/// errors abort immediately.
///
/// # Errors
///
/// Returns [`ImportError`] for unreadable input, invalid options, a missing
/// non-dry store, or a fatal database failure.
pub fn import_jsonl<R: BufRead>(
    mut reader: R,
    mut store: Option<&mut EventStore>,
    options: &ImportOptions,
) -> Result<ImportSummary, ImportError> {
    validate_options(options)?;
    let privacy = PrivacyPolicy::new(&options.exclude_paths)?;
    if !options.dry_run && store.is_none() {
        return Err(ImportError::MissingStore);
    }

    let mut summary = ImportSummary {
        dry_run: options.dry_run,
        ..ImportSummary::default()
    };
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            break;
        }
        summary.lines_read += 1;
        let record = line.trim_end_matches(['\r', '\n']);
        if record.trim().is_empty() {
            continue;
        }
        summary.events_seen += 1;

        let event = match Event::from_json_str(record) {
            Ok(event) => event,
            Err(error) => {
                let code = match &error {
                    EventParseError::Json(_) => ImportDiagnosticCode::InvalidJson,
                    EventParseError::Validation(_) => ImportDiagnosticCode::InvalidEvent,
                };
                reject(&mut summary, options, code, error.to_string());
                continue;
            }
        };

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
        summary.validated += 1;
        if options.dry_run {
            continue;
        }

        let source = SourceIdentity {
            adapter: event.source.clone(),
            instance_key: options.instance_key.clone(),
            display_name: options.display_name.clone(),
        };
        let projection = search_projection(&event, options);
        let result = store
            .as_deref_mut()
            .ok_or(ImportError::MissingStore)?
            .insert_event(&source, &event, &projection);
        match result {
            Ok(InsertOutcome::Inserted { .. }) => summary.inserted += 1,
            Ok(InsertOutcome::Duplicate { .. }) => summary.duplicates += 1,
            Ok(InsertOutcome::ConflictQuarantined { .. }) => summary.conflicts += 1,
            Err(error) if is_record_rejection(&error) => {
                reject(
                    &mut summary,
                    options,
                    ImportDiagnosticCode::StoreRejected,
                    error.to_string(),
                );
            }
            Err(error) => return Err(error.into()),
        }
    }

    Ok(summary)
}

fn validate_options(options: &ImportOptions) -> Result<(), ImportError> {
    if options.instance_key.trim().is_empty() {
        return Err(ImportError::InvalidOptions(
            "instance_key must not be empty".to_owned(),
        ));
    }
    if options
        .display_name
        .as_ref()
        .is_some_and(|name| name.trim().is_empty())
    {
        return Err(ImportError::InvalidOptions(
            "display_name must not be empty".to_owned(),
        ));
    }
    if options
        .projects
        .iter()
        .any(|project| project.trim().is_empty())
    {
        return Err(ImportError::InvalidOptions(
            "project selections must not be empty".to_owned(),
        ));
    }
    if options
        .index_metadata
        .iter()
        .any(|key| key.trim().is_empty())
    {
        return Err(ImportError::InvalidOptions(
            "metadata keys must not be empty".to_owned(),
        ));
    }
    Ok(())
}

fn project_selected(event: &Event, projects: &[String]) -> bool {
    projects.is_empty()
        || event
            .project
            .as_ref()
            .is_some_and(|project| projects.contains(project))
}

fn search_projection(event: &Event, options: &ImportOptions) -> SearchProjection {
    search_projection_from(event, options.index_tool_input, &options.index_metadata)
}

/// Derive the redaction-approved search projection for one canonical event.
///
/// Shared by streaming import and by the `reindex` rebuild so that both derive
/// exactly the same free-text and exact-signature projection from the same
/// gates: `index_tool_input` (searchable tool input plus the normalized
/// operation signature) and `index_metadata` (already-redacted metadata keys
/// promoted to searchable text). The event is assumed already redaction
/// processed by the caller.
pub(crate) fn search_projection_from(
    event: &Event,
    index_tool_input: bool,
    index_metadata: &[String],
) -> SearchProjection {
    let tool_input_text = if index_tool_input {
        event
            .tool
            .as_ref()
            .and_then(|tool| tool.input.as_ref())
            .map(value_as_text)
    } else {
        None
    };
    let searchable_text = index_metadata
        .iter()
        .filter_map(|key| event.metadata.get(key))
        .map(value_as_text)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    SearchProjection {
        tool_input_text,
        searchable_text: (!searchable_text.is_empty()).then_some(searchable_text),
        signature: signature_projection(event, index_tool_input),
    }
}

/// Derive the redaction-approved normalized signature for the retrieval index.
///
/// The command text a signature embeds originates from tool input, so a
/// signature is indexed only when the source's tool input is already
/// redaction-approved for indexing. This keeps the exact-signature index built
/// exclusively from redaction-approved projections.
fn signature_projection(event: &Event, index_tool_input: bool) -> Option<String> {
    index_tool_input
        .then(|| autophagy_events::signature::normalize_operation(event))
        .flatten()
        .map(|operation| operation.operation_key())
}

fn value_as_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        value => value.to_string(),
    }
}

fn reject(
    summary: &mut ImportSummary,
    options: &ImportOptions,
    code: ImportDiagnosticCode,
    message: String,
) {
    summary.rejected += 1;
    if summary.diagnostics.len() < options.max_diagnostics {
        summary.diagnostics.push(ImportDiagnostic {
            line: summary.lines_read,
            code,
            message,
        });
    } else {
        summary.diagnostics_suppressed += 1;
    }
}

const fn is_record_rejection(error: &StoreError) -> bool {
    matches!(
        error,
        StoreError::InvalidEvent(_)
            | StoreError::InvalidSource { .. }
            | StoreError::SourceMismatch { .. }
            | StoreError::SessionSourceConflict { .. }
            | StoreError::SessionSequenceConflict { .. }
            | StoreError::SequenceOutOfRange { .. }
            | StoreError::ArtifactOrdinalOutOfRange { .. }
    )
}
