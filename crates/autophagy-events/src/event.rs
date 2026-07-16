use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::{EventId, EventParseError, SessionId, ValidationError, ValidationErrors};

/// Protocol version supported by this crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SpecVersion {
    /// Agent Event Protocol version 0.1.
    #[serde(rename = "aep/0.1")]
    V0_1,
}

impl SpecVersion {
    /// Return the stable wire-format value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V0_1 => "aep/0.1",
        }
    }
}

/// Normalized kind of agent activity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum EventKind {
    /// A source session began.
    #[serde(rename = "session.started")]
    SessionStarted,
    /// A source session ended.
    #[serde(rename = "session.ended")]
    SessionEnded,
    /// A user submitted a prompt.
    #[serde(rename = "prompt.submitted")]
    PromptSubmitted,
    /// An agent decision was recorded.
    #[serde(rename = "decision.recorded")]
    DecisionRecorded,
    /// A tool invocation began.
    #[serde(rename = "tool.called")]
    ToolCalled,
    /// A tool invocation completed successfully.
    #[serde(rename = "tool.completed")]
    ToolCompleted,
    /// A tool invocation failed.
    #[serde(rename = "tool.failed")]
    ToolFailed,
    /// A file was read.
    #[serde(rename = "file.read")]
    FileRead,
    /// A file was changed.
    #[serde(rename = "file.changed")]
    FileChanged,
    /// A test failed.
    #[serde(rename = "test.failed")]
    TestFailed,
    /// A test passed.
    #[serde(rename = "test.passed")]
    TestPassed,
    /// A user corrected agent behavior.
    #[serde(rename = "user.corrected_agent")]
    UserCorrectedAgent,
    /// A user rejected a proposed action.
    #[serde(rename = "user.rejected_action")]
    UserRejectedAction,
    /// Agent context was compacted.
    #[serde(rename = "context.compacted")]
    ContextCompacted,
}

impl EventKind {
    /// Return the stable wire-format value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionStarted => "session.started",
            Self::SessionEnded => "session.ended",
            Self::PromptSubmitted => "prompt.submitted",
            Self::DecisionRecorded => "decision.recorded",
            Self::ToolCalled => "tool.called",
            Self::ToolCompleted => "tool.completed",
            Self::ToolFailed => "tool.failed",
            Self::FileRead => "file.read",
            Self::FileChanged => "file.changed",
            Self::TestFailed => "test.failed",
            Self::TestPassed => "test.passed",
            Self::UserCorrectedAgent => "user.corrected_agent",
            Self::UserRejectedAction => "user.rejected_action",
            Self::ContextCompacted => "context.compacted",
        }
    }

    const fn requires_tool(self) -> bool {
        matches!(
            self,
            Self::ToolCalled | Self::ToolCompleted | Self::ToolFailed
        )
    }
}

/// A tool invocation normalized from an agent-specific representation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    /// Stable tool name, such as `bash` or `read_file`.
    pub name: String,
    /// Tool input in its most faithful JSON representation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    /// Process-style result code, when meaningful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// Observed wall-clock duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Explicit extension point for source-specific values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

/// Normalized category of an artifact referenced by an event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ArtifactKind {
    /// A filesystem path.
    #[serde(rename = "file")]
    File,
    /// A Git commit.
    #[serde(rename = "git.commit")]
    GitCommit,
    /// A Git diff or patch.
    #[serde(rename = "git.diff")]
    GitDiff,
    /// A test or test result.
    #[serde(rename = "test")]
    Test,
    /// Captured or summarized command output.
    #[serde(rename = "command.output")]
    CommandOutput,
    /// An artifact not represented by another v0.1 kind.
    #[serde(rename = "other")]
    Other,
}

impl ArtifactKind {
    /// Return the stable wire-format value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::GitCommit => "git.commit",
            Self::GitDiff => "git.diff",
            Self::Test => "test",
            Self::CommandOutput => "command.output",
            Self::Other => "other",
        }
    }
}

/// A file, commit, test, or other durable object referenced by an event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    /// Artifact category serialized as `type`.
    #[serde(rename = "type")]
    pub kind: ArtifactKind,
    /// Local or repository-relative path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// URI for a non-path locator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Content digest, including its algorithm prefix when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Explicit extension point for source-specific values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

/// One normalized Agent Event Protocol event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    /// Serialized protocol version.
    pub spec_version: SpecVersion,
    /// Stable, opaque event identifier.
    pub event_id: EventId,
    /// Stable, opaque source-session identifier.
    pub session_id: SessionId,
    /// Time at which the source activity occurred.
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    /// Stable event ordering within a session, when supplied by the adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    /// Adapter name such as `claude-code`, `codex`, or `generic-jsonl`.
    pub source: String,
    /// Normalized activity kind serialized as `type`.
    #[serde(rename = "type")]
    pub kind: EventKind,
    /// Project path after configured path-policy handling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Causal or lifecycle parent event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<EventId>,
    /// Tool details for tool lifecycle events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolCall>,
    /// Durable objects referenced by the event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    /// Explicit extension point for source-specific values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

