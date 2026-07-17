-- Derived detection-findings cache (v2).
--
-- Every findings-consuming command (patterns, digest, mutations propose,
-- mutations synthesize, status --with-findings) otherwise re-runs a full
-- deterministic detection pass over every stored event on each invocation —
-- tens of seconds of silent recomputation on a large corpus. This table
-- memoizes the serialized detection report keyed by a content fingerprint of its
-- exact inputs, so an unchanged corpus at unchanged thresholds answers instantly
-- while any change to the events, thresholds, project filter, or detector spec
-- produces a different key and misses.
--
-- The cache is derived, deterministic, and fully reconstructable at any time by
-- deleting every row: it holds no new source text, only the exact evidence
-- identifiers the detectors already produce. See ADR 0013. It is ordered after
-- the v1 baseline and, like every migration from the first release onward, is
-- immutable — add new migrations, never edit this one.

CREATE TABLE findings_cache (
  cache_key    BLOB PRIMARY KEY CHECK (length(cache_key) = 32),
  generation   BLOB NOT NULL CHECK (length(generation) = 32),
  report_json  TEXT NOT NULL CHECK (json_valid(report_json)),
  created_at   TEXT NOT NULL
) STRICT;
