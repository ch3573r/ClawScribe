use crate::openai::model_catalog;
use serde::{Deserialize, Serialize};
use tauri::command;

/// Anthropic (Claude) model information returned to frontend
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AnthropicModel {
    pub id: String,
    pub display_name: Option<String>,
}

/// API response model from Anthropic
#[derive(Debug, Deserialize)]
struct AnthropicApiModel {
    id: String,
    display_name: Option<String>,
    #[allow(dead_code)]
    created_at: Option<String>,
}

/// API response wrapper from Anthropic
#[derive(Debug, Deserialize)]
struct AnthropicApiResponse {
    data: Vec<AnthropicApiModel>,
}

/// Check if model is a chat-capable model
fn is_chat_model(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    // Include Claude models only
    id.starts_with("claude-")
}

/// List chat models available to the selected provider credential.
#[command]
pub async fn get_anthropic_models(
    state: tauri::State<'_, crate::state::AppState>,
    api_key: Option<String>,
) -> Result<Vec<AnthropicModel>, String> {
    // The webview sends a key only when the user just typed one; otherwise the
    // stored key is used here and never leaves the core.
    let api_key = crate::database::repositories::setting::SettingsRepository::effective_api_key(
        state.db_manager.pool(),
        "claude",
        api_key,
    )
    .await
    .map_err(|_| "Failed to read the saved API key".to_string())?;
    fetch_anthropic_models(api_key).await
}

/// The model list for a key the core already holds.
async fn fetch_anthropic_models(api_key: Option<String>) -> Result<Vec<AnthropicModel>, String> {
    let api_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or("missing_api_key")?;
    let client = model_catalog::client()?;
    let request = client
        .get("https://api.anthropic.com/v1/models")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01");
    let api_response: AnthropicApiResponse = model_catalog::read(request).await?;

    // Filter to only chat models and map to our struct
    let models: Vec<AnthropicModel> = api_response
        .data
        .into_iter()
        .filter(|m| is_chat_model(&m.id))
        .map(|m| AnthropicModel {
            id: m.id,
            display_name: m.display_name,
        })
        .collect();

    Ok(models)
}
