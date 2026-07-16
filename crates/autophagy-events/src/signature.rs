//! Deterministic, model-free normalization of tool operations into stable
//! signatures.
//!
//! A normalized signature collapses incidental variation (tool aliases,
//! whitespace, and the concrete project prefix) so that the same underlying
//! operation produces the same string across sessions. The normalization is a
//! pure function of an [`Event`]; it never consults a model, the filesystem, or
//! the network. Both the deterministic pattern detectors and the retrieval
//! signature index build on this single implementation so their identities stay
//! byte-for-byte consistent.

use serde_json::Value;

use crate::Event;

/// A normalized tool operation: its canonical tool name and command text.
///
/// Construct one with [`normalize_operation`]. The stable string projections
/// ([`operation_key`](OperationSignature::operation_key) and
/// [`failure_signature`](OperationSignature::failure_signature)) are versioned
/// so a future normalization change can introduce a new prefix without
/// silently reinterpreting stored signatures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationSignature {
    tool: String,
    command: String,
}

impl OperationSignature {
    /// Canonical tool name after alias normalization (for example `shell`).
    #[must_use]
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// Normalized command text with the project prefix replaced by `$PROJECT`.
    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }

    /// Outcome-independent operation identity: `operation/v1|<tool>|<command>`.
    #[must_use]
    pub fn operation_key(&self) -> String {
        format!("operation/v1|{}|{}", self.tool, self.command)
    }

    /// Failure identity including an exit code:
    /// `failure/v1|<tool>|<command>|exit:<code>`.
    #[must_use]
    pub fn failure_signature(&self, exit_code: i64) -> String {
        format!("failure/v1|{}|{}|exit:{exit_code}", self.tool, self.command)
    }

    /// Deterministic human-readable label: `<tool>: <command>`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}: {}", self.tool, self.command)
    }
}

/// Normalize the tool operation an event describes, if one is inspectable.
///
/// Returns `None` when the event carries no tool call, no inspectable command
/// string, or a command that normalizes to the empty string. The result is a
/// pure function of the event's tool name, tool input, and project path.
#[must_use]
pub fn normalize_operation(event: &Event) -> Option<OperationSignature> {
    let tool = event.tool.as_ref()?;
    let name = normalize_tool(&tool.name);
    let command = command(tool.input.as_ref()?)?;
    let command = normalize_command(&command, event.project.as_deref());
    if command.is_empty() {
        return None;
    }
    Some(OperationSignature {
        tool: name,
        command,
    })
}

fn normalize_tool(tool: &str) -> String {
    match tool.trim().to_ascii_lowercase().as_str() {
        "bash" | "exec" | "exec_command" | "shell" | "terminal" => "shell".to_owned(),
        other => other.to_owned(),
    }
}

fn command(input: &Value) -> Option<String> {
    match input {
        Value::String(value) => Some(value.clone()),
        Value::Object(object) => object
            .get("command")
            .or_else(|| object.get("cmd"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

fn normalize_command(command: &str, project: Option<&str>) -> String {
    let command = project.map_or_else(
        || command.to_owned(),
        |project| command.replace(project, "$PROJECT"),
    );
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use time::OffsetDateTime;

    use super::normalize_operation;
    use crate::{Event, EventId, EventKind, SessionId, SpecVersion, ToolCall};

    fn tool_event(input: serde_json::Value, project: Option<&str>) -> Event {
        Event {
            spec_version: SpecVersion::V0_1,
            event_id: EventId::new("evt_signature"),
            session_id: SessionId::new("ses_signature"),
            timestamp: OffsetDateTime::UNIX_EPOCH,
            sequence: Some(0),
            source: "codex".to_owned(),
            kind: EventKind::ToolFailed,
            project: project.map(str::to_owned),
            parent_event_id: None,
            tool: Some(ToolCall {
                name: "Bash".to_owned(),
                input: Some(input),
                exit_code: Some(2),
                duration_ms: None,
                metadata: BTreeMap::new(),
            }),
            artifacts: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn normalizes_tool_alias_whitespace_and_project_prefix() {
        let event = tool_event(
            json!("cargo   test  /workspace/project/crate"),
            Some("/workspace/project"),
        );
        let operation = normalize_operation(&event).expect("operation");
        assert_eq!(operation.tool(), "shell");
        assert_eq!(operation.command(), "cargo test $PROJECT/crate");
        assert_eq!(
            operation.operation_key(),
            "operation/v1|shell|cargo test $PROJECT/crate"
        );
        assert_eq!(
            operation.failure_signature(2),
            "failure/v1|shell|cargo test $PROJECT/crate|exit:2"
        );
    }

    #[test]
    fn reads_structured_command_input() {
        let event = tool_event(json!({"command": "pytest -q"}), None);
        let operation = normalize_operation(&event).expect("operation");
        assert_eq!(operation.operation_key(), "operation/v1|shell|pytest -q");
    }

    #[test]
    fn rejects_uninspectable_or_empty_commands() {
        assert!(normalize_operation(&tool_event(json!({"other": 1}), None)).is_none());
        assert!(normalize_operation(&tool_event(json!("   "), None)).is_none());
    }
}
