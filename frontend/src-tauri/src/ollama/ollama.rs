use futures_util::StreamExt;
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::process::Command;
use std::sync::Mutex;
use tauri::{command, AppHandle, Emitter, Runtime};
use tokio::time::{sleep, timeout, Duration};

static DOWNLOADING_MODELS: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

struct ModelDownload<'a>(&'a str);

impl<'a> ModelDownload<'a> {
    fn acquire(model_name: &'a str) -> Result<Self, String> {
        let mut downloading = DOWNLOADING_MODELS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !downloading.insert(model_name.to_owned()) {
            return Err(format!("Model {model_name} is already being downloaded"));
        }
        Ok(Self(model_name))
    }
}

impl Drop for ModelDownload<'_> {
    fn drop(&mut self) {
        // No lock is held while downloading. Dropping a cancelled future
        // releases the entry just like a normal return.
        DOWNLOADING_MODELS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(self.0);
    }
}

// Error categorization for better error handling and user feedback
#[derive(Debug)]
pub enum OllamaError {
    Timeout,
    NetworkError(String),
    InvalidEndpoint(String),
    ServerError(String),
    NoModelsFound,
    ParseError(String),
}

impl std::fmt::Display for OllamaError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            OllamaError::Timeout => write!(f, "Request timed out after 5 seconds. Please check if the Ollama server is running."),
            OllamaError::NetworkError(msg) => write!(f, "Network error: {}. Please check your connection and endpoint URL.", msg),
            OllamaError::InvalidEndpoint(msg) => write!(f, "Invalid endpoint: {}. Please check the URL format.", msg),
            OllamaError::ServerError(msg) => write!(f, "Ollama server error: {}", msg),
            OllamaError::NoModelsFound => write!(f, "No models found on the Ollama server. Please pull models using 'ollama pull <model>'."),
            OllamaError::ParseError(msg) => write!(f, "Failed to parse server response: {}", msg),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OllamaModel {
    pub name: String,
    pub id: String,
    pub size: String,
    pub modified: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaApiResponse {
    models: Vec<OllamaApiModel>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaApiModel {
    name: String,
    model: String,
    modified_at: String,
    size: i64,
}

// Helper function to check if endpoint is localhost
fn is_localhost_endpoint(endpoint: Option<&str>) -> bool {
    match endpoint {
        None | Some("") => true,
        Some(url) => url.contains("localhost") || url.contains("127.0.0.1") || url.contains("::1"),
    }
}

// Helper function to validate endpoint URL format
fn validate_endpoint_url(url: &str) -> Result<(), OllamaError> {
    if url.is_empty() {
        return Ok(()); // Empty is valid (uses default)
    }

    // Check if URL starts with http:// or https://
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(OllamaError::InvalidEndpoint(
            "URL must start with http:// or https://".to_string(),
        ));
    }

    url::Url::parse(url)
        .map(|_| ())
        .map_err(|_| OllamaError::InvalidEndpoint("URL must include a valid host".to_owned()))
}

#[command]
pub async fn get_ollama_models(endpoint: Option<String>) -> Result<Vec<OllamaModel>, String> {
    // Validate endpoint format if provided
    if let Some(ref ep) = endpoint {
        if let Err(e) = validate_endpoint_url(ep) {
            return Err(e.to_string());
        }
    }

    // Add timeout wrapper (5 seconds max)
    match timeout(
        Duration::from_secs(5),
        get_models_via_http_with_retry(endpoint.as_deref()),
    )
    .await
    {
        Ok(Ok(models)) => {
            if models.is_empty() {
                Err(OllamaError::NoModelsFound.to_string())
            } else {
                Ok(models)
            }
        }
        Ok(Err(http_err)) => {
            // Only fallback to CLI if endpoint is localhost/empty
            if is_localhost_endpoint(endpoint.as_deref()) {
                get_models_via_cli()
                    .map_err(|cli_err| format!("{}\n\nAlso tried CLI: {}", http_err, cli_err))
            } else {
                Err(http_err)
            }
        }
        Err(_) => Err(OllamaError::Timeout.to_string()),
    }
}

// HTTP request with retry logic and exponential backoff
async fn get_models_via_http_with_retry(
    endpoint: Option<&str>,
) -> Result<Vec<OllamaModel>, String> {
    const MAX_RETRIES: u32 = 2;
    const INITIAL_BACKOFF_MS: u64 = 300;

    let mut last_error = String::new();

    for attempt in 0..=MAX_RETRIES {
        match get_models_via_http_async(endpoint).await {
            Ok(models) => return Ok(models),
            Err(e) => {
                last_error = e.clone();

                // Don't retry on certain errors
                if e.contains("Invalid endpoint") || e.contains("404") {
                    return Err(e);
                }

                // If not the last attempt, wait with exponential backoff
                if attempt < MAX_RETRIES {
                    let backoff_duration = INITIAL_BACKOFF_MS * 2_u64.pow(attempt);
                    sleep(Duration::from_millis(backoff_duration)).await;
                }
            }
        }
    }

    Err(format!(
        "Failed after {} retries: {}",
        MAX_RETRIES, last_error
    ))
}

async fn get_models_via_http_async(endpoint: Option<&str>) -> Result<Vec<OllamaModel>, String> {
    let client = Client::new();
    let base_url = endpoint.unwrap_or("http://localhost:11434");
    let url = format!("{}/api/tags", base_url);

    let response = client
        .get(&url)
        .timeout(Duration::from_secs(3)) // Per-request timeout
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                OllamaError::NetworkError("Connection timed out".to_string()).to_string()
            } else if e.is_connect() {
                OllamaError::NetworkError(format!(
                    "Cannot connect to {}. Please check if the server is running.",
                    base_url
                ))
                .to_string()
            } else {
                OllamaError::NetworkError(e.to_string()).to_string()
            }
        })?;

    if !response.status().is_success() {
        return Err(OllamaError::ServerError(format!(
            "HTTP {}: Server returned an error",
            response.status()
        ))
        .to_string());
    }

    let api_response: OllamaApiResponse = response
        .json()
        .await
        .map_err(|e| OllamaError::ParseError(e.to_string()).to_string())?;

    Ok(api_response
        .models
        .into_iter()
        .map(|m| OllamaModel {
            name: m.name,
            id: m.model,
            size: format_size(m.size),
            modified: m.modified_at,
        })
        .collect())
}

