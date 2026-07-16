# Contributor instructions

Autophagy is local-first infrastructure. Preserve user privacy, evidence
traceability, and reversibility in every change.

## Pull requests

- Prefix every pull request title with exactly one of `feat:`, `fix:`, or
  `maint:`.
- Use `feat:` for new behavior, `fix:` for bug fixes, and `maint:` for
  dependency, tooling, cleanup, or operational maintenance.
- Do not add agent or tool labels to pull request titles.
- Keep pull requests small enough to review as one coherent claim.

## Engineering constraints

- Keep the default path local-only and offline-capable.
- Never persist secrets or raw cloud payloads without explicit user consent.
- Every derived finding must retain exact evidence identifiers.
- Prefer deterministic, inspectable behavior over model-generated prose.
- Version public protocols and stored schemas before changing them.
- Do not add autonomous execution permissions by default.

## Quality gate

Run `mise install` once and `mise run check` before requesting review. Do not use
an unpinned system Rust toolchain for repository tasks.
