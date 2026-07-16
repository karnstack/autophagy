use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
};

use autophagy_events::{
    Artifact, ArtifactKind, Event, EventId, EventKind, SessionId, SpecVersion, ToolCall,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::ADAPTER_NAME;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct FileState {
    pub next_sequence: u64,
    pub started: bool,
    pub project: Option<String>,
    pub pending_tools: HashMap<String, PendingTool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PendingTool {
    pub name: String,
    pub input: Option<Value>,
    pub event_id: String,
}

pub(crate) struct NormalizeContext<'a> {
    pub relative_path: &'a str,
    pub line: u64,
    pub include_content: bool,
}

pub(crate) struct RecordOutcome {
    pub events: Vec<Event>,
    pub supported: bool,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn normalize_record(
    record: &Value,
    state: &mut FileState,
    context: &NormalizeContext<'_>,
) -> Result<RecordOutcome, String> {
    let object = record.as_object().ok_or("record must be a JSON object")?;
    let record_type = string(object, "type").unwrap_or_default();
    let Some(payload) = object.get("payload").and_then(Value::as_object) else {
        return Ok(RecordOutcome {
            events: Vec::new(),
            supported: false,
        });
    };
    let payload_type = string(payload, "type").unwrap_or_default();
    let session = session_id(context.relative_path);
    let mut events = Vec::new();

    if record_type == "session_meta" {
        let timestamp = parse_timestamp(object, payload)?;
        state.project = string(payload, "cwd").map(str::to_owned);
        if !state.started {
            let mut metadata = provenance(object, payload, context, None);
            if let Some(native) = string(payload, "id").or_else(|| string(payload, "session_id")) {
                metadata.insert(
                    "codex.session_id".to_owned(),
                    Value::String(native.to_owned()),
                );
            }
            events.push(base_event(
                state,
                &session,
                timestamp,
                EventKind::SessionStarted,
                event_id(context.relative_path, context.line, "session-start", 0),
                metadata,
            ));
            state.started = true;
        }
        return Ok(RecordOutcome {
            events,
            supported: true,
        });
    }

    if record_type == "turn_context" {
        if let Some(cwd) = string(payload, "cwd") {
            state.project = Some(cwd.to_owned());
        }
        return Ok(RecordOutcome {
            events,
            supported: false,
        });
    }

    let supported = (record_type == "event_msg"
        && matches!(payload_type, "user_message" | "agent_message"))
        || (record_type == "response_item"
            && matches!(
                payload_type,
                "function_call"
                    | "custom_tool_call"
                    | "function_call_output"
                    | "custom_tool_call_output"
            ))
        || record_type == "compacted";
    if !supported {
        return Ok(RecordOutcome {
            events,
            supported: false,
        });
    }
    let timestamp = parse_timestamp(object, payload)?;
    if supported && !state.started {
        events.push(base_event(
            state,
            &session,
            timestamp,
            EventKind::SessionStarted,
            event_id(context.relative_path, context.line, "session-start", 0),
            provenance(object, payload, context, None),
        ));
        state.started = true;
    }

    match (record_type, payload_type) {
        ("event_msg", "user_message") => {
            let mut metadata = provenance(object, payload, context, None);
            if context.include_content {
                if let Some(message) = payload.get("message") {
                    metadata.insert("codex.content".to_owned(), message.clone());
                }
            }
            events.push(base_event(
                state,
                &session,
                timestamp,
                EventKind::PromptSubmitted,
                event_id(context.relative_path, context.line, "prompt", 0),
                metadata,
            ));
        }
        ("event_msg", "agent_message") => {
            let mut metadata = provenance(object, payload, context, None);
            if context.include_content {
                if let Some(message) = payload.get("message") {
                    metadata.insert("codex.content".to_owned(), message.clone());
                }
            }
            events.push(base_event(
                state,
                &session,
                timestamp,
                EventKind::DecisionRecorded,
                event_id(context.relative_path, context.line, "decision", 0),
                metadata,
            ));
        }
        ("response_item", "function_call" | "custom_tool_call") => normalize_tool_call(
            object,
            payload,
            timestamp,
            &session,
            state,
            context,
            &mut events,
        )?,
        ("response_item", "function_call_output" | "custom_tool_call_output") => {
            normalize_tool_output(
                object,
                payload,
                timestamp,
                &session,
                state,
                context,
                &mut events,
            )?;
        }
        ("compacted", _) => {
            let mut metadata = provenance(object, payload, context, None);
            if context.include_content {
                if let Some(message) = payload.get("message") {
                    metadata.insert("codex.content".to_owned(), message.clone());
                }
            }
            events.push(base_event(
                state,
                &session,
                timestamp,
                EventKind::ContextCompacted,
                event_id(context.relative_path, context.line, "compacted", 0),
                metadata,
            ));
        }
        _ => {
            return Ok(RecordOutcome {
                events,
                supported: false,
            });
        }
    }
    Ok(RecordOutcome {
        events,
        supported: true,
    })
}

#[allow(clippy::too_many_arguments)]
fn normalize_tool_call(
    object: &Map<String, Value>,
    payload: &Map<String, Value>,
    timestamp: OffsetDateTime,
    session: &SessionId,
    state: &mut FileState,
    context: &NormalizeContext<'_>,
    events: &mut Vec<Event>,
) -> Result<(), String> {
    let call_id = string(payload, "call_id")
        .or_else(|| string(payload, "id"))
        .ok_or("tool call is missing call_id")?;
    let name = string(payload, "name")
        .ok_or("tool call is missing name")?
        .to_owned();
    let input = payload
        .get("arguments")
        .or_else(|| payload.get("input"))
        .map(parse_embedded_json);
    let id = event_id(context.relative_path, context.line, "tool-called", 0);
    let mut event = base_event(
        state,
        session,
        timestamp,
        EventKind::ToolCalled,
        id.clone(),
        provenance(object, payload, context, None),
    );
    event.artifacts = file_artifacts(input.as_ref());
    event.tool = Some(ToolCall {
        name: name.clone(),
        input: input.clone(),
        exit_code: None,
        duration_ms: None,
        metadata: BTreeMap::new(),
    });
    state.pending_tools.insert(
        call_id.to_owned(),
        PendingTool {
            name,
            input,
            event_id: id.as_str().to_owned(),
        },
    );
    events.push(event);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn normalize_tool_output(
    object: &Map<String, Value>,
    payload: &Map<String, Value>,
    timestamp: OffsetDateTime,
    session: &SessionId,
    state: &mut FileState,
    context: &NormalizeContext<'_>,
    events: &mut Vec<Event>,
) -> Result<(), String> {
    let call_id = string(payload, "call_id").ok_or("tool output is missing call_id")?;
    let Some(pending) = state.pending_tools.remove(call_id) else {
        return Ok(());
    };
    let output = payload.get("output");
    let exit_code = output.and_then(find_exit_code);
    let failed = exit_code.is_some_and(|code| code != 0) || output.is_some_and(indicates_failure);
    let kind = if failed {
        EventKind::ToolFailed
    } else {
        EventKind::ToolCompleted
    };
    let mut metadata = provenance(object, payload, context, None);
    metadata.insert(
        "codex.call_id".to_owned(),
        Value::String(call_id.to_owned()),
    );
    if context.include_content {
        if let Some(output) = output {
            metadata.insert("codex.content".to_owned(), output.clone());
        }
    }
    let mut event = base_event(
        state,
        session,
        timestamp,
        kind,
        event_id(context.relative_path, context.line, "tool-result", 0),
        metadata,
    );
    event.parent_event_id = Some(EventId::new(pending.event_id));
    event.artifacts = file_artifacts(pending.input.as_ref());
    event.tool = Some(ToolCall {
        name: pending.name,
        input: pending.input,
        exit_code: if failed {
            Some(exit_code.unwrap_or(1))
        } else {
            exit_code.or(Some(0))
        },
        duration_ms: None,
        metadata: BTreeMap::new(),
    });
    events.push(event);
    Ok(())
}

fn base_event(
    state: &mut FileState,
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

fn provenance(
    object: &Map<String, Value>,
    payload: &Map<String, Value>,
    context: &NormalizeContext<'_>,
    item_index: Option<usize>,
) -> BTreeMap<String, Value> {
    let mut metadata = BTreeMap::from([
        (
            "codex.source_file".to_owned(),
            Value::String(context.relative_path.to_owned()),
        ),
        ("codex.line".to_owned(), json!(context.line)),
        (
            "codex.record_type".to_owned(),
            object.get("type").cloned().unwrap_or(Value::Null),
        ),
    ]);
    if let Some(kind) = payload.get("type") {
        metadata.insert("codex.payload_type".to_owned(), kind.clone());
    }
    if let Some(call_id) = string(payload, "call_id") {
        metadata.insert(
            "codex.call_id".to_owned(),
            Value::String(call_id.to_owned()),
        );
    }
    if let Some(index) = item_index {
        metadata.insert("codex.item_index".to_owned(), json!(index));
    }
    metadata
}

fn parse_timestamp(
    object: &Map<String, Value>,
    payload: &Map<String, Value>,
) -> Result<OffsetDateTime, String> {
    let raw = string(object, "timestamp")
        .or_else(|| string(payload, "timestamp"))
        .ok_or("supported record is missing timestamp")?;
    OffsetDateTime::parse(raw, &Rfc3339).map_err(|error| format!("invalid timestamp: {error}"))
}

fn session_id(relative_path: &str) -> SessionId {
    SessionId::new(format!("ses_codex_{}", digest(relative_path)))
}

fn event_id(relative_path: &str, line: u64, role: &str, index: usize) -> EventId {
    EventId::new(format!(
        "evt_codex_{}",
        digest(&format!("{relative_path}\0{line}\0{role}\0{index}"))
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

fn parse_embedded_json(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| value.clone())
}

fn output_value(value: &Value) -> Value {
    match value {
        Value::String(text) => serde_json::from_str(text).unwrap_or_else(|_| value.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        }
        _ => value.clone(),
    }
}

fn find_exit_code(value: &Value) -> Option<i64> {
    let value = output_value(value);
    find_named_i64(&value, &["exit_code", "exitCode"])
}

fn find_named_i64(value: &Value, keys: &[&str]) -> Option<i64> {
    match value {
        Value::Object(object) => keys
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_i64))
            .or_else(|| {
                object
                    .values()
                    .find_map(|value| find_named_i64(value, keys))
            }),
        Value::Array(values) => values.iter().find_map(|value| find_named_i64(value, keys)),
        _ => None,
    }
}

fn indicates_failure(value: &Value) -> bool {
    let value = output_value(value);
    match &value {
        Value::Object(object) => {
            object.get("is_error").and_then(Value::as_bool) == Some(true)
                || object.get("success").and_then(Value::as_bool) == Some(false)
                || object
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|status| matches!(status, "failed" | "error"))
                || object.values().any(indicates_failure)
        }
        Value::Array(values) => values.iter().any(indicates_failure),
        _ => false,
    }
}

fn file_artifacts(input: Option<&Value>) -> Vec<Artifact> {
    let Some(input) = input else {
        return Vec::new();
    };
    ["file_path", "path"]
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
