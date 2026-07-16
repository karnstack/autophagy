# Contributing to Autophagy

Autophagy is early. The most valuable contributions are small, testable changes
to the event contract, import fixtures, redaction behavior, and deterministic
detectors.

## Development

The repository uses mise as its authoritative tool manager. Tool versions and
quality-gate tasks are pinned in `.mise.toml`.

```sh
mise install
mise run check
```

## Protocol changes

AEP is a versioned public contract. A behavior-changing schema edit requires:

1. a decision record describing compatibility impact;
2. updated JSON Schema and Rust types;
3. valid and invalid fixtures;
4. an explicit migration or compatibility story.

## Pull requests

Use exactly one approved title prefix: `feat:`, `fix:`, or `maint:`. Explain the
claim being made, the evidence used to verify it, and any privacy implications.