fn get_models_via_cli() -> Result<Vec<OllamaModel>, String> {
    let output = Command::new("ollama").arg("list").output().map_err(|e| {
        OllamaError::NetworkError(format!("Ollama CLI not found or not in PATH: {}", e)).to_string()
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(OllamaError::ServerError(format!("Ollama CLI error: {}", stderr)).to_string());
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    let mut models = Vec::new();

    // Skip the header line
    for line in output_str.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            models.push(OllamaModel {
                name: parts[0].to_string(),
                id: parts[1].to_string(),
                size: format!("{} {}", parts[2], parts[3]),
                modified: parts[4..].join(" "),
            });
        }
    }

    if models.is_empty() {
        return Err(OllamaError::NoModelsFound.to_string());
    }

    Ok(models)
}

fn format_size(size: i64) -> String {
    if size < 1024 {
        format!("{} B", size)
    } else if size < 1024 * 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else if size < 1024 * 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", size as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub status: String,
    pub completed: u64,
    pub total: u64,
}

#[command]
pub async fn pull_ollama_model<R: Runtime>(
    app_handle: AppHandle<R>,
    model_name: String,
    endpoint: Option<String>,
) -> Result<(), String> {
    pull_model(&model_name, endpoint.as_deref(), |event, payload| {
        let _ = app_handle.emit(event, payload);
    })
    .await
}

