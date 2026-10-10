-- Reference excerpts require a separate choice for the actual conversation owner.
-- Permissions are deliberately excluded from portable archives.
CREATE TABLE knowledge_document_permissions (
    owner_id TEXT PRIMARY KEY REFERENCES knowledge_owners(id) ON DELETE CASCADE,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1))
);
