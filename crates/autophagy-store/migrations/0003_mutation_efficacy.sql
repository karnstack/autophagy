-- Post-install mutation efficacy tracking (observational, non-causal).
--
-- Once a mutation is installed as a repo-scoped skill, the only honest question
-- left is empirical: does the exact failure signature it addresses recur less
-- after `installed_at` than before? This table stores the deterministic,
-- model-free answer to that question — a versioned `efficacy/0.1` report keyed
-- by a content fingerprint of its exact inputs (the mutation, its selectors, the
-- install timestamp, and the evaluation clock). See ADR 0015.
--
-- History is intentional. Efficacy is a moving measurement: each evaluation runs
-- against more post-install observation time, so multiple rows accumulate per
-- mutation over time and the table deliberately does NOT constrain to one row
-- per mutation. That accumulation is the point of tracking.
--
-- Unlike replay and shadow, efficacy carries no lifecycle gate: it never
-- advances or retires a mutation, so registration performs no state transition.
-- It is ordered after the v2 findings cache and, like every migration from the
-- first release onward, is immutable — add new migrations, never edit this one.

CREATE TABLE mutation_efficacy (
  efficacy_id   TEXT PRIMARY KEY CHECK (efficacy_id LIKE 'eff_%'),
  mutation_id   TEXT NOT NULL REFERENCES mutation_candidates(mutation_id) ON DELETE CASCADE,
  report_json   TEXT NOT NULL CHECK (json_valid(report_json)),
  content_hash  BLOB NOT NULL CHECK (length(content_hash) = 32),
  verdict       TEXT NOT NULL CHECK (
                  verdict IN ('improved', 'regressed', 'unchanged', 'insufficient_data')
                ),
  created_at    TEXT NOT NULL
) STRICT;

-- Exact evidence identifiers for every failure event counted in either window,
-- mirroring mutation_shadow_evidence. Rows cascade on event deletion.
CREATE TABLE mutation_efficacy_evidence (
  efficacy_id  TEXT NOT NULL REFERENCES mutation_efficacy(efficacy_id) ON DELETE CASCADE,
  event_id     TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  ordinal      INTEGER NOT NULL CHECK (ordinal >= 0),
  PRIMARY KEY (efficacy_id, ordinal),
  UNIQUE (efficacy_id, event_id)
) STRICT;

CREATE INDEX mutation_efficacy_candidate_time
  ON mutation_efficacy(mutation_id, created_at, efficacy_id);

-- Removing any cited evidence event drops the efficacy report that cited it, so
-- a stored report can never reference a deleted event. This mirrors the shadow
-- and replay evidence triggers in shape, but deletes only the report — never the
-- mutation candidate. Efficacy is observational and gates no lifecycle state, so
-- losing an efficacy report must not retire an otherwise-valid mutation (ADR
-- 0015). Deleting the report cascades to its remaining evidence rows.
CREATE TRIGGER mutation_efficacy_evidence_removed
AFTER DELETE ON mutation_efficacy_evidence
BEGIN
  DELETE FROM mutation_efficacy WHERE efficacy_id = OLD.efficacy_id;
END;
