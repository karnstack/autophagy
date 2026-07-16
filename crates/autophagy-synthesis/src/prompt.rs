//! Deterministic prompt construction for model-backed providers.
//!
//! The prompt is built only from the structured [`SynthesisRequest`] — the
//! deterministic template's fields, the allowed evidence identifiers, and the
//! hard constraints. It never includes raw session transcripts, raw event
//! payloads, secrets, or anything beyond those template-derived fields. The
//! text is fully determined by the request, so a fixed request always produces
//! a byte-identical prompt (asserted in tests). This keeps exactly what leaves
//! the process auditable and reviewable in one place.

use serde_json::{Value, json};

use crate::provider::SynthesisRequest;

/// Upper bound on tokens a provider may generate for one candidate.
///
/// The structured response is small; this cap keeps a misbehaving or verbose
/// model from producing an unbounded (and costly) completion. It is passed to
/// the runtime as `num_predict` (Ollama) or `max_tokens` (OpenAI-compatible).
pub const MAX_COMPLETION_TOKENS: u32 = 1024;

/// The system prompt. It states the boundary rules the model must obey. Nothing
/// here is model-, provider-, or request-specific, so it is a single reviewable
/// constant.
pub const SYSTEM_PROMPT: &str = "\
You improve a single mutation candidate for a local coding agent. You are a \
proposer, not an authority: everything you return is re-validated \
deterministically and discarded if it breaks a rule.

Rules you must obey:
1. Return ONLY a single JSON object matching the provided schema. No prose, no \
markdown, no code fences.
2. Cite ONLY the event identifiers given to you. Never invent, guess, or \
reformat an event id. At least two supporting events are required.
3. Use ONLY the trigger selectors given to you. Never invent a new selector.
4. Request NO permissions. Every permission array must be empty and network \
must be false. This candidate is a review-only advisory instruction.
5. Keep the counterexample events disjoint from the supporting events.
6. You may sharpen the title, statement, expected result, instruction, failure \
cases, and exclusions so they are more specific and falsifiable. Do not soften \
them into generic advice. Do not claim certainty the evidence does not support.
7. If the evidence does not support a concrete, honest improvement, return the \
baseline fields unchanged.";

/// Build the user prompt from the structured request. Deterministic for a fixed
/// request.
#[must_use]
pub fn user_prompt(request: &SynthesisRequest) -> String {
    let constraints = &request.constraints;
    let baseline = &request.baseline;
    let mut prompt = String::new();
    let section = |prompt: &mut String, heading: &str| {
        prompt.push('\n');
        prompt.push_str(heading);
        prompt.push('\n');
    };

    prompt.push_str("Finding under review:\n");
    push_field(&mut prompt, "finding_id", &request.finding_id);
    push_field(&mut prompt, "detector", request.detector.as_str());
    push_field(&mut prompt, "signature", &request.signature);

    section(&mut prompt, "Allowed evidence (cite only these):");
    push_list(
        &mut prompt,
        "supporting_event_ids",
        &constraints.allowed_supporting_event_ids,
    );
    push_list(
        &mut prompt,
        "counterexample_event_ids",
        &constraints.allowed_counterexample_event_ids,
    );

    section(&mut prompt, "Allowed trigger selectors (use only these):");
    push_list(
        &mut prompt,
        "trigger_selectors",
        &constraints.allowed_trigger_selectors,
    );

    section(&mut prompt, "Baseline candidate (enrich or keep):");
    push_field(&mut prompt, "title", &baseline.title);
    push_field(&mut prompt, "statement", &baseline.statement);
    push_field(&mut prompt, "expected_result", &baseline.expected_result);
    push_field(&mut prompt, "instruction", &baseline.instruction);
    push_list(&mut prompt, "failure_cases", &baseline.failure_cases);
    push_list(&mut prompt, "exclusions", &baseline.exclusions);
    push_list(
        &mut prompt,
        "supporting_event_ids",
        &baseline.supporting_event_ids,
    );
    push_list(
        &mut prompt,
        "counterexample_event_ids",
        &baseline.counterexample_event_ids,
    );

    section(
        &mut prompt,
        "Return the improved candidate as a single JSON object. Permissions must \
be empty and network false.",
    );
    prompt
}

fn push_field(prompt: &mut String, key: &str, value: &str) {
    prompt.push_str("- ");
    prompt.push_str(key);
    prompt.push_str(": ");
    prompt.push_str(value);
    prompt.push('\n');
}

fn push_list(prompt: &mut String, key: &str, values: &[String]) {
    prompt.push_str("- ");
    prompt.push_str(key);
    prompt.push(':');
    if values.is_empty() {
        prompt.push_str(" (none)\n");
        return;
    }
    prompt.push('\n');
    for value in values {
        prompt.push_str("  - ");
        prompt.push_str(value);
        prompt.push('\n');
    }
}

/// The JSON Schema handed to the runtime's structured-output feature so the
/// model must emit the synthesis response shape. It mirrors
/// `docs/specs/synthesis/0.1/response.schema.json` inline (no `$ref`) because
/// local runtimes vary in reference support. The boundary re-validates every
/// field regardless, so this schema is a coercion hint, not the trust anchor.
#[must_use]
pub fn response_json_schema() -> Value {
    let string = json!({ "type": "string" });
    let string_array = json!({ "type": "array", "items": { "type": "string" } });
    let empty_array = json!({ "type": "array", "items": { "type": "string" }, "maxItems": 0 });
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "title", "statement", "expected_result", "instruction",
            "failure_cases", "exclusions", "supporting_event_ids",
            "counterexample_event_ids", "trigger_selectors", "permissions"
        ],
        "properties": {
            "title": string,
            "statement": string,
            "expected_result": string,
            "instruction": string,
            "failure_cases": string_array,
            "exclusions": string_array,
            "supporting_event_ids": string_array,
            "counterexample_event_ids": string_array,
            "trigger_selectors": string_array,
            "permissions": {
                "type": "object",
                "additionalProperties": false,
                "required": [
                    "filesystem_read", "filesystem_write", "commands",
                    "environment", "network"
                ],
                "properties": {
                    "filesystem_read": empty_array,
                    "filesystem_write": empty_array,
                    "commands": empty_array,
                    "environment": empty_array,
                    "network": { "type": "boolean" }
                }
            }
        }
    })
}
