use std::fmt::Write as _;

use autophagy_events::{Event, signature::normalize_operation};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) struct FailureOperation {
    pub signature: String,
    pub success_key: String,
    pub label: String,
}

pub(crate) fn failure_operation(event: &Event, include_exit: bool) -> Option<FailureOperation> {
    let operation = normalize_operation(event)?;
    let success_key = operation.operation_key();
    let signature = if include_exit {
        operation.failure_signature(event.tool.as_ref()?.exit_code?)
    } else {
        success_key.clone()
    };
    Some(FailureOperation {
        signature,
        success_key,
        label: operation.label(),
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

fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}
