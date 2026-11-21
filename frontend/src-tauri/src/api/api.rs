use log::{error as log_error, info as log_info, warn as log_warn};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::{AppHandle, Emitter, Runtime};

use crate::credentials::SecretStatus;
use crate::{
    audio::transcription::cloud::{
        mai_transcribe::MaiTranscribeProvider, mime_type_for_path,
        openai_whisper::OpenAiWhisperProvider, transcribe_mai_file, validate_provider_upload_size,
        CloudTranscriptSegment, CloudTranscriptionProvider, DEFAULT_CLOUD_WHISPER_MODEL,
        DEFAULT_MAI_TRANSCRIBE_MODEL, PROVIDER_CLOUD_WHISPER, PROVIDER_MAI_TRANSCRIBE,
    },
    database::{
        models::MeetingModel,
        repositories::{
            meeting::MeetingsRepository, setting::SettingsRepository,
            transcript::TranscriptsRepository,
        },
    },
    state::AppState,
    summary::CustomOpenAIConfig,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TranscriptSearchResult {
    pub id: String,
    pub title: String,
    #[serde(rename = "matchContext")]
    pub match_context: String,
    pub timestamp: String,
}

/// The summary model configuration as the webview sees it. A stored key is
/// reported only as `apiKeyStatus` (whether there is one, and a short hint).
#[derive(Debug, Serialize)]
pub struct ModelConfig {
    pub provider: String,
    pub model: String,
    #[serde(rename = "whisperModel")]
    pub whisper_model: String,
    #[serde(rename = "apiKeyStatus")]
    pub api_key_status: SecretStatus,
    #[serde(rename = "ollamaEndpoint")]
    pub ollama_endpoint: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct SaveModelConfigRequest {
    pub provider: String,
    pub model: String,
    #[serde(rename = "whisperModel")]
    pub whisper_model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "ollamaEndpoint")]
    pub ollama_endpoint: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetApiKeyRequest {
    pub provider: String,
}

/// The custom OpenAI-compatible provider settings as the webview sees them:
/// everything but the key, which is reported as `apiKeyStatus`.
#[derive(Debug, Serialize)]
pub struct CustomOpenAIConfigView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_problem: Option<String>,
    pub allow_unencrypted: bool,
    pub endpoint: String,
    #[serde(rename = "apiKeyStatus")]
    pub api_key_status: SecretStatus,
    pub model: String,
    #[serde(rename = "timeoutSeconds")]
    pub timeout_seconds: Option<u64>,
    pub organization: Option<String>,
    pub project: Option<String>,
    #[serde(rename = "maxTokens")]
    pub max_tokens: Option<i32>,
    #[serde(rename = "contextWindow")]
    pub context_window: Option<usize>,
    pub temperature: Option<f32>,
    #[serde(rename = "topP")]
    pub top_p: Option<f32>,
}

impl From<CustomOpenAIConfig> for CustomOpenAIConfigView {
    fn from(config: CustomOpenAIConfig) -> Self {
        Self {
            api_key_status: SecretStatus::of(config.api_key.as_deref()),
            destination_problem: config.destination_problem,
            allow_unencrypted: config.allow_unencrypted,
            endpoint: config.endpoint,
            model: config.model,
            timeout_seconds: config.timeout_seconds,
            organization: config.organization,
            project: config.project,
            max_tokens: config.max_tokens,
            context_window: config.context_window,
            temperature: config.temperature,
            top_p: config.top_p,
        }
    }
}

/// The key for a custom-provider test: the one typed for this test, else the
/// stored key, but the stored one only for the endpoint it was saved with.
async fn custom_openai_test_key(
    pool: &sqlx::SqlitePool,
    supplied: Option<String>,
    endpoint: &str,
) -> Result<Option<String>, String> {
    if let Some(key) = clean_string(supplied) {
        return Ok(Some(key));
    }
    let stored = SettingsRepository::get_custom_openai_config(pool)
        .await
        .map_err(|_| "Failed to read the saved connection settings".to_string())?;
    match stored {
        Some(config) if config.api_key.as_deref().is_some_and(|key| !key.is_empty()) => {
            if crate::openai::secret_destination::same_destination(&config.endpoint, endpoint) {
                Ok(config.api_key)
            } else {
                Err("Enter the API key to test a different endpoint.".to_string())
            }
        }
        _ => Ok(None),
    }
}

