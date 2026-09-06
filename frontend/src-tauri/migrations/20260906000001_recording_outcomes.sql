CREATE TABLE recording_outcomes (
    meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
    audio_save_failed INTEGER NOT NULL DEFAULT 0,
    transcription_incomplete INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE transcript_revisions (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    transcripts_json TEXT NOT NULL
);
CREATE INDEX transcript_revisions_meeting ON transcript_revisions(meeting_id, created_at);
