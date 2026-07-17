//! Deterministic, model-free normalization of tool operations into stable
//! signatures.
//!
//! A normalized signature collapses incidental variation so that the same
//! underlying operation produces the same string across sessions. Two kinds of
//! variation are removed:
//!
//! - **Aliases and layout** — tool aliases (`bash`/`exec`/`shell` → `shell`),
//!   repeated whitespace, and the concrete project prefix (→ `$PROJECT`).
//! - **Volatile tokens** — absolute and home-relative (`~/…`) paths, URLs,
//!   UUIDs, long hex runs, and long digit runs are replaced with stable
//!   placeholders (`«path»`, `«url»`,
//!   `«uuid»`, `«hex»`, `«n»`). Real agent commands are long one-off compounds
//!   full of scratchpad directories, session UUIDs, and timestamps; without this
//!   pass two semantically identical failures never share a byte-identical
//!   signature and never register as recurring. Command *structure* — binaries,
//!   subcommands, flags, and shell operators — is deliberately preserved, so
//!   `cargo test -p a` and `cargo test -p b` stay distinct while `cd /x && go
//!   build` and `cd /y && go build` collapse to one shape.
//!
//! The normalization is a pure, total function of an [`Event`]; it never
//! consults a model, the filesystem, the network, the locale, or the clock, and
//! normalizing an already-normalized command is a fixed point (idempotent). Both
//! the deterministic pattern detectors and the retrieval signature index build
//! on this single implementation so their identities stay byte-for-byte
//! consistent.
//!
//! # Versioning
//!
//! Every minted selector embeds [`SIGNATURE_SPEC_VERSION`]. The volatile-token
//! normalization was introduced in `v2`; `v1` selectors embedded literal command
//! text. The two grammars are intentionally non-interoperable: a `v1` selector
//! never matches a freshly minted `v2` signature. Already-registered mutations
//! keep their immutable `v1` trigger selectors as valid audit records; a stored
//! or indexed signature is re-minted under `v2` by rebuilding the projection
//! (`autophagy reindex --index-tool-input`). See ADR 0014.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::Value;

use crate::Event;

/// Current signature-grammar version token embedded in every minted selector
/// (`operation/<version>|…`, `failure/<version>|…`).
///
/// Bump this when the normalization in [`normalize_operation`] changes so a new
/// grammar cannot be silently confused with signatures minted under the old one.
pub const SIGNATURE_SPEC_VERSION: &str = "v2";

/// A normalized tool operation: its canonical tool name and command text.
///
/// Construct one with [`normalize_operation`]. The stable string projections
/// ([`operation_key`](OperationSignature::operation_key) and
/// [`failure_signature`](OperationSignature::failure_signature)) are versioned
/// so a future normalization change can introduce a new prefix without silently
/// reinterpreting stored signatures.
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

    /// Normalized command text: the project prefix replaced by `$PROJECT` and
    /// every volatile token replaced by a stable placeholder.
    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }

    /// Outcome-independent operation identity:
    /// `operation/<version>|<tool>|<command>`.
    #[must_use]
    pub fn operation_key(&self) -> String {
        format!(
            "operation/{SIGNATURE_SPEC_VERSION}|{}|{}",
            self.tool, self.command
        )
    }

    /// Failure identity including an exit code:
    /// `failure/<version>|<tool>|<command>|exit:<code>`.
    #[must_use]
    pub fn failure_signature(&self, exit_code: i64) -> String {
        format!(
            "failure/{SIGNATURE_SPEC_VERSION}|{}|{}|exit:{exit_code}",
            self.tool, self.command
        )
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

/// Normalize a raw command string exactly as [`normalize_operation`] does.
///
/// Exposed for direct testing and for callers that already hold command text
/// outside an [`Event`]; it applies the identical, deterministic pass.
#[must_use]
pub fn normalize_command_text(command: &str, project: Option<&str>) -> String {
    normalize_command(command, project)
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

// Shell characters that bound a token: a volatile path starts right after one of
// them (or at the start of the command) and never crosses one. Kept in a single
// fragment so the boundary and the path-interior classes stay in sync.
const BOUNDARY_CLASS: &str = r#"\s=:'"(){}\[\],&|;<>$`"#;

/// URL-like tokens: `scheme://…`. Their host, path, and query are all volatile.
static URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"[a-zA-Z][a-zA-Z0-9+.\-]*://[^\s'"(){}\[\],&|;<>`]*"#).expect("valid url regex")
});

/// Characters allowed inside a filesystem path token.
const PATH_INTERIOR: &str = r#"[^\s'"(){}\[\],&|;<>$`#]"#;

/// POSIX absolute paths with at least two segments (`/a/b…`). Requiring two
/// segments avoids collapsing single-slash tokens such as sed flags (`/g`) or a
/// bare division operator. Group 1 preserves the boundary delimiter (start,
/// whitespace, `=`, a quote, …) so the shell structure around the path survives.
static ABSOLUTE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(^|[{BOUNDARY_CLASS}])(/{PATH_INTERIOR}+/{PATH_INTERIOR}*)"
    ))
    .expect("valid absolute-path regex")
});

