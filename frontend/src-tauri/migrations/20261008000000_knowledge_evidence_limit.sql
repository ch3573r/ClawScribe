-- Match knowledge::evidence::MAX_EVIDENCE_ENTRIES (1024).
-- No shipped table has a foreign key to knowledge_request_evidence. Its only
-- index is the primary-key autoindex; the source-delete trigger below is the
-- only trigger on another table that references it. Recreate that trigger
-- verbatim after the transactional SQLite table rebuild.
DROP TRIGGER knowledge_conversation_source_delete;

CREATE TABLE knowledge_request_evidence_expanded (
    request_id TEXT NOT NULL REFERENCES knowledge_requests(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal > 0 AND ordinal <= 1024),
    reference_json TEXT NOT NULL,
    display_json TEXT NOT NULL,
    PRIMARY KEY(request_id,ordinal)
);
INSERT INTO knowledge_request_evidence_expanded
    (request_id,ordinal,reference_json,display_json)
    SELECT request_id,ordinal,reference_json,display_json FROM knowledge_request_evidence;
DROP TABLE knowledge_request_evidence;
ALTER TABLE knowledge_request_evidence_expanded RENAME TO knowledge_request_evidence;

CREATE TRIGGER knowledge_conversation_source_delete BEFORE DELETE ON knowledge_sources BEGIN
    UPDATE knowledge_messages SET content='' WHERE request_id IN
        (SELECT request_id FROM knowledge_request_sources WHERE source_id=OLD.id);
    DELETE FROM knowledge_request_evidence WHERE request_id IN
        (SELECT request_id FROM knowledge_request_sources WHERE source_id=OLD.id);
    UPDATE knowledge_requests SET status='invalidated',question='',scope_json='{}',input_json='{}',
        frozen_ids_json='[]',failure='source_deleted' WHERE id IN
        (SELECT request_id FROM knowledge_request_sources WHERE source_id=OLD.id);
    DELETE FROM knowledge_request_sources WHERE request_id IN
        (SELECT id FROM knowledge_requests WHERE status='invalidated');
END;
