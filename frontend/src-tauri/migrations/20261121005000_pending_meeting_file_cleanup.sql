CREATE TABLE pending_meeting_file_cleanup (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    meeting_id TEXT NOT NULL,
    title TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('recording', 'reference', 'outputs', 'codex', 'exports')),
    target TEXT NOT NULL,
    reason TEXT,
    kept INTEGER NOT NULL DEFAULT 0 CHECK (kept IN (0, 1))
);

CREATE INDEX pending_meeting_file_cleanup_target ON pending_meeting_file_cleanup(kind, target);
