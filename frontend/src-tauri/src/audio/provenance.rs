/// Facts known by the native audio path at the time the meeting is saved.
#[derive(Clone, Copy, Default)]
pub(crate) struct RecordingProvenance {
    pub source: Option<&'static str>,
    pub duration_seconds: Option<f64>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl RecordingProvenance {
    pub fn recording(duration_seconds: Option<f64>, marker_created_at: Option<&str>) -> Self {
        Self {
            source: Some("recording"),
            duration_seconds: measured_duration(duration_seconds),
            started_at: marker_created_at
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|started| started.to_utc()),
        }
    }

    pub fn import(duration_seconds: Option<f64>) -> Self {
        Self {
            source: Some("import"),
            duration_seconds: measured_duration(duration_seconds),
            started_at: None,
        }
    }
}

pub(super) fn measured_duration(seconds: Option<f64>) -> Option<f64> {
    seconds.filter(|seconds| seconds.is_finite() && *seconds > 0.0)
}
