-- Originals and extracted blocks are canonical; attachment relations have their own owners.
CREATE TABLE knowledge_documents (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL UNIQUE REFERENCES knowledge_sources(id) ON DELETE CASCADE,
    display_name TEXT NOT NULL,
    format TEXT NOT NULL CHECK(format IN ('pdf','docx','txt','md')),
    file_size INTEGER NOT NULL CHECK(file_size >= 0 AND file_size <= 26214400),
    sha256 TEXT NOT NULL CHECK(length(sha256)=64),
    storage_name TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE(sha256,format),
    CHECK(source_id='document:' || id),
    CHECK(storage_name=id || '.' || format)
);
CREATE TABLE knowledge_document_blocks (
    id TEXT PRIMARY KEY,
    document_id TEXT NOT NULL REFERENCES knowledge_documents(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal > 0),
    page INTEGER CHECK(page > 0 AND page <= 500),
    paragraph INTEGER NOT NULL CHECK(paragraph > 0),
    text TEXT NOT NULL,
    UNIQUE(document_id,ordinal)
);
CREATE TABLE knowledge_document_attachments (
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    document_id TEXT NOT NULL REFERENCES knowledge_documents(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    PRIMARY KEY(meeting_id,document_id)
);
CREATE INDEX knowledge_document_attachment_owners ON knowledge_document_attachments(document_id,meeting_id);
CREATE VIRTUAL TABLE knowledge_document_fts USING fts5(
    block_id UNINDEXED, source_id UNINDEXED, text,
    tokenize = 'unicode61 remove_diacritics 0'
);
CREATE TRIGGER knowledge_document_block_insert AFTER INSERT ON knowledge_document_blocks BEGIN
    INSERT INTO knowledge_document_fts(rowid,block_id,source_id,text)
        VALUES (NEW.rowid,NEW.id,'document:' || NEW.document_id,NEW.text);
    UPDATE knowledge_sources SET revision=revision+1 WHERE id='document:' || NEW.document_id;
END;
CREATE TRIGGER knowledge_document_block_delete AFTER DELETE ON knowledge_document_blocks BEGIN
    DELETE FROM knowledge_document_fts WHERE rowid=OLD.rowid;
    UPDATE knowledge_sources SET revision=revision+1 WHERE id='document:' || OLD.document_id;
END;
CREATE TRIGGER knowledge_document_block_update AFTER UPDATE OF document_id,page,paragraph,text ON knowledge_document_blocks
WHEN NEW.document_id IS NOT OLD.document_id OR NEW.page IS NOT OLD.page OR
     NEW.paragraph IS NOT OLD.paragraph OR NEW.text IS NOT OLD.text BEGIN
    DELETE FROM knowledge_document_fts WHERE rowid=OLD.rowid;
    INSERT INTO knowledge_document_fts(rowid,block_id,source_id,text)
        VALUES (NEW.rowid,NEW.id,'document:' || NEW.document_id,NEW.text);
    UPDATE knowledge_sources SET revision=revision+1 WHERE id='document:' || OLD.document_id;
    UPDATE knowledge_sources SET revision=revision+1 WHERE id='document:' || NEW.document_id AND NEW.document_id != OLD.document_id;
END;
CREATE TRIGGER knowledge_document_metadata AFTER UPDATE OF display_name,sha256 ON knowledge_documents
WHEN NEW.display_name IS NOT OLD.display_name OR NEW.sha256 IS NOT OLD.sha256 BEGIN
    UPDATE knowledge_sources SET revision=revision+1 WHERE id=NEW.source_id;
END;
CREATE TRIGGER knowledge_document_source_delete BEFORE DELETE ON knowledge_sources
WHEN OLD.kind='document' BEGIN
    DELETE FROM knowledge_document_fts WHERE source_id=OLD.id;
END;
