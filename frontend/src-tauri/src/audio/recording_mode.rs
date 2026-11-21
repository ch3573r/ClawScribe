use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_store::StoreExt;

pub(crate) static CHANGE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    #[default]
    Live,
    AudioOnly,
}

impl RecordingMode {
    pub fn transcribes(self) -> bool {
        self == Self::Live
    }
    pub fn saves_audio(self, auto_save: bool) -> bool {
        self == Self::AudioOnly || auto_save
    }
}

/// Old recording markers predate the mode field and only supported live capture.
pub(crate) fn marker_transcribes(marker: &serde_json::Value) -> bool {
    match marker.get("recording_mode") {
        None => true,
        Some(value) => value.as_str() == Some("live"),
    }
}

pub fn load<R: Runtime>(app: &AppHandle<R>) -> Result<RecordingMode, String> {
    let store = app
        .store("recording_mode.json")
        .map_err(|_| "Could not load recording mode")?;
    match store.get("mode") {
        None => Ok(RecordingMode::Live),
        Some(value) => serde_json::from_value(value)
            .map_err(|_| "Recording mode is invalid. Select it again in Settings.".into()),
    }
}

#[tauri::command]
pub async fn get_recording_mode<R: Runtime>(app: AppHandle<R>) -> Result<RecordingMode, String> {
    tauri::async_runtime::spawn_blocking(move || load(&app))
        .await
        .map_err(|_| "Could not load recording mode")?
}

#[tauri::command]
pub async fn set_recording_mode<R: Runtime>(
    app: AppHandle<R>,
    mode: RecordingMode,
) -> Result<(), String> {
    let _change = CHANGE_LOCK.lock().await;
    let worker_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        replace(&worker_app, Some(serde_json::json!(mode)))
    })
    .await
    .map_err(|_| "Could not save recording mode")??;
    notify_changed(&app, mode);
    Ok(())
}

pub(crate) fn replace<R: Runtime>(
    app: &AppHandle<R>,
    mode: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let store = app
        .store("recording_mode.json")
        .map_err(|_| "Could not load recording mode")?;
    crate::behavior_settings::replace(store.as_ref(), "mode", mode)
        .map_err(|_| "Could not save recording mode".into())
}

pub(crate) fn notify_changed<R: Runtime>(app: &AppHandle<R>, mode: RecordingMode) {
    let _ = app.emit("recording-mode-changed", mode);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_live_and_legacy_recordings_can_wake_recovered_summaries() {
        assert!(marker_transcribes(&serde_json::json!({})));
        assert!(marker_transcribes(
            &serde_json::json!({"recording_mode": "live"})
        ));
        assert!(!marker_transcribes(
            &serde_json::json!({"recording_mode": "audio_only"})
        ));
        assert!(!marker_transcribes(
            &serde_json::json!({"recording_mode": "unknown"})
        ));
    }
    #[test]
    fn audio_only_always_preserves_audio_and_never_transcribes() {
        assert!(!RecordingMode::AudioOnly.transcribes());
        assert!(RecordingMode::AudioOnly.saves_audio(false));
        assert!(RecordingMode::AudioOnly.saves_audio(true));
        assert!(RecordingMode::default().transcribes());
        assert!(!RecordingMode::Live.saves_audio(false));
        assert!(serde_json::from_str::<RecordingMode>("\"unknown\"").is_err());
        assert_eq!(
            serde_json::from_str::<RecordingMode>("\"audio_only\"").unwrap(),
            RecordingMode::AudioOnly
        );
    }
}