impl Event {
    /// Parse JSON and enforce all v0.1 semantic invariants.
    ///
    /// # Errors
    ///
    /// Returns [`EventParseError`] when JSON decoding or semantic validation
    /// fails.
    pub fn from_json_str(input: &str) -> Result<Self, EventParseError> {
        let event: Self = serde_json::from_str(input)?;
        event.validate()?;
        Ok(event)
    }

    /// Parse UTF-8 JSON bytes and enforce all v0.1 semantic invariants.
    ///
    /// # Errors
    ///
    /// Returns [`EventParseError`] when JSON decoding or semantic validation
    /// fails.
    pub fn from_json_slice(input: &[u8]) -> Result<Self, EventParseError> {
        let event: Self = serde_json::from_slice(input)?;
        event.validate()?;
        Ok(event)
    }

    /// Validate constraints that cannot be expressed by Rust's type system.
    ///
    /// # Errors
    ///
    /// Returns every semantic failure found in the event.
    pub fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = Vec::new();

        validate_id(self.event_id.as_str(), "evt_", "event_id", &mut errors);
        validate_id(self.session_id.as_str(), "ses_", "session_id", &mut errors);

        if let Some(parent) = &self.parent_event_id {
            validate_id(parent.as_str(), "evt_", "parent_event_id", &mut errors);
            if parent == &self.event_id {
                errors.push(ValidationError::new(
                    "parent_event_id",
                    "self_parent",
                    "parent event must differ from event_id",
                ));
            }
        }

        validate_nonempty(&self.source, 128, "source", &mut errors);
        if let Some(project) = &self.project {
            validate_nonempty(project, usize::MAX, "project", &mut errors);
        }

        match (&self.kind, &self.tool) {
            (kind, None) if kind.requires_tool() => errors.push(ValidationError::new(
                "tool",
                "required",
                "tool lifecycle events require tool details",
            )),
            (EventKind::ToolCalled, Some(tool)) if tool.exit_code.is_some() => {
                errors.push(ValidationError::new(
                    "tool.exit_code",
                    "not_allowed",
                    "tool.called cannot contain an exit code",
                ));
            }
            (EventKind::ToolCompleted, Some(tool))
                if tool.exit_code.is_some_and(|code| code != 0) =>
            {
                errors.push(ValidationError::new(
                    "tool.exit_code",
                    "expected_zero",
                    "tool.completed exit code must be zero",
                ));
            }
            (EventKind::ToolFailed, Some(tool)) if tool.exit_code.is_none() => {
                errors.push(ValidationError::new(
                    "tool.exit_code",
                    "required",
                    "tool.failed requires an exit code",
                ));
            }
            (EventKind::ToolFailed, Some(tool)) if tool.exit_code == Some(0) => {
                errors.push(ValidationError::new(
                    "tool.exit_code",
                    "expected_nonzero",
                    "tool.failed exit code must be non-zero",
                ));
            }
            _ => {}
        }

        if let Some(tool) = &self.tool {
            validate_nonempty(&tool.name, 128, "tool.name", &mut errors);
        }

        if self.artifacts.len() > 256 {
            errors.push(ValidationError::new(
                "artifacts",
                "too_many",
                "an event can reference at most 256 artifacts",
            ));
        }
        for (index, artifact) in self.artifacts.iter().enumerate() {
            validate_artifact(artifact, index, &mut errors);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationErrors::new(errors))
        }
    }
}

fn validate_artifact(artifact: &Artifact, index: usize, errors: &mut Vec<ValidationError>) {
    let base = format!("artifacts[{index}]");
    let locators = [
        ("path", artifact.path.as_deref()),
        ("uri", artifact.uri.as_deref()),
        ("digest", artifact.digest.as_deref()),
    ];
    if locators.iter().all(|(_, value)| value.is_none()) {
        errors.push(ValidationError::new(
            &base,
            "missing_locator",
            "artifact requires a path, uri, or digest",
        ));
    }
    for (field, value) in locators {
        if let Some(value) = value {
            validate_nonempty(value, usize::MAX, &format!("{base}.{field}"), errors);
        }
    }
}

fn validate_id(value: &str, prefix: &str, path: &str, errors: &mut Vec<ValidationError>) {
    let suffix = value.strip_prefix(prefix);
    let valid = suffix.is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.len() <= 127
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    });
    if !valid {
        errors.push(ValidationError::new(
            path,
            "invalid_id",
            format!(
                "must start with {prefix} and contain only A-Z, a-z, 0-9, '.', '_', ':', or '-'"
            ),
        ));
    }
}

fn validate_nonempty(value: &str, max_len: usize, path: &str, errors: &mut Vec<ValidationError>) {
    if value.trim().is_empty() {
        errors.push(ValidationError::new(
            path,
            "empty",
            "must not be empty or whitespace",
        ));
    } else if value.chars().count() > max_len {
        errors.push(ValidationError::new(
            path,
            "too_long",
            format!("must contain at most {max_len} characters"),
        ));
    }
}
