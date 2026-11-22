ALTER TABLE saved_live_answers ADD COLUMN source_dependencies_complete INTEGER NOT NULL DEFAULT 0
    CHECK (source_dependencies_complete IN (0, 1));

-- Source and attachment IDs can be reused after deletion. A fresh incarnation
-- prevents a completed in-memory reply from being saved against a replacement.
ALTER TABLE knowledge_sources ADD COLUMN live_answer_incarnation TEXT NOT NULL DEFAULT '';
UPDATE knowledge_sources SET live_answer_incarnation = lower(hex(randomblob(16)));
CREATE TRIGGER knowledge_live_answer_source_insert AFTER INSERT ON knowledge_sources BEGIN
    UPDATE knowledge_sources SET live_answer_incarnation = lower(hex(randomblob(16))) WHERE id = NEW.id;
END;

CREATE TABLE saved_live_answer_attachment_instances (
    meeting_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    incarnation TEXT NOT NULL DEFAULT (lower(hex(randomblob(16)))),
    PRIMARY KEY (meeting_id, document_id),
    FOREIGN KEY (meeting_id, document_id)
        REFERENCES knowledge_document_attachments(meeting_id, document_id) ON DELETE CASCADE
);
INSERT INTO saved_live_answer_attachment_instances (meeting_id, document_id)
    SELECT meeting_id, document_id FROM knowledge_document_attachments;
CREATE TRIGGER knowledge_live_answer_attachment_insert AFTER INSERT ON knowledge_document_attachments BEGIN
    INSERT INTO saved_live_answer_attachment_instances (meeting_id, document_id)
        VALUES (NEW.meeting_id, NEW.document_id);
END;

CREATE TABLE saved_live_answer_dependencies (
    answer_id TEXT NOT NULL REFERENCES saved_live_answers(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    source_id TEXT NOT NULL REFERENCES knowledge_sources(id) ON DELETE CASCADE,
    document_id TEXT,
    attachment_meeting_id TEXT,
    reference_json TEXT NOT NULL CHECK (json_valid(reference_json)),
    PRIMARY KEY (answer_id, ordinal),
    FOREIGN KEY (attachment_meeting_id, document_id)
        REFERENCES knowledge_document_attachments(meeting_id, document_id) ON DELETE CASCADE,
    CHECK ((document_id IS NULL) = (attachment_meeting_id IS NULL))
);
CREATE INDEX saved_live_answer_dependencies_by_source ON saved_live_answer_dependencies(source_id);
CREATE INDEX saved_live_answer_dependencies_by_attachment
    ON saved_live_answer_dependencies(attachment_meeting_id, document_id)
    WHERE document_id IS NOT NULL;

-- Delete the whole answer, including uncited prose and excerpts, before its
-- dependency keys can cascade away. Older answers have no complete provenance.
CREATE TRIGGER knowledge_live_answer_source_delete BEFORE DELETE ON knowledge_sources BEGIN
    DELETE FROM saved_live_answers WHERE id IN (
        SELECT answer_id FROM saved_live_answer_dependencies WHERE source_id = OLD.id
    ) OR source_dependencies_complete = 0;
END;

CREATE TRIGGER knowledge_live_answer_attachment_delete BEFORE DELETE ON knowledge_document_attachments BEGIN
    DELETE FROM saved_live_answers WHERE id IN (
        SELECT answer_id FROM saved_live_answer_dependencies
        WHERE attachment_meeting_id = OLD.meeting_id AND document_id = OLD.document_id
    ) OR source_dependencies_complete = 0;
END;
