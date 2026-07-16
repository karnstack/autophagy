use std::{collections::BTreeMap, fmt::Write as _};

use autophagy_events::{
    Artifact, ArtifactKind, Event, EventId, EventKind, SessionId, SpecVersion, ToolCall,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::ADAPTER_NAME;

/// Per-session normalization state persisted in the incremental cursor.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct SessionState {
    pub next_sequence: u64,
    pub started: bool,
    pub project: Option<String>,
    /// Highest message identifier already normalized (message IDs ascend).
    pub high_water: Option<String>,
}

pub(crate) struct NormalizeContext<'a> {
    pub session_id: &'a str,
    pub project_id: &'a str,
    pub include_content: bool,
}

pub(crate) struct MessageOutcome {
    pub events: Vec<Event>,
    pub supported: bool,
}

/// Emit the session-start event once, recording the working directory.
pub(crate) fn session_started_event(
    info: &Value,
    state: &mut SessionState,
    context: &NormalizeContext<'_>,
) -> Result<Option<Event>, String> {
    let object = info
        .as_object()
        .ok_or("session info must be a JSON object")?;
    if let Some(directory) = string(object, "directory") {
        state.project = Some(directory.to_owned());
    }
    if state.started {
        return Ok(None);
    }
    let timestamp = created_timestamp(object)?;
    let session = session_id(context);
    let mut metadata = base_metadata(context);
    metadata.insert(
        "opencode.session_id".to_owned(),
        Value::String(context.session_id.to_owned()),
    );
    if let Some(version) = object.get("version") {
        metadata.insert("opencode.version".to_owned(), version.clone());
    }
    let event = base_event(
        state,
        &session,
        timestamp,
        EventKind::SessionStarted,
        event_id(context.session_id, "session", "session-start", 0),
        metadata,
    );
    state.started = true;
    Ok(Some(event))
}

/// Normalize one message and its parts into ordered AEP events.
pub(crate) fn normalize_message(
    info: &Value,
    parts: &[Value],
    state: &mut SessionState,
    context: &NormalizeContext<'_>,
) -> Result<MessageOutcome, String> {
    let object = info
        .as_object()
        .ok_or("message info must be a JSON object")?;
    let message_id = string(object, "id")
        .ok_or("message is missing id")?
        .to_owned();
    let role = string(object, "role").unwrap_or_default();
    if !matches!(role, "user" | "assistant") {
        return Ok(MessageOutcome {
            events: Vec::new(),
            supported: false,
        });
    }
    let timestamp = created_timestamp(object)?;
    let session = session_id(context);
    let mut events = Vec::new();

    if role == "user" {
        let mut metadata = message_metadata(context, &message_id, role);
        if context.include_content {
            let text = collect_text(parts);
            if !text.is_empty() {
                metadata.insert("opencode.content".to_owned(), Value::String(text));
            }
        }
        events.push(base_event(
            state,
            &session,
            timestamp,
            EventKind::PromptSubmitted,
            event_id(context.session_id, &message_id, "prompt", 0),
            metadata,
        ));
        return Ok(MessageOutcome {
            events,
            supported: true,
        });
    }

    // Assistant message: an optional decision plus one call/result pair per tool part.
    let text = collect_text(parts);
    if !text.trim().is_empty() {
        let mut metadata = message_metadata(context, &message_id, role);
        if context.include_content {
            metadata.insert("opencode.content".to_owned(), Value::String(text));
        }
        events.push(base_event(
            state,
            &session,
            timestamp,
            EventKind::DecisionRecorded,
            event_id(context.session_id, &message_id, "decision", 0),
            metadata,
        ));
    }

    for (index, part) in parts.iter().enumerate() {
        let Some(part) = part.as_object() else {
            continue;
        };
        if string(part, "type") != Some("tool") {
            continue;
        }
        normalize_tool_part(
            part,
            index,
            timestamp,
            &session,
            &message_id,
            state,
            context,
            &mut events,
        );
    }

    Ok(MessageOutcome {
        events,
        supported: true,
    })
}

