use anyhow::Result;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_store::StoreExt;

use crate::audio::recording_mode::{self, RecordingMode};
use crate::database::repositories::setting::SettingsRepository;
use crate::state::AppState;
use crate::summary::summary_engine::{self, ModelManagerState};
use crate::summary::{automatic, llm_client::LLMProvider};

static SETUP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OnboardingStatus {
    pub version: String,
    pub completed: bool,
    pub current_step: u8,
    pub model_status: ModelStatus,
    pub last_updated: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ModelStatus {
    pub parakeet: String, // Legacy status or a downloaded/deferred setup snapshot
    pub summary: String,  // Generic field for summary model (Qwen 3.5 or legacy Gemma variants)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_summary_model: Option<String>,
}

impl Default for OnboardingStatus {
    fn default() -> Self {
        Self {
            version: "1.0".to_string(),
            completed: false,
            current_step: 1,
            model_status: ModelStatus {
                parakeet: "not_downloaded".to_string(),
                summary: "not_downloaded".to_string(), // Changed from gemma
                selected_summary_model: None,
            },
            last_updated: chrono::Utc::now().to_rfc3339(),
        }
    }
}

pub(crate) fn is_complete<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.store("onboarding-status.json")
        .ok()
        .and_then(|store| store.get("status"))
        .and_then(|value| serde_json::from_value::<OnboardingStatus>(value).ok())
        .is_some_and(|status| status.completed)
}

/// Load onboarding status from store
pub async fn load_onboarding_status<R: Runtime>(app: &AppHandle<R>) -> Result<OnboardingStatus> {
    // Try to load from Tauri store
    let store = match app.store("onboarding-status.json") {
        Ok(store) => store,
        Err(e) => {
            warn!("Failed to access onboarding store: {}, using defaults", e);
            return Ok(OnboardingStatus::default());
        }
    };

    // Try to get the status from store
    let status = if let Some(value) = store.get("status") {
        match serde_json::from_value::<OnboardingStatus>(value.clone()) {
            Ok(s) => {
                info!(
                    "Loaded onboarding status from store - Step: {}, Completed: {}",
                    s.current_step, s.completed
                );
                s
            }
            Err(e) => {
                warn!(
                    "Failed to deserialize onboarding status: {}, using defaults",
                    e
                );
                OnboardingStatus::default()
            }
        }
    } else {
        info!("No stored onboarding status found, using defaults");
        OnboardingStatus::default()
    };

    Ok(status)
}

/// Save onboarding status to store
fn save_onboarding_status<R: Runtime>(app: &AppHandle<R>, status: &OnboardingStatus) -> Result<()> {
    info!(
        "Saving onboarding status: step={}, completed={}",
        status.current_step, status.completed
    );

    // Get or create store
    let store = app
        .store("onboarding-status.json")
        .map_err(|e| anyhow::anyhow!("Failed to access onboarding store: {}", e))?;

    // Update last_updated timestamp
    let mut status = status.clone();
    status.last_updated = chrono::Utc::now().to_rfc3339();

    // Serialize status to JSON value
    let status_value = serde_json::to_value(&status)
        .map_err(|e| anyhow::anyhow!("Failed to serialize onboarding status: {}", e))?;

    let previous = store.get("status");
    store.set("status", status_value);

    if let Err(error) = store.save() {
        if let Some(previous) = previous {
            store.set("status", previous);
        } else {
            store.delete("status");
        }
        return Err(anyhow::anyhow!(
            "Failed to save onboarding store to disk: {error}"
        ));
    }

    info!("Successfully persisted onboarding status to disk");
    Ok(())
}

/// Reset onboarding status (delete from store)
pub async fn reset_onboarding_status<R: Runtime>(app: &AppHandle<R>) -> Result<()> {
    let _setup = SETUP_LOCK.lock().await;
    info!("Resetting onboarding status");

    let store = app
        .store("onboarding-status.json")
        .map_err(|e| anyhow::anyhow!("Failed to access onboarding store: {}", e))?;

    // Clear the status key
    store.delete("status");

    // Persist deletion to disk
    store
        .save()
        .map_err(|e| anyhow::anyhow!("Failed to save onboarding store after reset: {}", e))?;

    info!("Successfully reset onboarding status");
    Ok(())
}

/// Tauri commands for onboarding status
#[tauri::command]
pub async fn get_onboarding_status<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Option<OnboardingStatus>, String> {
    let status = load_onboarding_status(&app)
        .await
        .map_err(|e| format!("Failed to load onboarding status: {}", e))?;

    // Return None if it's the default (never saved before)
    // Check if we have any saved data by seeing if the store has the key
    let store = app
        .store("onboarding-status.json")
        .map_err(|e| format!("Failed to access store: {}", e))?;

    if store.get("status").is_none() {
        Ok(None)
    } else {
        Ok(Some(status))
    }
}

