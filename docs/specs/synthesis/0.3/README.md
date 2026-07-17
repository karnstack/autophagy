# Synthesis manifest v0.3

`synthesis-manifest/0.3` is an additive revision of the
[synthesis-manifest/0.2](../0.2/README.md) manifest. It exists so agent-CLI-backed
providers — the authenticated `claude` (Claude Code) and `codex` CLIs — can be
configured as synthesis model backends. The normative schema is
[`manifest.schema.json`](manifest.schema.json).

The synthesis **response** contract is unchanged; it remains
[synthesis/0.1](../0.1/response.schema.json). Only the manifest changed, so only
the manifest is versioned here.

## What changed from v0.2

- Two new `format` values: `claude_cli` and `codex_cli`. For these formats the
  manifest `path` is the CLI **binary** (an absolute path, or a bare name
  resolved via `PATH`), not an endpoint URL, and `name` stays a human-readable
  label.
- One optional field, `model` — the model identifier passed to the CLI's
  `--model` flag. When absent, the CLI's own configured default model is used.

Everything else — `timeouts`, `api_key_env`, `resource_hints`, and the strict
`additionalProperties: false` rule — is identical to v0.2. Both new formats and
the `model` field require `synthesis-manifest/0.3`; using either under an older
`spec_version` is a precise, actionable error.

## Agent-CLI provider semantics

An agent-CLI provider spawns the configured CLI as a subprocess, hands it the
same deterministic synthesis prompt the HTTP providers use, requests JSON-only
output, disables tool/command execution for the run, and parses the CLI's result
envelope into a synthesis response. A hard wall-clock timeout (default 120 s,
overridable with `timeouts.request_ms`) kills a hung child.

Unlike the local HTTP providers, these CLIs reach the **vendor's cloud** through
the user's existing login. There is no loopback option, so an agent-CLI provider
always requires the explicit `--allow-remote-endpoint` consent flag; without it
synthesis refuses before spawning anything. What leaves the machine is exactly
the structured prompt (deterministic baseline text, hard constraints, and cited
event **identifiers**) — never raw payloads or transcripts. Usage is billed to
the user's existing plan through their logged-in CLI; Autophagy stores no API
key for these providers.

## Example

```json
{
  "spec_version": "synthesis-manifest/0.3",
  "name": "codex-cli-login",
  "format": "codex_cli",
  "path": "/opt/homebrew/bin/codex",
  "revision": "cli-0.x",
  "capabilities": ["mutation_synthesis"],
  "resource_hints": { "min_memory_mb": 512 },
  "timeouts": { "request_ms": 180000 },
  "model": "gpt-5-codex"
}
```

See [ADR 0010](../../decisions/0010-agent-cli-synthesis-providers.md) for the
design rationale, the verified CLI invocation table, and the privacy stance.
