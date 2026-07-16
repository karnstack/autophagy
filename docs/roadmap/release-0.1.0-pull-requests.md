# 0.1.0 release pull-request sequence

With the alpha sequence complete, these slices round out the first release:
one more review-only installation target, an optional local-model synthesis
path, two more native adapters, a macOS menu-bar experience, continuous
ingestion, and the packaging pass that ships `0.1.0` itself.

## PR 19 — Retrieval CLI and ranking fixes

Status: complete.

- `autophagy search` fails fast as an argument error when neither a query nor
  `--signature` is given, instead of a runtime store error
- dual signature-and-full-text matches are no longer misclassified or dropped
  when they fall outside one source's top-N `LIMIT`

Exit: retrieval results are correct at the ranking boundary regardless of
`limit`.

## PR 20 — First release readiness

Status: complete.

- `CHANGELOG.md` (Keep a Changelog) summarizing PRs #1–#18
- `.github/workflows/release.yml`, dormant until a `v*` tag push, building the
  CLI and macOS app bundle and drafting (never publishing) a GitHub release
- README status and install/build sections updated for the completed alpha

Exit: the repository can be tagged and released without any code changes.

## PR 21 — Claude Code installation target

Status: complete.

- shadow-passed mutations materialize as repo-scoped Claude Code skills at
  `.claude/skills/autophagy-<id>/SKILL.md`
- one `InstallTarget`-parameterized path shared with the existing Codex target
- `--target codex|claude-code` on the CLI; migration 0007 for the registry

Exit: a reviewed mutation can be installed into either of two agents through
the same explicit, reversible path.

## PR 22 — Local model synthesis providers

Status: complete.

- Ollama and OpenAI-compatible providers behind the existing synthesis
  boundary, loopback-default and redirect-hardened
- explicit token accounting and a mutation/0.2 provenance block (provider,
  model, prompt/response token counts) on every synthesized candidate

Exit: an optional local model can propose richer candidates with measured,
visible cost and no contract bypass.

## PR 23 — Pi and OpenCode adapters

Status: complete.

- two more native session adapters, mirroring the Codex adapter's
  discovery/ingestion separation and incremental cursoring
- both pass the shared cross-adapter conformance harness

Exit: the store ingests four native agents (Claude Code, Codex, Pi, OpenCode)
without losing provenance.

## PR 24 — macOS menu-bar experience

Status: complete.

- always-available menu-bar extra: connection state, quick counts, candidate
  counts by lifecycle state, and the most recent candidates
- opt-in menu-bar-only (no Dock icon) runtime preference
- fixed a read-only open bug for cleanly-checkpointed WAL databases

Exit: the open database is glanceable without keeping the main window open.

## PR 25 and 26 — Watch mode and daemon lifecycle

Status: complete.

- a foreground `autophagy watch` loop and `autophagy daemon
  install|uninstall|status` (launchd on macOS, a systemd user unit on Linux)
- ingest-only, under the same redaction/privacy/projection gates as one-shot
  import; no store migration
- PR #26 is a same-day follow-up restoring a green `main` (`cargo fmt` and one
  clippy lint), no semantic change

Exit: agent history can be kept current automatically instead of by hand.

## PR 27 — 0.1.0 release packaging

Status: complete.

- workspace version `0.1.0-alpha.1` → `0.1.0`
- `CHANGELOG.md` consolidated into a dated `[0.1.0]` section covering PRs
  #19–#26, with an empty `[Unreleased]` kept on top
- README rewritten for a non-technical audience, with technical depth moved to
  a "For developers" section
- a template Homebrew formula and usage notes added under
  `packaging/homebrew/` for a future tap
- this roadmap document

Exit: the repository is ready to tag `v0.1.0`. No tag, GitHub release, or
publish step was performed by this PR — a human still does that manually.
