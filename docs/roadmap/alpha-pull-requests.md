# Alpha pull-request sequence

With the local mutation lifecycle complete, the next slices close the remaining
alpha gaps without weakening evidence or privacy boundaries.

## PR 1 — Successful recovery motifs

Status: complete.

- composite fail → recovery step → success detector
- direct-retry counterexamples and non-inflated occurrence scoring
- conservative zero-permission preflight mutation template

Exit: the product has three deterministic detector families and can propose a
reusable recovery without claiming correlation proves causality.

## PR 2 — Replay decision-point extraction

Status: complete.

- derive review drafts from exact mutation evidence and nearby session context
- preserve unknown counterfactual labels instead of inventing outcomes
- export a deterministic annotation workflow into Replay Suite v0.1

Exit: users no longer hand-author replay structure, but still review every
counterfactual label before evaluation.

## PR 3 — Exact and hybrid retrieval

Status: complete.

- exact normalized signature lookup alongside FTS5
- repository, recency, event-kind, and outcome filters
- versioned ranking explanations and retrieval evaluation fixtures

Exit: evidence and prior recoveries can be recalled predictably without a model.

## PR 4 — Local synthesis boundary

Status: complete.

- provider-neutral structured synthesis interface
- local model manifest and explicit insufficient-evidence behavior
- deterministic validation around all generated package fields

Exit: a local model may propose richer mutations but cannot bypass contracts or
evaluation gates.

## PR 5 — Native read-only experience

Status: complete.

- macOS onboarding and database selection
- sessions, patterns, mutations, and lifecycle audit views
- privacy settings and destructive-action confirmation

Exit: the complete local loop is inspectable without using JSON output or raw
SQLite tools.

## Release readiness

Status: in progress.

All five alpha slices above are complete, closing the alpha sequence. The first
release is now being prepared: the root `CHANGELOG.md` consolidates the history
of pull requests #1–#18, and `.github/workflows/release.yml` builds the CLI and
macOS app bundle and drafts a GitHub release on a `v*` tag push. The workspace
version remains `0.1.0-alpha.1` and nothing is published yet — a human performs
the actual release.
