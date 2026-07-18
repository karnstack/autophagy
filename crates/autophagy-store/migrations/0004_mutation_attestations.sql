-- Origin-claimed verification attestations carried by an imported genome.
--
-- A team genome (see ADR 0016) is a portable, redaction-gated bundle: one
-- developer exports a verified mutation candidate and another imports it. Trust
-- is deliberately NOT transplanted. The candidate always lands locally as a
-- fresh `candidate` (register_mutation), and the replay/shadow verification
-- reports the origin ran travel alongside it as DISPLAY-ONLY attestations.
--
-- This table stores those attestations. It is a museum label, not a lifecycle
-- gate: no state-advancing read path (register_replay, register_shadow, install,
-- efficacy) may ever consult it. The receiver must re-run challenge -> replay ->
-- shadow against its own local evidence to advance the candidate. The stored
-- `passed` flag records what the origin claimed; `hash_verified` records only
-- that the bundled report bytes still hash to their carried content hash (a
-- transit-integrity check), never that the receiver reproduced the result.
--
-- Ordered after post-install efficacy (0003) and, like every migration from the
-- first release onward, immutable — add new migrations, never edit this one.

CREATE TABLE mutation_attestations (
  attestation_id   TEXT PRIMARY KEY CHECK (attestation_id LIKE 'att_%'),
  mutation_id      TEXT NOT NULL REFERENCES mutation_candidates(mutation_id) ON DELETE CASCADE,
  kind             TEXT NOT NULL CHECK (kind IN ('replay', 'shadow')),
  origin_instance  TEXT NOT NULL,
  set_hash         TEXT NOT NULL,
  report_json      TEXT NOT NULL CHECK (json_valid(report_json)),
  content_hash     BLOB NOT NULL CHECK (length(content_hash) = 32),
  passed           INTEGER NOT NULL CHECK (passed IN (0, 1)),
  hash_verified    INTEGER NOT NULL CHECK (hash_verified IN (0, 1)),
  imported_at      TEXT NOT NULL,
  -- One attestation per (mutation, kind, evaluated set): re-importing the same
  -- genome is an idempotent no-op rather than a duplicate row.
  UNIQUE (mutation_id, kind, set_hash)
) STRICT;

CREATE INDEX mutation_attestations_candidate
  ON mutation_attestations(mutation_id, kind, set_hash);
