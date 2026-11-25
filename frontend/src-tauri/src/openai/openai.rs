use crate::database::repositories::setting::SettingsRepository;
use crate::openai::{model_catalog, secret_destination::same_destination};
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
    custom_endpoint: Option<String>,
) -> Result<Vec<OpenAIModel>, String> {
    let pool = state.db_manager.pool();
    if let Some(endpoint) = custom_endpoint {
        if !is_official_openai_endpoint(&endpoint) {
            return Err("catalog_unavailable".into());
        }
        let config = SettingsRepository::get_custom_openai_config(pool)
            .await
            .map_err(|_| "catalog_unavailable")?
            .filter(|config| {
                is_official_openai_endpoint(&config.endpoint)
                    && same_destination(&config.endpoint, &endpoint)
            });
        let (stored_key, organization, project) = config
            .map(|config| (config.api_key, config.organization, config.project))
            .unwrap_or_default();
        return fetch_openai_models(api_key.or(stored_key), organization, project).await;
    }
    let key = SettingsRepository::effective_api_key(pool, "openai", api_key)
        .await
        .map_err(|_| "Failed to read the saved API key".to_string())?;
    fetch_openai_models(key, None, None).await
}

fn is_official_openai_endpoint(value: &str) -> bool {
    url::Url::parse(value.trim()).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("api.openai.com")
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && matches!(
                url.path().trim_end_matches('/'),
                "" | "/v1" | "/v1/chat/completions" | "/v1/responses"
            )
    })
}

async fn fetch_openai_models(
    api_key: Option<String>,
    organization: Option<String>,
    project: Option<String>,
) -> Result<Vec<OpenAIModel>, String> {
    let api_key = api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or("missing_api_key")?;
    let client = model_catalog::client()?;
    let mut request = client
        .get("https://api.openai.com/v1/models")
        .bearer_auth(api_key);
    if let Some(organization) = organization {
        request = request.header("OpenAI-Organization", organization);
    }
    if let Some(project) = project {
        request = request.header("OpenAI-Project", project);
    }
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

#[cfg(test)]
mod tests {
    use super::is_official_openai_endpoint;

    #[test]
    fn custom_catalog_accepts_only_official_https_endpoints() {
        for accepted in [
            "https://api.openai.com",
            "https://api.openai.com:443/v1/",
            "https://api.openai.com/v1/chat/completions",
            "https://api.openai.com/v1/responses",
        ] {
            assert!(is_official_openai_endpoint(accepted), "{accepted}");
        }
        for rejected in [
            "http://api.openai.com/v1",
            "https://api.openai.com.example.com/v1",
            "https://api.openai.com:8443/v1",
            "https://api.openai.com@elsewhere.example/v1",
            "https://user@api.openai.com/v1",
            "https://api.openai.com/v1?redirect=example",
            "https://api.openai.com/other/v1",
        ] {
            assert!(!is_official_openai_endpoint(rejected), "{rejected}");
        }
    }
}
