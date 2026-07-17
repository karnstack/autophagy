CREATE TABLE mutation_candidates_v4 (
  mutation_id        TEXT PRIMARY KEY CHECK (mutation_id LIKE 'mut_%'),
  source_finding_id  TEXT NOT NULL UNIQUE CHECK (source_finding_id LIKE 'fnd_%'),
  source_detector    TEXT NOT NULL,
  equivalence_key    TEXT NOT NULL UNIQUE CHECK (equivalence_key LIKE 'eqv_%'),
  spec_version       TEXT NOT NULL CHECK (spec_version = 'mutation/0.1'),
  semantic_version   TEXT NOT NULL,
  state              TEXT NOT NULL CHECK (state IN ('candidate', 'challenged', 'replay_passed', 'rejected')),
  package_json       TEXT NOT NULL CHECK (json_valid(package_json)),
  content_hash       BLOB NOT NULL CHECK (length(content_hash) = 32),
  challenge_json     TEXT CHECK (challenge_json IS NULL OR json_valid(challenge_json)),
  rejection_reason   TEXT,
  created_at         TEXT NOT NULL,
  updated_at         TEXT NOT NULL
) STRICT;

CREATE TABLE mutation_evidence_v4 (
  mutation_id  TEXT NOT NULL REFERENCES mutation_candidates_v4(mutation_id) ON DELETE CASCADE,
  event_id     TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  role         TEXT NOT NULL CHECK (role IN ('support', 'counterexample')),
  ordinal      INTEGER NOT NULL CHECK (ordinal >= 0),
  PRIMARY KEY (mutation_id, role, ordinal),
  UNIQUE (mutation_id, event_id)
) STRICT;

CREATE TABLE mutation_transitions_v4 (
  transition_id  INTEGER PRIMARY KEY,
  mutation_id    TEXT NOT NULL REFERENCES mutation_candidates_v4(mutation_id) ON DELETE CASCADE,
  from_state     TEXT CHECK (from_state IS NULL OR from_state IN ('candidate', 'challenged', 'replay_passed')),
  to_state       TEXT NOT NULL CHECK (to_state IN ('candidate', 'challenged', 'replay_passed', 'rejected')),
  reason         TEXT NOT NULL,
  metadata_json  TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
  occurred_at    TEXT NOT NULL
) STRICT;

INSERT INTO mutation_candidates_v4 SELECT * FROM mutation_candidates;
INSERT INTO mutation_evidence_v4 SELECT * FROM mutation_evidence;
INSERT INTO mutation_transitions_v4 SELECT * FROM mutation_transitions;

DROP TRIGGER mutation_evidence_removed;
DROP TABLE mutation_evidence;
DROP TABLE mutation_transitions;
DROP TABLE mutation_candidates;

ALTER TABLE mutation_candidates_v4 RENAME TO mutation_candidates;
ALTER TABLE mutation_evidence_v4 RENAME TO mutation_evidence;
ALTER TABLE mutation_transitions_v4 RENAME TO mutation_transitions;

CREATE INDEX mutation_candidates_state_updated
  ON mutation_candidates(state, updated_at DESC);

CREATE INDEX mutation_transitions_candidate_time
  ON mutation_transitions(mutation_id, occurred_at, transition_id);

CREATE TRIGGER mutation_evidence_removed
AFTER DELETE ON mutation_evidence
BEGIN
  DELETE FROM mutation_candidates WHERE mutation_id = OLD.mutation_id;
END;

CREATE TABLE mutation_replays (
  replay_id          TEXT PRIMARY KEY CHECK (replay_id LIKE 'rpl_%'),
  mutation_id        TEXT NOT NULL REFERENCES mutation_candidates(mutation_id) ON DELETE CASCADE,
  scenario_set_hash  TEXT NOT NULL CHECK (scenario_set_hash LIKE 'rsh_%'),
  report_json        TEXT NOT NULL CHECK (json_valid(report_json)),
  content_hash       BLOB NOT NULL CHECK (length(content_hash) = 32),
  passed             INTEGER NOT NULL CHECK (passed IN (0, 1)),
  created_at         TEXT NOT NULL,
  UNIQUE (mutation_id, scenario_set_hash)
) STRICT;

CREATE INDEX mutation_replays_candidate_time
  ON mutation_replays(mutation_id, created_at, replay_id);

CREATE TABLE mutation_replay_evidence (
  replay_id  TEXT NOT NULL REFERENCES mutation_replays(replay_id) ON DELETE CASCADE,
  event_id   TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
  ordinal    INTEGER NOT NULL CHECK (ordinal >= 0),
  PRIMARY KEY (replay_id, ordinal),
  UNIQUE (replay_id, event_id)
) STRICT;

CREATE TRIGGER mutation_replay_evidence_removed
AFTER DELETE ON mutation_replay_evidence
BEGIN
  DELETE FROM mutation_candidates
  WHERE mutation_id = (
    SELECT mutation_id FROM mutation_replays WHERE replay_id = OLD.replay_id
  );
END;