/// The transcription configuration. `api_key` is never serialised, so a key
/// cannot reach the webview through this type; the webview reads the key's
/// status through `api_get_transcript_api_key_status`.
#[derive(Serialize, Deserialize)]
pub struct TranscriptConfig {
    #[serde(rename = "allowUnencrypted", default)]
    pub allow_unencrypted: bool,
    pub provider: String,
    pub model: String,
    #[serde(rename = "apiKey", skip_serializing, default)]
    pub api_key: Option<String>,
    #[serde(rename = "baseUrl", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct SaveTranscriptConfigRequest {
    pub provider: String,
    pub model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "baseUrl")]
    pub base_url: Option<String>,
    pub endpoint: Option<String>,
    pub region: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TranscriptProviderTestResult {
    pub provider: String,
    pub model: String,
    #[serde(rename = "segmentCount")]
    pub segment_count: usize,
    #[serde(rename = "wordTimestampCount")]
    pub word_timestamp_count: usize,
    #[serde(rename = "requiresLocalTimingGrid")]
    pub requires_local_timing_grid: bool,
    #[serde(rename = "previewText")]
    pub preview_text: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteMeetingRequest {
    pub meeting_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MeetingDetails {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub transcripts: Vec<MeetingTranscript>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MeetingTranscript {
    #[serde(default)]
    pub original_text: Option<String>,
    pub id: String,
    pub text: String,
    pub timestamp: String,
    // Recording-relative timestamps for audio-transcript synchronization
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_start_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_end_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    // Audio source label: "Me" (microphone) or "Participants" (system audio).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_timestamps: Option<Vec<TranscriptWord>>,
}

/// Meeting metadata without transcripts (for pagination)
#[derive(Debug, Serialize, Deserialize)]
pub struct MeetingMetadata {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_path: Option<String>,
}

/// Paginated transcripts response with total count
#[derive(Debug, Serialize, Deserialize)]
pub struct PaginatedTranscriptsResponse {
    pub transcripts: Vec<MeetingTranscript>,
    pub total_count: i64,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SaveMeetingTitleRequest {
    pub meeting_id: String,
    pub title: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SaveMeetingSummaryRequest {
    pub meeting_id: String,
    pub summary: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SaveTranscriptRequest {
    pub meeting_title: String,
    pub transcripts: Vec<TranscriptSegment>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub text: String,
    pub timestamp: String,
    // NEW: Recording-relative timestamps for playback synchronization
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_start_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_end_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    // Audio source label: "Me" (microphone) or "Participants" (system audio).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_timestamps: Option<Vec<TranscriptWord>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_source: Option<TranscriptWordTimestampSource>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptWordTimestampSource {
    Real,
    Estimated,
}

fn normalize_speaker_label(speaker: Option<String>) -> Option<String> {
    let trimmed = speaker?.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.chars().take(64).collect())
    }
}

fn parse_word_timestamps(value: Option<&str>) -> Option<Vec<TranscriptWord>> {
    value.and_then(|json| serde_json::from_str::<Vec<TranscriptWord>>(json).ok())
}

// API Commands for Tauri

#[tauri::command]
pub async fn api_get_meetings<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    auth_token: Option<String>,
) -> Result<Vec<Meeting>, String> {
    let _ = auth_token; // Legacy IPC compatibility; local persistence needs no token.
    log_info!("Listing local meetings");
    let pool = state.db_manager.pool();
    match MeetingsRepository::get_meetings(pool).await {
        Ok(meetings) => {
            log_info!("Successfully got {} meetings", meetings.len());
            Ok(meetings
                .into_iter()
                .map(|meeting| Meeting {
                    id: meeting.id,
                    title: meeting.title,
                    created_at: meeting.created_at,
                    updated_at: meeting.updated_at,
                })
                .collect())
        }
        Err(e) => {
            log_error!("Error getting meetings");
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn api_search_transcripts<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    query: String,
    auth_token: Option<String>,
) -> Result<Vec<TranscriptSearchResult>, String> {
    let _ = auth_token; // Legacy IPC compatibility; local persistence needs no token.
    log_info!("Searching local transcripts");

    let pool = state.db_manager.pool();

    match TranscriptsRepository::search_transcripts(pool, &query).await {
        Ok(results) => {
            log_info!(
                "Search completed successfully with {} results.",
                results.len()
            );
            Ok(results)
        }
        Err(e) => {
            log_error!("Transcript search failed");
            Err(format!("Failed to search transcripts: {}", e))
        }
    }
}

#[tauri::command]
pub(crate) async fn api_search_library(
    state: tauri::State<'_, AppState>,
    query: String,
    scope: Option<crate::knowledge::types::KnowledgeScope>,
    one_per_meeting: Option<bool>,
) -> Result<crate::knowledge::store::literal::LinkedHits, String> {
    crate::knowledge::store::literal::linked(
        state.db_manager.pool(),
        &query,
        scope.as_ref(),
        one_per_meeting.unwrap_or_default(),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn api_get_model_config<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    _auth_token: Option<String>,
) -> Result<Option<ModelConfig>, String> {
    log_info!("api_get_model_config called (native)");
    model_config_view(
        &crate::credentials::platform_store(),
        state.db_manager.pool(),
    )
    .await
}

/// The summary model settings as the webview sees them: a key status, never a key.
pub(crate) async fn model_config_view(
    store: &crate::credentials::Store,
    pool: &sqlx::SqlitePool,
) -> Result<Option<ModelConfig>, String> {
    match SettingsRepository::get_model_config(pool).await {
        Ok(Some(config)) => {
            log_info!(
                "✅ Found model config in database: provider={}, model={}, whisperModel={}, ollamaEndpoint={:?}",
                &config.provider,
                &config.model,
                &config.whisper_model,
                &config.ollama_endpoint
            );
            match SettingsRepository::api_key_status_with(store, pool, &config.provider).await {
                Ok(api_key_status) => {
                    log_info!("Successfully retrieved model config.");
                    Ok(Some(ModelConfig {
                        provider: config.provider,
                        model: config.model,
                        whisper_model: config.whisper_model,
                        api_key_status,
                        ollama_endpoint: config.ollama_endpoint,
                    }))
                }
                Err(e) => {
                    log_error!("Failed to get API key for provider");
                    Err(e.to_string())
                }
            }
        }
        Ok(None) => {
            log_warn!("⚠️ No model config found in database - database may be empty or settings table not initialized");
            Ok(None)
        }
        Err(e) => {
            log_error!("❌ Failed to get model config from database");
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn api_save_model_config<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    provider: String,
    model: String,
    whisper_model: String,
    api_key: Option<String>,
    ollama_endpoint: Option<String>,
    _auth_token: Option<String>,
) -> Result<serde_json::Value, String> {
    let pool = state.db_manager.pool();
    use crate::summary::{automatic, llm_client::LLMProvider};
    let _route = automatic::ROUTE_CHANGE_LOCK.lock().await;
    let new_provider = LLMProvider::from_str(&provider).ok();
    let saved = SettingsRepository::get_model_config(pool)
        .await
        .map_err(|_| "Could not read the saved summary model settings")?;
    if saved.as_ref().is_some_and(|old| {
        LLMProvider::from_str(&old.provider).ok() != new_provider
            || old.model.trim() != model.trim()
            || (new_provider == Some(LLMProvider::Ollama)
                && !automatic::same_endpoint(
                    old.ollama_endpoint
                        .as_deref()
                        .unwrap_or("http://localhost:11434"),
                    ollama_endpoint
                        .as_deref()
                        .unwrap_or("http://localhost:11434"),
                ))
    }) {
        automatic::invalidate_external_review(&app)?;
    }

    if let Err(e) = SettingsRepository::save_model_config(
        pool,
        &provider,
        &model,
        &whisper_model,
        ollama_endpoint.as_deref(),
    )
    .await
    {
        log_error!("❌ Failed to save model config to database");
        return Err(e.to_string());
    }

    // Skip API key saving for operator-managed providers.
    if let Some(key) = api_key {
        if !key.is_empty()
            && provider != "custom-openai"
            && provider != "openclaw"
            && provider != "codex"
            && provider != "claude-code"
        {
            log_info!("🔑 API key provided, saving...");
            if let Err(e) = SettingsRepository::save_api_key(pool, &provider, &key).await {
                log_error!("❌ Failed to save API key");
                return Err(e.to_string());
            }
        }
    }

    // Trigger graceful shutdown of built-in AI sidecar if it's running
    // This ensures that if the user switched models/providers, the old one is cleaned up
    // The shutdown happens in the background, so it won't block the UI
    if let Err(_e) = crate::summary::summary_engine::client::shutdown_sidecar_gracefully().await {
        log_warn!("Failed to initiate graceful sidecar shutdown");
    }

    log_info!("✅ Successfully saved model configuration to database");
    Ok(
        serde_json::json!({ "status": "success", "message": "Model configuration saved successfully" }),
    )
}

/// Whether a summary provider has a stored key, with a short hint. The key
/// itself is never returned; requests that need it use it inside the core.
#[tauri::command]
pub async fn api_get_api_key_status<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    provider: String,
) -> Result<SecretStatus, String> {
    SettingsRepository::api_key_status(state.db_manager.pool(), &provider)
        .await
        .map_err(|e| {
            log_error!("Failed to read provider credential status");
            e.to_string()
        })
}

#[tauri::command]
pub async fn api_get_transcript_config<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    _auth_token: Option<String>,
) -> Result<Option<TranscriptConfig>, String> {
    log_info!("api_get_transcript_config called (native)");
    transcript_config_view(state.db_manager.pool()).await
}

/// The transcription settings as the webview sees them: no key (its status is
/// read separately).
pub(crate) async fn transcript_config_view(
    pool: &sqlx::SqlitePool,
) -> Result<Option<TranscriptConfig>, String> {
    match SettingsRepository::get_transcript_config(pool).await {
        Ok(Some(config)) => {
            log_info!("Loaded transcription configuration");
            // The key stays in the core: the webview gets its status separately.
            Ok(Some(TranscriptConfig {
                allow_unencrypted: config.cloud_http_opt_in(&config.provider),
                provider: config.provider.clone(),
                model: config.model,
                api_key: None,
                base_url: config.cloud_whisper_base_url,
                endpoint: config.mai_transcribe_endpoint,
                region: config.mai_transcribe_region,
            }))
        }
        Ok(None) => {
            log_info!("No transcript config found, returning default.");
            Ok(Some(TranscriptConfig {
                allow_unencrypted: false,
                provider: "parakeet".to_string(),
                model: crate::config::DEFAULT_PARAKEET_MODEL.to_string(),
                api_key: None,
                base_url: None,
                endpoint: None,
                region: None,
            }))
        }
        Err(e) => {
            log_error!("Failed to get transcript config");
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn api_save_transcript_config<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    provider: String,
    model: String,
    allow_unencrypted: Option<bool>,
    api_key: Option<String>,
    base_url: Option<String>,
    endpoint: Option<String>,
    region: Option<String>,
    _auth_token: Option<String>,
) -> Result<serde_json::Value, String> {
    log_info!(
        "api_save_transcript_config called (native) for provider '{}'",
        &provider
    );
    let pool = state.db_manager.pool();

    let allow_unencrypted = allow_unencrypted.unwrap_or(false);
    match provider.as_str() {
        PROVIDER_CLOUD_WHISPER => crate::openai::secret_destination::validate_secret_destination(
            clean_optional(base_url.as_deref()).unwrap_or("https://api.openai.com/v1"),
            allow_unencrypted,
        )?,
        PROVIDER_MAI_TRANSCRIBE => crate::openai::secret_destination::validate_secret_destination(
            clean_optional(endpoint.as_deref()).unwrap_or(""),
            allow_unencrypted,
        )?,
        _ => {}
    }
    let clean_base_url = base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let clean_endpoint = endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let clean_region = region
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let new_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    // Refuse before anything is written: a bad new key, or a stored key that
    // would follow a changed address without being entered again.
    if let Some(problem) = new_key.and_then(crate::credentials::new_secret_problem) {
        return Err(problem.to_string());
    }
    if let Err(e) = SettingsRepository::require_key_for_new_destination(
        pool,
        &provider,
        clean_base_url,
        clean_endpoint,
        new_key.is_some(),
    )
    .await
    {
        return Err(e.to_string());
    }
    if let Err(e) = SettingsRepository::save_transcript_config(pool, &provider, &model).await {
        log_error!("Failed to save transcript config");
        return Err(e.to_string());
    }

    // The new key goes first: the address saved next is then the one the new
    // key was entered for, and a failed key save leaves the old address alone.
    if let Some(key) = new_key {
        log_info!("API key provided, saving for transcript provider...");
        if let Err(e) = SettingsRepository::save_transcript_api_key(pool, &provider, key).await {
            log_error!("Failed to save transcript API key");
            return Err(e.to_string());
        }
    }

    if let Err(e) = SettingsRepository::save_transcript_provider_config(
        pool,
        &provider,
        clean_base_url,
        clean_endpoint,
        clean_region,
        allow_unencrypted,
        new_key.is_some(),
    )
    .await
    {
        log_error!("Failed to save transcript provider config");
        return Err(e.to_string());
    }

    log_info!("Successfully saved transcript configuration.");
    Ok(
        serde_json::json!({ "status": "success", "message": "Transcript configuration saved successfully" }),
    )
}

/// Whether a hosted transcription provider has a stored key, with a short
/// hint. The key itself is never returned.
#[tauri::command]
pub async fn api_get_transcript_api_key_status<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    provider: String,
) -> Result<SecretStatus, String> {
    SettingsRepository::transcript_api_key_status(state.db_manager.pool(), &provider)
        .await
        .map_err(|e| {
            log_error!("Failed to read transcription credential status");
            e.to_string()
        })
}

/// Removes a hosted transcription provider's stored key.
#[tauri::command]
pub async fn api_clear_transcript_api_key<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    provider: String,
) -> Result<(), String> {
    SettingsRepository::clear_transcript_api_key(state.db_manager.pool(), &provider)
        .await
        .map_err(|e| {
            log_error!("Failed to remove the transcription credential");
            e.to_string()
        })
}

#[tauri::command]
pub async fn api_test_transcript_provider<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    provider: String,
    model: String,
    allow_unencrypted: Option<bool>,
    api_key: Option<String>,
    base_url: Option<String>,
    endpoint: Option<String>,
    _region: Option<String>,
    audio_path: String,
    language: Option<String>,
    _auth_token: Option<String>,
) -> Result<TranscriptProviderTestResult, String> {
    log_info!(
        "api_test_transcript_provider called (native) for provider '{}'",
        &provider
    );
    // The audio is a file the user chose in the dialog the core opened
    // (`pick_transcription_test_audio`), once, or one of the user's own recordings.
    let roots = crate::audio::recording_preferences::recording_roots(&app).await;
    let (audio_path, grant) = {
        match crate::file_grants::take(crate::file_grants::Purpose::TestAudio, &audio_path) {
            Ok(granted) => (granted.path.clone(), Some(granted)),
            Err(_) if crate::file_grants::inside_roots(Path::new(&audio_path), &roots) => {
                (std::path::PathBuf::from(&audio_path), None)
            }
            Err(message) => return Err(message),
        }
    };
    let result = test_transcript_provider_with(
        &state,
        provider,
        model,
        allow_unencrypted,
        api_key,
        base_url,
        endpoint,
        &audio_path,
        language,
    )
    .await;
    if result.is_err() {
        if let Some(granted) = &grant {
            crate::file_grants::give_back(crate::file_grants::Purpose::TestAudio, granted);
        }
    }
    result
}

/// Whether a provider test may use plain HTTP. A request from the webview can
/// switch it off, but never on over a saved "no"; without a request the saved
/// setting applies, and only to the saved destination.
fn test_http_opt_in(
    requested: Option<bool>,
    saved: Option<bool>,
    destination_is_saved: bool,
) -> bool {
    match (requested, saved) {
        (Some(true), Some(false)) => false,
        (Some(requested), _) => requested,
        (None, saved) => destination_is_saved && saved == Some(true),
    }
}

async fn test_transcript_provider_with(
    state: &tauri::State<'_, AppState>,
    provider: String,
    model: String,
    allow_unencrypted: Option<bool>,
    api_key: Option<String>,
    base_url: Option<String>,
    endpoint: Option<String>,
    audio_path: &Path,
    language: Option<String>,
) -> Result<TranscriptProviderTestResult, String> {
    let audio_size_bytes =
        crate::audio::transcription::cloud::validate_test_audio_path(audio_path).await?;
    let pool = state.db_manager.pool();
    let saved_config = SettingsRepository::get_transcript_config(pool)
        .await
        .map_err(|_| "Failed to read saved transcript settings")?;
    let saved_endpoint = saved_config
        .as_ref()
        .and_then(|config| match provider.as_str() {
            PROVIDER_CLOUD_WHISPER => Some(
                config
                    .cloud_whisper_base_url
                    .as_deref()
                    .unwrap_or("https://api.openai.com/v1"),
            ),
            PROVIDER_MAI_TRANSCRIBE => config.mai_transcribe_endpoint.as_deref(),
            _ => None,
        });
    let requested = match provider.as_str() {
        PROVIDER_CLOUD_WHISPER => clean_optional(base_url.as_deref()).or_else(|| {
            saved_endpoint
                .is_none()
                .then_some("https://api.openai.com/v1")
        }),
        PROVIDER_MAI_TRANSCRIBE => clean_optional(endpoint.as_deref()),
        _ => return Err("Unsupported cloud transcription provider".into()),
    };
    let destination = crate::audio::transcription::cloud::test_destination(
        saved_endpoint,
        requested,
        clean_optional(api_key.as_deref()).is_some(),
    )?;
    let saved_opt_in = saved_config
        .as_ref()
        .map(|config| config.cloud_http_opt_in(&provider));
    // A request from the webview can switch plain HTTP off, never on over a
    // saved "no": the saved setting decides, and an unsaved one is honoured.
    let allow_unencrypted = test_http_opt_in(
        allow_unencrypted,
        saved_opt_in,
        crate::audio::transcription::cloud::test_destination(
            saved_endpoint,
            Some(&destination),
            false,
        )
        .is_ok(),
    );
    crate::openai::secret_destination::validate_secret_destination(
        &destination,
        allow_unencrypted,
    )?;
    let api_key = match clean_optional(api_key.as_deref()).map(str::to_string) {
        Some(key) => key,
        None => SettingsRepository::get_transcript_api_key(pool, &provider)
            .await
            .map_err(|e| format!("Failed to read saved cloud transcription key: {e}"))?
            .and_then(|value| clean_string(Some(value)))
            .ok_or_else(|| "Cloud transcription API key is missing".to_string())?,
    };

    let file_name = audio_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("test-audio.wav");
    let mime_type = mime_type_for_path(audio_path);
    let language = clean_optional(language.as_deref());

    match provider.as_str() {
        PROVIDER_CLOUD_WHISPER => {
            let base_url = destination;
            let model = clean_string(Some(model))
                .unwrap_or_else(|| DEFAULT_CLOUD_WHISPER_MODEL.to_string());
            validate_provider_upload_size(
                PROVIDER_CLOUD_WHISPER,
                Some(&base_url),
                audio_size_bytes,
            )
            .map_err(|error| {
                format!(
                    "Hosted Whisper test failed ({}): {error}",
                    error.category().as_str()
                )
            })?;
            let audio = tokio::fs::read(audio_path)
                .await
                .map_err(|e| format!("Failed to read test audio file: {e}"))?;
            let provider_client = OpenAiWhisperProvider::new(base_url, api_key, model.clone())
                .with_http_opt_in(allow_unencrypted);
            let segments = provider_client
                .transcribe_file(audio, file_name, mime_type, language)
                .await
                .map_err(|error| {
                    format!(
                        "Hosted Whisper test failed ({}): {error}",
                        error.category().as_str()
                    )
                })?;
            build_cloud_test_result(PROVIDER_CLOUD_WHISPER, &model, &segments, true)
        }
        PROVIDER_MAI_TRANSCRIBE => {
            let endpoint = destination;
            let model = clean_string(Some(model))
                .unwrap_or_else(|| DEFAULT_MAI_TRANSCRIBE_MODEL.to_string());
            let provider_client = MaiTranscribeProvider::new(endpoint, api_key, model.clone())
                .with_http_opt_in(allow_unencrypted);
            let cancelled = std::sync::atomic::AtomicBool::new(false);
            let segments = transcribe_mai_file(&provider_client, audio_path, language, &cancelled)
                .await
                .map_err(|error| {
                    format!(
                        "MAI-Transcribe test failed ({}): {error}",
                        error.category().as_str()
                    )
                })?;
            build_cloud_test_result(PROVIDER_MAI_TRANSCRIBE, &model, &segments, false)
        }
        _ => Err(format!(
            "Provider '{}' does not support hosted transcription testing",
            provider
        )),
    }
}

fn build_cloud_test_result(
    provider: &str,
    model: &str,
    segments: &[CloudTranscriptSegment],
    expect_word_timestamps: bool,
) -> Result<TranscriptProviderTestResult, String> {
    if segments.is_empty() {
        return Err("Provider returned no transcript segments".to_string());
    }
    if !segments
        .iter()
        .any(|segment| !segment.text.trim().is_empty())
    {
        return Err("Provider returned only empty transcript text".to_string());
    }

    let word_timestamp_count = segments
        .iter()
        .map(|segment| segment.words.as_ref().map_or(0, Vec::len))
        .sum::<usize>();
    if expect_word_timestamps && word_timestamp_count == 0 {
        return Err(
            "Hosted Whisper returned text but no word timestamps; diarization would lose real word timing"
                .to_string(),
        );
    }
    if !expect_word_timestamps && word_timestamp_count > 0 {
        return Err(
            "MAI-Transcribe returned word timestamps; ClawScribe must not promote them to Real"
                .to_string(),
        );
    }

    Ok(TranscriptProviderTestResult {
        provider: provider.to_string(),
        model: model.to_string(),
        segment_count: segments.len(),
        word_timestamp_count,
        requires_local_timing_grid: segments
            .iter()
            .any(|segment| segment.requires_local_timing_grid),
        preview_text: preview_text(segments),
    })
}

fn preview_text(segments: &[CloudTranscriptSegment]) -> String {
    let joined = segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    let mut preview = joined.chars().take(180).collect::<String>();
    if joined.chars().count() > preview.chars().count() {
        preview.push_str("...");
    }
    preview
}

fn clean_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn clean_optional(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// Removes a summary provider's stored key (and its credential-store entry).
#[tauri::command]
pub async fn api_clear_api_key<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    provider: String,
) -> Result<(), String> {
    log_info!("api_clear_api_key called for provider '{}'", &provider);
    SettingsRepository::delete_api_key(state.db_manager.pool(), &provider)
        .await
        .map_err(|e| {
            log_error!("Failed to remove the provider credential");
            e.to_string()
        })
}

#[tauri::command]
pub async fn api_delete_meeting<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    auth_token: Option<String>,
    delete_files: bool,
) -> Result<serde_json::Value, String> {
    let _work = crate::updates::admit_work()?;
    let recording_job = claim_meeting_deletion_job(delete_files)?.map(std::sync::Arc::new);
    let _ = auth_token; // Legacy IPC compatibility; local persistence needs no token.
    log_info!("Deleting local meeting");

    let pool = state.db_manager.pool();
    let deleted = MeetingsRepository::delete_from_library(pool, &meeting_id, delete_files)
        .await
        .map_err(|_| "Could not delete the meeting from the library.")?
        .ok_or("The meeting was not found or was already deleted.")?;
    let _ = app.emit("library-meeting-deleted", &meeting_id);

    let locations = crate::library::file_cleanup::CleanupLocations::for_app(&app).await;
    let mut cleanup_pending = false;
    for id in deleted.cleanup_ids {
        let result =
            crate::library::file_cleanup::attempt(pool, &locations, id, recording_job.clone())
                .await;
        cleanup_pending |= !matches!(result, Ok(None));
    }
    if cleanup_pending {
        log_warn!("Some deleted meeting files are pending cleanup");
    }
    Ok(serde_json::json!({
        "status": "success",
        "folder_path": deleted.folder,
        "cleanup_pending": cleanup_pending
    }))
}

fn claim_meeting_deletion_job(
    delete_files: bool,
) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, String> {
    if !delete_files {
        return Ok(None);
    }
    crate::audio::inference::claim_job().map(Some).map_err(|_| {
        "Recording files can't be deleted while a recording or transcription runs. Uncheck 'Also delete the recording files' or try again when it finishes.".into()
    })
}

#[cfg(test)]
mod meeting_deletion_tests {
    use super::*;

    #[tokio::test]
    async fn database_only_deletion_is_allowed_during_inference() {
        let _test_lock = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let _busy = crate::audio::inference::claim_job().unwrap();
        let error = claim_meeting_deletion_job(true).unwrap_err();
        assert!(error.contains("Uncheck 'Also delete the recording files'"));
        assert!(MeetingsRepository::get_meeting(&pool, "review-test")
            .await
            .unwrap()
            .is_some());
        let _job = claim_meeting_deletion_job(false).unwrap();
        assert!(MeetingsRepository::delete_meeting(&pool, "review-test")
            .await
            .unwrap());
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM meetings WHERE id = 'review-test'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }
}

#[tauri::command]
pub async fn api_get_meeting<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    state: tauri::State<'_, AppState>,
    auth_token: Option<String>,
) -> Result<MeetingDetails, String> {
    let _ = auth_token; // Legacy IPC compatibility; local persistence needs no token.
    log_info!("Loading local meeting");

    let pool = state.db_manager.pool();

    match MeetingsRepository::get_meeting(pool, &meeting_id).await {
        Ok(Some(meeting)) => {
            log_info!("Successfully retrieved meeting {}", meeting_id);
            Ok(meeting)
        }
        Ok(None) => {
            log_warn!("Meeting not found");
            Err(format!("Meeting not found: {}", meeting_id))
        }
        Err(e) => {
            log_error!("Error retrieving meeting");
            Err(format!("Failed to retrieve meeting: {}", e))
        }
    }
}

/// Get meeting metadata without transcripts (for pagination)
#[tauri::command]
pub async fn api_get_meeting_metadata<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<MeetingMetadata, String> {
    log_info!(
        "api_get_meeting_metadata called for meeting_id: {}",
        meeting_id
    );

    let pool = state.db_manager.pool();

    match MeetingsRepository::get_meeting_metadata(pool, &meeting_id).await {
        Ok(Some(meeting)) => {
            log_info!("Successfully retrieved meeting metadata {}", meeting_id);
            Ok(MeetingMetadata {
                id: meeting.id,
                title: meeting.title,
                created_at: meeting.created_at,
                updated_at: meeting.updated_at,
                folder_path: meeting.folder_path,
            })
        }
        Ok(None) => {
            log_warn!("Meeting not found");
            Err(format!("Meeting not found: {}", meeting_id))
        }
        Err(e) => {
            log_error!("Error retrieving meeting metadata");
            Err(format!("Failed to retrieve meeting metadata: {}", e))
        }
    }
}

/// Get paginated transcripts for a meeting
#[tauri::command]
pub async fn api_get_meeting_transcripts<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    limit: i64,
    offset: i64,
    state: tauri::State<'_, AppState>,
) -> Result<PaginatedTranscriptsResponse, String> {
    log_info!(
        "api_get_meeting_transcripts called for meeting_id: {}, limit: {}, offset: {}",
        meeting_id,
        limit,
        offset
    );

    let pool = state.db_manager.pool();

    match MeetingsRepository::get_meeting_transcripts_paginated(pool, &meeting_id, limit, offset)
        .await
    {
        Ok((transcripts, total_count)) => {
            log_info!(
                "Successfully retrieved {} transcripts for meeting {} (total: {})",
                transcripts.len(),
                meeting_id,
                total_count
            );

            // Convert Transcript to MeetingTranscript
            let meeting_transcripts = transcripts
                .into_iter()
                .map(|t| MeetingTranscript {
                    original_text: t.original_transcript,
                    id: t.id,
                    text: t.transcript,
                    timestamp: t.timestamp,
                    audio_start_time: t.audio_start_time,
                    audio_end_time: t.audio_end_time,
                    duration: t.duration,
                    speaker: t.speaker,
                    word_timestamps: parse_word_timestamps(t.word_timestamps_json.as_deref()),
                })
                .collect::<Vec<_>>();

            let has_more = (offset + meeting_transcripts.len() as i64) < total_count;

            Ok(PaginatedTranscriptsResponse {
                transcripts: meeting_transcripts,
                total_count,
                has_more,
            })
        }
        Err(e) => {
            log_error!("Error retrieving transcripts for meeting");
            Err(format!("Failed to retrieve transcripts: {}", e))
        }
    }
}

#[tauri::command]
pub async fn api_update_transcript_speaker<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    transcript_id: String,
    speaker: Option<String>,
) -> Result<serde_json::Value, String> {
    let speaker = normalize_speaker_label(speaker);
    let pool = state.db_manager.pool();

    match TranscriptsRepository::update_transcript_speaker(
        pool,
        &meeting_id,
        &transcript_id,
        speaker.as_deref(),
    )
    .await
    {
        Ok(0) => Err(format!("Transcript {} was not found", transcript_id)),
        Ok(updated) => {
            let _ = app.emit(
                "transcript-speaker-updated",
                serde_json::json!({"meeting_id": meeting_id}),
            );
            Ok(serde_json::json!({"updated": updated, "speaker": speaker}))
        }
        Err(e) => {
            log_error!("Failed to update speaker for transcript in meeting");
            Err(format!("Failed to update speaker label: {}", e))
        }
    }
}

#[tauri::command]
pub async fn api_update_transcript_speakers_matching<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    from_speaker: Option<String>,
    speaker: Option<String>,
) -> Result<serde_json::Value, String> {
    let from_speaker = normalize_speaker_label(from_speaker);
    let speaker = normalize_speaker_label(speaker);
    let pool = state.db_manager.pool();

    match TranscriptsRepository::update_transcript_speakers_matching(
        pool,
        &meeting_id,
        from_speaker.as_deref(),
        speaker.as_deref(),
    )
    .await
    {
        Ok(updated) => {
            if updated > 0 {
                let _ = app.emit(
                    "transcript-speaker-updated",
                    serde_json::json!({"meeting_id": meeting_id}),
                );
            }
            Ok(serde_json::json!({"updated": updated, "speaker": speaker}))
        }
        Err(e) => {
            log_error!("Failed to update matching speaker labels in meeting");
            Err(format!("Failed to update matching speaker labels: {}", e))
        }
    }
}

