-- Semantic search can page directly through each selected source.
CREATE INDEX knowledge_chunks_source_page ON knowledge_chunks(source_id, revision, generation, id);
