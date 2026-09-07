use reqwest::{header, Client};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::info;

const REQUEST_TIMEOUT_DURATION: Duration = Duration::from_secs(300);

// Generic structure for OpenAI-compatible API chat messages
#[derive(Debug, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

// Generic structure for OpenAI-compatible API chat requests
#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

// Claude-specific request structure
#[derive(Debug, Serialize)]
pub struct ClaudeRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: String,
    pub messages: Vec<ChatMessage>,
}

/// LLM Provider enumeration for multi-provider support
#[derive(Debug, Clone, PartialEq)]
pub enum LLMProvider {
    OpenAI,
    Claude,
    Groq,
    Ollama,
    OpenRouter,
    BuiltInAI,
    OpenAICompatible,
    CustomOpenAI,
    OpenClaw,
    Codex,
}

impl LLMProvider {
    /// Parse provider from string (case-insensitive)
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(Self::OpenAI),
            "claude" => Ok(Self::Claude),
            "groq" => Ok(Self::Groq),
            "ollama" => Ok(Self::Ollama),
            "openrouter" => Ok(Self::OpenRouter),
            "builtin-ai" | "local-llama" | "localllama" => Ok(Self::BuiltInAI),
            "openai-compatible" | "api-key" => Ok(Self::OpenAICompatible),
            "custom-openai" => Ok(Self::CustomOpenAI),
            "openclaw" | "openclaw-managed" => Ok(Self::OpenClaw),
            "codex" | "codex-login" | "codex-chatgpt" => Ok(Self::Codex),
            _ => Err(format!("Unsupported LLM provider: {}", s)),
        }
    }
}