#[tauri::command]
pub async fn api_save_meeting_title<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    title: String,
    auth_token: Option<String>,
) -> Result<serde_json::Value, String> {
    let _ = auth_token; // Legacy IPC compatibility; local persistence needs no token.
    let _install_guard = crate::updates::admit_work()?;
    let pool = state.db_manager.pool();
    match MeetingsRepository::update_meeting_title(pool, &meeting_id, &title).await {
        Ok(true) => Ok(serde_json::json!({"message": "Meeting title saved successfully"})),
        Ok(false) => Err("Meeting was not found".into()),
        Err(_) => {
            log_error!("Could not persist the meeting title");
            Err("Could not save the meeting title".into())
        }
    }
}

fn parse_recording_transcripts(
    values: Vec<serde_json::Value>,
) -> Result<Vec<TranscriptSegment>, String> {
    values
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(|_| "Invalid transcript segment data".into())
}

fn read_recording_snapshot(
    folder: Option<&std::path::Path>,
) -> Result<Vec<TranscriptSegment>, String> {
    let path = folder
        .ok_or("Recording folder unavailable")?
        .join("transcripts.json");
    let file = std::fs::File::open(path).map_err(|_| "Saved transcript could not be opened")?;
    let snapshot: serde_json::Value = serde_json::from_reader(std::io::BufReader::new(file))
        .map_err(|_| "Saved transcript could not be read")?;
    let mut segments = snapshot
        .get("segments")
        .and_then(|value| value.as_array())
        .cloned()
        .ok_or("Saved transcript has no segment list")?;
    for segment in &mut segments {
        if let Some(object) = segment.as_object_mut() {
            let timestamp = object
                .get("display_time")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(""));
            object.insert("timestamp".into(), timestamp);
        }
    }
    parse_recording_transcripts(segments)
}

