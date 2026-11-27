ALTER TABLE meetings ADD COLUMN saved_audio_removed_by_user INTEGER NOT NULL DEFAULT 0
    CHECK (saved_audio_removed_by_user IN (0, 1));
