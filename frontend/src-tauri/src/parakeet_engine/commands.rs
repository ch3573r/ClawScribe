use crate::parakeet_engine::{DownloadProgress, ModelInfo, ParakeetEngine};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::{command, AppHandle, Emitter, Manager, Runtime};

// Global parakeet engine
pub static PARAKEET_ENGINE: Mutex<Option<Arc<ParakeetEngine>>> = Mutex::new(None);

// Global models directory path (set during app initialization)
static MODELS_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Initialize the models directory path using app_data_dir
/// This should be called during app setup before parakeet_init
pub fn set_models_directory<R: Runtime>(app: &AppHandle<R>) {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .expect("Failed to get app data dir");

    let models_dir = app_data_dir.join("models");

    // Create directory if it doesn't exist
    if !models_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&models_dir) {
            log::error!("Failed to create models directory: {}", e);
            return;
        }
    }

    log::info!("Parakeet models directory set to: {}", models_dir.display());

    let mut guard = MODELS_DIR.lock().unwrap();
    *guard = Some(models_dir);
}

/// Get the configured models directory
fn get_models_directory() -> Option<PathBuf> {
    MODELS_DIR.lock().unwrap().clone()
}

#[command]
pub async fn parakeet_init() -> Result<(), String> {
    let mut guard = PARAKEET_ENGINE.lock().unwrap();
    if guard.is_some() {
        return Ok(());
    }

    let models_dir = get_models_directory();
    let engine = ParakeetEngine::new_with_models_dir(models_dir)
        .map_err(|e| format!("Failed to initialize Parakeet engine: {}", e))?;
    *guard = Some(Arc::new(engine));
    Ok(())
}

#[command]
pub async fn parakeet_get_available_models() -> Result<Vec<ModelInfo>, String> {
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        engine
            .discover_models()
            .await
            .map_err(|e| format!("Failed to discover Parakeet models: {}", e))
    } else {
        Err("Parakeet engine not initialized".to_string())
    }
}

#[command]
pub async fn parakeet_load_model<R: Runtime>(
    app_handle: AppHandle<R>,
    model_name: String,
) -> Result<(), String> {
    let _job = crate::audio::inference::claim_job()?;
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        // Emit model loading started event
        if let Err(e) = app_handle.emit(
            "parakeet-model-loading-started",
            serde_json::json!({
                "modelName": model_name
            }),
        ) {
            log::error!("Failed to emit parakeet-model-loading-started event: {}", e);
        }

        let result = engine
            .load_model(&model_name)
            .await
            .map_err(|e| format!("Failed to load Parakeet model: {}", e));

        // Emit model loading completed/failed event
        if result.is_ok() {
            if let Err(e) = app_handle.emit(
                "parakeet-model-loading-completed",
                serde_json::json!({
                    "modelName": model_name
                }),
            ) {
                log::error!(
                    "Failed to emit parakeet-model-loading-completed event: {}",
                    e
                );
            }
        } else if let Err(ref error) = result {
            if let Err(e) = app_handle.emit(
                "parakeet-model-loading-failed",
                serde_json::json!({
                    "modelName": model_name,
                    "error": error
                }),
            ) {
                log::error!("Failed to emit parakeet-model-loading-failed event: {}", e);
            }
        }

        result
    } else {
        Err("Parakeet engine not initialized".to_string())
    }
}

#[command]
pub async fn parakeet_get_current_model() -> Result<Option<String>, String> {
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        Ok(engine.get_current_model().await)
    } else {
        Err("Parakeet engine not initialized".to_string())
    }
}

#[command]
pub async fn parakeet_is_model_loaded() -> Result<bool, String> {
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        Ok(engine.is_model_loaded().await)
    } else {
        Err("Parakeet engine not initialized".to_string())
    }
}

#[command]
pub async fn parakeet_has_available_models() -> Result<bool, String> {
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        let models = engine
            .discover_models()
            .await
            .map_err(|e| format!("Failed to discover Parakeet models: {}", e))?;

        // Check if at least one model is available
        let available_models: Vec<_> = models
            .iter()
            .filter(|model| matches!(model.status, crate::parakeet_engine::ModelStatus::Available))
            .collect();

        Ok(!available_models.is_empty())
    } else {
        Ok(false)
    }
}

