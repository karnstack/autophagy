# Team Genome v0.1

A **genome** is a single self-contained JSON file (`.genome.json`) that carries
one verified mutation candidate from the machine that produced it to another.
One developer exports a mutation they have taken through challenge, replay, and
shadow; a teammate imports it as a fresh review-only candidate. It is
file-based, local-first, and offline: exporting writes a file, importing reads
one, and no network I/O ever occurs. The normative contract is
[`bundle.schema.json`](bundle.schema.json); `valid/` and `invalid/` hold
fixtures the parser and builder are tested against. See ADR 0016 for the design.

## Why a bundle, not just the package

A mutation package alone is not portable. Two facts of the local store force the
genome to carry more:

- **The evidence foreign-key wall.** `register_mutation` fails unless every
  supporting and counterexample `event_id` already exists locally, and deleting
  a cited event cascade-kills the candidate. A genome therefore carries the
  exact AEP events its hypothesis cites so registration can succeed on the
  receiver.
- **Content-hash-locked reports.** Replay and shadow reports embed their exact
  `source_event_ids`, and the store checks that set precisely. Rewriting event
  ids would break the hashes, so the bundle carries the **original** events
  (redacted), keeping their ids valid.

## Contents

- `spec_version` — always `genome/0.1`.
- `genome_id` — `gen_` + SHA-256 hex of the canonical bundle content with the
  `genome_id` field itself removed. Object keys serialize in sorted order, so
  the id is independent of field ordering. The importer recomputes it and
  rejects the bundle on mismatch (it was altered after export).
- `exported_at` — RFC 3339 export time.
- `origin` — `instance_key` and `autophagy_version` of the exporter.
- `mutation` — the complete immutable mutation package, governed by the
  `mutation/*` contract, with its reviewable free text scrubbed (below).
- `evidence_events` — the redacted AEP events, in stable `event_id` order, each
  governed by the `aep/*` contract.
- `attestations` — origin-claimed verification reports (below).
- `transitions` — the origin's lifecycle history, display-only.
- `redaction` — `redacted_fields` (count of string fields the secret rules
  changed) and a `policy` description.

## Redaction gate

Redaction runs at **both** ends, so the receiver never has to trust that the
sender redacted correctly:

- **At export**, every event runs through the redaction policy (built-in secret
  rules plus the configured `import.exclude_paths`). A path-excluded event
  **aborts** the export, naming the event — silently dropping it would break the
  reports that cite it. The mutation's reviewable text (title, hypothesis,
  intervention instruction, exclusions, failure cases) is scrubbed with the same
  secret rules.
- **At import**, the receiver re-runs its own policy over every event before it
  is stored, and a path-excluded event aborts the import.

## Attestations are origin-claimed, not verified

Trust is never transplanted. The candidate always lands on the receiver as a
fresh `candidate`; **lifecycle state does not travel**. The origin's replay and
shadow reports travel as **display-only attestations**:

- `kind` — `replay` or `shadow`.
- `id`, `set_hash` — the origin's report identity and evaluated-set hash.
- `report_json` — the complete versioned report, exactly as the origin stored it
  (a deterministic, model-free evaluation summary: event ids, selectors, integer
  counts, and outcome enums — never raw payloads).
- `content_hash` — SHA-256 hex of `report_json`, a **transit-integrity**
  fingerprint. On import the receiver recomputes it; a match proves the report
  bytes were not altered after export, **not** that the receiver reproduced the
  result.
- `passed` — what the origin claimed.

The receiver must re-run challenge → replay → shadow against its own local
evidence to advance the candidate. No lifecycle read path (replay, shadow,
install, efficacy gating) ever consults an attestation.

## Import outcomes

Import ingests the events through the normal path (validation, redaction,
idempotency, quarantine), then registers the mutation:

- **Duplicate** — the same package already exists locally: a friendly no-op.
- **Equivalent existing** — an equivalent trigger/intervention already exists
  under another id: reported, nothing written.
- **Content conflict** — the same id exists with different content: an error,
  nothing written.
- A conflicting reuse of an evidence `event_id` (the store would quarantine)
  aborts the import cleanly, because evidence integrity cannot be satisfied.

`--dry-run` prints the full plan and writes nothing.
