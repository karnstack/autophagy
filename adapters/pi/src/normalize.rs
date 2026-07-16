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

fn unsupported() -> RecordOutcome {
    RecordOutcome {
        events: Vec::new(),
        supported: false,
    }
}

pub(crate) fn normalize_record(
    record: &Value,
    state: &mut FileState,
    context: &NormalizeContext<'_>,
) -> Result<RecordOutcome, String> {
    let object = record.as_object().ok_or("record must be a JSON object")?;
    let record_type = string(object, "type").unwrap_or_default();
    let session = session_id(context.relative_path);
    let mut events = Vec::new();

    match record_type {
        "session" => {
            let timestamp = parse_timestamp(object)?;
            state.project = string(object, "cwd").map(str::to_owned);
            if !state.started {
                let mut metadata = provenance(object, context);
                if let Some(native) = string(object, "id") {
                    metadata.insert("pi.session_id".to_owned(), Value::String(native.to_owned()));
                }
                if let Some(version) = object.get("version") {
                    metadata.insert("pi.version".to_owned(), version.clone());
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
            Ok(RecordOutcome {
                events,
                supported: true,
            })
        }
        "message" => normalize_message(object, state, context, &session),
        // Everything else is structurally valid but carries no
        // AEP-normalizable activity and is intentionally skipped. This includes
        // `model_change`, `thinking_level_change`, and application-specific
        // `custom_message` records.
        _ => Ok(unsupported()),
    }
}

fn normalize_message(
    object: &Map<String, Value>,
    state: &mut FileState,
    context: &NormalizeContext<'_>,
    session: &SessionId,
) -> Result<RecordOutcome, String> {
    let Some(message) = object.get("message").and_then(Value::as_object) else {
        return Ok(unsupported());
    };
    let role = string(message, "role").unwrap_or_default();
    if !matches!(role, "user" | "assistant" | "toolResult" | "bashExecution") {
        return Ok(unsupported());
    }
    let timestamp = parse_timestamp(object)?;
    let mut events = Vec::new();
    ensure_started(state, session, timestamp, context, &mut events);

    match role {
        "user" => {
            let mut metadata = provenance(object, context);
            metadata.insert("pi.role".to_owned(), Value::String("user".to_owned()));
            if context.include_content {
                if let Some(content) = message.get("content") {
                    metadata.insert("pi.content".to_owned(), content.clone());
                }
            }
            events.push(base_event(
                state,
                session,
                timestamp,
                EventKind::PromptSubmitted,
                event_id(context.relative_path, context.line, "prompt", 0),
                metadata,
            ));
        }
        "assistant" => normalize_assistant(
            message,
            object,
            timestamp,
            session,
            state,
            context,
            &mut events,
        ),
        "toolResult" => {
            normalize_tool_result(
                message,
                object,
                timestamp,
                session,
                state,
                context,
                &mut events,
            );
        }
        "bashExecution" => {
            normalize_bash_execution(
                message,
                object,
                timestamp,
                session,
                state,
                context,
                &mut events,
            );
        }
        _ => unreachable!("role guarded above"),
    }

    Ok(RecordOutcome {
        events,
        supported: true,
    })
}

#[allow(clippy::too_many_arguments)]
fn normalize_assistant(
    message: &Map<String, Value>,
    object: &Map<String, Value>,
    timestamp: OffsetDateTime,
    session: &SessionId,
    state: &mut FileState,
    context: &NormalizeContext<'_>,
    events: &mut Vec<Event>,
) {
    let blocks = message.get("content").and_then(Value::as_array);
    let text = blocks
        .map(|blocks| collect_text(blocks))
        .unwrap_or_default();
    if !text.trim().is_empty() {
        let mut metadata = provenance(object, context);
        metadata.insert("pi.role".to_owned(), Value::String("assistant".to_owned()));
        if context.include_content {
            if let Some(content) = message.get("content") {
                metadata.insert("pi.content".to_owned(), content.clone());
            }
        }
        events.push(base_event(
            state,
            session,
            timestamp,
            EventKind::DecisionRecorded,
            event_id(context.relative_path, context.line, "decision", 0),
            metadata,
        ));
    }
    let Some(blocks) = blocks else {
        return;
    };
    for (index, block) in blocks.iter().enumerate() {
        let Some(block) = block.as_object() else {
            continue;
        };
        if string(block, "type") != Some("toolCall") {
            continue;
        }
        let Some(call_id) = string(block, "id") else {
            continue;
        };
        let Some(name) = string(block, "name") else {
            continue;
        };
        let name = name.to_owned();
        let input = block.get("arguments").cloned();
        let id = event_id(context.relative_path, context.line, "tool-called", index);
        let mut metadata = provenance(object, context);
        metadata.insert(
            "pi.tool_call_id".to_owned(),
            Value::String(call_id.to_owned()),
        );
        let mut event = base_event(
            state,
            session,
            timestamp,
            EventKind::ToolCalled,
            id.clone(),
            metadata,
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
    }
}

#[allow(clippy::too_many_arguments)]
fn normalize_tool_result(
    message: &Map<String, Value>,
    object: &Map<String, Value>,
    timestamp: OffsetDateTime,
    session: &SessionId,
    state: &mut FileState,
    context: &NormalizeContext<'_>,
    events: &mut Vec<Event>,
) {
    let Some(call_id) = string(message, "toolCallId") else {
        return;
    };
    let Some(pending) = state.pending_tools.remove(call_id) else {
        // No matching call: never invent a tool identity or relationship.
        return;
    };
    let failed = message.get("isError").and_then(Value::as_bool) == Some(true);
    let kind = if failed {
        EventKind::ToolFailed
    } else {
        EventKind::ToolCompleted
    };
    let mut metadata = provenance(object, context);
    metadata.insert("pi.role".to_owned(), Value::String("toolResult".to_owned()));
    metadata.insert(
        "pi.tool_call_id".to_owned(),
        Value::String(call_id.to_owned()),
    );
    if context.include_content {
        if let Some(content) = message.get("content") {
            metadata.insert("pi.content".to_owned(), content.clone());
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
        exit_code: Some(i64::from(failed)),
        duration_ms: None,
        metadata: BTreeMap::new(),
    });
    events.push(event);
}

#[allow(clippy::too_many_arguments)]
fn normalize_bash_execution(
    message: &Map<String, Value>,
    object: &Map<String, Value>,
    timestamp: OffsetDateTime,
    session: &SessionId,
    state: &mut FileState,
    context: &NormalizeContext<'_>,
    events: &mut Vec<Event>,
) {
    let command = string(message, "command").unwrap_or_default().to_owned();
    let input = json!({ "command": command });
    let called_id = event_id(context.relative_path, context.line, "tool-called", 0);
    let mut called_metadata = provenance(object, context);
    called_metadata.insert(
        "pi.role".to_owned(),
        Value::String("bashExecution".to_owned()),
    );
    let mut called = base_event(
        state,
        session,
        timestamp,
        EventKind::ToolCalled,
        called_id.clone(),
        called_metadata,
    );
    called.tool = Some(ToolCall {
        name: "bash".to_owned(),
        input: Some(input.clone()),
        exit_code: None,
        duration_ms: None,
        metadata: BTreeMap::new(),
    });
    events.push(called);

    let cancelled = message.get("cancelled").and_then(Value::as_bool) == Some(true);
    let exit_code = message.get("exitCode").and_then(Value::as_i64);
    let failed = cancelled || exit_code.is_some_and(|code| code != 0);
    let kind = if failed {
        EventKind::ToolFailed
    } else {
        EventKind::ToolCompleted
    };
    let mut result_metadata = provenance(object, context);
    result_metadata.insert(
        "pi.role".to_owned(),
        Value::String("bashExecution".to_owned()),
    );
    if cancelled {
        result_metadata.insert("pi.cancelled".to_owned(), Value::Bool(true));
    }
    if context.include_content {
        if let Some(output) = message.get("output") {
            result_metadata.insert("pi.content".to_owned(), output.clone());
        }
    }
    let mut result = base_event(
        state,
        session,
        timestamp,
        kind,
        event_id(context.relative_path, context.line, "tool-result", 0),
        result_metadata,
    );
    result.parent_event_id = Some(EventId::new(called_id.as_str().to_owned()));
    result.tool = Some(ToolCall {
        name: "bash".to_owned(),
        input: Some(input),
        exit_code: if failed {
            Some(exit_code.filter(|code| *code != 0).unwrap_or(1))
        } else {
            exit_code.or(Some(0))
        },
        duration_ms: None,
        metadata: BTreeMap::new(),
    });
    events.push(result);
}

fn ensure_started(
    state: &mut FileState,
    session: &SessionId,
    timestamp: OffsetDateTime,
    context: &NormalizeContext<'_>,
    events: &mut Vec<Event>,
) {
    if state.started {
        return;
    }
    events.push(base_event(
        state,
        session,
        timestamp,
        EventKind::SessionStarted,
        event_id(context.relative_path, context.line, "session-start", 0),
        provenance_bare(context),
    ));
    state.started = true;
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
    context: &NormalizeContext<'_>,
) -> BTreeMap<String, Value> {
    let mut metadata = provenance_bare(context);
    metadata.insert(
        "pi.record_type".to_owned(),
        object.get("type").cloned().unwrap_or(Value::Null),
    );
    if let Some(id) = string(object, "id") {
        metadata.insert("pi.record_id".to_owned(), Value::String(id.to_owned()));
    }
    metadata
}

fn provenance_bare(context: &NormalizeContext<'_>) -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "pi.source_file".to_owned(),
            Value::String(context.relative_path.to_owned()),
        ),
        ("pi.line".to_owned(), json!(context.line)),
    ])
}

fn parse_timestamp(object: &Map<String, Value>) -> Result<OffsetDateTime, String> {
    let raw = string(object, "timestamp").ok_or("record is missing timestamp")?;
    OffsetDateTime::parse(raw, &Rfc3339).map_err(|error| format!("invalid timestamp: {error}"))
}

fn collect_text(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter_map(Value::as_object)
        .filter(|block| string(block, "type") == Some("text"))
        .filter_map(|block| string(block, "text"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn session_id(relative_path: &str) -> SessionId {
    SessionId::new(format!("ses_pi_{}", digest(relative_path)))
}

fn event_id(relative_path: &str, line: u64, role: &str, index: usize) -> EventId {
    EventId::new(format!(
        "evt_pi_{}",
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
