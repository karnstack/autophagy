# Synthesis manifest v0.2

`synthesis-manifest/0.2` is an additive revision of the
[synthesis-manifest/0.1](../0.1/README.md) manifest. It exists so HTTP-backed
providers (Ollama and OpenAI-compatible local servers) can be configured without
breaking the strict `additionalProperties: false` v0.1 contract. The normative
schema is [`manifest.schema.json`](manifest.schema.json).

The synthesis **response** contract is unchanged; it remains
[synthesis/0.1](../0.1/response.schema.json). Only the manifest changed, so only
the manifest is versioned here.

## What changed from v0.1

Two optional fields are added:

- `timeouts` — an object with optional `connect_ms` and `request_ms`. Timeouts
  are always enforced by HTTP providers; this only lets an operator override the
  sane defaults (3 s connect, 60 s total).
- `api_key_env` — the **name** of an environment variable that holds the API key
  for a hosted OpenAI-compatible endpoint. The key value is never stored in the
  manifest, the database, logs, or output. The provider reads it from the
  environment at call time and sends it as an `Authorization: Bearer` header. A
  named-but-unset variable is a precise, actionable error.

Everything else is identical to v0.1.

## Endpoint semantics for HTTP providers

For the `ollama` and `openai_compatible` formats the manifest `path` is the
endpoint base URL (for example `http://localhost:11434`) and `name` is the model
identifier sent to that endpoint (for example the Ollama model tag). By default
the endpoint host must be loopback (`localhost`, `127.0.0.0/8`, or `::1`); a
non-loopback host is refused unless the caller passes `--allow-remote-endpoint`.
This keeps the default path local-only.

## Example

```json
{
  "spec_version": "synthesis-manifest/0.2",
  "name": "qwen3-coder:30b",
  "format": "ollama",
  "path": "http://localhost:11434",
  "revision": "30b-a3b",
  "capabilities": ["mutation_synthesis"],
  "resource_hints": { "min_memory_mb": 24576 },
  "timeouts": { "connect_ms": 2000, "request_ms": 90000 }
}
```

See [ADR 0006](../../decisions/0006-model-synthesis-providers.md) for the design
rationale and the privacy stance.
