// audio/transcription/engine.rs
//
// TranscriptionEngine enum and model initialization/validation logic.

use super::provider::TranscriptionProvider;
use log::info;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};

// ============================================================================
// TRANSCRIPTION ENGINE ENUM
// ============================================================================

// Transcription engine abstraction to support multiple providers
pub enum TranscriptionEngine {
    Whisper(Arc<crate::whisper_engine::WhisperEngine>), // Direct access (backward compat)
    Parakeet(Arc<crate::parakeet_engine::ParakeetEngine>), // Direct access (backward compat)
    Provider(Arc<dyn TranscriptionProvider>),           // Trait-based (preferred for new code)
}

impl TranscriptionEngine {
    /// Check if the engine has a model loaded
    pub async fn is_model_loaded(&self) -> bool {
        match self {
            Self::Whisper(engine) => engine.is_model_loaded().await,
            Self::Parakeet(engine) => engine.is_model_loaded().await,
            Self::Provider(provider) => provider.is_model_loaded().await,
        }
    }

    /// Get the current model name
    pub async fn get_current_model(&self) -> Option<String> {
        match self {
            Self::Whisper(engine) => engine.get_current_model().await,
            Self::Parakeet(engine) => engine.get_current_model().await,
            Self::Provider(provider) => provider.get_current_model().await,
        }
    }

    /// Get the provider name for logging
    pub fn provider_name(&self) -> &str {
        match self {
            Self::Whisper(_) => "Whisper (direct)",
            Self::Parakeet(_) => "Parakeet (direct)",
            Self::Provider(provider) => provider.provider_name(),
        }
    }
}

// ============================================================================
// MODEL VALIDATION AND INITIALIZATION
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveModelFailure {
    Missing,
    Downloading,
    LoadFailed,
    UnsupportedProvider,
}

impl LiveModelFailure {
    pub(crate) fn code(self) -> String {
        let kind = match self {
            Self::Missing => "missing",
            Self::Downloading => "downloading",
            Self::LoadFailed => "load_failed",
            Self::UnsupportedProvider => "unsupported_provider",
        };
        format!("live_model:{kind}")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ModelFileState {
    Missing,
    Invalid,
    Downloading,
    Available,
}

impl From<&crate::parakeet_engine::ModelStatus> for ModelFileState {
    fn from(status: &crate::parakeet_engine::ModelStatus) -> Self {
        use crate::parakeet_engine::ModelStatus;
        match status {
            ModelStatus::Available => Self::Available,
            ModelStatus::Downloading { .. } => Self::Downloading,
            ModelStatus::Error(_) | ModelStatus::Corrupted { .. } => Self::Invalid,
            ModelStatus::Missing => Self::Missing,
        }
    }
}

impl From<&crate::whisper_engine::ModelStatus> for ModelFileState {
    fn from(status: &crate::whisper_engine::ModelStatus) -> Self {
        use crate::whisper_engine::ModelStatus;
        match status {
            ModelStatus::Available => Self::Available,
            ModelStatus::Downloading { .. } => Self::Downloading,
            ModelStatus::Error(_) | ModelStatus::Corrupted { .. } => Self::Invalid,
            ModelStatus::Missing => Self::Missing,
        }
    }
}

pub(crate) fn check_model_files(
    models: impl Iterator<Item = ModelFileState>,
) -> Result<(), LiveModelFailure> {
    match models.max().unwrap_or(ModelFileState::Missing) {
        ModelFileState::Available => Ok(()),
        ModelFileState::Downloading => Err(LiveModelFailure::Downloading),
        ModelFileState::Invalid => Err(LiveModelFailure::LoadFailed),
        ModelFileState::Missing => Err(LiveModelFailure::Missing),
    }
}

fn effective_live_provider(provider: &str) -> Option<&'static str> {
    match provider {
        "" | "localWhisper" => Some("localWhisper"),
        "parakeet" => Some("parakeet"),
        "nemotron" => Some("nemotron"),
        hosted if super::cloud::is_cloud_provider(Some(hosted)) => Some("parakeet"),
        _ => None,
    }
}

#[cfg(test)]
mod live_readiness_tests {
    use super::*;

