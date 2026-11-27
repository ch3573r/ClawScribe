CREATE TABLE speaker_profile_preferences (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    auto_apply INTEGER NOT NULL CHECK (auto_apply IN (0, 1) AND auto_apply <= enabled),
    generation INTEGER NOT NULL
);

INSERT INTO speaker_profile_preferences (id, enabled, auto_apply, generation) VALUES (1, 0, 0, 0);

CREATE TABLE speaker_profiles (
    name_key TEXT NOT NULL,
    model TEXT NOT NULL,
    name TEXT NOT NULL,
    embedding BLOB NOT NULL,
    PRIMARY KEY (name_key, model)
);

CREATE TABLE speaker_profile_suggestions (
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    speaker TEXT NOT NULL,
    fingerprint BLOB NOT NULL,
    name_key TEXT NOT NULL,
    model TEXT NOT NULL,
    PRIMARY KEY (meeting_id, speaker),
    FOREIGN KEY (name_key, model) REFERENCES speaker_profiles(name_key, model) ON DELETE CASCADE
);

CREATE INDEX speaker_profile_suggestions_profile ON speaker_profile_suggestions (name_key, model);
