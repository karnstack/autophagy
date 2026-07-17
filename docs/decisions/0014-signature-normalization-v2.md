# ADR 0014: Normalize volatile tokens in recurrence signatures (v2)

- Status: accepted
- Date: 2026-07-17

## Context

Recurrence detection is only as good as the signature that groups operations. A
`v1` signature embedded the full literal command text after two cosmetic passes
(tool-alias folding and the project-prefix `$PROJECT` substitution):
`operation/v1|shell|<command>` and `failure/v1|shell|<command>|exit:<code>`.

Real coding-agent commands are long, one-shot compounds dense with volatile
tokens — absolute scratchpad and session directories, UUIDs, content hashes,
timestamps, ports, and line numbers. Two semantically identical operations almost
never produce a byte-identical `v1` command string, so they never share a
signature and never register as recurring.

A hands-on audit of the real Claude Code history on the development machine made
the gap concrete. Importing 35,497 events across 151 sessions produced **276
candidate failure signatures and 0 findings**, and the entire near-miss list was
1-occurrence entries whose only variation was volatile tokens, for example:

- `until [ -f /private/tmp/claude-501/-Users-…/<uuid>/tasks/<hex>.done ]; do sleep 5; done; …`
- `JAR=/private/tmp/claude-501/-Users-…/<uuid>/scratchpad/cookies.txt # 1. create geoset curl -s -b $JAR -X POST http://…`

The full 69,400-event database yielded exactly 2 findings, both from short,
literally-repeated commands (`cd zuzoto && go build ./... 2>&1`). On its most
common agent the product read as dead. The signature grammar — not the detectors
or thresholds — was the bottleneck.

Signatures are a versioned public contract (they are minted into evidence
packets, indexed for exact retrieval, and embedded verbatim in mutation trigger
selectors). Per the non-negotiable engineering constraints, changing how they are
minted requires a version bump, a decision record, and an explicit compatibility
story. It must also stay deterministic, inspectable, and model-free.

## Decision

Introduce a deterministic signature-normalization pass and bump the signature
grammar from `v1` to `v2`. `autophagy_events::signature::normalize_operation`
(the single implementation both the detectors and the retrieval index build on)
now, after the existing project-prefix and whitespace passes, replaces volatile
tokens with stable placeholders. The rules apply in order, most specific first,
over the whitespace-collapsed command string; command *structure* — binaries,
subcommands, flags, and shell operators — is deliberately preserved.

| Volatile token | Rule | Placeholder |
| --- | --- | --- |
| URL | `scheme://…` up to the next shell delimiter | `«url»` |
| Absolute path | `/seg/seg…` (≥2 segments) at a shell boundary | `«path»` |
| Home path | `~/…` at a shell boundary (≥1 segment) | `«path»` |
| UUID | RFC 4122 shape `8-4-4-4-12` | `«uuid»` |
| Hex run | ≥8 hex chars containing at least one `a–f` letter | `«hex»` |
| Digit run | ≥4 consecutive digits | `«n»` |

Design properties:

- **Structure preserved.** `cargo test -p autophagy-store` and
  `cargo test -p autophagy-cli` stay distinct; `cd /a/b && go build` and
  `cd /c/d && go build` collapse to one shape. The two-segment guard on absolute
  paths avoids collapsing single-slash tokens such as sed flags (`/g`) or a bare
  division operator; the `~/` prefix is unambiguous and needs no such guard.
- **Numbers vs hashes.** Hex runs are matched before digit runs, but a run with
  no `a–f` letter (a plain long number such as a date `20260717`) is left for the
  digit rule, so it reads as `«n»` rather than `«hex»`.
- **Pure and total.** The pass is a function of the command text alone — no
  locale, clock, filesystem, network, or model. It is idempotent: normalizing an
  already-normalized command is a fixed point (the placeholders cannot re-match
  any rule). Distinct structures stay distinct.

The version bump is applied at every mint site: `operation/v2`, `failure/v2`,
`recovery/v2`, and `correction/v2`. The correction family carries no command text
and its normalization is unchanged, but it bumps too so the whole selector
grammar advances as one coherent version. The single source of truth is
`SIGNATURE_SPEC_VERSION` in `autophagy-events`.

`DETECTION_SPEC_VERSION` bumps to `detection/v2` in lockstep. It is folded into
the findings-cache key (ADR 0013), so every entry minted under the `v1` grammar
misses and recomputes automatically — no stale report can be served.

The grammar is documented and fixture-pinned under
[`docs/specs/signature/v2/`](../specs/signature/v2/README.md); the `v2` mint
functions are validated against that schema in `autophagy-events`.

## Real-data verification

Re-importing the same Claude Code history (35,682 events, 151 sessions) into a
throwaway database confirms the mechanism and its honest limits:

- **Operation-level recurrence lifts sharply.** Distinct operation shapes that
  recur at the default gate (≥3 occurrences across ≥2 sessions) rose from **2
  under the `v1` literal grammar to 44 under `v2`** — a 22× gain. Volatile
  one-shots now share sensible shapes: `operation/v2|shell|«path»` (72×),
  `cat «path»` (28×), `reins screenshot --tab «n»` (14×). The distribution is
  healthy, not a degenerate mega-bucket (the largest is 0.4% of indexed rows).
- **Failure findings remain data-dependent.** This corpus holds 390 tool
  failures forming 276 distinct failing shells, and — verified directly — **not
  one repeats even twice**, under either grammar. The failure detector therefore
  still reports 276 candidates / 0 findings here: the user's failing commands are
  genuinely one-shot exploratory invocations, so there is no recurrence to
  surface. `v2` removes the signature bottleneck; it does not manufacture
  recurrence that the history does not contain.

The end-to-end detector gain is proven deterministically in
`autophagy-patterns` (`volatile_paths_recur_as_one_failure_under_v2`): three
byte-distinct failing commands differing only by scratchpad path now share one
`failure/v2` signature and qualify as a single finding across three sessions —
exactly the class of failure the `v1` grammar discarded.

## Compatibility

- **No historical rewrite.** Canonical events are untouched; nothing migrates
  stored evidence. Only derived, re-derivable projections change.
- **Registered mutations stay valid.** The mutation registry is append-only and
  audit-logged; already-registered candidates keep their immutable `v1` trigger
  selectors as valid records. A `v1` selector simply no longer matches a
  freshly minted `v2` signature. Re-proposing a finding generates a fresh `v2`
  candidate (a distinct equivalence key), which is the intended path forward; the
  deterministic generator refuses to parse a `v1` selector into a `v2` candidate.
- **Stored index re-mints via `reindex`.** The exact-signature index is a derived
  projection. `autophagy reindex --index-tool-input` re-mints every row under
  `v2` from the untouched canonical events. Because a database indexed before this
  change carries `operation/v1|…` rows that no longer match new signatures,
  `autophagy status` detects the mismatch (`EventStore::signatures_below_version`)
  and prints a one-line hint to reindex.

## Privacy

Normalization is strictly subtractive on derived text: it replaces volatile
tokens with fixed placeholders, so a `v2` signature contains *less* literal
command text than the `v1` signature it supersedes — absolute paths, URLs,
UUIDs, hashes, and long numbers no longer appear in the derived signature at all.
Raw events are unchanged, no new text is persisted, and the signature is still
indexed only from the redaction-approved projection. The change reduces, and
never increases, the sensitive surface of derived data.
