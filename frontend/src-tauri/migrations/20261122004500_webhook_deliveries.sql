CREATE TABLE webhook_deliveries (
    id TEXT PRIMARY KEY NOT NULL,
    event TEXT NOT NULL,
    meeting_id TEXT REFERENCES meetings(id) ON DELETE CASCADE,
    completion_key TEXT,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    outcome TEXT NOT NULL DEFAULT 'pending',
    http_status INTEGER,
    error_class TEXT,
    UNIQUE (event, meeting_id, completion_key)
);

CREATE INDEX webhook_deliveries_latest ON webhook_deliveries (started_at DESC);
CREATE INDEX webhook_deliveries_meeting ON webhook_deliveries (meeting_id);