fn select_recording_transcripts(
    saved: Result<Vec<TranscriptSegment>, String>,
    fallback: Vec<serde_json::Value>,
) -> Result<(Vec<TranscriptSegment>, bool), String> {
    match saved {
        Ok(segments) => Ok((segments, false)),
        Err(_) => Ok((parse_recording_transcripts(fallback)?, true)),
    }
}

#[cfg(test)]
mod recording_snapshot_tests {
    use super::*;

    #[test]
    fn missing_and_invalid_snapshots_fall_back_to_valid_ui_segments() {
        let root = tempfile::tempdir().unwrap();
        for contents in [
            None,
            Some("not json"),
            Some(r#"{"segments":[{"invalid":true}]}"#),
        ] {
            if let Some(contents) = contents {
                std::fs::write(root.path().join("transcripts.json"), contents).unwrap();
            }
            let fallback = vec![
                serde_json::json!({"id":"ui", "text":"Synthetic fallback", "timestamp":"00:01"}),
            ];
            let (segments, used_fallback) =
                select_recording_transcripts(read_recording_snapshot(Some(root.path())), fallback)
                    .unwrap();
            assert!(used_fallback);
            assert_eq!(segments[0].id, "ui");
        }
        assert!(select_recording_transcripts(
            Err("unavailable".into()),
            vec![serde_json::json!({})]
        )
        .is_err());
    }

    #[test]
    fn valid_backend_snapshot_remains_authoritative() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("transcripts.json"), r#"{"segments":[{"id":"backend","text":"Synthetic saved sentence","display_time":"00:02"}]}"#).unwrap();
        let (segments, used_fallback) =
            select_recording_transcripts(read_recording_snapshot(Some(root.path())), vec![])
                .unwrap();
        assert!(!used_fallback);
        assert_eq!(segments[0].id, "backend");
        assert_eq!(segments[0].timestamp, "00:02");
    }
}

