-- Remembered opt-in only. Live sessions, requests and history remain ephemeral.
ALTER TABLE knowledge_settings ADD COLUMN live_text_sharing INTEGER NOT NULL DEFAULT 0
    CHECK(live_text_sharing IN (0,1));