#[tauri::command]
pub async fn save_onboarding_status_cmd<R: Runtime>(
    app: AppHandle<R>,
    status: OnboardingStatus,
) -> Result<(), String> {
    let _setup = SETUP_LOCK.lock().await;
    save_onboarding_status(&app, &status)
        .map_err(|e| format!("Failed to save onboarding status: {}", e))
}

#[tauri::command]
pub async fn complete_onboarding<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    mode: RecordingMode,
    local_notes_model: Option<String>,
    automatic_notes: Option<bool>,
) -> Result<(), String> {
    let _setup = SETUP_LOCK.lock().await;
    let mut status = load_onboarding_status(&app)
        .await
        .map_err(|_| "Could not read setup status")?;
    if status.completed {
        return Ok(());
    }

    // Audio-only setup has no model dependency. Completed setup is durable;
    // live model availability after setup is always read from the engine.
    status.model_status.parakeet = if mode == RecordingMode::Live {
        crate::parakeet_engine::commands::parakeet_init().await?;
        if !crate::parakeet_engine::commands::parakeet_has_available_models().await? {
            return Err("No speech model is available".into());
        }
        "downloaded".into()
    } else {
        "deferred".into()
    };

    status.model_status.summary = "deferred".into();
    status.model_status.selected_summary_model = None;
    if let Some(model) = local_notes_model {
        if summary_engine::get_model_by_name(&model).is_none() {
            return Err("Unknown local notes model".into());
        }
        let ready = summary_engine::builtin_ai_is_model_ready(
            app.clone(),
            app.state::<ModelManagerState>(),
            model.clone(),
            Some(true),
        )
        .await
        .unwrap_or(false);
        if ready {
            status.model_status.summary = "downloaded".into();
            status.model_status.selected_summary_model = Some(model);
        }
    }
    let model = status.model_status.selected_summary_model.as_deref();
    if automatic_notes.is_some() && model.is_none() {
        return Err("The local notes model is unavailable.".into());
    }
    status.completed = true;
    status.current_step = 4;

    let _route = automatic::ROUTE_CHANGE_LOCK.lock().await;
    let _mode = recording_mode::CHANGE_LOCK.lock().await;
    let pool = state.db_manager.pool();
    let previous_model = if model.is_some() {
        SettingsRepository::get_model_config(pool)
            .await
            .map_err(|_| "Could not read the saved summary model preference")?
    } else {
        None
    };
    let previous_mode = recording_mode::replace(&app, Some(serde_json::json!(mode)))?;
    let mut changed_model = false;
    let result = async {
        if let Some(model) = model.filter(|model| {
            previous_model.as_ref().is_none_or(|old| {
                LLMProvider::from_str(&old.provider) != Ok(LLMProvider::BuiltInAI)
                    || old.model.trim() != *model
            })
        }) {
            automatic::invalidate_external_review(&app)?;
            SettingsRepository::save_model_config(
                pool,
                "builtin-ai",
                model,
                previous_model
                    .as_ref()
                    .map_or("large-v3", |old| old.whisper_model.as_str()),
                previous_model
                    .as_ref()
                    .and_then(|old| old.ollama_endpoint.as_deref()),
            )
            .await
            .map_err(|_| "Could not save the local notes model preference")?;
            changed_model = true;
        }
        automatic::finish_builtin_onboarding(&app, automatic_notes, || {
            save_onboarding_status(&app, &status)
                .map_err(|_| "Could not save completed setup".into())
        })
    }
    .await;
    if let Err(error) = result {
        if recording_mode::replace(&app, previous_mode).is_err() {
            warn!("Could not restore the recording mode after setup failed");
        }
        if let Some(previous) = previous_model.filter(|_| changed_model) {
            if SettingsRepository::save_model_config(
                pool,
                &previous.provider,
                &previous.model,
                &previous.whisper_model,
                previous.ollama_endpoint.as_deref(),
            )
            .await
            .is_err()
            {
                warn!("Could not restore the summary model selection after setup failed");
            }
        }
        return Err(error);
    }

    recording_mode::notify_changed(&app, mode);
    crate::tray::update_tray_menu(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onboarding_status_deserializes_without_selected_summary_model() {
        let status: OnboardingStatus = serde_json::from_str(
            r#"{
                "version": "1.0",
                "completed": true,
                "current_step": 4,
                "model_status": {
                    "parakeet": "downloaded",
                    "summary": "downloaded"
                },
                "last_updated": "2026-05-30T00:00:00Z"
            }"#,
        )
        .expect("old onboarding status should remain compatible");

        assert_eq!(status.model_status.selected_summary_model, None);
    }
}
