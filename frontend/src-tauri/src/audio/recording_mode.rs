use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_store::StoreExt;

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
    let worker_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let store = worker_app
            .store("recording_mode.json")
            .map_err(|_| "Could not load recording mode")?;
        let previous = store.get("mode");
        store.set("mode", serde_json::json!(mode));
        if store.save().is_err() {
            match previous {
                Some(value) => store.set("mode", value),
                None => {
                    store.delete("mode");
                }
            }
            return Err("Could not save recording mode".into());
        }
        Ok(())
    })
    .await
    .map_err(|_| "Could not save recording mode")??;
    let _ = app.emit("recording-mode-changed", mode);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
