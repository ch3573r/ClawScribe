CREATE TABLE recording_calendar_drafts (
    recording_id TEXT PRIMARY KEY NOT NULL,
    calendar_link TEXT NOT NULL CHECK (json_valid(calendar_link)),
    revision INTEGER NOT NULL CHECK (revision > 0)
);
