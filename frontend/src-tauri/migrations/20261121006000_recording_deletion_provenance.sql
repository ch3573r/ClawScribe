ALTER TABLE recording_library_deletions ADD COLUMN folder_path TEXT;
ALTER TABLE recording_library_deletions ADD COLUMN meeting_created_at_ms INTEGER;
ALTER TABLE recording_library_deletions ADD COLUMN deleted_at_ms INTEGER;

DROP TRIGGER remember_recording_library_deletion;
CREATE TRIGGER remember_recording_library_deletion
AFTER DELETE ON meetings
BEGIN
    INSERT INTO recording_library_deletions(meeting_id, folder_path, meeting_created_at_ms, deleted_at_ms)
    VALUES (OLD.id, NULLIF(TRIM(OLD.folder_path), ''), CAST(ROUND(unixepoch(OLD.created_at, 'subsec') * 1000) AS INTEGER),
            CAST(ROUND(unixepoch('subsec') * 1000) AS INTEGER))
    ON CONFLICT(meeting_id) DO UPDATE SET folder_path=excluded.folder_path,
        meeting_created_at_ms=excluded.meeting_created_at_ms, deleted_at_ms=excluded.deleted_at_ms;
END;
