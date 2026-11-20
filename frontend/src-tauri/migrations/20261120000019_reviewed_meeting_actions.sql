ALTER TABLE meetings ADD COLUMN reviewed_actions TEXT
    CHECK (reviewed_actions IS NULL OR (json_valid(reviewed_actions) AND json_type(reviewed_actions) = 'array'));
