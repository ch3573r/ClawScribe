-- Keep a text-free substring index for literal library search. SQLite's built-in
-- word tokenizer cannot find fragments inside words or punctuation-only terms.
CREATE INDEX knowledge_chunks_row_span ON knowledge_chunks(source_id, transcript_id, start_byte, end_byte, revision DESC);

CREATE VIRTUAL TABLE knowledge_title_literal USING fts5(text, content='', contentless_delete=1, tokenize='trigram');
INSERT INTO knowledge_title_literal(rowid,text) SELECT rowid,replace(replace(title, char(304), 'i' || char(775)), char(8490), 'k') FROM meetings;
CREATE TRIGGER knowledge_title_literal_insert AFTER INSERT ON meetings BEGIN
    INSERT INTO knowledge_title_literal(rowid,text) VALUES(NEW.rowid,replace(replace(NEW.title, char(304), 'i' || char(775)), char(8490), 'k'));
END;
CREATE TRIGGER knowledge_title_literal_update AFTER UPDATE OF title ON meetings WHEN NEW.title IS NOT OLD.title BEGIN
    DELETE FROM knowledge_title_literal WHERE rowid=OLD.rowid;
    INSERT INTO knowledge_title_literal(rowid,text) VALUES(NEW.rowid,replace(replace(NEW.title, char(304), 'i' || char(775)), char(8490), 'k'));
END;
CREATE TRIGGER knowledge_title_literal_delete AFTER DELETE ON meetings BEGIN
    DELETE FROM knowledge_title_literal WHERE rowid=OLD.rowid;
END;
CREATE VIRTUAL TABLE knowledge_transcript_literal USING fts5(text, content='', contentless_delete=1, tokenize='trigram');
INSERT INTO knowledge_transcript_literal(rowid,text) SELECT rowid,replace(replace(transcript, char(304), 'i' || char(775)), char(8490), 'k') FROM transcripts;
CREATE TRIGGER knowledge_transcript_literal_insert AFTER INSERT ON transcripts BEGIN
    INSERT INTO knowledge_transcript_literal(rowid,text) VALUES(NEW.rowid,replace(replace(NEW.transcript, char(304), 'i' || char(775)), char(8490), 'k'));
END;
CREATE TRIGGER knowledge_transcript_literal_update AFTER UPDATE OF transcript ON transcripts WHEN NEW.transcript IS NOT OLD.transcript BEGIN
    DELETE FROM knowledge_transcript_literal WHERE rowid=OLD.rowid;
    INSERT INTO knowledge_transcript_literal(rowid,text) VALUES(NEW.rowid,replace(replace(NEW.transcript, char(304), 'i' || char(775)), char(8490), 'k'));
END;
CREATE TRIGGER knowledge_transcript_literal_delete AFTER DELETE ON transcripts BEGIN
    DELETE FROM knowledge_transcript_literal WHERE rowid=OLD.rowid;
END;


-- A saved passage can survive unrelated row edits. Changing meeting metadata
-- still invalidates references because it was part of the provider's context.
ALTER TABLE knowledge_sources ADD COLUMN metadata_revision INTEGER NOT NULL DEFAULT 1;
-- A pre-upgrade revision may already have changed the title or date. Keep
-- previously stale legacy evidence stale when that history is unavailable.
UPDATE knowledge_sources SET metadata_revision=revision WHERE kind='meeting';
DROP TRIGGER knowledge_meeting_metadata;
CREATE TRIGGER knowledge_meeting_metadata AFTER UPDATE OF title,created_at ON meetings
WHEN NEW.title IS NOT OLD.title OR NEW.created_at IS NOT OLD.created_at BEGIN
    UPDATE knowledge_sources SET revision=revision+1, metadata_revision=revision+1 WHERE meeting_id=NEW.id;
END;