#[tauri::command]
pub async fn api_save_transcript<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_title: String,
    transcripts: Vec<serde_json::Value>,
    folder_path: Option<String>,
    auth_token: Option<String>,
    recording_outcome: Option<crate::audio::outcome::RecordingOutcome>,
    use_saved_recording: Option<bool>,
    calendar_link: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let _ = auth_token; // Legacy IPC compatibility; local persistence needs no token.
    log_info!("Saving meeting transcript");

    // The webview does not decide where a meeting's files are: the folder must
    // be one the core created for a recording (the session that just ended), or
    // a marked recording inside a recording root being recovered. What is
    // stored and read is the core's own text for it.
    let (folder_path, recording_id, recovered_live, marker) = match folder_path
        .filter(|folder| !folder.trim().is_empty())
    {
        Some(requested) => {
            let roots = crate::audio::recording_preferences::recording_roots(&app).await;
            // A folder this process created for a recording may be saved again (a
            // retry of the same save finds its meeting). Any other folder, one
            // from an earlier run being recovered, must not already belong to a
            // meeting: that one is repaired, not saved a second time.
            let created_here =
                crate::database::meeting_files::core_folder_text(std::path::Path::new(&requested))
                    .is_some();
            let asked = requested.clone();
            let (authorised, recording_id, stamped_id, transcribes, marker) =
                tokio::task::spawn_blocking(move || {
                    use crate::database::meeting_files;
                    let authorised =
                        meeting_files::authorize_new_meeting_folder(&requested, &roots)?;
                    if crate::audio::recording_commands::active_recording_folder()
                        .protects(std::path::Path::new(&authorised))
                    {
                        return Err(
                            "The recording is still being finalized. Try again after it stops."
                                .into(),
                        );
                    }
                    let marker = meeting_files::read_marker(std::path::Path::new(&authorised))
                        .ok_or("The recording folder is no longer available.")?;
                    if meeting_files::has_invalid_marker_id(&marker) {
                        return Err("The recording marker has an invalid meeting ID.".into());
                    }
                    let recording_id = meeting_files::recording_id(&marker).map(str::to_owned);
                    let stamped_id = meeting_files::stamped_meeting_id(&marker).map(str::to_owned);
                    let transcribes = crate::audio::recording_mode::marker_transcribes(&marker);
                    Ok::<_, String>((authorised, recording_id, stamped_id, transcribes, marker))
                })
                .await
                .map_err(|_| "Could not check the recording folder".to_string())??;
            if !created_here && stamped_id.is_none() && recording_id.is_none() {
                let existing: Option<String> = sqlx::query_scalar(
                    "SELECT id FROM meetings WHERE folder_path COLLATE NOCASE IN (?, ?) LIMIT 1",
                )
                .bind(&authorised)
                .bind(&asked)
                .fetch_optional(state.db_manager.pool())
                .await
                .map_err(|_| "Could not check the library".to_string())?;
                if existing.is_some() {
                    return Err("This recording is already saved in your library. Use Recover on it to repair that meeting instead of saving it again.".into());
                }
            }
            let deleted: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM recording_library_deletions AS deleted
                 WHERE meeting_id IN (?, ?) AND NOT EXISTS
                     (SELECT 1 FROM meetings WHERE id = deleted.meeting_id))",
            )
            .bind(stamped_id.as_deref())
            .bind(recording_id.as_deref())
            .fetch_one(state.db_manager.pool())
            .await
            .map_err(|_| "Could not check the library".to_string())?;
            if deleted {
                return Err("This meeting was deleted from this library.".into());
            }
            (
                Some(authorised),
                recording_id.or(stamped_id),
                transcribes,
                Some(marker),
            )
        }
        None => (None, None, false, None),
    };

    let (transcripts_to_save, used_fallback) = if use_saved_recording.unwrap_or(false) {
        let folder = folder_path.clone();
        let saved = tokio::task::spawn_blocking(move || {
            read_recording_snapshot(folder.as_deref().map(std::path::Path::new))
        })
        .await
        .unwrap_or_else(|_| Err("Saved transcript reader failed".into()));
        select_recording_transcripts(saved, transcripts)?
    } else {
        (parse_recording_transcripts(transcripts)?, false)
    };

    let mut save_warnings = Vec::new();
    // Invalid optional calendar metadata must not prevent the recording from being saved.
    let calendar_link = calendar_link.and_then(|value| {
        match serde_json::from_value::<crate::library::calendar::CalendarLink>(value)
            .map_err(|_| "Invalid calendar link.".to_string())
            .and_then(crate::library::calendar::CalendarLink::normalized)
            .and_then(|link| link.encoded())
        {
            Ok(link) => Some(link),
            Err(_) => {
                save_warnings
                    .push("The calendar event could not be linked. The recording was saved.");
                None
            }
        }
    });
    let mut recording_outcome = recording_outcome.unwrap_or_default();
    if used_fallback {
        recording_outcome.transcription_incomplete = true;
        save_warnings.push("The final transcript snapshot was unavailable. Saved the transcript visible in the app; review it for missing lines.");
        log_warn!("Library save used the UI transcript because the final snapshot was unavailable");
    }
    let mut measured = crate::audio::provenance::RecordingProvenance::default();
    if let Some((folder, marker)) = folder_path
        .as_ref()
        .map(std::path::PathBuf::from)
        .zip(marker)
    {
        let saved = tokio::task::spawn_blocking(move || {
            let measured = crate::audio::interrupted::recovered_provenance(&folder, &marker, None);
            (
                measured,
                crate::audio::outcome::RecordingOutcome::read(&folder),
            )
        })
        .await;
        let disk_outcome = saved.map(|(provenance, outcome)| {
            measured = provenance;
            outcome
        });
        match disk_outcome {
            Ok(Ok(Some(saved))) => {
                recording_outcome.audio_save_failed |= saved.audio_save_failed;
                recording_outcome.transcription_incomplete |= saved.transcription_incomplete;
                recording_outcome.capture_incomplete |= saved.capture_incomplete;
                recording_outcome.recording_files_incomplete |= saved.recording_files_incomplete;
            }
            Ok(Ok(None)) => {}
            _ => {
                recording_outcome.recording_files_incomplete = true;
                save_warnings.push("The recording warning file could not be read. Review the saved meeting and keep its folder.");
            }
        }
    }
    let pool = state.db_manager.pool();
    let stamp_target = folder_path.clone();

    let saved = match (folder_path, recording_id) {
        (Some(folder), Some(recording_id)) => {
            TranscriptsRepository::save_recording_handoff(
                pool,
                &recording_id,
                &meeting_title,
                &transcripts_to_save,
                folder,
                &recording_outcome,
                measured,
            )
            .await
        }
        (folder, _) => {
            TranscriptsRepository::save_recording(
                pool,
                &meeting_title,
                &transcripts_to_save,
                folder,
                Some(&recording_outcome),
                calendar_link,
                measured,
            )
            .await
        }
    };
    match saved {
        Ok(meeting_id) => {
            log_info!("Meeting and transcript saved");
            // Tie the folder to this meeting, so deleting the meeting later can
            // verify it removes only its own recording.
            if let Some(folder) = stamp_target {
                // An idempotent retry may find this ID under a different
                // restored path. Do not stamp the redundant copy.
                let owns_folder: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ? AND folder_path = ?)",
                )
                .bind(&meeting_id)
                .bind(&folder)
                .fetch_one(pool)
                .await
                .unwrap_or(false);
                if owns_folder {
                    let id = meeting_id.clone();
                    let stamped = tokio::task::spawn_blocking(move || {
                        crate::database::meeting_files::stamp_meeting_id(
                            std::path::Path::new(&folder),
                            &id,
                        )
                    })
                    .await;
                    if !matches!(stamped, Ok(Ok(()))) {
                        log_warn!("The recording folder could not be marked with its meeting");
                    }
                }
            }
            if recovered_live {
                crate::audio::recording_commands::wake_saved_recording_summary(
                    &app,
                    meeting_id.clone(),
                );
            }
            Ok(serde_json::json!({
                "status": "success",
                "message": "Transcript saved successfully",
                "meeting_id": meeting_id,
                "warning": if save_warnings.is_empty() { None } else { Some(save_warnings.join(" ")) },
                "recording_outcome": recording_outcome
            }))
        }
        Err(e) => {
            log_error!("Failed to save meeting transcript");
            Err(format!("Failed to save transcript: {}", e))
        }
    }
}