/// Generates a summary using the specified LLM provider
///
/// # Arguments
/// * `client` - Reqwest HTTP client (reused for performance)
/// * `provider` - The LLM provider to use
/// * `model_name` - The specific model to use (e.g., "gpt-4", "claude-3-opus")
/// * `api_key` - API key for the provider (not needed for Ollama)
/// * `system_prompt` - System instructions for the LLM
/// * `user_prompt` - User query/content to process
/// * `ollama_endpoint` - Optional custom Ollama endpoint (defaults to localhost:11434)
/// * `custom_openai_endpoint` - Optional custom OpenAI-compatible endpoint
/// * `max_tokens` - Optional max tokens (for CustomOpenAI provider)
/// * `temperature` - Optional temperature (for CustomOpenAI provider)
/// * `top_p` - Optional top_p (for CustomOpenAI provider)
/// * `app_data_dir` - Optional app data directory (for BuiltInAI provider)
/// * `cancellation_token` - Optional token to cancel the request
///
/// # Returns
/// The generated summary text or an error message
pub async fn generate_summary(
    client: &Client,
    provider: &LLMProvider,
    model_name: &str,
    api_key: &str,
    system_prompt: &str,
    user_prompt: &str,
    ollama_endpoint: Option<&str>,
    custom_openai_endpoint: Option<&str>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    app_data_dir: Option<&PathBuf>,
    cancellation_token: Option<&CancellationToken>,
) -> Result<String, String> {
    // Check if cancelled before starting
    if let Some(token) = cancellation_token {
        if token.is_cancelled() {
            return Err("Summary generation was cancelled".to_string());
        }
    }

    // Handle BuiltInAI provider separately (uses local sidecar, no HTTP API)
    if provider == &LLMProvider::BuiltInAI {
        let app_data_dir = app_data_dir
            .ok_or_else(|| "app_data_dir is required for BuiltInAI provider".to_string())?;

        return crate::summary::summary_engine::generate_with_builtin(
            app_data_dir,
            model_name,
            system_prompt,
            user_prompt,
            cancellation_token,
        )
        .await
        .map_err(|e| e.to_string());
    }

    let (api_url, mut headers) = match provider {
        LLMProvider::OpenAI => (
            "https://api.openai.com/v1/chat/completions".to_string(),
            header::HeaderMap::new(),
        ),
        LLMProvider::Groq => (
            "https://api.groq.com/openai/v1/chat/completions".to_string(),
            header::HeaderMap::new(),
        ),
        LLMProvider::OpenRouter => (
            "https://openrouter.ai/api/v1/chat/completions".to_string(),
            header::HeaderMap::new(),
        ),
        LLMProvider::Ollama => {
            let host = ollama_endpoint
                .map(|s| s.to_string())
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            (
                format!("{}/api/chat", host.trim_end_matches('/')),
                header::HeaderMap::new(),
            )
        }
        LLMProvider::CustomOpenAI => {
            let endpoint = custom_openai_endpoint
                .ok_or_else(|| "Custom OpenAI endpoint not configured".to_string())?;
            (
                format!("{}/chat/completions", endpoint.trim_end_matches('/')),
                header::HeaderMap::new(),
            )
        }
        LLMProvider::OpenAICompatible => {
            let endpoint = custom_openai_endpoint
                .ok_or_else(|| "OpenAI-compatible endpoint not configured".to_string())?;
            (
                format!("{}/chat/completions", endpoint.trim_end_matches('/')),
                header::HeaderMap::new(),
            )
        }
        LLMProvider::OpenClaw => {
            let endpoint = custom_openai_endpoint
                .ok_or_else(|| "OpenClaw model endpoint not configured".to_string())?;
            (
                endpoint.trim_end_matches('/').to_string(),
                header::HeaderMap::new(),
            )
        }
        LLMProvider::Claude => {
            let mut header_map = header::HeaderMap::new();
            header_map.insert(
                "x-api-key",
                api_key
                    .parse()
                    .map_err(|_| "Invalid API key format".to_string())?,
            );
            header_map.insert(
                "anthropic-version",
                "2023-06-01"
                    .parse()
                    .map_err(|_| "Invalid anthropic version".to_string())?,
            );
            (
                "https://api.anthropic.com/v1/messages".to_string(),
                header_map,
            )
        }
        LLMProvider::BuiltInAI => {
            // This case is handled earlier with early returns
            unreachable!("BuiltInAI is handled before this match statement")
        }
        LLMProvider::Codex => {
            return Err(
                "Codex provider is handled by CodexProcessingProvider, not the HTTP LLM client"
                    .to_string(),
            );
        }
    };

    // Add authorization header for non-Claude providers
    if provider != &LLMProvider::Claude {
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", api_key)
                .parse()
                .map_err(|_| "Invalid authorization header".to_string())?,
        );
    }
    headers.insert(
        header::CONTENT_TYPE,
        "application/json"
            .parse()
            .map_err(|_| "Invalid content type".to_string())?,
    );

    // Build request body based on provider
    let output_tokens = max_tokens.unwrap_or(super::context_budget::DEFAULT_OUTPUT_TOKENS as u32);
    let request_body = if provider == &LLMProvider::Ollama {
        let context = super::context_budget::ollama_context(model_name, ollama_endpoint).await;
        let budget = super::context_budget::input_budget(
            context,
            output_tokens as usize,
            system_prompt.len(),
        )?;
        if user_prompt.len() > budget {
            return Err(
                "Meeting request exceeds the configured Ollama context. Shorten the prompt.".into(),
            );
        }
        serde_json::json!({"model": model_name, "stream": false,
            "messages": [{"role": "system", "content": system_prompt}, {"role": "user", "content": user_prompt}],
            "options": {"num_ctx": context, "num_predict": output_tokens}})
    } else if provider != &LLMProvider::Claude {
        // For operator-managed OpenAI-compatible endpoints, apply optional parameters if provided.
        let (max_tokens_val, temperature_val, top_p_val) = if matches!(
            provider,
            LLMProvider::CustomOpenAI | LLMProvider::OpenAICompatible | LLMProvider::OpenClaw
        ) {
            (max_tokens, temperature, top_p)
        } else {
            (None, None, None)
        };

        serde_json::json!(ChatRequest {
            model: model_name.to_string(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user_prompt.to_string(),
                }
            ],
            max_tokens: Some(max_tokens_val.unwrap_or(output_tokens)),
            temperature: temperature_val,
            top_p: top_p_val,
        })
    } else {
        serde_json::json!(ClaudeRequest {
            system: system_prompt.to_string(),
            model: model_name.to_string(),
            max_tokens: 2048,
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: user_prompt.to_string(),
            }]
        })
    };

    info!(
        "🐞 LLM Request to {}: model={}",
        provider_name(provider),
        model_name
    );

    with_cancellation(cancellation_token, async {
        let response = client
            .post(api_url)
            .headers(headers)
            .json(&request_body)
            .timeout(REQUEST_TIMEOUT_DURATION)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    format!(
                        "LLM request timed out after {} seconds",
                        REQUEST_TIMEOUT_DURATION.as_secs()
                    )
                } else {
                    format!("Failed to send request to LLM: {}", error.without_url())
                }
            })?;
        if !response.status().is_success() {
            return Err(format!(
                "LLM API request failed with HTTP {}. Check provider settings and retry.",
                response.status().as_u16()
            ));
        }
        parse_response(provider, read_response_json(response).await?)
    })
    .await
}