#[command]
pub async fn parakeet_validate_model_ready() -> Result<String, String> {
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        // Check if a model is currently loaded
        if engine.is_model_loaded().await {
            if let Some(current_model) = engine.get_current_model().await {
                return Ok(current_model);
            }
        }

        // No model loaded, check if any models are available to load
        let models = engine
            .discover_models()
            .await
            .map_err(|e| format!("Failed to discover Parakeet models: {}", e))?;

        let available_models: Vec<_> = models
            .iter()
            .filter(|model| matches!(model.status, crate::parakeet_engine::ModelStatus::Available))
            .collect();

        if available_models.is_empty() {
            return Err(
                "No Parakeet models are available. Please download a model to enable fast transcription."
                    .to_string(),
            );
        }

        // Try to load the first available model (prefer int8 for speed)
        let first_model = available_models
            .iter()
            .find(|m| m.quantization == crate::parakeet_engine::QuantizationType::Int8)
            .or_else(|| available_models.first())
            .unwrap();

        engine
            .load_model(&first_model.name)
            .await
            .map_err(|e| format!("Failed to load Parakeet model {}: {}", first_model.name, e))?;

        Ok(first_model.name.clone())
    } else {
        Err("Parakeet engine not initialized".to_string())
    }
}

/// Load the chosen model, or another available model from the same engine.
pub(crate) async fn parakeet_validate_model_ready_for(
    configured_model: Option<String>,
) -> Result<String, crate::audio::transcription::engine::LiveModelFailure> {
    use crate::audio::transcription::engine::{
        check_model_files, LiveModelFailure, ModelFileState,
    };
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        // Check available models
        let models = engine
            .discover_models()
            .await
            .map_err(|_| LiveModelFailure::LoadFailed)?;

        let available_models: Vec<_> = models
            .iter()
            .filter(|model| matches!(model.status, crate::parakeet_engine::ModelStatus::Available))
            .collect();

        check_model_files(
            models
                .iter()
                .map(|model| ModelFileState::from(&model.status)),
        )?;

        // Try to load user's configured model if specified
        let model_name = if let Some(configured_model) = configured_model {
            // Check if configured model is available
            if available_models.iter().any(|m| m.name == configured_model) {
                log::info!(
                    "Loading user's configured Parakeet model: {}",
                    configured_model
                );
                configured_model
            } else {
                log::warn!(
                    "Configured Parakeet model '{}' not found, falling back to first available int8 model",
                    configured_model
                );
                // Prefer int8 quantization for best speed/quality tradeoff
                available_models
                    .iter()
                    .find(|m| m.quantization == crate::parakeet_engine::QuantizationType::Int8)
                    .or_else(|| available_models.first())
                    .unwrap()
                    .name
                    .clone()
            }
        } else {
            // No configured model, prefer int8 for best speed/quality balance
            log::info!("No configured model, loading first available int8 Parakeet model");
            available_models
                .iter()
                .find(|m| m.quantization == crate::parakeet_engine::QuantizationType::Int8)
                .or_else(|| available_models.first())
                .unwrap()
                .name
                .clone()
        };

        engine.load_model(&model_name).await.map_err(|error| {
            log::warn!("Live model load failed: {error}");
            LiveModelFailure::LoadFailed
        })?;

        Ok(model_name)
    } else {
        Err(LiveModelFailure::LoadFailed)
    }
}

#[command]
pub async fn parakeet_transcribe_audio(audio_data: Vec<f32>) -> Result<String, String> {
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        engine
            .transcribe_audio(audio_data)
            .await
            .map_err(|e| format!("Parakeet transcription failed: {}", e))
    } else {
        Err("Parakeet engine not initialized".to_string())
    }
}

#[command]
pub async fn parakeet_get_models_directory() -> Result<String, String> {
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        let path = engine.get_models_directory().await;
        Ok(path.to_string_lossy().to_string())
    } else {
        Err("Parakeet engine not initialized".to_string())
    }
}