/// Opens the meeting's recording folder in the system file explorer
#[tauri::command]
pub async fn open_meeting_folder<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<(), String> {
    log_info!("open_meeting_folder called for meeting_id: {}", meeting_id);

    let pool = state.db_manager.pool();

    // Get meeting with folder_path
    let meeting: Option<MeetingModel> = sqlx::query_as(
        "SELECT id, title, created_at, updated_at, folder_path FROM meetings WHERE id = ?",
    )
    .bind(&meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("Database error: {}", e))?;

    match meeting {
        Some(m) => {
            if let Some(folder_path) = m.folder_path {
                log_info!("Opening meeting folder");

                // Only an existing, unlinked directory inside a recording root is
                // opened; UNC and extended-length text is refused before any
                // filesystem call could reach the network.
                let roots = crate::audio::recording_preferences::recording_roots(&app).await;
                let folder = tokio::task::spawn_blocking(move || {
                    crate::database::meeting_files::authorize_folder_to_open(&folder_path, &roots)
                })
                .await
                .map_err(|_| "Could not check the recording folder".to_string())?
                .map_err(|_| {
                    log_warn!("The meeting folder is not an allowed recording folder");
                    "The recording folder is not available".to_string()
                })?;

                // Open folder based on OS
                #[cfg(target_os = "macos")]
                {
                    std::process::Command::new("open")
                        .arg(&folder)
                        .spawn()
                        .map_err(|e| format!("Failed to open folder: {}", e))?;
                }

                #[cfg(target_os = "windows")]
                {
                    crate::local_path::open_in_explorer(&folder)?;
                }

                #[cfg(target_os = "linux")]
                {
                    std::process::Command::new("xdg-open")
                        .arg(&folder)
                        .spawn()
                        .map_err(|e| format!("Failed to open folder: {}", e))?;
                }

                log_info!("Successfully opened folder");
                Ok(())
            } else {
                log_warn!("Meeting has no folder_path set");
                Err("Recording folder path not available for this meeting".to_string())
            }
        }
        None => {
            log_warn!("Meeting not found");
            Err("Meeting not found".to_string())
        }
    }
}

#[tauri::command]
pub async fn open_external_url(url: String) -> Result<(), String> {
    crate::external_url::open_url_in_default_browser(&url)
}

// ===== CUSTOM OPENAI API COMMANDS =====

