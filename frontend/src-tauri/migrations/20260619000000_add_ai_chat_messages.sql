-- Transcript-grounded "chat with your meeting" history.
-- One row per turn (user question or assistant reply); the transcript itself is
-- not duplicated here — it's re-assembled from `transcripts` at send time.
CREATE TABLE IF NOT EXISTS ai_chat_messages (
    id TEXT PRIMARY KEY NOT NULL,
    meeting_id TEXT NOT NULL,
    role TEXT NOT NULL,            -- 'user' | 'assistant'
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

-- Ordered history lookups per meeting.
CREATE INDEX IF NOT EXISTS idx_ai_chat_messages_meeting
    ON ai_chat_messages(meeting_id, created_at);
