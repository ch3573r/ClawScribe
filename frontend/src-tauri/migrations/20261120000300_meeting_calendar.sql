ALTER TABLE meetings ADD COLUMN calendar_link TEXT
    CHECK (calendar_link IS NULL OR json_valid(calendar_link));

CREATE TABLE calendar_series_templates (
    source_id TEXT NOT NULL,
    series_id TEXT NOT NULL,
    template_id TEXT NOT NULL,
    PRIMARY KEY (source_id, series_id)
);