/// Saves the custom OpenAI configuration
/// This configuration is stored as JSON and includes endpoint, apiKey, model, and optional parameters
#[tauri::command]
pub async fn api_save_custom_openai_config<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    endpoint: String,
    allow_unencrypted: Option<bool>,
    api_key: Option<String>,
    model: String,
    max_tokens: Option<i32>,
    context_window: Option<usize>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    timeout_seconds: Option<u64>,
    organization: Option<String>,
    project: Option<String>,
) -> Result<serde_json::Value, String> {
    log_info!("api_save_custom_openai_config called");

    // Validate required fields
    if endpoint.trim().is_empty() {
        return Err("Endpoint URL is required".to_string());
    }
    if model.trim().is_empty() {
        return Err("Model name is required".to_string());
    }

    crate::openai::secret_destination::validate_secret_destination(
        endpoint.trim(),
        allow_unencrypted.unwrap_or(false),
    )?;

    // Validate optional numeric parameters
    if let Some(temp) = temperature {
        if !(0.0..=2.0).contains(&temp) {
            return Err("Temperature must be between 0.0 and 2.0".to_string());
        }
    }
    if let Some(top) = top_p {
        if !(0.0..=1.0).contains(&top) {
            return Err("Top P must be between 0.0 and 1.0".to_string());
        }
    }
    if let Some(tokens) = max_tokens {
        if tokens < 1 {
            return Err("Max tokens must be at least 1".to_string());
        }
    }
    if let Some(seconds) = timeout_seconds {
        if seconds < 5 {
            return Err("Timeout must be at least 5 seconds".to_string());
        }
    }

    let config = CustomOpenAIConfig {
        destination_problem: None,
        allow_unencrypted: allow_unencrypted.unwrap_or(false),
        endpoint: endpoint.trim().to_string(),
        // No key here means "keep the stored one" (see `save_custom_openai_config`).
        api_key: api_key.filter(|k| !k.trim().is_empty()),
        model: model.trim().to_string(),
        timeout_seconds,
        organization: organization.filter(|value| !value.trim().is_empty()),
        project: project.filter(|value| !value.trim().is_empty()),
        max_tokens,
        context_window,
        temperature,
        top_p,
    };

    let pool = state.db_manager.pool();

    use crate::summary::{automatic, llm_client::LLMProvider};
    let _route = automatic::ROUTE_CHANGE_LOCK.lock().await;
    let selected = SettingsRepository::get_model_config(pool)
        .await
        .map_err(|_| "Could not read the saved summary model settings")?;
    if selected.is_some_and(|setting| {
        matches!(
            LLMProvider::from_str(&setting.provider),
            Ok(LLMProvider::CustomOpenAI | LLMProvider::OpenAICompatible)
        )
    }) {
        let old = SettingsRepository::get_custom_openai_config(pool)
            .await
            .map_err(|_| "Could not read the saved compatible provider settings")?;
        let same_route = old.is_some_and(|old| {
            automatic::same_endpoint(&old.endpoint, &config.endpoint)
                && old.model.trim() == config.model
                && old.allow_unencrypted == config.allow_unencrypted
                && old.organization.as_deref().map(str::trim)
                    == config.organization.as_deref().map(str::trim)
                && old.project.as_deref().map(str::trim) == config.project.as_deref().map(str::trim)
        });
        if !same_route {
            automatic::invalidate_external_review(&app)?;
        }
    }

    match SettingsRepository::save_custom_openai_config(pool, &config).await {
        Ok(()) => {
            log_info!("Saved custom OpenAI configuration");
            Ok(serde_json::json!({
                "status": "success",
                "message": "Custom OpenAI configuration saved successfully"
            }))
        }
        Err(e) => {
            log_error!("❌ Failed to save custom OpenAI config");
            Err(format!("Failed to save custom OpenAI configuration: {}", e))
        }
    }
}

/// Gets the custom OpenAI configuration
#[tauri::command]
pub async fn api_get_custom_openai_config<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<Option<CustomOpenAIConfigView>, String> {
    log_info!("api_get_custom_openai_config called");
    custom_openai_config_view(
        &crate::credentials::platform_store(),
        state.db_manager.pool(),
    )
    .await
}

/// The custom provider settings as the webview sees them: a key status, never a key.
pub(crate) async fn custom_openai_config_view(
    store: &crate::credentials::Store,
    pool: &sqlx::SqlitePool,
) -> Result<Option<CustomOpenAIConfigView>, String> {
    match SettingsRepository::get_custom_openai_config_with(store, pool).await {
        Ok(config) => {
            if config.is_some() {
                log_info!("Found custom OpenAI configuration");
            } else {
                log_info!("No custom OpenAI config found");
            }
            Ok(config.map(CustomOpenAIConfigView::from))
        }
        Err(e) => {
            log_error!("❌ Failed to get custom OpenAI config");
            Err(format!("Failed to get custom OpenAI configuration: {}", e))
        }
    }
}

/// Removes the custom provider's stored key; the rest of its settings stay.
#[tauri::command]
pub async fn api_clear_custom_openai_api_key<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    SettingsRepository::clear_custom_openai_api_key(state.db_manager.pool())
        .await
        .map_err(|e| {
            log_error!("Failed to remove the custom provider credential");
            e.to_string()
        })
}

/// Tests the connection to a custom OpenAI-compatible endpoint
/// Makes a minimal request to verify the endpoint is reachable and responds correctly
#[tauri::command]
pub async fn api_test_custom_openai_connection<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    endpoint: String,
    allow_unencrypted: Option<bool>,
    api_key: Option<String>,
    model: String,
    max_tokens: Option<i32>,
    context_window: Option<usize>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    timeout_seconds: Option<u64>,
    organization: Option<String>,
    project: Option<String>,
) -> Result<serde_json::Value, String> {
    log_info!("api_test_custom_openai_connection called");

    crate::openai::secret_destination::validate_secret_destination(
        endpoint.trim(),
        allow_unencrypted.unwrap_or(false),
    )?;

    let api_key = custom_openai_test_key(state.db_manager.pool(), api_key, &endpoint).await?;
    let config = CustomOpenAIConfig {
        destination_problem: None,
        allow_unencrypted: allow_unencrypted.unwrap_or(false),
        endpoint,
        api_key,
        model,
        timeout_seconds,
        organization,
        project,
        max_tokens,
        context_window,
        temperature,
        top_p,
    };
    let provider_config = crate::summary::openai_provider::config_from_custom_openai(config);
    let provider =
        crate::summary::openai_provider::OpenAICompatibleProcessingProvider::new(provider_config)?;
    let status = provider.test_connection().await?;
    if status.success {
        Ok(serde_json::json!({
            "status": "success",
            "message": status.message,
            "http_status": 200
        }))
    } else {
        Err(status.message)
    }
}

#[tauri::command]
pub async fn api_test_custom_openai_processing<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    endpoint: String,
    allow_unencrypted: Option<bool>,
    api_key: Option<String>,
    model: String,
    max_tokens: Option<i32>,
    context_window: Option<usize>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    timeout_seconds: Option<u64>,
    organization: Option<String>,
    project: Option<String>,
) -> Result<serde_json::Value, String> {
    let api_key = custom_openai_test_key(state.db_manager.pool(), api_key, &endpoint).await?;
    let config = CustomOpenAIConfig {
        destination_problem: None,
        allow_unencrypted: allow_unencrypted.unwrap_or(false),
        endpoint,
        api_key,
        model,
        timeout_seconds,
        organization,
        project,
        max_tokens,
        context_window,
        temperature,
        top_p,
    };
    let provider_config = crate::summary::openai_provider::config_from_custom_openai(config);
    let provider =
        crate::summary::openai_provider::OpenAICompatibleProcessingProvider::new(provider_config)?;
    let output_dir = std::env::temp_dir().join("clawscribe-openai-compatible-test");
    let result = provider
        .process_meeting(
            crate::summary::openai_provider::OpenAICompatibleMeetingProcessRequest {
                sources: Vec::new(),
                meeting_id: "clawscribe-openai-compatible-test".to_string(),
                meeting_title: Some("ClawScribe OpenAI-compatible test".to_string()),
                transcript: "[00:00] Alice: We will send the release checklist today.\n[00:05] Bob: I will review the installer by tomorrow.".to_string(),
                custom_prompt: None,
                output_dir: Some(output_dir),
            },
            None,
        )
        .await?;
    Ok(serde_json::json!({
        "status": "success",
        "message": "Test meeting processed successfully",
        "outputJsonPath": result.output_json_path,
        "notesMarkdownPath": result.notes_markdown_path,
        "followUpEmailPath": result.follow_up_email_path,
        "processingLogPath": result.processing_log_path
    }))
}

#[cfg(test)]
mod secret_tests {
    use super::*;

    const FAKE_KEY: &str = "fake-api-key-value-7777";