async fn pull_model(
    model_name: &str,
    endpoint: Option<&str>,
    emit: impl Fn(&str, serde_json::Value),
) -> Result<(), String> {
    let base_url = endpoint
        .filter(|url| !url.is_empty())
        .unwrap_or("http://localhost:11434");
    validate_endpoint_url(base_url).map_err(|error| error.to_string())?;
    let _download = ModelDownload::acquire(model_name)?;

    let result = download_model(model_name, base_url, |progress| {
        emit(
            "ollama-model-download-progress",
            serde_json::json!({ "modelName": model_name, "progress": progress }),
        );
    })
    .await;
    match &result {
        Ok(()) => {
            emit(
                "ollama-model-download-complete",
                serde_json::json!({ "modelName": model_name }),
            );
            log::info!("Ollama model {model_name} downloaded successfully");
        }
        Err(error) => emit(
            "ollama-model-download-error",
            serde_json::json!({ "modelName": model_name, "error": error }),
        ),
    }
    result
}

async fn download_model(
    model_name: &str,
    base_url: &str,
    report_progress: impl Fn(u8),
) -> Result<(), String> {
    let client = Client::new();
    let url = format!("{}/api/pull", base_url.trim_end_matches('/'));

    let payload = serde_json::json!({
        "name": model_name,
        "stream": true
    });

    let response = client
        .post(&url)
        .json(&payload)
        .timeout(Duration::from_secs(600)) // 10 minutes timeout for pulling
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                format!("Download timed out. The model may be large, please try using the Ollama CLI: ollama pull {}", model_name)
            } else if e.is_connect() {
                "Cannot connect to the Ollama server. Check if it is running.".to_owned()
            } else {
                format!("Failed to download model: {}", e.without_url())
            }
        })?;

    if !response.status().is_success() {
        return Err(format!(
            "Failed to pull model (HTTP {}).",
            response.status()
        ));
    }

    // Process streaming response (NDJSON format)
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut last_progress = 0u8;

    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|error| format!("Failed to read stream: {}", error.without_url()))?;

        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete lines
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim().to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }

            // Parse JSON line
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
                // Extract progress if available
                if let (Some(completed), Some(total)) = (
                    json.get("completed").and_then(|v| v.as_u64()),
                    json.get("total").and_then(|v| v.as_u64()),
                ) {
                    if total > 0 {
                        let progress =
                            (u128::from(completed.min(total)) * 100 / u128::from(total)) as u8;
                        // Ollama reports per-layer progress, which can restart at zero.
                        if progress > last_progress {
                            log::info!(
                                "Ollama download progress for {}: {}%",
                                model_name,
                                progress
                            );

                            report_progress(progress);
                            last_progress = progress;
                        }
                    }
                }

                // Check for error status
                if let Some(error) = json.get("error").and_then(|v| v.as_str()) {
                    return Err(format!("Ollama error: {error}"));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpSocket};

    #[tokio::test]
    async fn pull_can_retry_after_connection_failure() {
        let socket = TcpSocket::new_v4().unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let endpoint = format!("http://{}", socket.local_addr().unwrap());

        for _ in 0..2 {
            let error = pull_model("test-retry", Some(&endpoint), |_, _| {})
                .await
                .unwrap_err();
            assert!(error.contains("Cannot connect"), "{error}");
        }
    }

    #[tokio::test]
    async fn pull_rejects_invalid_endpoints() {
        for endpoint in ["file:///ollama", "http://"] {
            let error = pull_model("test-endpoint", Some(endpoint), |_, _| {})
                .await
                .unwrap_err();
            assert!(error.starts_with("Invalid endpoint"), "{error}");
        }
    }

    #[tokio::test]
    async fn pull_progress_stays_monotonic_when_a_new_layer_starts() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            let body = concat!(
                "{\"completed\":60,\"total\":100}\n",
                "{\"completed\":5,\"total\":100}\n",
                "{\"completed\":75,\"total\":100}\n",
                "{\"completed\":100,\"total\":100}\n",
                "{\"status\":\"success\"}\n",
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let progress = RefCell::new(Vec::new());
        pull_model("test-progress", Some(&endpoint), |event, payload| {
            if event == "ollama-model-download-progress" {
                progress
                    .borrow_mut()
                    .push(payload["progress"].as_u64().unwrap());
            }
        })
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(progress.into_inner(), [60, 75, 100]);
    }
}
