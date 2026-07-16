# Model Strategy

## 6. Decision and Model Strategy

> **Decision rule**
>
> The model proposes. Autophagy proves. The user promotes.

### 6.1 Three-brain architecture

| Component | Implementation | Responsibility |
| --- | --- | --- |
| Scout | Deterministic detectors + hybrid retrieval | Find candidate repeated behavior. |
| Scientist | Local or user-configured LLM | Form a falsifiable hypothesis and generate a concrete mutation. |
| Judge | Replay, tests, shadow observations, metrics | Determine whether the mutation measurably improves outcomes. |

### 6.2 Open-source default model bundle

| Role | Recommended default | Notes |
| --- | --- | --- |
| Inference runtime | llama.cpp | Embedded or managed by the app; cross-platform; GGUF; local structured output. |
| Embeddings | Qwen3-Embedding-0.6B | Small local retrieval model; combine with FTS/BM25 and exact signals. |
| Standard analysis | Qwen3 8B, 4-bit | Good default for 16 GB Apple Silicon machines. |
| Low-memory analysis | Qwen3 4B, 4-bit | For 8 GB systems; fewer and more conservative proposals. |
| Advanced mutation generation | Qwen3-Coder 30B-A3B, quantized | Optional for machines with more memory or external local servers. |
| Bring your own model | Ollama, LM Studio, MLX, OpenAI-compatible endpoints, cloud APIs | Core functionality remains available offline; stronger models improve synthesis only. |

### 6.3 Model routing configuration

```yaml
models:
  embedding:
    provider: local
    model: qwen3-embedding-0.6b

  extractor:
    provider: local
    model: qwen3-8b-q4

  mutation_generator:
    provider: ollama
    model: qwen3-coder:30b

  replay_judge:
    provider: openai-compatible
    model: user-selected-model
```

### 6.4 Confidence scoring

The LLM’s self-reported confidence should contribute little or nothing.
Promotion confidence is derived from observed evidence.

```text
confidence = recurrence × evidence_quality × specificity × replay_success × shadow_precision × freshness
```

### 6.5 Anti-bullshit constraints

- Require at least two independent pieces of evidence for normal candidates.
- Require exact references to sessions, commands, files, diffs, tests, or
  corrections.
- Reject generic advice such as “improve error handling.”
- Require trigger and exclusion conditions.
- Prefer the earliest reliable intervention point.
- Search for counterexamples and related existing mutations.
- Allow “nothing useful found” and “insufficient evidence.”
- Measure false interventions after deployment.