/// Cancellation covers the entire operation, including a response body that may
/// arrive long after its HTTP headers. Dropping the future closes its request.
pub(crate) async fn with_cancellation<T>(
    token: Option<&CancellationToken>,
    operation: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    if let Some(token) = token {
        tokio::select! {
            biased;
            _ = token.cancelled() => Err("Summary generation was cancelled".into()),
            result = operation => result,
        }
    } else {
        operation.await
    }
}

/// Bound provider responses before JSON parsing; summaries must not allocate an
/// unlimited response from a misconfigured endpoint.
pub(crate) async fn read_response_json(
    mut response: reqwest::Response,
) -> Result<serde_json::Value, String> {
    const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Failed to read LLM response: {}", error.without_url()))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(
                "LLM response exceeded the 8 MiB limit. Reduce the output limit and retry.".into(),
            );
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| "Invalid LLM response JSON".into())
}

pub(crate) fn parse_response(
    provider: &LLMProvider,
    response: serde_json::Value,
) -> Result<String, String> {
    let reason = match provider {
        LLMProvider::Claude => response.get("stop_reason"),
        LLMProvider::Ollama => response.get("done_reason"),
        _ => response.pointer("/choices/0/finish_reason"),
    }
    .and_then(serde_json::Value::as_str);
    if matches!(
        reason,
        Some("length" | "max_tokens" | "model_context_window_exceeded")
    ) {
        return Err("The model reached its output or context limit before finishing. No complete response was produced. Increase the output/context limit or choose a more concise template and retry.".into());
    }
    if matches!(
        reason,
        Some("content_filter" | "refusal" | "tool_calls" | "function_call")
    ) {
        return Err(
            "The model did not complete a text response. Check provider settings and retry.".into(),
        );
    }
    let content = match provider {
        LLMProvider::Claude => response
            .get("content")
            .and_then(serde_json::Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        LLMProvider::Ollama => response
            .pointer("/message/content")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        _ => response
            .pointer("/choices/0/message/content")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    };
    content
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| "The model returned no usable text response.".into())
}

/// Helper function to get provider name for logging
fn provider_name(provider: &LLMProvider) -> &str {
    match provider {
        LLMProvider::OpenAI => "OpenAI",
        LLMProvider::Claude => "Claude",
        LLMProvider::Groq => "Groq",
        LLMProvider::Ollama => "Ollama",
        LLMProvider::BuiltInAI => "Built-in AI",
        LLMProvider::OpenRouter => "OpenRouter",
        LLMProvider::OpenAICompatible => "OpenAI-compatible",
        LLMProvider::CustomOpenAI => "Custom OpenAI",
        LLMProvider::OpenClaw => "OpenClaw managed auth",
        LLMProvider::Codex => "Codex",
    }
}

