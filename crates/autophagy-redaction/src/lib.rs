//! Deterministic privacy enforcement before event persistence.

use autophagy_events::Event;
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde_json::Value;

/// Replacement written in place of recognized credential material.
pub const REDACTED: &str = "[REDACTED]";

/// Compiled path and secret policy for one import invocation.
pub struct PrivacyPolicy {
    exclusions: GlobSet,
    rules: Vec<SecretRule>,
}

struct SecretRule {
    expression: Regex,
    replacement: &'static str,
}

/// Outcome of applying privacy policy to one normalized event.
#[derive(Clone, Debug, PartialEq)]
pub struct PrivacyOutcome {
    /// Sanitized event, or `None` when path policy excluded it.
    pub event: Option<Event>,
    /// Number of string fields changed by secret rules.
    pub redacted_fields: u64,
}

impl PrivacyPolicy {
    /// Compile a conservative default secret policy and caller path exclusions.
    ///
    /// # Errors
    /// Returns an error when an exclusion is not a valid glob expression.
    pub fn new(exclude_paths: &[String]) -> Result<Self, PrivacyError> {
        let mut builder = GlobSetBuilder::new();
        for pattern in exclude_paths {
            if pattern.trim().is_empty() {
                return Err(PrivacyError::BlankExclusion);
            }
            builder.add(
                Glob::new(pattern).map_err(|source| PrivacyError::InvalidGlob {
                    pattern: pattern.clone(),
                    source,
                })?,
            );
        }
        Ok(Self {
            exclusions: builder.build()?,
            rules: default_rules(),
        })
    }

    /// Exclude path-matched events and redact secrets from retained payloads.
    #[must_use]
    pub fn apply(&self, event: &Event) -> PrivacyOutcome {
        if event
            .project
            .as_deref()
            .is_some_and(|path| self.exclusions.is_match(path))
            || event.artifacts.iter().any(|artifact| {
                artifact
                    .path
                    .as_deref()
                    .is_some_and(|path| self.exclusions.is_match(path))
            })
        {
            return PrivacyOutcome {
                event: None,
                redacted_fields: 0,
            };
        }

        let mut event = event.clone();
        let mut redacted_fields = 0;
        if let Some(tool) = &mut event.tool {
            if let Some(input) = &mut tool.input {
                redact_value(input, &self.rules, &mut redacted_fields);
            }
            for value in tool.metadata.values_mut() {
                redact_value(value, &self.rules, &mut redacted_fields);
            }
        }
        for value in event.metadata.values_mut() {
            redact_value(value, &self.rules, &mut redacted_fields);
        }
        for artifact in &mut event.artifacts {
            if let Some(path) = &mut artifact.path {
                redact_string(path, &self.rules, &mut redacted_fields);
            }
            if let Some(uri) = &mut artifact.uri {
                redact_string(uri, &self.rules, &mut redacted_fields);
            }
            for value in artifact.metadata.values_mut() {
                redact_value(value, &self.rules, &mut redacted_fields);
            }
        }
        PrivacyOutcome {
            event: Some(event),
            redacted_fields,
        }
    }
}

fn default_rules() -> Vec<SecretRule> {
    [
        (r"\bsk-[A-Za-z0-9_-]{16,}\b", REDACTED),
        (r"\bgh[pousr]_[A-Za-z0-9]{20,}\b", REDACTED),
        (r"\bAKIA[A-Z0-9]{16}\b", REDACTED),
        (r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]{16,}", "Bearer [REDACTED]"),
        (
            r#"(?i)\b(api[_-]?key|access[_-]?token|password|secret)\s*[:=]\s*["']?[A-Za-z0-9._~+/=-]{8,}["']?"#,
            "$1=[REDACTED]",
        ),
    ]
    .into_iter()
    .map(|(expression, replacement)| SecretRule {
        expression: Regex::new(expression).expect("built-in secret regex must compile"),
        replacement,
    })
    .collect()
}

fn redact_value(value: &mut Value, rules: &[SecretRule], count: &mut u64) {
    match value {
        Value::String(value) => redact_string(value, rules, count),
        Value::Array(values) => {
            for value in values {
                redact_value(value, rules, count);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                redact_value(value, rules, count);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn redact_string(value: &mut String, rules: &[SecretRule], count: &mut u64) {
    let original = value.clone();
    for rule in rules {
        *value = rule
            .expression
            .replace_all(value, rule.replacement)
            .into_owned();
    }
    if *value != original {
        *count += 1;
    }
}

/// Privacy policy compilation failure.
#[derive(Debug, thiserror::Error)]
pub enum PrivacyError {
    /// Empty patterns are rejected instead of matching unpredictably.
    #[error("path exclusion must not be blank")]
    BlankExclusion,
    /// A caller-supplied glob was malformed.
    #[error("invalid path exclusion '{pattern}': {source}")]
    InvalidGlob {
        /// Rejected pattern.
        pattern: String,
        /// Glob parser failure.
        source: globset::Error,
    },
    /// Glob set construction failed.
    #[error("could not compile path exclusions: {0}")]
    GlobSet(#[from] globset::Error),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use autophagy_events::{Event, EventId, EventKind, SessionId, SpecVersion, ToolCall};
    use serde_json::json;
    use time::OffsetDateTime;

    use super::*;

    #[test]
    fn redacts_nested_secrets_without_changing_source_event() {
        let event = fixture_event("/repo/public");
        let outcome = PrivacyPolicy::new(&[]).expect("policy").apply(&event);
        let sanitized = outcome.event.expect("retained");
        assert_eq!(outcome.redacted_fields, 2);
        let encoded = serde_json::to_string(&sanitized).expect("JSON");
        assert!(!encoded.contains("sk-abcdefghijklmnop"));
        assert!(!encoded.contains("ghp_abcdefghijklmnopqrstuvwxyz"));
        assert!(encoded.contains(REDACTED));
        assert!(
            serde_json::to_string(&event)
                .expect("source JSON")
                .contains("sk-abcdefghijklmnop")
        );
    }

    #[test]
    fn excludes_projects_and_artifacts_by_glob() {
        let policy = PrivacyPolicy::new(&["**/private/**".to_owned()]).expect("policy");
        assert!(
            policy
                .apply(&fixture_event("/repo/private/client"))
                .event
                .is_none()
        );
        assert!(policy.apply(&fixture_event("/repo/public")).event.is_some());
    }

    fn fixture_event(project: &str) -> Event {
        Event {
            spec_version: SpecVersion::V0_1,
            event_id: EventId::new("evt_redaction"),
            session_id: SessionId::new("ses_redaction"),
            timestamp: OffsetDateTime::UNIX_EPOCH,
            sequence: Some(0),
            source: "fixture".to_owned(),
            kind: EventKind::ToolCalled,
            project: Some(project.to_owned()),
            parent_event_id: None,
            tool: Some(ToolCall {
                name: "shell".to_owned(),
                input: Some(json!({"command":"API_KEY=abcdefgh12345678 sk-abcdefghijklmnop"})),
                exit_code: None,
                duration_ms: None,
                metadata: BTreeMap::new(),
            }),
            artifacts: Vec::new(),
            metadata: BTreeMap::from([(
                "token".to_owned(),
                json!("ghp_abcdefghijklmnopqrstuvwxyz"),
            )]),
        }
    }
}
