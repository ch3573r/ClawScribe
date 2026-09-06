ALTER TABLE transcripts ADD COLUMN original_transcript TEXT;

CREATE TABLE transcript_edit_batches (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    changes_json TEXT NOT NULL,
    undone INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX transcript_edit_batches_meeting ON transcript_edit_batches(meeting_id, created_at);

CREATE TABLE transcript_file_sync (
    meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE
);
