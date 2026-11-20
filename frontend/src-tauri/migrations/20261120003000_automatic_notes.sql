ALTER TABLE meetings ADD COLUMN summary_content_revision INTEGER NOT NULL DEFAULT 0;

-- Match the saved passages, their order, speaker labels, and citation identity.
CREATE TRIGGER summary_transcript_insert AFTER INSERT ON transcripts BEGIN
    UPDATE meetings SET summary_content_revision = summary_content_revision + 1 WHERE id = NEW.meeting_id;
END;
CREATE TRIGGER summary_transcript_delete AFTER DELETE ON transcripts BEGIN
    UPDATE meetings SET summary_content_revision = summary_content_revision + 1 WHERE id = OLD.meeting_id;
END;
CREATE TRIGGER summary_transcript_update AFTER UPDATE OF
    id, meeting_id, transcript, speaker, audio_start_time, audio_end_time ON transcripts
WHEN NEW.id IS NOT OLD.id OR NEW.meeting_id IS NOT OLD.meeting_id OR
    NEW.transcript IS NOT OLD.transcript OR NEW.speaker IS NOT OLD.speaker OR
    NEW.audio_start_time IS NOT OLD.audio_start_time OR NEW.audio_end_time IS NOT OLD.audio_end_time BEGIN
    UPDATE meetings SET summary_content_revision = summary_content_revision + 1
        WHERE id IN (OLD.meeting_id, NEW.meeting_id);
END;

ALTER TABLE summary_processes ADD COLUMN automatic_origin TEXT;
ALTER TABLE summary_processes ADD COLUMN automatic_request_fingerprint TEXT;
ALTER TABLE summary_processes ADD COLUMN automatic_resume_attempted INTEGER NOT NULL DEFAULT 0;
