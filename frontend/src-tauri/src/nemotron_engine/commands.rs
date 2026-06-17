// nemotron_engine/commands.rs
//
// Tauri commands for the Nemotron engine: init, list, download, cancel,
// validate-and-load, and the DirectML toggle. Mirrors parakeet_engine::commands.

use crate::nemotron_engine::nemotron_engine::{
    ModelInfo, NemotronEngine, NEMOTRON_MODEL, USE_NEMOTRON_DIRECTML,
};
use crate::parakeet_engine::ModelStatus;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tauri::{command, AppHandle, Emitter, Manager, Runtime};

pub static NEMOTRON_ENGINE: Mutex<Option<Arc<NemotronEngine>>> = Mutex::new(None);

#[command]
pub async fn nemotron_init<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    {
        let guard = NEMOTRON_ENGINE.lock().unwrap();
        if guard.is_some() {
            return Ok(());
        }
    }
    // Use the same models root as the other engines (app_data_dir/models).
    let models_dir = app
        .path()
        .app_data_dir()
        .map(|d| d.join("models"))
        .ok();
    let engine = NemotronEngine::new_with_models_dir(models_dir)
        .map_err(|e| format!("Failed to initialize Nemotron engine: {}", e))?;
    *NEMOTRON_ENGINE.lock().unwrap() = Some(Arc::new(engine));
    Ok(())
}

fn engine() -> Option<Arc<NemotronEngine>> {
    NEMOTRON_ENGINE.lock().unwrap().as_ref().cloned()
}

#[command]
pub async fn nemotron_get_available_models<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<ModelInfo>, String> {
    // Lazily initialize the engine so callers (e.g. the import/retranscribe model
    // picker) can list Nemotron even when it isn't the selected provider yet.
    nemotron_init(app).await?;
    let engine = engine().ok_or_else(|| "Nemotron engine not initialized".to_string())?;
    engine
        .discover_models()
        .await
        .map_err(|e| format!("Failed to discover Nemotron models: {}", e))
}

#[command]
pub async fn nemotron_download_model<R: Runtime>(
    app: AppHandle<R>,
    model_name: String,
) -> Result<(), String> {
    let engine = engine().ok_or_else(|| "Nemotron engine not initialized".to_string())?;
    let _ = engine.discover_models().await;

    let app_cb = app.clone();
    let name_cb = model_name.clone();
    let cb = Box::new(move |progress: crate::parakeet_engine::DownloadProgress| {
        let _ = app_cb.emit(
            "nemotron-model-download-progress",
            serde_json::json!({
                "modelName": name_cb,
                "progress": progress.percent,
                "downloaded_mb": progress.downloaded_mb,
                "total_mb": progress.total_mb,
                "speed_mbps": progress.speed_mbps,
                "status": if progress.percent == 100 { "completed" } else { "downloading" }
            }),
        );
    });

    match engine.download_model_detailed(&model_name, Some(cb)).await {
        Ok(()) => {
            let _ = app.emit(
                "nemotron-model-download-complete",
                serde_json::json!({ "modelName": model_name }),
            );
            Ok(())
        }
        Err(e) => {
            let _ = app.emit(
                "nemotron-model-download-error",
                serde_json::json!({ "modelName": model_name, "error": e.to_string() }),
            );
            Err(format!("Download failed: {}", e))
        }
    }
}

#[command]
pub async fn nemotron_cancel_download(model_name: String) -> Result<(), String> {
    if let Some(engine) = engine() {
        engine.cancel_download(&model_name).await;
    }
    Ok(())
}

/// Ensure a Nemotron model is downloaded and loaded for the configured engine.
pub async fn nemotron_validate_model_ready_with_config<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<String, String> {
    let engine = engine().ok_or_else(|| "Nemotron engine not initialized".to_string())?;

    if engine.is_model_loaded().await {
        if let Some(current) = engine.get_current_model().await {
            return Ok(current);
        }
    }

    // Resolve the configured model name (single v1 model otherwise).
    let configured = match crate::api::api::api_get_transcript_config(app.clone(), app.state(), None)
        .await
    {
        Ok(Some(cfg)) if cfg.provider == "nemotron" && !cfg.model.is_empty() => cfg.model,
        _ => NEMOTRON_MODEL.to_string(),
    };

    let models = engine
        .discover_models()
        .await
        .map_err(|e| format!("Failed to discover Nemotron models: {}", e))?;

    let available = models
        .iter()
        .any(|m| m.name == configured && matches!(m.status, ModelStatus::Available));
    if !available {
        return Err(format!(
            "Nemotron model '{}' is not downloaded. Download it from Settings to enable streaming transcription.",
            configured
        ));
    }

    engine
        .load_model(&configured)
        .await
        .map_err(|e| format!("Failed to load Nemotron model '{}': {}", configured, e))?;
    Ok(configured)
}

#[command]
pub async fn set_nemotron_use_directml(enabled: bool) -> Result<(), String> {
    USE_NEMOTRON_DIRECTML.store(enabled, Ordering::Relaxed);
    if let Some(engine) = engine() {
        engine.unload_model().await;
    }
    log::info!("Nemotron DirectML set to {enabled}; unloaded model so it reloads with the change");
    Ok(())
}

#[command]
pub async fn get_nemotron_use_directml() -> Result<bool, String> {
    Ok(USE_NEMOTRON_DIRECTML.load(Ordering::Relaxed))
}
