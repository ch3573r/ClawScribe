CREATE TABLE meeting_tags (
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    tag TEXT NOT NULL COLLATE NOCASE,
    PRIMARY KEY (meeting_id, tag)
);
CREATE INDEX meeting_tags_tag ON meeting_tags(tag);

CREATE TABLE meeting_bookmarks (
    id TEXT PRIMARY KEY,
    meeting_id TEXT REFERENCES meetings(id) ON DELETE CASCADE,
    folder_path TEXT,
    seconds REAL NOT NULL CHECK(seconds >= 0),
    label TEXT NOT NULL,
    CHECK(meeting_id IS NOT NULL OR folder_path IS NOT NULL)
);
CREATE INDEX meeting_bookmarks_meeting ON meeting_bookmarks(meeting_id);
CREATE INDEX meeting_bookmarks_folder ON meeting_bookmarks(folder_path);
