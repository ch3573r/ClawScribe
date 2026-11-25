ALTER TABLE knowledge_owners ADD COLUMN title TEXT
    CHECK (title IS NULL OR (kind = 'library' AND length(title) BETWEEN 1 AND 100));
