# ADR 0015: Track post-install mutation efficacy with recurrence windows

- Status: accepted
- Date: 2026-07-19

## Context

Autophagy carries a mutation from a detected finding through challenge, replay,
shadow, and finally installation as a repo-scoped skill. Every gate up to
installation is a *prospective* judgement: replay reasons about annotated
counterfactuals, shadow measures would-be trigger precision. None of them answers
the only question that matters once a mutation is live: **does the failure
signature it addresses actually recur less after it was installed?**

That question is answerable locally, deterministically, and without a model. The
store already holds everything needed: `mutation_installations.installed_at` (an
RFC 3339, NOT NULL install timestamp), the exact normalized-signature index
(`event_signatures`), and `events.occurred_at` / `event_type` / `exit_code`. A
mutation's immutable trigger selector names the exact failure signature to count.

Two engineering constraints bound the design. The default path stays local and
offline — this is a COUNT-style query plus pure arithmetic, no model. And every
derived finding must retain exact evidence identifiers — the report cites the
`event_id`s it counted. Adding a stored table takes an ordered, immutable
migration (0003) and this record.

## Decision

Add an **observational, non-causal** efficacy measurement. It compares how often
the addressed failure signature recurred in two equal-length windows anchored on
`installed_at`, and never touches the mutation lifecycle.

**Windows.** The post-window is `[installed_at, evaluated_at]`; the pre-window is
the equal-length window immediately before install,
`[installed_at − post_duration, installed_at)`. Equal lengths make the two rates
directly comparable. The evaluation clock is passed in and echoed in the report,
so a report is reproducible from `(database-state, installed_at, evaluated_at)`.

**Matching rule (`failure_signature_recurrence`).** Resolved empirically against
the real development database (see below). The mutation's trigger selector is a
`failure/<v>|<tool>|<command>|exit:<code>` signature, but the exact-signature
index stores only the *operation* projection `operation/<v>|<tool>|<command>` —
the failure grammar never appears in `event_signatures`. So a failure selector
cannot be matched against the index directly. Instead the store decomposes the
selector into its operation signature and exit code, and counts the events that
are indexed under that operation signature **and** are `tool.failed` **and** carry
the matching exit code. (Command text can contain `|`, so the failure form is
parsed by trimming the trailing `|exit:<code>`, never by splitting on `|`.) An
`operation/<v>|…` selector matches any indexed `tool.failed` for that operation.

**Coverage diagnostics.** The exact-signature index is partial by construction
(old imports predate indexing; unprojectable commands carry no row). The report
therefore always states `total_failures` (every in-span `tool.failed`) versus
`classifiable_failures` (those with an index row), and never silently
undercounts: coverage below 50% forces an `insufficient_data` verdict with reason
`partial_index_coverage`.

**Verdict thresholds** (deterministic and inspectable, documented in
[`docs/specs/efficacy/0.1`](../specs/efficacy/0.1/README.md)): `insufficient_data`
when the post-window is under 7 days, when fewer than 2 total occurrences are
observed, or when coverage is below 50% — each triggered reason is listed.
Otherwise, from the change in weekly rate: a reduction of ≥20% is `improved`, an
increase of ≥20% is `regressed`, the ±20% band is `unchanged`; a brand-new
recurrence with no pre-window baseline is `regressed`.

**No lifecycle coupling.** Replay and shadow advance the mutation state on pass;
efficacy never does. `register_efficacy` writes an append-only report and
performs no state transition. History accumulates — each evaluation sees more
post-install time, so many reports per mutation are expected and the table does
not constrain to one row per mutation. This is also why the evidence-removal
trigger deletes only the efficacy *report* when a cited event is removed, rather
than deleting the mutation candidate the way the shadow and replay evidence
triggers do: losing an efficacy snapshot must never retire an otherwise-valid,
installed mutation.

**Crate boundary.** A new `autophagy-efficacy` crate holds the pure, deterministic
evaluation (mirroring `autophagy-shadow`); the store gathers occurrences and
coverage; the CLI wires them (`autophagy mutations efficacy <id>`). This keeps
the evaluator a pure function of its inputs and sits it at the replay/shadow tier
of the dependency graph. The report is versioned `efficacy/0.1` with a
content-derived `efficacy_id` (`eff_<sha256>`), so re-evaluating at the same clock
is idempotent and a new clock yields a new report — exactly the history semantics
above.

