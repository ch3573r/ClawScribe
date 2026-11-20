CREATE TABLE recording_library_deletions (
    meeting_id TEXT PRIMARY KEY NOT NULL
) WITHOUT ROWID;

-- A retained recording must not be reinserted by a delayed or retried handoff.
CREATE TRIGGER remember_recording_library_deletion
AFTER DELETE ON meetings
WHEN OLD.folder_path IS NOT NULL AND OLD.folder_path <> ''
BEGIN
    INSERT OR IGNORE INTO recording_library_deletions (meeting_id) VALUES (OLD.id);
END;
