# ADR 0016: Export and import mutation genomes between machines

- Status: accepted
- Date: 2026-07-19

## Context

Every mutation Autophagy produces is verified locally: it is challenged, then
replayed against annotated counterfactuals, then shadow-measured for trigger
precision, all against one machine's own evidence. That verification is
expensive and human-reviewed, and today it cannot leave the machine that did it.
A team wants the payoff of that work to be shareable — one developer takes a
mutation to `shadow_passed`, and a teammate should be able to pick it up — without
either of them shipping raw session content anywhere or a network appearing in
the default path.

Three facts of the local store shape what "shareable" can mean:

- **The evidence foreign-key wall.** `register_mutation` fails unless every
  supporting and counterexample `event_id` already exists in `events`, and
  deleting a cited event cascade-kills the candidate. A package alone is not
  registrable on another machine.
- **Content-hash-locked reports.** Replay and shadow reports embed their exact
  `source_event_ids`, and the store checks that set precisely
  (`*_report_matches_registration`). Rewriting event ids to anonymize them would
  break the hashes.
- **No register-as-verified path.** Lifecycle state advances only through the
  gated `register_replay` / `register_shadow`, each of which re-derives from
  local evidence. There is deliberately no way to assert a state directly.

The first two force any portable bundle to carry the **original** events
(redacted). The third means a receiver cannot — and must not — inherit a
lifecycle state; they have to re-verify.

## Decision

Add a file-based, offline **team genome** (`genome/0.1`): a single self-contained
JSON bundle (`.genome.json`) that one developer exports and another imports.

**Bundle format.** A genome carries `spec_version`, a content-derived `genome_id`
(`gen_` + SHA-256 hex of the canonical bundle with the id field removed —
recomputed and checked on import), `exported_at`, an `origin`
(`instance_key`, `autophagy_version`), the full immutable mutation package, the
redacted `evidence_events` its hypothesis and reports cite (in stable id order),
`attestations`, the origin `transitions` for context, and a `redaction`
summary. The normative contract and fixtures live at `docs/specs/genome/0.1`. The
bundle logic lives in a new `autophagy-genome` crate, which depends only on
`events`, `mutations`, and `redaction` and performs no storage or network I/O;
the CLI orchestrates gathering from and ingesting into the store.

**Trust is never transplanted.** The candidate always lands on the receiver as a
fresh `candidate` through the ordinary `register_mutation` path. Lifecycle
**state does not travel**. The origin's replay and shadow reports travel as
**display-only attestations**, stored in a new `mutation_attestations` table
(migration 0004, STRICT). No lifecycle read path — `register_replay`,
`register_shadow`, install, or efficacy gating — ever consults that table. Each
attestation records the origin's claim (`passed`) and a `hash_verified` flag that
reports only whether the carried report bytes still hash to their carried
`content_hash` — a transit-integrity check, never a local re-verification. The
receiver must re-run challenge → replay → shadow against local evidence to
advance the candidate; `mutations show` and `mutations list` say so in plain
language ("origin-claimed, not locally verified").

**Redaction gate at both ends.** At export, every event runs through
`PrivacyPolicy` (the built-in secret rules plus the configured
`import.exclude_paths`). A path-excluded event **aborts** the export, naming the
event: silently dropping it would break the content-hash-locked reports that cite
it, so the honest move is to refuse. The mutation's reviewable free text (title,
hypothesis, intervention instruction, exclusions, failure cases) is scrubbed with
the same secret rules through a new public `PrivacyPolicy::scrub_value` /
`scrub_string` API. At import, the receiver re-runs its own policy over every
event before storing it — it never trusts that the sender redacted correctly —
and a path-excluded event aborts the import.

**Import respects existing store semantics.** Events are ingested through the
normal path, so validation, idempotency, and quarantine are preserved. A
conflicting reuse of an event id (the store would quarantine) aborts the import
cleanly, because evidence integrity cannot be satisfied. The mutation
registration maps the store's verdicts directly: `Duplicate` is a friendly
no-op, `EquivalentExisting` reports which local mutation is equivalent and writes
nothing, and a content conflict errors with nothing written. `--dry-run` prints
the full plan and writes nothing.

## Real-data verification

The design was exercised against a copy of the author's real database (schema
v2, upgraded in place to v4 on open). A real `repeated_command_failure`
candidate was exported under an isolated `AUTOPHAGY_CONFIG_DIR`; the bundle was
inspected to confirm the evidence events carried only redaction-approved content
and the reviewable text was scrubbed; and it was imported into a fresh empty
database, landing as a `candidate`. The user's real database and configuration
were never touched.

## Privacy

A genome contains **redacted event content** — the exact AEP events the mutation
cites, run through the redaction policy at export and again at import. Exporting
is an explicit, user-initiated act that writes a local file the user then chooses
whether and how to share; Autophagy performs no network I/O at any point. The
attestation reports are deterministic, model-free evaluation summaries (event
ids, normalized selectors, integer counts, and outcome enums) and carry no raw
payloads. Path-excluded evidence aborts rather than leaks, at both ends.

## Consequences

- A new ordered, immutable migration (0004) and the `autophagy-genome` crate.
- `MutationDetails` gains an `attestations` field (additive); `mutations list`
  gains a text-only `(imported)` marker while its JSON surface is unchanged.
- Because state never travels, an imported mutation cannot be installed until the
  receiver re-verifies it locally. This is intended: verification is a property
  of local evidence, not a transferable certificate.

## Alternatives considered

- **Ship lifecycle state / a register-as-verified path.** Rejected: it would let
  an unverified claim install a behavior change, defeating the point of the
  gates.
- **Anonymize event ids instead of carrying events.** Rejected: it breaks the
  content-hash-locked reports and the evidence foreign-key wall.
- **A `[genome]` config section for default export policy overrides.** Deferred;
  v0.1 reuses `import.exclude_paths` so there is one redaction policy to reason
  about. A future revision can add per-genome policy without changing the wire
  format.