## Real-data verification

Resolved and verified against a throwaway copy of the author's real database
(69,400 events, 34,911 indexed signatures) — never the live database or config.

- **Selector-form gotcha, confirmed empirically.** Both registered candidates
  carry `failure/v1|shell|…|exit:1` selectors, while `event_signatures` holds
  `operation/v1|…` rows exclusively (34,911 of 34,911). For the `go build`
  candidate the operation signature resolves to 56 indexed rows: 28 `tool.called`,
  25 `tool.completed` (exit 0), and **3 `tool.failed` (exit 1)** — precisely the
  three failures the finding cites. The decompose-and-rejoin rule counts those
  three and nothing else.
- **End-to-end drive.** One candidate was driven on the copy through challenge →
  replay (12 scenarios, passed) → shadow (12 observations, precision 100%,
  passed) → install into a scratch git repo, producing a real
  `installed_at = 2026-07-18T20:54:57Z`.
- **Honest empty window.** Evaluated at the real install instant, the post-window
  is 0 days, so the verdict is `insufficient_data`
  (`post_window_too_short`, `sparse_occurrences`) — the correct answer when
  observation has not yet begun.
- **Verdict math on real evidence.** Evaluated at a later clock (the same real
  `installed_at`, `--now 2026-11-10`), the three historical `go build` failures
  fall in the pre-window and none recur after install:

  ```
  improved · 3 → 0 failures (0.2/wk → 0.0/wk) · -100.0% · 80.4% classifiable
  ```

  The report cites the exact three failure `event_id`s, and the 80.4% coverage
  figure (304 of 425 in-span `tool.failed` events indexed) demonstrates the
  partial-index diagnostic surfacing rather than hiding.

## Privacy

Strictly subtractive on derived data. The report cites exact `event_id`s and
counts only — never command text, never event content. Occurrences are matched
through the same redaction-approved signature index that already gates search, so
efficacy can only ever see what redaction already approved. No new source text is
persisted and nothing leaves the machine.

## Consequences

- A fourth deterministic, model-free measurement joins detection, replay, and
  shadow, closing the loop from "installed" to "did it help".
- `status` gains a one-line installed-mutation efficacy summary (only when
  installations exist); `mutations show` gains a latest-efficacy line.
- Efficacy is only as complete as the signature index; the coverage diagnostic
  makes that limit explicit rather than silent, and a low-coverage database is
  told to `reindex` through the existing status hint (ADR 0014).
- Migrations advance to schema v3; the table and its evidence cascade are
  immutable from here.

## Addendum (2026-07-19): `selector_grammar_mismatch` insufficient-data reason

A mutation installed under an older signature grammar exposed a misleading
report. Its trigger selector embeds a grammar version
(`failure/v1|shell|cd zuzoto && go build ./... 2>&1|exit:1`), but the database it
was evaluated against had its `event_signatures` index re-minted to grammar `v2`
by `reindex --index-tool-input`. The v1 operation key then matched zero rows even
though the pre-install failures existed (indexed under v2), so the report read
`insufficient data · 0 → 0 failures · no prior baseline · needs more observed
occurrences` — as if the failures had never happened.

The honest statement is that the selector's grammar is older than the index's and
the measurement cannot see those events. A v1 selector cannot be silently
translated to v2: the v2 normalization is not derivable from the v1 string.

**Decision.** Add a fourth `insufficient_reasons` variant,
`selector_grammar_mismatch`, to efficacy result **v0.1**. It fires when a
selector's grammar version is older than the newest grammar present in the index
*and* the index holds no rows of that older grammar (a partial or skipped reindex
that still carries the old grammar does not trip it — the selector can still match
those rows). The text output states the mismatch explicitly and directs the user
to re-propose from current findings.

**Why this is not a version bump.** The change is a purely additive enum member.
Existing efficacy/0.1 reports never carried this value and remain valid; every
existing fixture still passes. Per the repo rule that a behavior-changing schema
edit needs a decision record plus updated schema, Rust types, and fixtures, this
addendum is the decision record: `result.schema.json` gains the enum value,
`InsufficientReason` gains the variant, and `valid/selector_grammar_mismatch.json`
is added (a real report generated from the reindexed showcase database). A new
`Store::signature_grammar_versions` supplies the index grammar versions the
evaluator compares against; no migration is required (it is a read-only query over
the existing `event_signatures` table).
