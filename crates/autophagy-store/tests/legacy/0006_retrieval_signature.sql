-- Exact normalized-signature index for deterministic hybrid retrieval.
--
-- Each row links one canonical event to its redaction-approved normalized
-- operation signature. The signature is supplied through the same explicit
-- search projection that gates free-text FTS content, so raw tool input never
-- becomes searchable text implicitly. Rows cascade on event deletion, which
-- keeps the retrieval index consistent with quarantine, prune, and delete-all
-- semantics without any new deletion path.
CREATE TABLE event_signatures (
  event_row_id  INTEGER PRIMARY KEY REFERENCES events(row_id) ON DELETE CASCADE,
  signature     TEXT NOT NULL CHECK (length(signature) > 0)
) STRICT;

CREATE INDEX event_signatures_lookup
  ON event_signatures(signature, event_row_id);
