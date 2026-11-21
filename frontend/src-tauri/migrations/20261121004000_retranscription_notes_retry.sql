-- Local dispatch bookkeeping. Portable meeting archives exclude this table.
CREATE TABLE retranscription_notes_retry (
    meeting_id TEXT PRIMARY KEY NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    profile TEXT NOT NULL,
    publication TEXT NOT NULL
) WITHOUT ROWID;
