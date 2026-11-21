use crate::openai::model_catalog;
use serde::{Deserialize, Serialize};
use tauri::command;

/// OpenAI model information returned to frontend
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OpenAIModel {
    pub id: String,
}

/// API response model from OpenAI
#[derive(Debug, Deserialize)]
struct OpenAIApiModel {
    id: String,
    #[allow(dead_code)]
    object: String,
    #[allow(dead_code)]
    owned_by: String,
}

/// API response wrapper from OpenAI
#[derive(Debug, Deserialize)]
struct OpenAIApiResponse {
    data: Vec<OpenAIApiModel>,
}

/// Check if model is a chat-capable model (filter out embedding, tts, etc.)
fn is_chat_model(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    // Include gpt-*, chatgpt-* and o1/o3/o4 models (with or without a suffix)
    // Exclude embedding, tts, transcription, image, live voice and other non-chat models
    (id.starts_with("gpt-")
        || id.starts_with("o1")
        || id.starts_with("o3")
        || id.starts_with("o4")
        || id.starts_with("chatgpt-"))
        && !id.contains("embedding")
        && !id.contains("tts")
        && !id.contains("whisper")
        && !id.contains("transcribe")
        && !id.contains("image")
        && !id.starts_with("gpt-live")
        && !id.contains("dall-e")
        && !id.contains("babbage")
        && !id.contains("davinci")
        && !id.contains("instruct")
        && !id.contains("realtime")
        && !id.contains("audio")
}

/// List chat models available to the selected provider credential.
#[command]
pub async fn get_openai_models(
    state: tauri::State<'_, crate::state::AppState>,
    api_key: Option<String>,
) -> Result<Vec<OpenAIModel>, String> {
    // The webview sends a key only when the user just typed one; otherwise the
    // stored key is used here and never leaves the core.
    let api_key = crate::database::repositories::setting::SettingsRepository::effective_api_key(
        state.db_manager.pool(),
        "openai",
        api_key,
    )
    .await
    .map_err(|_| "Failed to read the saved API key".to_string())?;
    fetch_openai_models(api_key).await
}

/// The model list for a key the core already holds.
pub async fn fetch_openai_models(api_key: Option<String>) -> Result<Vec<OpenAIModel>, String> {
    let api_key = api_key
        .filter(|key| !key.trim().is_empty())
        .ok_or("missing_api_key")?;
    let client = model_catalog::client()?;
    let request = client
        .get("https://api.openai.com/v1/models")
        .header("Authorization", format!("Bearer {}", api_key));
    let api_response: OpenAIApiResponse = model_catalog::read(request).await?;

    // Filter to only chat models and map to our struct
    let models: Vec<OpenAIModel> = api_response
        .data
        .into_iter()
        .filter(|m| is_chat_model(&m.id))
        .map(|m| OpenAIModel { id: m.id })
        .collect();

    Ok(models)
}
