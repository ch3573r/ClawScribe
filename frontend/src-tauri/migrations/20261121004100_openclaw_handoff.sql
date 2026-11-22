CREATE TABLE openclaw_handoffs (
    meeting_id TEXT PRIMARY KEY NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    destination TEXT,
    updated_at TEXT NOT NULL,
    status_json TEXT NOT NULL
);

CREATE INDEX openclaw_handoffs_destination ON openclaw_handoffs (destination, updated_at DESC);