#[command]
pub async fn parakeet_download_model<R: Runtime>(
    app_handle: AppHandle<R>,
    model_name: String,
) -> Result<(), String> {
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        // Create progress callback that emits detailed events
        let app_handle_clone = app_handle.clone();
        let model_name_clone = model_name.clone();

        let progress_callback = Box::new(move |progress: DownloadProgress| {
            log::info!(
                "Parakeet download progress for {}: {:.1} MB / {:.1} MB ({:.1} MB/s) - {}%",
                model_name_clone,
                progress.downloaded_mb,
                progress.total_mb,
                progress.speed_mbps,
                progress.percent
            );

            // Emit download progress event with detailed info
            if let Err(e) = app_handle_clone.emit(
                "parakeet-model-download-progress",
                serde_json::json!({
                    "modelName": model_name_clone,
                    "progress": progress.percent,
                    "downloaded_bytes": progress.downloaded_bytes,
                    "total_bytes": progress.total_bytes,
                    "downloaded_mb": progress.downloaded_mb,
                    "total_mb": progress.total_mb,
                    "speed_mbps": progress.speed_mbps,
                    "status": if progress.percent == 100 { "completed" } else { "downloading" }
                }),
            ) {
                log::error!("Failed to emit parakeet download progress event: {}", e);
            }
        });

        // Ensure models are discovered before downloading
        // This populates available_models so we don't get "Model not found" error
        if let Err(e) = engine.discover_models().await {
            log::warn!("Failed to discover models before download: {}", e);
            // Continue anyway, maybe it will work if the model is already known
        }

        let result = engine
            .download_model_detailed(&model_name, Some(progress_callback))
            .await;

        match result {
            Ok(()) => {
                // Emit completion event
                if let Err(e) = app_handle.emit(
                    "parakeet-model-download-complete",
                    serde_json::json!({
                        "modelName": model_name
                    }),
                ) {
                    log::error!("Failed to emit parakeet download complete event: {}", e);
                }

                // Update tray menu to reflect model is now available
                log::info!("Parakeet model download complete - updating tray menu");
                crate::tray::update_tray_menu(&app_handle);

                Ok(())
            }
            // The cancellation command owns the UI event. An old cancelled
            // invocation must not overwrite the state of its replacement.
            Err(e) if e.is::<crate::model_download::Cancelled>() => Ok(()),
            Err(e) => {
                // Emit error event
                if let Err(emit_e) = app_handle.emit(
                    "parakeet-model-download-error",
                    serde_json::json!({
                        "modelName": model_name,
                        "error": e.to_string()
                    }),
                ) {
                    log::error!("Failed to emit parakeet download error event: {}", emit_e);
                }
                Err(format!("Failed to download Parakeet model: {}", e))
            }
        }
    } else {
        Err("Parakeet engine not initialized".to_string())
    }
}

#[command]
pub async fn parakeet_cancel_download<R: Runtime>(
    app_handle: AppHandle<R>,
    model_name: String,
) -> Result<(), String> {
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        engine
            .cancel_download(&model_name)
            .await
            .map_err(|e| format!("Failed to cancel Parakeet download: {}", e))?;

        // Emit cancellation event to update UI (global toast and component state)
        let _ = app_handle.emit(
            "parakeet-model-download-progress",
            serde_json::json!({
                "modelName": model_name,
                "progress": 0,
                "status": "cancelled"
            }),
        );

        log::info!("Parakeet download cancelled: {}", model_name);
        Ok(())
    } else {
        Err("Parakeet engine not initialized".to_string())
    }
}

#[command]
pub async fn parakeet_retry_download<R: Runtime>(
    app_handle: AppHandle<R>,
    model_name: String,
) -> Result<(), String> {
    log::info!("Retrying download for: {}", model_name);

    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        // Cancellation waits for the previous worker to close its files before
        // the next worker may reserve this model. Completed artifacts survive.
        engine
            .cancel_download(&model_name)
            .await
            .map_err(|e| e.to_string())?;
        engine.discover_models().await.map_err(|e| e.to_string())?;

        // Call regular download (emits events)
        parakeet_download_model(app_handle, model_name).await
    } else {
        Err("Parakeet engine not initialized".to_string())
    }
}

#[command]
pub async fn parakeet_delete_corrupted_model(model_name: String) -> Result<String, String> {
    let _job = crate::audio::inference::claim_job()?;
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        engine
            .delete_model(&model_name)
            .await
            .map_err(|e| format!("Failed to delete Parakeet model: {}", e))
    } else {
        Err("Parakeet engine not initialized".to_string())
    }
}

