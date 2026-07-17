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

Return ONLY a single JSON object with EXACTLY these keys and shapes. No prose, \
no markdown, no code fences, no extra keys:

{
  \"title\": \"string\",
  \"statement\": \"string\",
  \"expected_result\": \"string\",
  \"instruction\": \"string\",
  \"failure_cases\": [\"string\"],
  \"exclusions\": [\"string\"],
  \"supporting_event_ids\": [\"string\"],
  \"counterexample_event_ids\": [\"string\"],
  \"trigger_selectors\": [\"string\"],
  \"permissions\": {\"filesystem_read\": [], \"filesystem_write\": [], \
\"commands\": [], \"environment\": [], \"network\": false}
}

Rules you must obey:
1. `permissions` MUST be exactly the object shown above: an object with those \
five keys, every array empty and \"network\" the boolean false. It is never an \
array, never omitted, and never has any other key. This candidate is a \
review-only advisory instruction that requests no permissions.
2. Cite ONLY the event identifiers given to you, verbatim, in \
`supporting_event_ids` and `counterexample_event_ids`. Never invent, guess, or \
reformat an event id. At least two supporting events are required, and the \
counterexample events must be disjoint from the supporting events.
3. Put ONLY the trigger selectors given to you, verbatim, in \
`trigger_selectors`. Never invent a new selector.
4. `failure_cases` must list at least one concrete falsification-or-harm case. \
Every string field must be non-empty.
5. You may sharpen the title, statement, expected_result, instruction, \
failure_cases, and exclusions so they are more specific and falsifiable. Do not \
soften them into generic advice. Do not claim certainty the evidence does not \
support.
6. If the evidence does not support a concrete, honest improvement, return the \
baseline fields unchanged — still in the exact shape above.";

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