    fn custom_config() -> CustomOpenAIConfig {
        CustomOpenAIConfig {
            destination_problem: None,
            allow_unencrypted: false,
            endpoint: "https://gateway.example.com/v1".into(),
            api_key: Some(FAKE_KEY.into()),
            model: "model-a".into(),
            timeout_seconds: Some(60),
            organization: None,
            project: None,
            max_tokens: None,
            context_window: Some(8192),
            temperature: None,
            top_p: None,
        }
    }

    #[test]
    fn the_summary_model_config_serialises_a_status_and_never_a_key() {
        let config = ModelConfig {
            provider: "openai".into(),
            model: "model-a".into(),
            whisper_model: "large-v3".into(),
            api_key_status: SecretStatus::of(Some(FAKE_KEY)),
            ollama_endpoint: None,
        };
        let json = serde_json::to_value(&config).unwrap();
        assert!(!json.to_string().contains(FAKE_KEY));
        assert!(json.get("apiKey").is_none());
        assert_eq!(
            json["apiKeyStatus"],
            serde_json::json!({"stored": true, "hint": "7777"})
        );
        assert!(!format!("{config:?}").contains(FAKE_KEY));
    }

    #[test]
    fn the_transcript_config_never_serialises_its_key_even_when_it_holds_one() {
        let config = TranscriptConfig {
            allow_unencrypted: false,
            provider: "cloud-whisper".into(),
            model: "whisper-1".into(),
            api_key: Some(FAKE_KEY.into()),
            base_url: Some("https://api.example.com/v1".into()),
            endpoint: None,
            region: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains(FAKE_KEY));
        assert!(!json.contains("apiKey"));
    }

    #[test]
    fn the_custom_provider_view_has_a_status_and_no_key_and_keeps_the_other_fields() {
        let view = CustomOpenAIConfigView::from(custom_config());
        let json = serde_json::to_value(&view).unwrap();
        assert!(!json.to_string().contains(FAKE_KEY));
        assert!(json.get("apiKey").is_none());
        assert_eq!(
            json["apiKeyStatus"],
            serde_json::json!({"stored": true, "hint": "7777"})
        );
        // The names the settings screen reads are unchanged.
        assert_eq!(json["endpoint"], "https://gateway.example.com/v1");
        assert_eq!(json["model"], "model-a");
        assert_eq!(json["timeoutSeconds"], 60);
        assert_eq!(json["contextWindow"], 8192);
        assert_eq!(json["allow_unencrypted"], false);
        let without_key = CustomOpenAIConfigView::from(CustomOpenAIConfig {
            api_key: None,
            ..custom_config()
        });
        assert_eq!(
            serde_json::to_value(&without_key).unwrap()["apiKeyStatus"],
            serde_json::json!({"stored": false, "hint": null})
        );
    }

    #[test]
    fn debug_output_of_the_custom_provider_settings_never_contains_the_key() {
        assert!(!format!("{:?}", custom_config()).contains(FAKE_KEY));
    }

    /// Every window of `len` characters of a key: if none appears in what the
    /// webview receives, no part of the key longer than the hint does.
    fn windows(key: &str, len: usize) -> Vec<String> {
        let chars: Vec<char> = key.chars().collect();
        chars
            .windows(len)
            .map(|window| window.iter().collect())
            .collect()
    }

    fn assert_no_part_of_the_key(output: &str, key: &str, what: &str) {
        assert!(!output.contains(key), "{what} contains a whole key");
        for part in windows(key, 5) {
            assert!(!output.contains(&part), "{what} contains part of a key");
        }
    }

    #[tokio::test]
    async fn no_getter_returns_a_stored_key_or_more_than_its_hint() {
        use crate::credentials::test_support::MemoryStore;
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let memory = MemoryStore::new();
        let store: crate::credentials::Store = memory.clone();

        // One fake key per place a key can be stored.
        let mut keys = Vec::new();
        let next_key = |keys: &mut Vec<String>| {
            let key = format!("zzfakeQ{:02}x7kL9mV4wR8tB5nC1d", keys.len());
            keys.push(key.clone());
            key
        };
        let summary = ["openai", "claude", "ollama", "groq", "openrouter"];
        let transcript = [
            "localWhisper",
            "deepgram",
            "elevenLabs",
            "groq",
            "openai",
            "cloud-whisper",
            "mai-transcribe",
        ];
        for provider in summary {
            let key = next_key(&mut keys);
            SettingsRepository::save_api_key_with(&store, &pool, provider, &key)
                .await
                .unwrap();
        }
        for provider in transcript {
            let key = next_key(&mut keys);
            SettingsRepository::save_transcript_api_key_with(&store, &pool, provider, &key)
                .await
                .unwrap();
        }
        let custom_key = next_key(&mut keys);
        SettingsRepository::save_custom_openai_config_with(
            &store,
            &pool,
            &CustomOpenAIConfig {
                api_key: Some(custom_key),
                ..custom_config()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            memory.entries(),
            keys.len(),
            "every key is in the credential store"
        );

        let mut outputs: Vec<(String, String)> = Vec::new();
        for provider in summary {
            SettingsRepository::save_model_config(&pool, provider, "model-a", "large-v3", None)
                .await
                .unwrap();
            let view = model_config_view(&store, &pool).await.unwrap().unwrap();
            outputs.push((
                format!("model config for {provider}"),
                serde_json::to_string(&view).unwrap(),
            ));
            let status = SettingsRepository::api_key_status_with(&store, &pool, provider)
                .await
                .unwrap();
            assert!(status.stored, "{provider} reports a stored key");
            outputs.push((
                format!("summary status for {provider}"),
                serde_json::to_string(&status).unwrap(),
            ));
        }
        for provider in transcript {
            SettingsRepository::save_transcript_config(&pool, provider, "model-a")
                .await
                .unwrap();
            let view = transcript_config_view(&pool).await.unwrap().unwrap();
            outputs.push((
                format!("transcript config for {provider}"),
                serde_json::to_string(&view).unwrap(),
            ));
            let status =
                SettingsRepository::transcript_api_key_status_with(&store, &pool, provider)
                    .await
                    .unwrap();
            assert!(status.stored, "{provider} reports a stored key");
            outputs.push((
                format!("transcript status for {provider}"),
                serde_json::to_string(&status).unwrap(),
            ));
        }
        let custom_view = custom_openai_config_view(&store, &pool)
            .await
            .unwrap()
            .unwrap();
        outputs.push((
            "custom provider config".into(),
            serde_json::to_string(&custom_view).unwrap(),
        ));
        // Debug output counts as output too.
        outputs.push(("custom provider debug".into(), format!("{custom_view:?}")));

        assert!(outputs.len() > 20);
        for (what, output) in &outputs {
            for key in &keys {
                assert_no_part_of_the_key(output, key, what);
            }
            // The hint, when there is one, is the last four characters at most.
            if let Ok(status) = serde_json::from_str::<serde_json::Value>(output) {
                if let Some(hint) = status.get("hint").and_then(|hint| hint.as_str()) {
                    assert_eq!(hint, "nC1d");
                }
            }
        }
    }

    #[test]
    fn a_request_cannot_switch_plain_http_on_over_a_saved_no() {
        // (requested, saved, destination is the saved one) -> allowed
        for (requested, saved, same, expected) in [
            (Some(true), Some(false), true, false),
            (Some(true), Some(false), false, false),
            (Some(true), Some(true), true, true),
            (Some(true), None, false, true),
            (Some(false), Some(true), true, false),
            (None, Some(true), true, true),
            (None, Some(true), false, false),
            (None, Some(false), true, false),
            (None, None, true, false),
        ] {
            assert_eq!(
                test_http_opt_in(requested, saved, same),
                expected,
                "{requested:?} {saved:?} {same}"
            );
        }
    }

    #[test]
    fn a_stored_key_is_only_offered_for_its_own_endpoint() {
        assert!(crate::openai::secret_destination::same_destination(
            "https://gateway.example.com/v1",
            "https://gateway.example.com/v1/"
        ));
        assert!(crate::openai::secret_destination::same_destination(
            " HTTPS://Gateway.Example.com/v1 ",
            "https://gateway.example.com/v1"
        ));
        assert!(!crate::openai::secret_destination::same_destination(
            "https://gateway.example.com/v1",
            "https://other.example.com/v1"
        ));
        assert!(!crate::openai::secret_destination::same_destination(
            "https://gateway.example.com/v1",
            "https://gateway.example.com/v2"
        ));
    }
}
