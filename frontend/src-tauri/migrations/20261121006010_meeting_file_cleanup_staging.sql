ALTER TABLE pending_meeting_file_cleanup ADD COLUMN stage_token TEXT;
ALTER TABLE pending_meeting_file_cleanup ADD COLUMN staged INTEGER NOT NULL DEFAULT 0 CHECK (staged IN (0, 1));