/// Home-relative paths (`~/…`). The `~/` prefix is unambiguous, so a single
/// segment is enough (`~/cio`); no two-segment guard is needed.
static HOME_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(^|[{BOUNDARY_CLASS}])(~/{PATH_INTERIOR}*)"))
        .expect("valid home-path regex")
});

/// RFC 4122-shaped UUIDs.
static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b")
        .expect("valid uuid regex")
});

/// Runs of eight or more hex characters (git SHAs, object ids, content hashes).
/// A run with no hex *letter* is a plain number and is left for [`DIGITS`].
static HEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9a-fA-F]{8,}").expect("valid hex regex"));

/// Runs of four or more digits (timestamps, ports, line numbers, pids).
static DIGITS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]{4,}").expect("valid digit regex"));

fn normalize_command(command: &str, project: Option<&str>) -> String {
    // 1. Replace the concrete project prefix with a stable marker so the same
    //    operation reads identically across repositories.
    let command = project.map_or_else(
        || command.to_owned(),
        |project| command.replace(project, "$PROJECT"),
    );
    // 2. Collapse every run of whitespace to a single space.
    let command = command.split_whitespace().collect::<Vec<_>>().join(" ");
    // 3. Replace volatile tokens with stable placeholders, most specific first,
    //    preserving surrounding command structure. Ordering matters: URLs before
    //    paths (a URL's `//host` is not a filesystem path), and hex before digits
    //    (a hex id may embed digit runs). None of the placeholders can re-match a
    //    later rule, so the pass is idempotent.
    let command = URL.replace_all(&command, "«url»");
    let command = ABSOLUTE_PATH.replace_all(&command, "${1}«path»");
    let command = HOME_PATH.replace_all(&command, "${1}«path»");
    let command = UUID.replace_all(&command, "«uuid»");
    let command = HEX.replace_all(&command, |caps: &Captures<'_>| {
        let matched = &caps[0];
        if matched.bytes().any(|byte| byte.is_ascii_alphabetic()) {
            "«hex»".to_owned()
        } else {
            // A pure-digit run is a number, not a hash: leave it for DIGITS so it
            // reads as «n» rather than «hex».
            matched.to_owned()
        }
    });
    let command = DIGITS.replace_all(&command, "«n»");
    command.into_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use time::OffsetDateTime;

    use super::{SIGNATURE_SPEC_VERSION, normalize_command_text, normalize_operation};
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

    fn norm(command: &str) -> String {
        normalize_command_text(command, None)
    }

    #[test]
    fn version_token_is_v2() {
        assert_eq!(SIGNATURE_SPEC_VERSION, "v2");
    }

    #[test]
    fn normalizes_tool_alias_whitespace_and_project_prefix() {
        let event = tool_event(
            json!("cargo   test  /workspace/project/crate"),
            Some("/workspace/project"),
        );
        let operation = normalize_operation(&event).expect("operation");
        assert_eq!(operation.tool(), "shell");
        // The single-segment `$PROJECT/crate` suffix is in-repo structure, not a
        // volatile absolute path, so it is preserved.
        assert_eq!(operation.command(), "cargo test $PROJECT/crate");
        assert_eq!(
            operation.operation_key(),
            "operation/v2|shell|cargo test $PROJECT/crate"
        );
        assert_eq!(
            operation.failure_signature(2),
            "failure/v2|shell|cargo test $PROJECT/crate|exit:2"
        );
    }

    #[test]
    fn reads_structured_command_input() {
        let event = tool_event(json!({"command": "pytest -q"}), None);
        let operation = normalize_operation(&event).expect("operation");
        assert_eq!(operation.operation_key(), "operation/v2|shell|pytest -q");
    }

    #[test]
    fn rejects_uninspectable_or_empty_commands() {
        assert!(normalize_operation(&tool_event(json!({"other": 1}), None)).is_none());
        assert!(normalize_operation(&tool_event(json!("   "), None)).is_none());
    }

    // --- Per-rule normalization -------------------------------------------

    #[test]
    fn collapses_absolute_paths_but_keeps_structure() {
        assert_eq!(
            norm("cd /home/user/project && go build"),
            "cd «path» && go build"
        );
        assert_eq!(
            norm("cat /private/tmp/claude-501/x/scratchpad/cookies.txt"),
            "cat «path»"
        );
        // Boundary delimiters around the path survive: `-f`, `[`, `]`.
        assert_eq!(norm("test [ -f /var/run/a/b.pid ]"), "test [ -f «path» ]");
        // A path attached to an assignment keeps the `KEY=` structure.
        assert_eq!(
            norm("JAR=/private/tmp/x/y/cookies.txt curl"),
            "JAR=«path» curl"
        );
    }

    #[test]
    fn collapses_home_relative_paths() {
        // Distinct repositories under the home directory share one shape.
        assert_eq!(
            norm("cd ~/code/ui && gh pr checks"),
            "cd «path» && gh pr checks"
        );
        assert_eq!(
            norm("cd ~/code/ui && gh pr checks"),
            norm("cd ~/code/cdp && gh pr checks")
        );
        // A single home-relative segment still normalizes.
        assert_eq!(norm("ls ~/cio"), "ls «path»");
    }

    #[test]
    fn different_paths_same_structure_collapse_together() {
        assert_eq!(
            norm("cd /a/first && go build ./..."),
            norm("cd /b/second && go build ./...")
        );
    }

    #[test]
    fn preserves_command_structure_across_binaries_and_flags() {
        // No volatile tokens: distinct invocations must stay distinct.
        assert_ne!(
            norm("cargo test -p autophagy-store"),
            norm("cargo test -p autophagy-cli")
        );
        assert_eq!(
            norm("cargo test -p autophagy-store"),
            "cargo test -p autophagy-store"
        );
    }

    #[test]
    fn collapses_urls() {
        assert_eq!(
            norm("curl -s -X POST https://api.example.com/v1/x?token=abc"),
            "curl -s -X POST «url»"
        );
    }

    #[test]
    fn collapses_uuids_hex_and_digits() {
        assert_eq!(
            norm("kill e7fb40d6-5a91-4164-b6d2-4494d95a30b4"),
            "kill «uuid»"
        );
        assert_eq!(norm("git show a7a903af82db154a3"), "git show «hex»");
        assert_eq!(norm("nc localhost 8080"), "nc localhost «n»");
        // Short numbers are stable structure and are left intact.
        assert_eq!(norm("sleep 5"), "sleep 5");
        assert_eq!(norm("head -3 file"), "head -3 file");
    }

    #[test]
    fn pure_digit_run_reads_as_number_not_hex() {
        // Eight digits are hex-shaped but carry no hex letter: they must read as
        // a number, and hex-shaped ids with a letter must read as hex.
        assert_eq!(norm("echo 20260717"), "echo «n»");
        assert_eq!(norm("echo deadbeef12"), "echo «hex»");
    }

    #[test]
    fn near_miss_audit_examples_recur_across_sessions() {
        let a = norm(
            "until [ -f /private/tmp/claude-501/-Users-x/e7fb40d6-5a91-4164-b6d2-4494d95a30b4/tasks/a7a903af82db154a3.done ]; do sleep 5; done",
        );
        let b = norm(
            "until [ -f /private/tmp/claude-501/-Users-y/cb114e1e-bf4e-4b5e-9b34-bc95f435d061/tasks/9f2c1e77bd0a4c11.done ]; do sleep 5; done",
        );
        assert_eq!(a, b, "same shell shape must produce one signature");
        assert_eq!(a, "until [ -f «path» ]; do sleep 5; done");
    }

    #[test]
    fn normalization_is_idempotent() {
        for command in [
            "cd /a/b && go build ./...",
            "curl https://x.example/y?z=1 && kill e7fb40d6-5a91-4164-b6d2-4494d95a30b4",
            "JAR=/private/tmp/x/y/c.txt run 12345 a7a903af82db154a3",
            "cargo test -p autophagy-store",
        ] {
            let once = norm(command);
            let twice = normalize_command_text(&once, None);
            assert_eq!(
                once, twice,
                "normalizing twice must equal once for `{command}`"
            );
        }
    }

    #[test]
    fn minted_signatures_and_fixtures_match_the_v2_schema() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../docs/specs/signature/v2/schema.json"))
                .expect("schema JSON");
        let validator = jsonschema::validator_for(&schema).expect("compile schema");

        // Signatures minted by the code under test satisfy the published grammar.
        let event = tool_event(json!("cd /a/b && go build ./..."), None);
        let operation = normalize_operation(&event).expect("operation");
        for minted in [operation.operation_key(), operation.failure_signature(1)] {
            assert!(
                validator.is_valid(&serde_json::Value::String(minted.clone())),
                "schema rejected minted signature {minted}"
            );
        }

        // Every published valid fixture is accepted; every invalid one rejected.
        for fixture in [
            include_str!("../../../docs/specs/signature/v2/valid/operation.json"),
            include_str!("../../../docs/specs/signature/v2/valid/operation_with_pipe.json"),
            include_str!("../../../docs/specs/signature/v2/valid/failure.json"),
            include_str!("../../../docs/specs/signature/v2/valid/recovery.json"),
            include_str!("../../../docs/specs/signature/v2/valid/correction.json"),
        ] {
            let instance: serde_json::Value = serde_json::from_str(fixture).expect("fixture JSON");
            assert!(
                validator.is_valid(&instance),
                "schema rejected valid {instance}"
            );
        }
        for fixture in [
            include_str!("../../../docs/specs/signature/v2/invalid/superseded_v1_version.json"),
            include_str!("../../../docs/specs/signature/v2/invalid/failure_missing_exit.json"),
            include_str!("../../../docs/specs/signature/v2/invalid/operation_empty_tool.json"),
            include_str!("../../../docs/specs/signature/v2/invalid/failure_noninteger_exit.json"),
        ] {
            let instance: serde_json::Value = serde_json::from_str(fixture).expect("fixture JSON");
            assert!(
                !validator.is_valid(&instance),
                "schema accepted invalid {instance}"
            );
        }
    }

    #[test]
    fn distinct_structures_stay_distinct_after_normalization() {
        let commands = [
            "cargo test -p a",
            "cargo test -p b",
            "cargo build",
            "go build ./...",
            "cd «path» && go build",
        ];
        for (i, left) in commands.iter().enumerate() {
            for right in &commands[i + 1..] {
                assert_ne!(
                    norm(left),
                    norm(right),
                    "`{left}` and `{right}` must differ"
                );
            }
        }
    }
}
