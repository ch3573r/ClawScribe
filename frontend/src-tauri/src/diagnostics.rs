use crate::api::TranscriptConfig;
use crate::audio::devices::configuration::{AudioDevice, DeviceType};
use crate::audio::diarization::{
    speaker_diarization_runtime_status, SpeakerDiarizationRuntimeStatus,
};
use crate::audio::recording_commands;
use crate::database::repositories::setting::SettingsRepository;
use crate::nemotron_engine::commands::{nemotron_get_available_models, NEMOTRON_ENGINE};
use crate::openclaw::{get_openclaw_config_status, OpenClawConfigStatus};
use crate::parakeet_engine::commands::{
    get_parakeet_use_directml, parakeet_get_available_models, parakeet_get_current_model,
    parakeet_get_models_directory, parakeet_init, parakeet_is_model_loaded,
};
use crate::state::AppState;
use crate::summary::codex_provider::{codex_check_installation, CodexInstallationStatus};
use crate::teams_detection::{get_teams_detection_status, TeamsDetectionStatus};
use crate::whisper_engine::commands::{
    whisper_get_acceleration_status, whisper_get_available_models, whisper_get_current_model,
    whisper_get_models_directory, whisper_init, whisper_is_model_loaded, WhisperAccelerationStatus,
};
use crate::whisper_engine::system_monitor::{SystemMonitor, SystemResources};
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, State};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSnapshot {
    pub app: AppDiagnosticInfo,
    pub system_resources: Option<SystemResources>,
    pub acceleration: Option<WhisperAccelerationStatus>,
    pub transcription: TranscriptionDiagnostics,
    pub audio: AudioDiagnostics,
    pub recording: serde_json::Value,
    pub diarization: SpeakerDiarizationRuntimeStatus,
    pub integrations: IntegrationDiagnostics,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDiagnosticInfo {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub build_profile: String,
    pub app_data_dir: Option<String>,
    pub logs_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionDiagnostics {
    pub selected_provider: String,
    pub selected_model: String,
    pub api_key_configured: bool,
    pub parakeet_directml_enabled: Option<bool>,
    pub engines: Vec<ModelEngineDiagnostics>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEngineDiagnostics {
    pub provider: String,
    pub loaded: Option<bool>,
    pub current_model: Option<String>,
    pub selected_model_present: Option<bool>,
    pub downloaded_models: usize,
    pub total_models: usize,
    pub models_directory: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDiagnostics {
    pub input_devices: usize,
    pub output_devices: usize,
    pub total_devices: usize,
    pub device_names: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationDiagnostics {
    pub teams: Option<TeamsDetectionStatus>,
    pub openclaw: Option<OpenClawConfigStatus>,
    pub codex: Option<CodexInstallationStatus>,
}

#[tauri::command]
pub async fn get_diagnostics_snapshot<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> Result<DiagnosticSnapshot, String> {
    let mut errors = Vec::new();
    let app_data_dir = app.path().app_data_dir().ok();
    let logs_dir = app_data_dir.as_ref().map(|dir| dir.join("logs"));

    let acceleration = match whisper_get_acceleration_status().await {
        Ok(status) => Some(status),
        Err(error) => {
            errors.push(format!("acceleration: {error}"));
            None
        }
    };

    let system_resources = match current_system_resources().await {
        Ok(resources) => Some(resources),
        Err(error) => {
            errors.push(format!("system_resources: {error}"));
            None
        }
    };

    let transcript_config = transcript_config(&state).await.unwrap_or_else(|error| {
        errors.push(format!("transcription_config: {error}"));
        default_transcript_config()
    });

    let parakeet_directml_enabled = match get_parakeet_use_directml().await {
        Ok(enabled) => Some(enabled),
        Err(error) => {
            errors.push(format!("parakeet_directml: {error}"));
            None
        }
    };

    let audio = match audio_diagnostics().await {
        Ok(audio) => audio,
        Err(error) => {
            errors.push(format!("audio_devices: {error}"));
            AudioDiagnostics {
                input_devices: 0,
                output_devices: 0,
                total_devices: 0,
                device_names: Vec::new(),
                error: Some(error),
            }
        }
    };

    let teams = Some(get_teams_detection_status(None));
    let openclaw = match get_openclaw_config_status(app.clone()).await {
        Ok(status) => Some(status),
        Err(error) => {
            errors.push(format!("openclaw: {error}"));
            None
        }
    };
    let codex = match codex_check_installation(app.clone()).await {
        Ok(status) => Some(status),
        Err(error) => {
            errors.push(format!("codex: {error}"));
            None
        }
    };

    Ok(DiagnosticSnapshot {
        app: AppDiagnosticInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            build_profile: if cfg!(debug_assertions) {
                "debug".to_string()
            } else {
                "release".to_string()
            },
            app_data_dir: app_data_dir.map(path_to_string),
            logs_dir: logs_dir.map(path_to_string),
        },
        system_resources,
        acceleration,
        transcription: TranscriptionDiagnostics {
            selected_provider: transcript_config.provider.clone(),
            selected_model: transcript_config.model.clone(),
            api_key_configured: transcript_config.api_key.is_some(),
            parakeet_directml_enabled,
            engines: vec![
                whisper_model_diagnostics(&transcript_config).await,
                parakeet_model_diagnostics(&transcript_config).await,
                nemotron_model_diagnostics(&app, &transcript_config).await,
            ],
        },
        audio,
        recording: recording_commands::get_recording_state().await,
        diarization: speaker_diarization_runtime_status(),
        integrations: IntegrationDiagnostics {
            teams,
            openclaw,
            codex,
        },
        errors,
    })
}

async fn current_system_resources() -> Result<SystemResources, String> {
    let monitor = SystemMonitor::new();
    monitor
        .refresh_system_info()
        .await
        .map_err(|error| error.to_string())?;
    monitor
        .get_current_resources()
        .await
        .map_err(|error| error.to_string())
}

async fn transcript_config(state: &State<'_, AppState>) -> Result<TranscriptConfig, String> {
    let pool = state.db_manager.pool();
    match SettingsRepository::get_transcript_config(pool).await {
        Ok(Some(config)) => {
            let api_key = SettingsRepository::get_transcript_api_key(pool, &config.provider)
                .await
                .map_err(|error| error.to_string())?;
            Ok(TranscriptConfig {
                provider: config.provider,
                model: config.model,
                api_key,
                base_url: config.cloud_whisper_base_url,
                endpoint: config.mai_transcribe_endpoint,
                region: config.mai_transcribe_region,
            })
        }
        Ok(None) => Ok(default_transcript_config()),
        Err(error) => Err(error.to_string()),
    }
}

fn default_transcript_config() -> TranscriptConfig {
    TranscriptConfig {
        provider: "parakeet".to_string(),
        model: crate::config::DEFAULT_PARAKEET_MODEL.to_string(),
        api_key: None,
        base_url: None,
        endpoint: None,
        region: None,
    }
}

async fn audio_diagnostics() -> Result<AudioDiagnostics, String> {
    let devices = crate::audio::list_audio_devices()
        .await
        .map_err(|error| error.to_string())?;
    let input_devices = devices
        .iter()
        .filter(|device| matches!(device.device_type, DeviceType::Input))
        .count();
    let output_devices = devices
        .iter()
        .filter(|device| matches!(device.device_type, DeviceType::Output))
        .count();
    let device_names = devices.iter().map(audio_device_label).collect::<Vec<_>>();
    Ok(AudioDiagnostics {
        input_devices,
        output_devices,
        total_devices: devices.len(),
        device_names,
        error: None,
    })
}

fn audio_device_label(device: &AudioDevice) -> String {
    let kind = match device.device_type {
        DeviceType::Input => "input",
        DeviceType::Output => "output",
    };
    format!("{} ({kind})", device.name)
}

async fn whisper_model_diagnostics(config: &TranscriptConfig) -> ModelEngineDiagnostics {
    let _ = whisper_init().await;
    let loaded = whisper_is_model_loaded().await.ok();
    let current_model = whisper_get_current_model().await.ok().flatten();
    let models_directory = whisper_get_models_directory().await.ok();
    match whisper_get_available_models().await {
        Ok(models) => {
            let downloaded_models = models
                .iter()
                .filter(|model| {
                    matches!(model.status, crate::whisper_engine::ModelStatus::Available)
                })
                .count();
            let selected_model_present = (config.provider == "localWhisper").then(|| {
                models.iter().any(|model| {
                    model.name == config.model
                        && matches!(model.status, crate::whisper_engine::ModelStatus::Available)
                })
            });
            ModelEngineDiagnostics {
                provider: "localWhisper".to_string(),
                loaded,
                current_model,
                selected_model_present,
                downloaded_models,
                total_models: models.len(),
                models_directory,
                error: None,
            }
        }
        Err(error) => ModelEngineDiagnostics {
            provider: "localWhisper".to_string(),
            loaded,
            current_model,
            selected_model_present: None,
            downloaded_models: 0,
            total_models: 0,
            models_directory,
            error: Some(error),
        },
    }
}

async fn parakeet_model_diagnostics(config: &TranscriptConfig) -> ModelEngineDiagnostics {
    let _ = parakeet_init().await;
    let loaded = parakeet_is_model_loaded().await.ok();
    let current_model = parakeet_get_current_model().await.ok().flatten();
    let models_directory = parakeet_get_models_directory().await.ok();
    match parakeet_get_available_models().await {
        Ok(models) => {
            let downloaded_models = models
                .iter()
                .filter(|model| {
                    matches!(model.status, crate::parakeet_engine::ModelStatus::Available)
                })
                .count();
            let selected_model_present = (config.provider == "parakeet").then(|| {
                models.iter().any(|model| {
                    model.name == config.model
                        && matches!(model.status, crate::parakeet_engine::ModelStatus::Available)
                })
            });
            ModelEngineDiagnostics {
                provider: "parakeet".to_string(),
                loaded,
                current_model,
                selected_model_present,
                downloaded_models,
                total_models: models.len(),
                models_directory,
                error: None,
            }
        }
        Err(error) => ModelEngineDiagnostics {
            provider: "parakeet".to_string(),
            loaded,
            current_model,
            selected_model_present: None,
            downloaded_models: 0,
            total_models: 0,
            models_directory,
            error: Some(error),
        },
    }
}

async fn nemotron_model_diagnostics<R: Runtime>(
    app: &AppHandle<R>,
    config: &TranscriptConfig,
) -> ModelEngineDiagnostics {
    let models_directory = app
        .path()
        .app_data_dir()
        .ok()
        .map(|dir| path_to_string(dir.join("models").join("nemotron")));
    let (loaded, current_model) = {
        let engine = NEMOTRON_ENGINE.lock().unwrap().as_ref().cloned();
        if let Some(engine) = engine {
            (
                Some(engine.is_model_loaded().await),
                engine.get_current_model().await,
            )
        } else {
            (Some(false), None)
        }
    };
    match nemotron_get_available_models(app.clone()).await {
        Ok(models) => {
            let downloaded_models = models
                .iter()
                .filter(|model| {
                    matches!(model.status, crate::parakeet_engine::ModelStatus::Available)
                })
                .count();
            let selected_model_present = (config.provider == "nemotron").then(|| {
                models.iter().any(|model| {
                    model.name == config.model
                        && matches!(model.status, crate::parakeet_engine::ModelStatus::Available)
                })
            });
            ModelEngineDiagnostics {
                provider: "nemotron".to_string(),
                loaded,
                current_model,
                selected_model_present,
                downloaded_models,
                total_models: models.len(),
                models_directory,
                error: None,
            }
        }
        Err(error) => ModelEngineDiagnostics {
            provider: "nemotron".to_string(),
            loaded,
            current_model,
            selected_model_present: None,
            downloaded_models: 0,
            total_models: 0,
            models_directory,
            error: Some(error),
        },
    }
}

fn path_to_string(path: impl AsRef<std::path::Path>) -> String {
    path.as_ref().to_string_lossy().to_string()
}
