/// Facts known by the native audio path at the time the meeting is saved.
#[derive(Clone, Copy, Default)]
pub(crate) struct RecordingProvenance {
    pub source: Option<&'static str>,
    pub duration_seconds: Option<f64>,
}

impl RecordingProvenance {
    pub fn recording(duration_seconds: Option<f64>) -> Self {
        Self {
            source: Some("recording"),
            duration_seconds: measured_duration(duration_seconds),
        }
    }

    pub fn import(duration_seconds: Option<f64>) -> Self {
        Self {
            source: Some("import"),
            duration_seconds: measured_duration(duration_seconds),
        }
    }
}

pub(super) fn measured_duration(seconds: Option<f64>) -> Option<f64> {
    seconds.filter(|seconds| seconds.is_finite() && *seconds > 0.0)
}
