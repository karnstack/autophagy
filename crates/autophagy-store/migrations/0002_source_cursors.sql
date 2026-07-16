CREATE TABLE source_cursors (
  adapter         TEXT NOT NULL CHECK (length(trim(adapter)) BETWEEN 1 AND 128),
  instance_key    TEXT NOT NULL CHECK (length(trim(instance_key)) > 0),
  origin          TEXT NOT NULL CHECK (length(trim(origin)) > 0),
  byte_offset     INTEGER NOT NULL CHECK (byte_offset >= 0),
  line_number     INTEGER NOT NULL CHECK (line_number >= 0),
  head_hash       BLOB NOT NULL CHECK (length(head_hash) = 32),
  state_json      TEXT NOT NULL CHECK (json_valid(state_json)),
  updated_at      TEXT NOT NULL,
  PRIMARY KEY (adapter, instance_key, origin)
) STRICT;
