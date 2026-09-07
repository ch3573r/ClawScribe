-- Cover meeting filtering, counts and deterministic audio-order pagination.
CREATE INDEX IF NOT EXISTS idx_transcripts_meeting_audio_id
    ON transcripts(meeting_id, audio_start_time, id);
