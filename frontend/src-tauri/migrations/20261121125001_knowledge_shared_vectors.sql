-- Identical short rows can share a vector while retaining separate
-- canonical chunks. Existing per-row vectors remain readable without a rebuild.
CREATE TABLE knowledge_vector_aliases (
    chunk_id TEXT PRIMARY KEY REFERENCES knowledge_chunks(id) ON DELETE CASCADE,
    vector_chunk_id TEXT NOT NULL REFERENCES knowledge_vectors(chunk_id) ON DELETE CASCADE,
    CHECK(chunk_id <> vector_chunk_id)
);
CREATE INDEX knowledge_vector_aliases_owner ON knowledge_vector_aliases(vector_chunk_id);
