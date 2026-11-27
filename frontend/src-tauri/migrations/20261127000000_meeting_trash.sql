ALTER TABLE meetings ADD COLUMN deleted_at TEXT
    CHECK(deleted_at IS NULL OR datetime(deleted_at) IS deleted_at);
ALTER TABLE meetings ADD COLUMN delete_recording_on_purge INTEGER NOT NULL DEFAULT 1
    CHECK(delete_recording_on_purge IN (0, 1));

CREATE VIEW active_meetings AS
SELECT rowid, * FROM meetings WHERE deleted_at IS NULL;

CREATE INDEX idx_meetings_trash ON meetings(deleted_at) WHERE deleted_at IS NOT NULL;

CREATE VIEW visible_knowledge_requests AS
SELECT r.* FROM knowledge_requests r JOIN knowledge_owners o ON o.id=r.owner_id
WHERE NOT EXISTS (SELECT 1 FROM meetings m WHERE m.id=o.meeting_id AND m.deleted_at IS NOT NULL)
  AND NOT EXISTS (
      SELECT 1 FROM json_each(r.frozen_ids_json) j JOIN meetings m ON m.id=j.value
      WHERE m.deleted_at IS NOT NULL
  );
