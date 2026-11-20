ALTER TABLE meeting_bookmarks ADD COLUMN recording_id TEXT;

CREATE INDEX meeting_bookmarks_recording
ON meeting_bookmarks(recording_id) WHERE recording_id IS NOT NULL;

CREATE TABLE recording_summary_bookmarks (
    recording_id TEXT PRIMARY KEY NOT NULL
);

-- A native recording or its recovered folder uses its original recording ID
-- as the meeting ID. Attach its notes and consent in the meeting transaction.
CREATE TRIGGER attach_recording_bookmarks
AFTER INSERT ON meetings
BEGIN
    UPDATE meeting_bookmarks
    SET meeting_id = NEW.id, folder_path = NULL, recording_id = NULL
    WHERE meeting_id IS NULL AND (recording_id = NEW.id OR
        (recording_id IS NULL AND folder_path = NEW.folder_path));

    INSERT OR IGNORE INTO meeting_summary_bookmarks (meeting_id)
    SELECT recording_id FROM recording_summary_bookmarks WHERE recording_id = NEW.id;

    DELETE FROM recording_summary_bookmarks WHERE recording_id = NEW.id;
END;
