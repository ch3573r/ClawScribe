-- Authoritative conversations never reference derived chunks, vectors or jobs.
CREATE TABLE knowledge_owners (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('meeting','library')),
    meeting_id TEXT UNIQUE REFERENCES meetings(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    CHECK((kind='meeting' AND meeting_id IS NOT NULL AND id='meeting:' || meeting_id)
       OR (kind='library' AND meeting_id IS NULL AND id LIKE 'library:%'))
);
CREATE TABLE knowledge_requests (
    id TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL REFERENCES knowledge_owners(id) ON DELETE CASCADE,
    input_fingerprint TEXT NOT NULL,
    input_json TEXT NOT NULL DEFAULT '{}',
    question TEXT NOT NULL,
    scope_json TEXT NOT NULL,
    frozen_ids_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('preparing','running','completed','failed','interrupted','cancelled','invalidated')),
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    retrieval_mode TEXT NOT NULL DEFAULT 'keyword' CHECK(retrieval_mode IN ('keyword','hybrid')),
    failure TEXT,
    restored INTEGER NOT NULL DEFAULT 0 CHECK(restored IN (0,1)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    completed_at TEXT,
    UNIQUE(owner_id,id)
);
CREATE TABLE knowledge_messages (
    id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL REFERENCES knowledge_requests(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK(role IN ('user','assistant')),
    content TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE(request_id,role)
);
CREATE TABLE knowledge_request_evidence (
    request_id TEXT NOT NULL REFERENCES knowledge_requests(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal > 0 AND ordinal <= 64),
    reference_json TEXT NOT NULL,
    display_json TEXT NOT NULL,
    PRIMARY KEY(request_id,ordinal)
);
CREATE TABLE knowledge_request_sources (
    request_id TEXT NOT NULL REFERENCES knowledge_requests(id) ON DELETE CASCADE,
    source_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY(request_id,source_id)
);
CREATE INDEX knowledge_request_source_dependents ON knowledge_request_sources(source_id,request_id);
CREATE INDEX knowledge_request_owner_history ON knowledge_requests(owner_id,created_at,id);

-- Metadata is part of a prompt snapshot, even when canonical speech is unchanged.
CREATE TRIGGER knowledge_meeting_metadata AFTER UPDATE OF title,created_at ON meetings
WHEN NEW.title IS NOT OLD.title OR NEW.created_at IS NOT OLD.created_at BEGIN
    UPDATE knowledge_sources SET revision=revision+1 WHERE meeting_id=NEW.id;
END;

-- Run before source deletion, regardless of repository versus direct FK deletion.
-- Retain a redacted tombstone so an in-flight result can never recreate the turn.
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

CREATE TRIGGER knowledge_assistant_insert BEFORE INSERT ON knowledge_messages
WHEN NEW.role='assistant' AND NOT EXISTS
    (SELECT 1 FROM knowledge_requests WHERE id=NEW.request_id AND status IN ('running','completed')) BEGIN
    SELECT RAISE(ABORT,'Request does not accept an assistant result');
END;

CREATE TRIGGER knowledge_request_identity BEFORE UPDATE OF id,owner_id,input_fingerprint ON knowledge_requests
WHEN NEW.id IS NOT OLD.id OR NEW.owner_id IS NOT OLD.owner_id OR NEW.input_fingerprint IS NOT OLD.input_fingerprint BEGIN
    SELECT RAISE(ABORT,'Immutable request identity');
END;