/// Shared provider resolution for meeting chat and reviewed task polishing.
pub(crate) async fn generate_configured_text<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &tauri::State<'_, crate::state::AppState>,
    model: &str,
    model_name: &str,
    system: &str,
    user: &str,
) -> Result<String, String> {
    use super::openai_provider::{
        config_from_custom_openai, config_from_openai_api_key, OpenAICompatibleProcessingProvider,
    };
    use crate::database::repositories::setting::SettingsRepository;
    use tauri::Manager;

    let provider = LLMProvider::from_str(model)?;

    if matches!(provider, LLMProvider::Codex) {
        let codex = crate::summary::codex_provider::provider_from_app(app)
            .map_err(|e| format!("Codex app-server unavailable: {e}"))?;
        return codex.run_text_prompt(&format!("{system}\n\n{user}")).await;
    }

    let pool = state.db_manager.pool().clone();
    if matches!(
        provider,
        LLMProvider::CustomOpenAI | LLMProvider::OpenAICompatible
    ) {
        let configured = SettingsRepository::get_custom_openai_config(&pool)
            .await
            .map_err(|e| format!("Failed to read OpenAI-compatible config: {e}"))?;
        let mut config = match configured {
            Some(config) => config_from_custom_openai(config),
            None if provider == LLMProvider::OpenAICompatible => {
                let key = SettingsRepository::get_api_key(&pool, "openai")
                    .await
                    .map_err(|e| format!("Failed to read OpenAI API key: {e}"))?
                    .filter(|key| !key.trim().is_empty())
                    .ok_or(
                        "OpenAI-compatible chat requires an OpenAI API key or configured endpoint",
                    )?;
                config_from_openai_api_key(Some(key), model_name.to_string())
            }
            None => return Err("No OpenAI-compatible configuration found".into()),
        };
        config.model = model_name.to_string();
        return OpenAICompatibleProcessingProvider::new(config)?
            .send_text_prompt(system, user)
            .await;
    }
    let mut api_key = String::new();
    let mut ollama_endpoint: Option<String> = None;
    let mut custom_openai_endpoint: Option<String> = None;

    match provider {
        LLMProvider::Ollama | LLMProvider::BuiltInAI => {}
        LLMProvider::OpenClaw => {
            let cfg = crate::openclaw::load_config(app)
                .map_err(|e| format!("Failed to load OpenClaw config: {e}"))?;
            if !cfg.enabled || cfg.bearer_token.trim().is_empty() {
                return Err("OpenClaw handoff is disabled or missing a bearer token.".to_string());
            }
            custom_openai_endpoint = Some(cfg.model_endpoint);
            api_key = cfg.bearer_token;
        }
        _ => {
            api_key = SettingsRepository::get_api_key(&pool, model)
                .await
                .map_err(|e| format!("Failed to read API key: {e}"))?
                .filter(|k| !k.is_empty())
                .ok_or_else(|| format!("API key not found for {model}"))?;
        }
    }

    if provider == LLMProvider::Ollama {
        ollama_endpoint = SettingsRepository::get_model_config(&pool)
            .await
            .ok()
            .flatten()
            .and_then(|c| c.ollama_endpoint);
    }

    let app_data_dir = app.path().app_data_dir().ok();
    let client = reqwest::Client::new();
    generate_summary(
        &client,
        &provider,
        model_name,
        &api_key,
        system,
        user,
        ollama_endpoint.as_deref(),
        custom_openai_endpoint.as_deref(),
        None,
        None,
        None,
        app_data_dir.as_ref(),
        None,
    )
    .await
}

#[cfg(test)]
mod response_tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn incomplete_outputs_are_never_reported_as_complete() {
        for (provider, response) in [
            (
                LLMProvider::OpenAICompatible,
                json!({"choices":[{"message":{"content":"A partial decision"},"finish_reason":"length"}]}),
            ),
            (
                LLMProvider::Claude,
                json!({"content":[{"text":"A partial decision"}],"stop_reason":"max_tokens"}),
            ),
            (
                LLMProvider::Ollama,
                json!({"message":{"content":"A partial decision"},"done_reason":"length"}),
            ),
        ] {
            assert!(parse_response(&provider, response)
                .unwrap_err()
                .contains("before finishing"));
        }
    }

    #[test]
    fn claude_keeps_every_text_block_without_exposing_thinking() {
        assert_eq!(
            parse_response(
                &LLMProvider::Claude,
                json!({
                    "content":[{"type":"thinking","thinking":"private reasoning"},
                        {"type":"text","text":"Decision: postpone."},
                        {"type":"text","text":"Owner: not specified."}],
                    "stop_reason":"end_turn"
                })
            )
            .unwrap(),
            "Decision: postpone.\nOwner: not specified."
        );
    }

    #[test]
    fn compatible_endpoints_may_omit_finish_reason_but_not_content() {
        assert_eq!(
            parse_response(
                &LLMProvider::CustomOpenAI,
                json!({
                    "choices":[{"message":{"content":"  Complete answer  "}}]
                })
            )
            .unwrap(),
            "Complete answer"
        );
        assert!(parse_response(
            &LLMProvider::CustomOpenAI,
            json!({
                "choices":[{"message":{"content":null},"finish_reason":"stop"}]
            })
        )
        .is_err());
    }

    #[tokio::test]
    async fn cancellation_aborts_a_body_after_response_headers_arrive() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (headers_tx, headers_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            socket.read(&mut request).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1024\r\nContent-Type: application/json\r\n\r\n{").await.unwrap();
            headers_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        let token = CancellationToken::new();
        let cancel = token.clone();
        let request = tokio::spawn(async move {
            generate_summary(
                &Client::new(),
                &LLMProvider::CustomOpenAI,
                "test",
                "",
                "system",
                "question",
                None,
                Some(&endpoint),
                None,
                None,
                None,
                None,
                Some(&token),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), headers_rx)
            .await
            .unwrap()
            .unwrap();
        // Let reqwest consume the headers; the response body remains incomplete.
        tokio::time::sleep(Duration::from_millis(25)).await;
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), request).await;
        server.abort();
        assert!(result.unwrap().unwrap().unwrap_err().contains("cancelled"));
    }
}
