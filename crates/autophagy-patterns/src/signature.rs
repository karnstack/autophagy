use std::fmt::Write as _;

use autophagy_events::Event;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) struct FailureOperation {
    pub signature: String,
    pub success_key: String,
    pub label: String,
}

pub(crate) fn failure_operation(event: &Event, include_exit: bool) -> Option<FailureOperation> {
    let tool = event.tool.as_ref()?;
    let tool_name = normalize_tool(&tool.name);
    let command = command(tool.input.as_ref()?)?;
    let command = normalize_command(&command, event.project.as_deref());
    if command.is_empty() {
        return None;
    }
    let success_key = format!("operation/v1|{tool_name}|{command}");
    let signature = if include_exit {
        format!("failure/v1|{tool_name}|{command}|exit:{}", tool.exit_code?)
    } else {
        success_key.clone()
    };
    Some(FailureOperation {
        signature,
        success_key,
        label: format!("{tool_name}: {command}"),
    })
}

pub(crate) fn correction_signature(event: &Event) -> Option<String> {
    [
        "autophagy.signature",
        "correction_signature",
        "correction_key",
    ]
    .iter()
    .find_map(|key| event.metadata.get(*key).and_then(Value::as_str))
    .map(normalize_label)
    .filter(|value| !value.is_empty())
}

pub(crate) fn correction_counterexample(event: &Event) -> Option<String> {
    let outcome = event
        .metadata
        .get("autophagy.outcome")
        .or_else(|| event.metadata.get("correction_outcome"))
        .and_then(Value::as_str)?;
    matches!(outcome, "followed" | "accepted" | "complied")
        .then(|| correction_signature(event))
        .flatten()
}

pub(crate) fn finding_id(detector: &str, signature: &str) -> String {
    let digest = Sha256::digest(format!("{detector}\0{signature}").as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("fnd_{encoded}")
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

fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}