#[allow(clippy::too_many_arguments)]
fn normalize_tool_part(
    part: &Map<String, Value>,
    index: usize,
    timestamp: OffsetDateTime,
    session: &SessionId,
    message_id: &str,
    state: &mut SessionState,
    context: &NormalizeContext<'_>,
    events: &mut Vec<Event>,
) {
    let tool = string(part, "tool").unwrap_or("tool").to_owned();
    let call_id = string(part, "callID").map(str::to_owned);
    let state_object = part.get("state").and_then(Value::as_object);
    let status = state_object
        .and_then(|value| string(value, "status"))
        .unwrap_or("pending");
    let input = state_object.and_then(|value| value.get("input")).cloned();

    let called_id = event_id(context.session_id, message_id, "tool-called", index);
    let mut called_metadata = message_metadata(context, message_id, "assistant");
    called_metadata.insert("opencode.tool".to_owned(), Value::String(tool.clone()));
    if let Some(call_id) = &call_id {
        called_metadata.insert(
            "opencode.call_id".to_owned(),
            Value::String(call_id.clone()),
        );
    }
    let mut called = base_event(
        state,
        session,
        timestamp,
        EventKind::ToolCalled,
        called_id.clone(),
        called_metadata,
    );
    called.artifacts = file_artifacts(input.as_ref());
    called.tool = Some(ToolCall {
        name: tool.clone(),
        input: input.clone(),
        exit_code: None,
        duration_ms: None,
        metadata: BTreeMap::new(),
    });
    events.push(called);

    let (kind, exit_code) = match status {
        "completed" => (EventKind::ToolCompleted, 0),
        "error" => (EventKind::ToolFailed, 1),
        // Pending or running tool calls have no terminal outcome to normalize.
        _ => return,
    };
    let mut result_metadata = message_metadata(context, message_id, "assistant");
    result_metadata.insert("opencode.tool".to_owned(), Value::String(tool.clone()));
    if let Some(call_id) = &call_id {
        result_metadata.insert(
            "opencode.call_id".to_owned(),
            Value::String(call_id.clone()),
        );
    }
    if context.include_content {
        if let Some(state_object) = state_object {
            if let Some(output) = state_object.get("output") {
                result_metadata.insert("opencode.content".to_owned(), output.clone());
            }
            if let Some(error) = state_object.get("error") {
                result_metadata.insert("opencode.error".to_owned(), error.clone());
            }
        }
    }
    let mut result = base_event(
        state,
        session,
        timestamp,
        kind,
        event_id(context.session_id, message_id, "tool-result", index),
        result_metadata,
    );
    result.parent_event_id = Some(EventId::new(called_id.as_str().to_owned()));
    result.artifacts = file_artifacts(input.as_ref());
    result.tool = Some(ToolCall {
        name: tool,
        input,
        exit_code: Some(exit_code),
        duration_ms: None,
        metadata: BTreeMap::new(),
    });
    events.push(result);
}

fn base_event(
    state: &mut SessionState,
    session_id: &SessionId,
    timestamp: OffsetDateTime,
    kind: EventKind,
    event_id: EventId,
    metadata: BTreeMap<String, Value>,
) -> Event {
    let sequence = state.next_sequence;
    state.next_sequence += 1;
    Event {
        spec_version: SpecVersion::V0_1,
        event_id,
        session_id: session_id.clone(),
        timestamp,
        sequence: Some(sequence),
        source: ADAPTER_NAME.to_owned(),
        kind,
        project: state.project.clone(),
        parent_event_id: None,
        tool: None,
        artifacts: Vec::new(),
        metadata,
    }
}

fn base_metadata(context: &NormalizeContext<'_>) -> BTreeMap<String, Value> {
    BTreeMap::from([(
        "opencode.project_id".to_owned(),
        Value::String(context.project_id.to_owned()),
    )])
}

fn message_metadata(
    context: &NormalizeContext<'_>,
    message_id: &str,
    role: &str,
) -> BTreeMap<String, Value> {
    let mut metadata = base_metadata(context);
    metadata.insert(
        "opencode.message_id".to_owned(),
        Value::String(message_id.to_owned()),
    );
    metadata.insert("opencode.role".to_owned(), Value::String(role.to_owned()));
    metadata
}

fn created_timestamp(object: &Map<String, Value>) -> Result<OffsetDateTime, String> {
    let created = object
        .get("time")
        .and_then(Value::as_object)
        .and_then(|time| time.get("created"))
        .and_then(Value::as_i64)
        .ok_or("record is missing time.created")?;
    let nanos = i128::from(created)
        .checked_mul(1_000_000)
        .ok_or("time.created out of range")?;
    OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .map_err(|error| format!("invalid time.created: {error}"))
}

fn collect_text(parts: &[Value]) -> String {
    parts
        .iter()
        .filter_map(Value::as_object)
        .filter(|part| string(part, "type") == Some("text"))
        .filter_map(|part| string(part, "text"))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn session_id(context: &NormalizeContext<'_>) -> SessionId {
    SessionId::new(format!(
        "ses_opencode_{}",
        digest(&format!("{}\0{}", context.project_id, context.session_id))
    ))
}

fn event_id(session_id: &str, message_id: &str, role: &str, index: usize) -> EventId {
    EventId::new(format!(
        "evt_opencode_{}",
        digest(&format!("{session_id}\0{message_id}\0{role}\0{index}"))
    ))
}

fn digest(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn string<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn file_artifacts(input: Option<&Value>) -> Vec<Artifact> {
    let Some(input) = input else {
        return Vec::new();
    };
    ["filePath", "file_path", "path"]
        .iter()
        .filter_map(|key| input.get(*key).and_then(Value::as_str))
        .map(|path| Artifact {
            kind: ArtifactKind::File,
            path: Some(path.to_owned()),
            uri: None,
            digest: None,
            metadata: BTreeMap::new(),
        })
        .collect::<Vec<_>>()
}
