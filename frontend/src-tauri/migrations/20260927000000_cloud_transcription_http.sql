-- NULL preserves the legacy private-network policy until explicitly saved.
ALTER TABLE transcript_settings ADD COLUMN cloudWhisperAllowUnencrypted INTEGER;
ALTER TABLE transcript_settings ADD COLUMN maiTranscribeAllowUnencrypted INTEGER;
