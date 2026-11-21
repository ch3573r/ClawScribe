CREATE TABLE recurring_meeting_preferences (
    source_id TEXT NOT NULL,
    series_id TEXT NOT NULL,
    summary_language TEXT,
    include_approved_notes INTEGER NOT NULL DEFAULT 1 CHECK (include_approved_notes IN (0, 1)),
    destination TEXT CHECK (destination IN ('planner', 'todo', 'email')),
    PRIMARY KEY (source_id, series_id)
);

CREATE TABLE approved_series_notes (
    meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
    source_id TEXT NOT NULL,
    series_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    approved_notes TEXT NOT NULL
);

CREATE TABLE saved_live_answers (
    id TEXT PRIMARY KEY NOT NULL,
    recording_id TEXT NOT NULL,
    meeting_id TEXT REFERENCES meetings(id) ON DELETE CASCADE,
    saved_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    answer TEXT NOT NULL
);

CREATE INDEX saved_live_answers_by_recording ON saved_live_answers(recording_id);
CREATE INDEX saved_live_answers_by_meeting ON saved_live_answers(meeting_id, saved_at, id);
