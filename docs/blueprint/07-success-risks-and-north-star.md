# Success, Risks, and North Star

## 16. Success Metrics

| Area | Metric |
| --- | --- |
| Activation | Percentage of users who import sessions and view an evidence-backed pattern within 15 minutes. |
| Signal quality | Pattern acceptance rate; percentage of weekly reports with zero generic or irrelevant findings. |
| Mutation quality | Replay success rate, shadow precision, false intervention rate, and promotion rate. |
| Outcome value | Avoided failed commands, reduced tool calls, estimated time saved, test pass improvement. |
| Trust | Percentage of users keeping capture enabled; mutation inspection rate; uninstall reasons. |
| Retention | Weekly active users who receive at least one useful recall, prevention, or promoted mutation. |
| Ecosystem | Number of reliable adapters and tested community mutations. |
| Revenue | Plus conversion, team expansion, and paid build renewal without weakening the open-source core. |

## 17. Key Product Risks

| Risk | Mitigation |
| --- | --- |
| Generic LLM advice | Deterministic candidate generation, structured output, evidence requirements, counterexample search, honest silence. |
| False confidence from replay | Label simulated versus executed replay, require shadow validation, preserve uncertainty. |
| Privacy concerns | Open source, local-first, no telemetry, clear permissions, redaction, inspectability, and deletion. |
| Platform copies feature | Remain cross-agent, open-standard, local, auditable, and stronger in evaluation than native memory features. |
| Too much scope | Prove one mutation type and one high-value detector family before expanding integrations. |
| High local model cost | Use deterministic detectors, compact evidence packets, small models, scheduled digestion, and BYOM. |
| Mutation causes harm | Explicit permissions, user approval, reversible install, shadow mode, and ongoing retirement. |
| No measurable value | Design every mutation around a metric and show saved failures, time, or tool calls. |

## 18. North Star

> **The product to build**
>
> Autophagy should be the shared learning layer underneath every coding agent on
> a developer’s machine. It should remember less, understand repetition better,
> and change future behavior only when it can show evidence that the change
> helps.

The first version does not need to solve general agent self-improvement. It
needs to deliver one undeniable moment: the user sees a recurring failure they
had stopped noticing, reviews a concrete mutation generated from their own
history, sees that it would have prevented the failure, activates it, and
watches the next agent avoid the mistake.
