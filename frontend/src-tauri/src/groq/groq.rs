use crate::openai::model_catalog;
use serde::{Deserialize, Serialize};
use tauri::command;

/// Groq model information returned to frontend
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GroqModel {
    pub id: String,
    pub owned_by: Option<String>,
}

/// API response model from Groq (OpenAI-compatible format)
#[derive(Debug, Deserialize)]
struct GroqApiModel {
    id: String,
    owned_by: Option<String>,
    #[allow(dead_code)]
    object: String,
}

/// API response wrapper from Groq
#[derive(Debug, Deserialize)]
struct GroqApiResponse {
    data: Vec<GroqApiModel>,
}

/// Check if model is a chat-capable model (filter out whisper, etc.)
fn is_chat_model(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    // Exclude speech, tool-use specific, guard and embedding models
    !id.contains("whisper")
        && !id.contains("orpheus")
        && !id.contains("tts")
        && !id.contains("embed")
        && !id.contains("guard")
        && !id.contains("tool-use")
}

/// List chat models available to the selected provider credential.
#[command]
pub async fn get_groq_models(
    state: tauri::State<'_, crate::state::AppState>,
    api_key: Option<String>,
) -> Result<Vec<GroqModel>, String> {
    // The webview sends a key only when the user just typed one; otherwise the
    // stored key is used here and never leaves the core.
    let api_key = crate::database::repositories::setting::SettingsRepository::effective_api_key(
        state.db_manager.pool(),
        "groq",
        api_key,
    )
    .await
    .map_err(|_| "Failed to read the saved API key".to_string())?;
    fetch_groq_models(api_key).await
}

/// The model list for a key the core already holds.
async fn fetch_groq_models(api_key: Option<String>) -> Result<Vec<GroqModel>, String> {
    let api_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or("missing_api_key")?;
    let client = model_catalog::client()?;
    let request = client
        .get("https://api.groq.com/openai/v1/models")
        .header("Authorization", format!("Bearer {}", api_key));
    let api_response: GroqApiResponse = model_catalog::read(request).await?;

    // Filter to only chat models and map to our struct
    let models: Vec<GroqModel> = api_response
        .data
        .into_iter()
        .filter(|m| is_chat_model(&m.id))
        .map(|m| GroqModel {
            id: m.id,
            owned_by: m.owned_by,
        })
        .collect();

    Ok(models)
}
