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
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::ADAPTER_NAME;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct FileState {
    pub next_sequence: u64,
    pub started: bool,
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

pub(crate) fn normalize_record(
    record: &Value,
    state: &mut FileState,
    context: &NormalizeContext<'_>,
) -> Result<RecordOutcome, String> {
    let object = record.as_object().ok_or("record must be a JSON object")?;
    let record_type = string(object, "type").unwrap_or_default();
    let timestamp = object
        .get("timestamp")
        .and_then(Value::as_str)
        .map(|value| OffsetDateTime::parse(value, &Rfc3339))
        .transpose()
        .map_err(|error| format!("invalid timestamp: {error}"))?;
    let session = session_id(object, context.relative_path);
    let project = string(object, "cwd").map(str::to_owned);
    let record_key = string(object, "uuid")
        .or_else(|| string(object, "id"))
        .map_or_else(|| format!("line-{}", context.line), str::to_owned);
    let mut events = Vec::new();

    let potentially_supported = matches!(record_type, "user" | "assistant" | "summary")
        || (record_type == "system"
            && string(object, "subtype").is_some_and(|value| value.contains("compact")));
    if potentially_supported && !state.started {
        let timestamp = timestamp.ok_or("supported record is missing timestamp")?;
        events.push(base_event(
            state,
            &session,
            timestamp,
            project.clone(),
            EventKind::SessionStarted,
            event_id(context.relative_path, &record_key, "session-start", 0),
            provenance(object, context, None),
        ));
        state.started = true;
    }

    match record_type {
        "user" => normalize_user(
            object,
            timestamp.ok_or("user record is missing timestamp")?,
            &session,
            project,
            &record_key,
            state,
            context,
            &mut events,
        )?,
        "assistant" => normalize_assistant(
            object,
            timestamp.ok_or("assistant record is missing timestamp")?,
            &session,
            project.as_ref(),
            &record_key,
            state,
            context,
            &mut events,
        )?,
        "summary" => {
            let mut metadata = provenance(object, context, None);
            if context.include_content {
                if let Some(summary) = object.get("summary") {
                    metadata.insert("claude.content".to_owned(), summary.clone());
                }
            }
            events.push(base_event(
                state,
                &session,
                timestamp.ok_or("summary record is missing timestamp")?,
                project,
                EventKind::ContextCompacted,
                event_id(context.relative_path, &record_key, "compacted", 0),
                metadata,
            ));
        }
        "system" if string(object, "subtype").is_some_and(|value| value.contains("compact")) => {
            events.push(base_event(
                state,
                &session,
                timestamp.ok_or("compaction record is missing timestamp")?,
                project,
                EventKind::ContextCompacted,
                event_id(context.relative_path, &record_key, "compacted", 0),
                provenance(object, context, None),
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
fn normalize_user(
    object: &Map<String, Value>,
    timestamp: OffsetDateTime,
    session: &SessionId,
    project: Option<String>,
    record_key: &str,
    state: &mut FileState,
    context: &NormalizeContext<'_>,
    events: &mut Vec<Event>,
) -> Result<(), String> {
    let message_content = object
        .get("message")
        .and_then(|message| message.get("content"));
    match message_content {
        Some(Value::String(text)) => {
            let mut metadata = provenance(object, context, None);
            if context.include_content {
                metadata.insert("claude.content".to_owned(), Value::String(text.clone()));
            }
            events.push(base_event(
                state,
                session,
                timestamp,
                project,
                EventKind::PromptSubmitted,
                event_id(context.relative_path, record_key, "prompt", 0),
                metadata,
            ));
        }
        Some(Value::Array(blocks)) => {
            let text = blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                let mut metadata = provenance(object, context, None);
                if context.include_content {
                    metadata.insert("claude.content".to_owned(), Value::String(text));
                }
                events.push(base_event(
                    state,
                    session,
                    timestamp,
                    project.clone(),
                    EventKind::PromptSubmitted,
                    event_id(context.relative_path, record_key, "prompt", 0),
                    metadata,
                ));
            }
            for (index, block) in blocks.iter().enumerate().filter(|(_, block)| {
                block.get("type").and_then(Value::as_str) == Some("tool_result")
            }) {
                normalize_tool_result(
                    object,
                    block,
                    index,
                    timestamp,
                    session,
                    project.clone(),
                    record_key,
                    state,
                    context,
                    events,
                )?;
            }
        }
        _ => return Err("user record has no supported message content".to_owned()),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn normalize_assistant(
    object: &Map<String, Value>,
    timestamp: OffsetDateTime,
    session: &SessionId,
    project: Option<&String>,
    record_key: &str,
    state: &mut FileState,
    context: &NormalizeContext<'_>,
    events: &mut Vec<Event>,
) -> Result<(), String> {
    let blocks = object
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .ok_or("assistant record has no content blocks")?;
    let text = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if !text.is_empty() {
        let mut metadata = provenance(object, context, None);
        if context.include_content {
            metadata.insert("claude.content".to_owned(), Value::String(text));
        }
        events.push(base_event(
            state,
            session,
            timestamp,
            project.cloned(),
            EventKind::DecisionRecorded,
            event_id(context.relative_path, record_key, "decision", 0),
            metadata,
        ));
    }
    for (index, block) in blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| block.get("type").and_then(Value::as_str) == Some("tool_use"))
    {
        let native_id = block
            .get("id")
            .and_then(Value::as_str)
            .ok_or("tool_use block is missing id")?;
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .ok_or("tool_use block is missing name")?
            .to_owned();
        let input = block.get("input").cloned();
        let id = event_id(context.relative_path, record_key, "tool-called", index);
        let mut event = base_event(
            state,
            session,
            timestamp,
            project.cloned(),
            EventKind::ToolCalled,
            id.clone(),
            provenance(object, context, Some(index)),
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
            native_id.to_owned(),
            PendingTool {
                name,
                input,
                event_id: id.as_str().to_owned(),
            },
        );
        events.push(event);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn normalize_tool_result(
    object: &Map<String, Value>,
    block: &Value,
    index: usize,
    timestamp: OffsetDateTime,
    session: &SessionId,
    project: Option<String>,
    record_key: &str,
    state: &mut FileState,
    context: &NormalizeContext<'_>,
    events: &mut Vec<Event>,
) -> Result<(), String> {
    let native_id = block
        .get("tool_use_id")
        .and_then(Value::as_str)
        .ok_or("tool_result block is missing tool_use_id")?;
    let Some(pending) = state.pending_tools.remove(native_id) else {
        // Compacted transcripts can retain a result after its call disappeared.
        // Skipping preserves evidence integrity instead of inventing tool data.
        return Ok(());
    };
    let failed = block
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let result_content = block.get("content");
    let exit_code = if failed {
        Some(parse_exit_code(result_content).unwrap_or(1))
    } else {
        Some(0)
    };
    let kind = if failed {
        EventKind::ToolFailed
    } else {
        EventKind::ToolCompleted
    };
    let mut metadata = provenance(object, context, Some(index));
    metadata.insert(
        "claude.tool_use_id".to_owned(),
        Value::String(native_id.to_owned()),
    );
    if context.include_content {
        if let Some(content) = result_content {
            metadata.insert("claude.content".to_owned(), content.clone());
        }
    }
    let mut event = base_event(
        state,
        session,
        timestamp,
        project,
        kind,
        event_id(context.relative_path, record_key, "tool-result", index),
        metadata,
    );
    event.parent_event_id = Some(EventId::new(pending.event_id));
    event.artifacts = file_artifacts(pending.input.as_ref());
    event.tool = Some(ToolCall {
        name: pending.name,
        input: pending.input,
        exit_code,
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
    project: Option<String>,
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
        project,
        parent_event_id: None,
        tool: None,
        artifacts: Vec::new(),
        metadata,
    }
}

fn provenance(
    object: &Map<String, Value>,
    context: &NormalizeContext<'_>,
    block_index: Option<usize>,
) -> BTreeMap<String, Value> {
    let mut metadata = BTreeMap::from([
        (
            "claude.source_file".to_owned(),
            Value::String(context.relative_path.to_owned()),
        ),
        ("claude.line".to_owned(), json!(context.line)),
    ]);
    if let Some(uuid) = string(object, "uuid") {
        metadata.insert(
            "claude.record_uuid".to_owned(),
            Value::String(uuid.to_owned()),
        );
    }
    if let Some(index) = block_index {
        metadata.insert("claude.block_index".to_owned(), json!(index));
    }
    metadata
}

fn session_id(_object: &Map<String, Value>, relative_path: &str) -> SessionId {
    SessionId::new(format!("ses_claude_{}", digest(relative_path)))
}

fn event_id(relative_path: &str, record_key: &str, role: &str, index: usize) -> EventId {
    EventId::new(format!(
        "evt_claude_{}",
        digest(&format!("{relative_path}\0{record_key}\0{role}\0{index}"))
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

fn parse_exit_code(content: Option<&Value>) -> Option<i64> {
    let text = match content? {
        Value::String(value) => value.as_str(),
        Value::Array(values) => values
            .iter()
            .find_map(|value| value.get("text").and_then(Value::as_str))?,
        _ => return None,
    };
    text.lines()
        .find_map(|line| line.trim().strip_prefix("Exit code ")?.trim().parse().ok())
}

fn file_artifacts(input: Option<&Value>) -> Vec<Artifact> {
    let Some(path) = input
        .and_then(|value| value.get("file_path"))
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    vec![Artifact {
        kind: ArtifactKind::File,
        path: Some(path.to_owned()),
        uri: None,
        digest: None,
        metadata: BTreeMap::new(),
    }]
}
