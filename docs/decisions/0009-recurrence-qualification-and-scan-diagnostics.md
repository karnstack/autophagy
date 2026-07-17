# ADR 0009: Recurrence-based qualification and never-silent scan diagnostics

- Status: accepted
- Date: 2026-07-17

## Context

The blueprint's phase-1 exit criterion is that a new user imports their history
and sees at least one evidence-backed repeated pattern. On real data this
failed: an import of tens of thousands of events across hundreds of sessions
produced zero findings and no explanation.

Two defects combined.

- **A fixture-tuned qualification gate.** `DetectorConfig` required
  `support_ratio_bps >= 5000` — the failing operation had to account for at
  least half of all its own occurrences. `support_ratio_bps` is
  `occurrences / (occurrences + counterexamples)`, where counterexamples are
  successful runs of the same normalized operation. Every real-world daily
  command succeeds far more often than it fails, so its failure share sits in
  the low single-digit-percent range. A genuine repeated failure — the same
  normalized build command failing across several distinct sessions — was
  silently discarded because it also succeeded many more times. The small demo
  fixtures passed only because they contain almost no counterexamples.

- **A silent zero.** When nothing qualified, the CLI printed only "no findings
  above threshold". The user could not tell whether ingestion failed, detectors
  ran, or a threshold filtered everything out.

"A repeated failure worth intervening on" is about recurrence and wasted effort
— the same failure signature recurring across sessions and days — not about the
operation's overall failure share. The old gate encoded the wrong idea.

## Decision

### Qualification is recurrence, not failure share

Qualification is decided by recurrence alone: at least `min_occurrences`
supporting events spread across at least `min_sessions` distinct sessions. These
keep their values (3 occurrences across 2 sessions). That bar already separates
a cross-session recurring pattern from a one-off or a single within-session
retry storm; the overall success rate of the operation is orthogonal to whether
its repeated failures waste effort.

`support_ratio_bps` is retained as a reported, inspectable component of both the
recurrence score and every finding, and it becomes an **optional anti-noise
floor** (`min_support_ratio_bps`) that **defaults to 0 (disabled)**. Raising the
floor only suppresses candidates whose failure share is vanishingly small; it is
never a majority-failure gate. The CLI `--min-support-ratio-bps` flag and its
help text were updated to describe this floor truthfully.

This choice — rather than deleting the ratio entirely or hard-coding a few
hundred basis points — keeps the knob available and honest while making the
default path recurrence-driven. The occurrence and session gates already provide
the anti-noise guarantee the old ratio was mistakenly relied upon for.

### Exit code stays in the failure signature

The failure signature is `failure/v1|<tool>|<command>|exit:<code>`, so the same
command failing with different exit codes forms distinct signatures. We measured
whether this fragments recurrence on the real corpus: of the 891 failure events,
860 shared a single exit code, and **zero** operations that would qualify with
exit codes merged were being split below threshold by the exit-code component. A
distinct exit code often means a genuinely different failure mode (for example a
test failure versus a command-not-found), so splitting keeps findings crisp and
actionable. We keep the exit code in the signature; this also avoids any change
to finding identity or the evidence contract.

### Never a silent zero

`autophagy-patterns` gains `detect_with_report`, which returns the qualified
findings **and** a `DetectionDiagnostics` summary computed in the same pass (no
second scan, no new persistence):

- events scanned and distinct sessions scanned,
- the number of candidate recurrence signatures seen across all detectors,
- the top few near-threshold `Observation`s — recurring candidates that did
  **not** qualify — each annotated with the single gate it missed
  (`min_occurrences`, `min_sessions`, or `min_support_ratio_bps`).

Observations are explicitly not findings: they carry the same inspectable
recurrence statistics but omit evidence and counterexample lineage, so they can
never be mistaken for — or promoted into — a finding, and they never feed
mutation generation.

The `digest` and `patterns` commands surface the scan size and candidate count
on every run, and render the observations block whenever findings are zero. The
JSON report shape for both commands gains `sessions_scanned`,
`candidate_signatures`, and `observations`. This is CLI output, not a stored or
wire contract; the evidence packet contract (`evidence/0.1`) is unchanged, and
its score fields already carried `support_ratio_bps`.

## Consequences

- On the real corpus the phase-1 exit criterion now passes: the same import that
  produced zero findings produces genuine repeated-command-failure findings,
  including a build command that fails across many sessions while mostly
  succeeding, plus a near-threshold observations list explaining the rest.
- Detectors remain deterministic, model-free, and order-independent; findings
  and diagnostics are stable across event orderings.
- A regression test encodes the failure mode directly: a corpus where an
  operation succeeds far more often than it fails must still yield a finding,
  and must convert to a `min_support_ratio_bps` observation only when that floor
  is explicitly raised.
- Callers that consumed the previous `patterns` JSON array must read
  `result.findings` instead; the addition is versioned in this ADR and the
  deterministic-findings guide.
