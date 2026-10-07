-- Derived indexes. Canonical transcript rows remain authoritative evidence.
CREATE TABLE knowledge_sources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL DEFAULT 'meeting' CHECK(kind = 'meeting'),
    meeting_id TEXT NOT NULL UNIQUE REFERENCES meetings(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL DEFAULT 1,
    generation INTEGER NOT NULL DEFAULT 1,
    semantic_revision INTEGER,
    semantic_space TEXT
);
CREATE TABLE knowledge_chunks (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES knowledge_sources(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    transcript_id TEXT NOT NULL,
    start_byte INTEGER NOT NULL,
    end_byte INTEGER NOT NULL,
    fingerprint TEXT NOT NULL,
    text TEXT NOT NULL,
    UNIQUE(source_id, generation, ordinal)
);
CREATE INDEX knowledge_chunks_generation ON knowledge_chunks(source_id, revision);
CREATE TABLE knowledge_vectors (
    chunk_id TEXT PRIMARY KEY REFERENCES knowledge_chunks(id) ON DELETE CASCADE,
    space TEXT NOT NULL,
    dimensions INTEGER NOT NULL CHECK(dimensions = 384),
    vector BLOB NOT NULL CHECK(length(vector) = 1536)
);
CREATE TABLE knowledge_index_jobs (
    source_id TEXT PRIMARY KEY REFERENCES knowledge_sources(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    generation INTEGER NOT NULL DEFAULT 1,
    attempts INTEGER NOT NULL DEFAULT 0,
    failure TEXT,
    paused INTEGER NOT NULL DEFAULT 0 CHECK(paused IN (0,1))
);
CREATE VIRTUAL TABLE knowledge_fts USING fts5(
    transcript_id UNINDEXED, source_id UNINDEXED, text,
    tokenize = 'unicode61 remove_diacritics 0'
);

INSERT INTO knowledge_sources(id,meeting_id) SELECT 'meeting:' || id,id FROM meetings;
INSERT INTO knowledge_index_jobs(source_id,revision) SELECT id,revision FROM knowledge_sources;
INSERT INTO knowledge_fts(transcript_id,source_id,text)
    SELECT id,'meeting:' || meeting_id,transcript FROM transcripts;

CREATE TRIGGER knowledge_meeting_insert AFTER INSERT ON meetings BEGIN
    INSERT INTO knowledge_sources(id,meeting_id) VALUES ('meeting:' || NEW.id,NEW.id);
    INSERT INTO knowledge_index_jobs(source_id,revision) VALUES ('meeting:' || NEW.id,1);
END;
CREATE TRIGGER knowledge_source_delete BEFORE DELETE ON knowledge_sources BEGIN
    DELETE FROM knowledge_fts WHERE source_id=OLD.id;
END;
CREATE TRIGGER knowledge_source_revision AFTER UPDATE OF revision ON knowledge_sources
WHEN NEW.revision != OLD.revision BEGIN
    UPDATE knowledge_sources SET generation=generation+1,semantic_revision=NULL,semantic_space=NULL WHERE id=NEW.id;
    INSERT INTO knowledge_index_jobs(source_id,revision,generation,attempts,failure,paused)
        SELECT id,revision,generation,0,NULL,0 FROM knowledge_sources WHERE id=NEW.id
        ON CONFLICT(source_id) DO UPDATE SET revision=excluded.revision,generation=excluded.generation,attempts=0,failure=NULL,paused=0;
END;
CREATE TRIGGER knowledge_transcript_insert AFTER INSERT ON transcripts BEGIN
    UPDATE knowledge_sources SET revision=revision+1 WHERE meeting_id=NEW.meeting_id;
    INSERT INTO knowledge_fts(transcript_id,source_id,text) VALUES (NEW.id,'meeting:' || NEW.meeting_id,NEW.transcript);
END;
CREATE TRIGGER knowledge_transcript_delete AFTER DELETE ON transcripts BEGIN
    UPDATE knowledge_sources SET revision=revision+1 WHERE meeting_id=OLD.meeting_id;
    DELETE FROM knowledge_fts WHERE transcript_id=OLD.id;
END;
CREATE TRIGGER knowledge_transcript_update AFTER UPDATE OF
    id,meeting_id,transcript,speaker,timestamp,audio_start_time,audio_end_time,duration,word_timestamps_json ON transcripts
WHEN NEW.id IS NOT OLD.id OR NEW.meeting_id IS NOT OLD.meeting_id OR
    NEW.transcript IS NOT OLD.transcript OR NEW.speaker IS NOT OLD.speaker OR
    NEW.timestamp IS NOT OLD.timestamp OR NEW.audio_start_time IS NOT OLD.audio_start_time OR
    NEW.audio_end_time IS NOT OLD.audio_end_time OR NEW.duration IS NOT OLD.duration OR
    NEW.word_timestamps_json IS NOT OLD.word_timestamps_json BEGIN
    UPDATE knowledge_sources SET revision=revision+1 WHERE meeting_id=OLD.meeting_id;
    UPDATE knowledge_sources SET revision=revision+1 WHERE meeting_id=NEW.meeting_id AND NEW.meeting_id != OLD.meeting_id;
    DELETE FROM knowledge_fts WHERE transcript_id=OLD.id;
    INSERT INTO knowledge_fts(transcript_id,source_id,text) VALUES (NEW.id,'meeting:' || NEW.meeting_id,NEW.transcript);
END;