/// Open the Parakeet models folder in the system file explorer
#[command]
pub async fn open_parakeet_models_folder() -> Result<(), String> {
    let models_dir = get_models_directory()
        .ok_or_else(|| "Parakeet models directory not initialized".to_string())?
        .join("parakeet");

    // Ensure directory exists before trying to open it
    if !models_dir.exists() {
        std::fs::create_dir_all(&models_dir)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    let folder_path = models_dir.to_string_lossy().to_string();

    #[cfg(target_os = "windows")]
    {
        crate::local_path::open_in_explorer(std::path::Path::new(&folder_path))?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    log::info!("Opened Parakeet models folder: {}", folder_path);
    Ok(())
}

/// Beta (opt-in): route Parakeet inference through the DirectML (Windows GPU)
/// execution provider. Unloads any loaded model so the change applies on the
/// next load. No effect unless the build includes the `directml` feature.
#[command]
pub async fn set_parakeet_use_directml<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    let _change = crate::behavior_settings::CHANGE_LOCK.lock().await;
    apply_parakeet_directml(&app, enabled).await
}

/// The change itself, shared by the command and the one-time hand-over of the
/// webview's old values: it refuses while another job holds the slot, stores the
/// value first, and unloads the model so it reloads with the change. The caller
/// holds `behavior_settings::CHANGE_LOCK`.
pub(crate) async fn apply_parakeet_directml<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    let app = app.clone();
    let _job = crate::audio::inference::claim_job()?;
    // Stored first: when the save fails the running value is unchanged.
    tauri::async_runtime::spawn_blocking(move || {
        crate::behavior_settings::save_parakeet_directml(&app, enabled)
    })
    .await
    .map_err(|_| "Could not save the setting".to_string())??;
    crate::parakeet_engine::parakeet_engine::USE_PARAKEET_DIRECTML
        .store(enabled, std::sync::atomic::Ordering::Relaxed);

    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };
    if let Some(engine) = engine {
        engine.unload_model().await;
    }
    log::info!("Parakeet DirectML set to {enabled}; unloaded model so it reloads with the change");
    Ok(())
}

/// Unloads a loaded Parakeet model so the next load uses changed settings, when
/// no other job holds the slot. Otherwise the change applies at the next load.
pub(crate) async fn unload_for_new_settings() {
    let Ok(_job) = crate::audio::inference::claim_job() else {
        log::warn!("Parakeet settings changed while a job was running; they apply at the next model load");
        return;
    };
    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };
    if let Some(engine) = engine {
        engine.unload_model().await;
    }
}

#[command]
pub async fn get_parakeet_use_directml() -> Result<bool, String> {
    Ok(
        crate::parakeet_engine::parakeet_engine::USE_PARAKEET_DIRECTML
            .load(std::sync::atomic::Ordering::Relaxed),
    )
}

#[command]
pub async fn get_parakeet_directml_session_disabled() -> Result<bool, String> {
    Ok(crate::parakeet_engine::worker::directml_session_disabled())
}

/// Beta opt-in: isolate Parakeet in a supervised process on the next load.
#[command]
pub async fn set_parakeet_use_worker<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    let _change = crate::behavior_settings::CHANGE_LOCK.lock().await;
    apply_parakeet_worker(&app, enabled).await
}

/// See `apply_parakeet_directml`.
pub(crate) async fn apply_parakeet_worker<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    let app = app.clone();
    let _job = crate::audio::inference::claim_job()
        .map_err(|error| format!("PARAKEET_WORKER_BUSY: {error}"))?;
    // Stored first: when the save fails the running value is unchanged.
    tauri::async_runtime::spawn_blocking(move || {
        crate::behavior_settings::save_parakeet_worker(&app, enabled)
    })
    .await
    .map_err(|_| "Could not save the setting".to_string())??;
    crate::parakeet_engine::parakeet_engine::USE_PARAKEET_WORKER
        .store(enabled, std::sync::atomic::Ordering::Relaxed);

    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };
    if let Some(engine) = engine {
        engine.unload_model().await;
    }
    log::info!(
        "Parakeet process isolation set to {enabled}; unloaded model so it reloads with the change"
    );
    Ok(())
}

#[command]
pub async fn get_parakeet_use_worker() -> Result<bool, String> {
    Ok(crate::parakeet_engine::parakeet_engine::USE_PARAKEET_WORKER
        .load(std::sync::atomic::Ordering::Relaxed))
}
