use super::DiagnosticSnapshot;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Runtime};

const MAX_REPORT_BYTES: usize = 4096;
const COLLECTION_CHECKS: &[&str] = &[
    "acceleration",
    "system_resources",
    "transcription_config",
    "parakeet_directml",
    "audio_devices",
    "codex",
    "recording_folder",
];

/// Keep only the support report from the last completed snapshot. Saving it
/// never reruns diagnostics, contacts a provider, or reads logs.
#[derive(Default)]
pub(crate) struct DiagnosticsReportState(Mutex<Option<Value>>);

impl DiagnosticsReportState {
    pub(super) fn remember(&self, snapshot: &DiagnosticSnapshot) {
        let report = build_report(snapshot);
        *self.0.lock().unwrap_or_else(|error| error.into_inner()) = Some(report);
    }
}

fn allowlisted(value: &str, choices: &'static [&'static str]) -> Option<&'static str> {
    choices.iter().copied().find(|choice| *choice == value)
}

fn build_report(snapshot: &DiagnosticSnapshot) -> Value {
    let transcription = &snapshot.transcription;
    let engines: Vec<_> = ["localWhisper", "parakeet", "nemotron"]
        .into_iter()
        .filter_map(|provider| {
            let engine = transcription
                .engines
                .iter()
                .find(|engine| engine.provider == provider)?;
            Some(json!({
                "provider": provider,
                "loaded": engine.loaded,
                "selectedModelPresent": engine.selected_model_present,
                "downloadedModels": engine.downloaded_models,
                "totalModels": engine.total_models,
                "checkFailed": engine.error.is_some(),
            }))
        })
        .collect();
    let collection_failures: Vec<_> = COLLECTION_CHECKS
        .iter()
        .copied()
        .filter(|check| {
            snapshot.errors.iter().any(|error| {
                error
                    .split_once(": ")
                    .is_some_and(|(source, _)| source == *check)
            })
        })
        .collect();
    let diarization = &snapshot.diarization;

    // Build a new object from stable labels, counts and flags. Never serialize
    // a snapshot section wholesale or copy arbitrary strings into this object.
    json!({
        "schemaVersion": 1,
        "app": {
            "version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "buildProfile": if cfg!(debug_assertions) { "debug" } else { "release" },
        },
        "system": snapshot.system_resources.as_ref().map(|system| json!({
            "cpuCores": system.cpu_cores,
            "totalMemoryMb": system.total_memory_mb,
        })),
        "whisperAcceleration": snapshot.acceleration.as_ref().map(|status| json!({
            "compiledBackend": allowlisted(&status.compiled_backend, &["Cpu", "Vulkan", "Cuda", "Metal", "HipBlas"]),
            "runtimeDetectedGpu": allowlisted(&status.runtime_detected_gpu, &["None", "Vulkan", "Cuda", "Metal", "OpenCL"]),
            "useGpu": status.use_gpu,
            "gpuAvailableButUnused": status.gpu_available_but_unused,
            "vulkanRuntimeMissing": status.vulkan_runtime_missing,
        })),
        "transcription": {
            "selectedProvider": allowlisted(&transcription.selected_provider, &[
                "localWhisper", "parakeet", "nemotron", "cloud-whisper", "mai-transcribe",
            ]),
            "parakeetDirectmlEnabled": transcription.parakeet_directml_enabled,
            "engines": engines,
        },
        "audio": {
            "inputDevices": snapshot.audio.input_devices,
            "outputDevices": snapshot.audio.output_devices,
            "checkFailed": snapshot.audio.error.is_some(),
        },
        "speakerSeparation": {
            "preferredProvider": allowlisted(&diarization.preferred_provider, &["cpu", "directml"]),
            "directmlCompiled": diarization.directml_compiled,
            "directmlSlow": diarization.directml_slow,
            "directmlUnavailable": diarization.directml_unavailable,
            "runtimeFilesPresent": diarization.runtime_dlls.iter().filter(|dll| dll.present).count(),
            "runtimeFilesExpected": diarization.runtime_dlls.len(),
        },
        "collectionFailures": collection_failures,
    })
}

pub(super) async fn save_report<R: Runtime>(
    app: AppHandle<R>,
    state: &DiagnosticsReportState,
) -> Result<bool, String> {
    let report = state
        .0
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .ok_or("Refresh diagnostics before saving a report.")?;
    // The picker and writer run in one command, so no path or report contents
    // cross the webview and no separate save grant is needed.
    let chosen = crate::file_grants::pick_save(
        app,
        "JSON report",
        "json",
        "ClawScribe-diagnostics.json".into(),
    )
    .await?;
    tokio::task::spawn_blocking(move || write_report(chosen, &report))
        .await
        .map_err(|_| "Could not save the diagnostics report.".to_string())?
}

fn write_report(chosen: Option<PathBuf>, report: &Value) -> Result<bool, String> {
    let Some(path) = chosen else {
        return Ok(false);
    };
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        return Err("Choose a filename ending in .json.".into());
    }
    crate::file_grants::save_target_parent_exists(&path)?;
    let mut bytes = serde_json::to_vec_pretty(report)
        .map_err(|_| "Could not prepare the diagnostics report.".to_string())?;
    bytes.push(b'\n');
    if bytes.len() > MAX_REPORT_BYTES {
        return Err("The diagnostics report exceeded its size limit.".into());
    }
    crate::library::atomic_write(&path, &bytes)?;
    Ok(true)
}

#[cfg(test)]
mod tests;
