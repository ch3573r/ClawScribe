CREATE TABLE meeting_summary_bookmarks (
    meeting_id TEXT PRIMARY KEY NOT NULL REFERENCES meetings(id) ON DELETE CASCADE
);