    #[test]
    fn hosted_batch_uses_parakeet_for_live_capture() {
        assert_eq!(effective_live_provider("cloud-whisper"), Some("parakeet"));
        assert_eq!(effective_live_provider("mai-transcribe"), Some("parakeet"));
        assert_eq!(
            effective_live_provider("localWhisper"),
            Some("localWhisper")
        );
        assert_eq!(effective_live_provider("nemotron"), Some("nemotron"));
        assert_eq!(effective_live_provider(""), Some("localWhisper"));
        assert_eq!(effective_live_provider("unrecognized"), None);
    }

    #[test]
    fn model_file_states_have_distinct_start_failures() {
        use ModelFileState::*;
        assert_eq!(
            check_model_files([Missing].into_iter()),
            Err(LiveModelFailure::Missing)
        );
        assert_eq!(
            check_model_files([Missing, Invalid].into_iter()),
            Err(LiveModelFailure::LoadFailed)
        );
        assert_eq!(
            check_model_files([Invalid, Downloading].into_iter()),
            Err(LiveModelFailure::Downloading)
        );
        assert_eq!(
            check_model_files([Downloading, Available].into_iter()),
            Ok(())
        );
    }
}

pub(crate) async fn live_transcription_route<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<LiveTranscriptionRoute, LiveModelFailure> {
    let state = app
        .try_state::<crate::state::AppState>()
        .ok_or(LiveModelFailure::LoadFailed)?;
    let config = crate::api::api::transcript_config_view(state.db_manager.pool())
        .await
        .map_err(|_| LiveModelFailure::LoadFailed)?;
    let Some(config) = config else {
        return Ok(LiveTranscriptionRoute {
            provider: Some("parakeet"),
            configured_model: None,
            hosted_batch_provider: None,
        });
    };
    let provider = effective_live_provider(&config.provider);
    let hosted = super::cloud::is_cloud_provider(Some(&config.provider));
    let configured_model = (!hosted && !config.model.is_empty()).then_some(config.model);
    Ok(LiveTranscriptionRoute {
        provider,
        configured_model,
        hosted_batch_provider: hosted.then_some(config.provider),
    })
}

pub(crate) struct LiveTranscriptionRoute {
    pub(crate) provider: Option<&'static str>,
    pub(crate) configured_model: Option<String>,
    hosted_batch_provider: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LiveTranscriptionReadiness {
    provider: Option<&'static str>,
    hosted_batch_provider: Option<String>,
    model_issue: Option<LiveModelFailure>,
}

#[tauri::command]
pub(crate) async fn get_live_transcription_readiness<R: Runtime>(
    app: AppHandle<R>,
    inspect_model: bool,
) -> Result<LiveTranscriptionReadiness, String> {
    let route = live_transcription_route(&app)
        .await
        .map_err(LiveModelFailure::code)?;
    let model_issue = if inspect_model {
        inspect_live_model(&app, &route).await.err()
    } else {
        None
    };
    Ok(LiveTranscriptionReadiness {
        provider: route.provider,
        hosted_batch_provider: route.hosted_batch_provider,
        model_issue,
    })
}

async fn inspect_live_model<R: Runtime>(
    app: &AppHandle<R>,
    route: &LiveTranscriptionRoute,
) -> Result<(), LiveModelFailure> {
    use crate::{nemotron_engine, parakeet_engine, whisper_engine};
    let failed = |_| LiveModelFailure::LoadFailed;
    match route.provider {
        Some("parakeet") => {
            parakeet_engine::commands::parakeet_init()
                .await
                .map_err(failed)?;
            let models = parakeet_engine::commands::parakeet_get_available_models()
                .await
                .map_err(failed)?;
            check_model_files(
                models
                    .iter()
                    .map(|model| ModelFileState::from(&model.status)),
            )
        }
        Some("localWhisper") => {
            whisper_engine::commands::whisper_init()
                .await
                .map_err(failed)?;
            let models = whisper_engine::commands::whisper_get_available_models()
                .await
                .map_err(failed)?;
            check_model_files(
                models
                    .iter()
                    .map(|model| ModelFileState::from(&model.status)),
            )
        }
        Some("nemotron") => {
            let name = route
                .configured_model
                .as_deref()
                .unwrap_or(nemotron_engine::nemotron_engine::NEMOTRON_MODEL);
            let models = nemotron_engine::commands::nemotron_get_available_models(app.clone())
                .await
                .map_err(failed)?;
            check_model_files(
                models
                    .iter()
                    .filter(|model| model.name == name)
                    .map(|model| ModelFileState::from(&model.status)),
            )
        }
        _ => Err(LiveModelFailure::UnsupportedProvider),
    }
}

/// Check and load the effective native engine. Inspection never starts a download.
pub async fn validate_transcription_model_ready<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<(), LiveModelFailure> {
    let route = live_transcription_route(app).await?;
    let failed = |_| LiveModelFailure::LoadFailed;
    let configured = route.configured_model;
    match route.provider {
        Some("localWhisper") => {
            crate::whisper_engine::commands::whisper_init()
                .await
                .map_err(failed)?;
            crate::whisper_engine::commands::whisper_validate_model_ready_for(configured).await?;
        }
        Some("parakeet") => {
            crate::parakeet_engine::commands::parakeet_init()
                .await
                .map_err(failed)?;
            crate::parakeet_engine::commands::parakeet_validate_model_ready_for(configured).await?;
        }
        Some("nemotron") => {
            crate::nemotron_engine::commands::nemotron_init(app.clone())
                .await
                .map_err(failed)?;
            crate::nemotron_engine::commands::nemotron_validate_model_ready_for(configured).await?;
        }
        _ => return Err(LiveModelFailure::UnsupportedProvider),
    }
    Ok(())
}

/// Get the implementation for the same route used by startup and Home.
pub async fn get_or_init_transcription_engine<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<TranscriptionEngine, String> {
    let route = live_transcription_route(app)
        .await
        .map_err(LiveModelFailure::code)?;
    // Initialize the appropriate engine based on provider
    match route.provider {
        Some("parakeet") => {
            info!("🦜 Initializing Parakeet transcription engine");

            // Get Parakeet engine
            let engine = {
                let guard = crate::parakeet_engine::commands::PARAKEET_ENGINE
                    .lock()
                    .unwrap();
                guard.as_ref().cloned()
            };

            match engine {
                Some(engine) => {
                    // Check if model is loaded
                    if engine.is_model_loaded().await {
                        let model_name = engine
                            .get_current_model()
                            .await
                            .unwrap_or_else(|| "unknown".to_string());
                        info!("✅ Parakeet model '{}' already loaded", model_name);
                        Ok(TranscriptionEngine::Parakeet(engine))
                    } else {
                        Err("Parakeet engine initialized but no model loaded. This should not happen after validation.".to_string())
                    }
                }
                None => Err(
                    "Parakeet engine not initialized. This should not happen after validation."
                        .to_string(),
                ),
            }
        }
        Some("nemotron") => {
            info!("🌊 Initializing Nemotron transcription engine");
            let engine = {
                let guard = crate::nemotron_engine::commands::NEMOTRON_ENGINE
                    .lock()
                    .unwrap();
                guard.as_ref().cloned()
            };
            match engine {
                Some(engine) if engine.is_model_loaded().await => {
                    let provider = Arc::new(
                        crate::audio::transcription::NemotronProvider::new(engine),
                    );
                    Ok(TranscriptionEngine::Provider(provider))
                }
                Some(_) => Err(
                    "Nemotron engine initialized but no model loaded. This should not happen after validation."
                        .to_string(),
                ),
                None => Err(
                    "Nemotron engine not initialized. This should not happen after validation."
                        .to_string(),
                ),
            }
        }
        Some("localWhisper") => {
            let engine = crate::whisper_engine::commands::WHISPER_ENGINE
                .lock()
                .unwrap()
                .as_ref()
                .cloned();
            match engine {
                Some(engine) if engine.is_model_loaded().await => {
                    Ok(TranscriptionEngine::Whisper(engine))
                }
                _ => Err("Whisper model was no longer loaded after recording validation.".into()),
            }
        }
        _ => Err(LiveModelFailure::UnsupportedProvider.code()),
    }
}
