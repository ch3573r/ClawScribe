-- Portable per-meeting provenance fields.
-- recording_source: 'recording' (live capture) or 'import' (file import); NULL for
--   older meetings where the source was not persisted.
-- recording_duration_s: measured active recording time or duration of the
--   imported or recovered media. NULL when no measurement is available.
ALTER TABLE meetings ADD COLUMN recording_source TEXT
    CHECK (recording_source IS NULL OR recording_source IN ('recording', 'import'));
ALTER TABLE meetings ADD COLUMN recording_duration_s REAL
    CHECK (recording_duration_s IS NULL OR
           (typeof(recording_duration_s) = 'real' AND recording_duration_s > 0
            AND recording_duration_s <= 1.7976931348623157e308));
