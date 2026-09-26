ALTER TABLE recording_outcomes ADD COLUMN capture_incomplete INTEGER NOT NULL DEFAULT 0;
ALTER TABLE recording_outcomes ADD COLUMN recording_files_incomplete INTEGER NOT NULL DEFAULT 0;
