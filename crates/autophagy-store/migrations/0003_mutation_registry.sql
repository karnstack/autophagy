CREATE TABLE mutation_candidates (
  mutation_id        TEXT PRIMARY KEY CHECK (mutation_id LIKE 'mut_%'),
  source_finding_id  TEXT NOT NULL UNIQUE CHECK (source_finding_id LIKE 'fnd_%'),
  source_detector    TEXT NOT NULL,
  equivalence_key    TEXT NOT NULL UNIQUE CHECK (equivalence_key LIKE 'eqv_%'),
  spec_version       TEXT NOT NULL CHECK (spec_version = 'mutation/0.1'),
  semantic_version   TEXT NOT NULL,
  state              TEXT NOT NULL CHECK (state IN ('candidate', 'challenged', 'rejected')),
  package_json       TEXT NOT NULL CHECK (json_valid(package_json)),
  content_hash       BLOB NOT NULL CHECK (length(content_hash) = 32),
  challenge_json     TEXT CHECK (challenge_json IS NULL OR json_valid(challenge_json)),
  rejection_reason   TEXT,
  created_at         TEXT NOT NULL,
  updated_at         TEXT NOT NULL
) STRICT;

CREATE TABLE mutation_evidence (
  mutation_id  TEXT NOT NULL REFERENCES mutation_candidates(mutation_id) ON DELETE CASCADE,
  event_id     TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  role         TEXT NOT NULL CHECK (role IN ('support', 'counterexample')),
  ordinal      INTEGER NOT NULL CHECK (ordinal >= 0),
  PRIMARY KEY (mutation_id, role, ordinal),
  UNIQUE (mutation_id, event_id)
) STRICT;

CREATE TABLE mutation_transitions (
  transition_id  INTEGER PRIMARY KEY,
  mutation_id    TEXT NOT NULL REFERENCES mutation_candidates(mutation_id) ON DELETE CASCADE,
  from_state     TEXT CHECK (from_state IS NULL OR from_state IN ('candidate', 'challenged')),
  to_state       TEXT NOT NULL CHECK (to_state IN ('candidate', 'challenged', 'rejected')),
  reason         TEXT NOT NULL,
  metadata_json  TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
  occurred_at    TEXT NOT NULL
) STRICT;

CREATE INDEX mutation_candidates_state_updated
  ON mutation_candidates(state, updated_at DESC);

CREATE INDEX mutation_transitions_candidate_time
  ON mutation_transitions(mutation_id, occurred_at, transition_id);

CREATE TRIGGER mutation_evidence_removed
AFTER DELETE ON mutation_evidence
BEGIN
  DELETE FROM mutation_candidates WHERE mutation_id = OLD.mutation_id;
END;
