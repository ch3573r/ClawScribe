use super::{
    classify_status_with_body, should_retry_status, CloudTranscriptSegment,
    CloudTranscriptionError, CloudTranscriptionProvider,
};
use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

const MAX_ATTEMPTS: usize = 3;
const API_VERSION: &str = "2025-10-15";
const MAX_SINGLE_PHRASE_SECONDS: f64 = 90.0;
const MAX_SINGLE_PHRASE_WORDS: usize = 120;

#[derive(Debug, Clone)]
pub struct MaiTranscribeProvider {
    endpoint: String,
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl MaiTranscribeProvider {
    pub fn new(endpoint: String, api_key: String, model: String) -> Self {
        Self {
            endpoint,
            api_key,
            model,
            client: reqwest::Client::new(),
        }
    }

    fn endpoint(&self) -> String {
        fast_transcription_endpoint(&self.endpoint)
    }

    fn form(
        &self,
        audio: Vec<u8>,
        file_name: &str,
        mime_type: &str,
        language: Option<&str>,
    ) -> Result<Form, CloudTranscriptionError> {
        let audio = Part::bytes(audio)
            .file_name(file_name.to_string())
            .mime_str(mime_type)
            .map_err(|e| {
                CloudTranscriptionError::auth_config(format!("Invalid audio MIME type: {e}"))
            })?;
        let mut definition = json!({
            "enhancedMode": {
                "enabled": true,
                "model": self.model
            }
        });
        if let Some(language) = language.map(str::trim).filter(|value| !value.is_empty()) {
            definition["locales"] = json!([normalize_mai_locale(language)]);
        }
        let definition = Part::text(definition.to_string())
            .mime_str("application/json")
            .map_err(|e| {
                CloudTranscriptionError::auth_config(format!(
                    "Invalid MAI definition MIME type: {e}"
                ))
            })?;

        Ok(Form::new()
            .part("audio", audio)
            .part("definition", definition))
    }
}

#[async_trait]
impl CloudTranscriptionProvider for MaiTranscribeProvider {
    async fn transcribe_file(
        &self,
        audio: Vec<u8>,
        file_name: &str,
        mime_type: &str,
        language: Option<&str>,
    ) -> Result<Vec<CloudTranscriptSegment>, CloudTranscriptionError> {
        for attempt in 1..=MAX_ATTEMPTS {
            let response = self
                .client
                .post(self.endpoint())
                .header("Ocp-Apim-Subscription-Key", &self.api_key)
                .multipart(self.form(audio.clone(), file_name, mime_type, language)?)
                .send()
                .await;

            match response {
                Ok(response) if response.status().is_success() => {
                    let payload = response
                        .json::<AzureFastTranscriptionResponse>()
                        .await
                        .map_err(|e| {
                            CloudTranscriptionError::auth_config(format!(
                                "Azure Speech transcription response was not valid JSON: {e}"
                            ))
                        })?;
                    return parse_fast_transcription(payload);
                }
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    let error = classify_status_with_body(status, "Azure Speech", &body);
                    if attempt < MAX_ATTEMPTS && should_retry_status(status) {
                        tokio::time::sleep(retry_delay(attempt)).await;
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => {
                    if attempt < MAX_ATTEMPTS {
                        tokio::time::sleep(retry_delay(attempt)).await;
                        continue;
                    }
                    return Err(CloudTranscriptionError::transient(format!(
                        "Azure Speech transcription request failed: {error}"
                    )));
                }
            }
        }

        Err(CloudTranscriptionError::transient(
            "Azure Speech transcription failed after retries",
        ))
    }
}

pub fn fast_transcription_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    if trimmed.contains("/speechtotext/transcriptions:transcribe") {
        if trimmed.contains("?") {
            trimmed.to_string()
        } else {
            format!("{trimmed}?api-version={API_VERSION}")
        }
    } else {
        format!("{trimmed}/speechtotext/transcriptions:transcribe?api-version={API_VERSION}")
    }
}

fn normalize_mai_locale(language: &str) -> String {
    language
        .split(['-', '_'])
        .next()
        .unwrap_or(language)
        .to_ascii_lowercase()
}

fn retry_delay(attempt: usize) -> Duration {
    Duration::from_millis(500 * attempt as u64)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AzureFastTranscriptionResponse {
    duration_milliseconds: Option<f64>,
    combined_phrases: Option<Vec<AzureCombinedPhrase>>,
    phrases: Option<Vec<AzurePhrase>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AzureCombinedPhrase {
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AzurePhrase {
    text: String,
    offset_milliseconds: f64,
    duration_milliseconds: f64,
}

pub(crate) fn parse_fast_transcription(
    payload: AzureFastTranscriptionResponse,
) -> Result<Vec<CloudTranscriptSegment>, CloudTranscriptionError> {
    let AzureFastTranscriptionResponse {
        duration_milliseconds,
        combined_phrases,
        phrases,
    } = payload;

    if let Some(phrases) = phrases.filter(|phrases| !phrases.is_empty()) {
        let mut segments = phrases
            .into_iter()
            .filter(|phrase| !phrase.text.trim().is_empty())
            .map(|phrase| {
                let start_seconds = phrase.offset_milliseconds / 1000.0;
                let end_seconds = start_seconds + phrase.duration_milliseconds.max(0.0) / 1000.0;
                CloudTranscriptSegment {
                    text: phrase.text,
                    start_seconds,
                    end_seconds,
                    words: None,
                    requires_local_timing_grid: false,
                }
            })
            .collect::<Vec<_>>();
        if !segments.is_empty() {
            mark_single_long_phrase_for_local_timing_grid(&mut segments);
            return Ok(segments);
        }
    }

    if let Some(text) = combined_phrases
        .and_then(|phrases| phrases.into_iter().next())
        .map(|phrase| phrase.text)
        .filter(|text| !text.trim().is_empty())
    {
        return Ok(vec![CloudTranscriptSegment {
            text,
            start_seconds: 0.0,
            end_seconds: duration_milliseconds.unwrap_or_default().max(0.0) / 1000.0,
            words: None,
            requires_local_timing_grid: true,
        }]);
    }

    Err(CloudTranscriptionError::auth_config(
        "Azure Speech transcription response did not contain transcript phrases",
    ))
}

fn mark_single_long_phrase_for_local_timing_grid(segments: &mut [CloudTranscriptSegment]) {
    if segments.len() != 1 {
        return;
    }

    let segment = &segments[0];
    let duration_seconds = (segment.end_seconds - segment.start_seconds).max(0.0);
    let words = word_count(&segment.text);
    if duration_seconds >= MAX_SINGLE_PHRASE_SECONDS && words >= MAX_SINGLE_PHRASE_WORDS {
        segments[0].requires_local_timing_grid = true;
    }
}

fn word_count(text: &str) -> usize {
    text.split_whitespace()
        .filter(|word| !word.trim().is_empty())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn appends_fast_transcription_path_and_api_version() {
        assert_eq!(
            fast_transcription_endpoint("https://example.cognitiveservices.azure.com"),
            "https://example.cognitiveservices.azure.com/speechtotext/transcriptions:transcribe?api-version=2025-10-15"
        );
        assert_eq!(
            fast_transcription_endpoint("https://example.cognitiveservices.azure.com/speechtotext/transcriptions:transcribe"),
            "https://example.cognitiveservices.azure.com/speechtotext/transcriptions:transcribe?api-version=2025-10-15"
        );
    }

    #[test]
    fn parses_phrase_level_timing_without_words() {
        let payload = AzureFastTranscriptionResponse {
            duration_milliseconds: Some(2_500.0),
            combined_phrases: None,
            phrases: Some(vec![
                AzurePhrase {
                    text: "hello there".to_string(),
                    offset_milliseconds: 1000.0,
                    duration_milliseconds: 900.0,
                },
                AzurePhrase {
                    text: "goodbye".to_string(),
                    offset_milliseconds: 2000.0,
                    duration_milliseconds: 500.0,
                },
            ]),
        };

        let parsed = parse_fast_transcription(payload).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].start_seconds, 1.0);
        assert_eq!(parsed[0].end_seconds, 1.9);
        assert!(parsed.iter().all(|segment| segment.words.is_none()));
        assert!(parsed
            .iter()
            .all(|segment| !segment.requires_local_timing_grid));
    }

    #[test]
    fn combined_only_transcript_requires_local_timing_grid() {
        let payload = AzureFastTranscriptionResponse {
            duration_milliseconds: Some(123_000.0),
            combined_phrases: Some(vec![AzureCombinedPhrase {
                text: "one big combined transcript".to_string(),
            }]),
            phrases: None,
        };

        let parsed = parse_fast_transcription(payload).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].end_seconds, 123.0);
        assert!(parsed[0].words.is_none());
        assert!(parsed[0].requires_local_timing_grid);
    }

    #[test]
    fn single_long_phrase_requires_local_timing_grid() {
        let payload = AzureFastTranscriptionResponse {
            duration_milliseconds: Some(180_000.0),
            combined_phrases: None,
            phrases: Some(vec![AzurePhrase {
                text: std::iter::repeat("word")
                    .take(130)
                    .collect::<Vec<_>>()
                    .join(" "),
                offset_milliseconds: 0.0,
                duration_milliseconds: 180_000.0,
            }]),
        };

        let parsed = parse_fast_transcription(payload).unwrap();

        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].requires_local_timing_grid);
    }

    #[tokio::test]
    async fn mocked_http_request_uses_fast_transcription_enhanced_mode() {
        let body = r#"{
            "durationMilliseconds": 1200,
            "phrases": [
                {
                    "text": "hello there",
                    "offsetMilliseconds": 100,
                    "durationMilliseconds": 900
                }
            ]
        }"#;
        let (endpoint, request_handle) =
            spawn_one_response_server("HTTP/1.1 200 OK", body, "application/json").await;
        let provider = MaiTranscribeProvider::new(
            endpoint,
            "speech-key".to_string(),
            "mai-transcribe-1.5".to_string(),
        );

        let segments = provider
            .transcribe_file(b"abc".to_vec(), "audio.wav", "audio/wav", Some("en-US"))
            .await
            .unwrap();
        let request = request_handle.await.unwrap();

        assert_eq!(segments.len(), 1);
        assert!(segments[0].words.is_none());
        assert!(request
            .starts_with("POST /speechtotext/transcriptions:transcribe?api-version=2025-10-15 "));
        assert!(request.contains(r#"name="audio""#));
        assert!(request.contains(r#"name="definition""#));
        assert!(request.contains("enhancedMode"));
        assert!(request.contains("mai-transcribe-1.5"));
        assert!(request.contains("locales"));
        assert!(request.contains("en"));
    }

    #[tokio::test]
    async fn mocked_http_bad_request_is_auth_config() {
        let (endpoint, request_handle) =
            spawn_one_response_server("HTTP/1.1 400 Bad Request", "{}", "application/json").await;
        let provider = MaiTranscribeProvider::new(
            endpoint,
            "speech-key".to_string(),
            "mai-transcribe-1.5".to_string(),
        );

        let error = provider
            .transcribe_file(b"abc".to_vec(), "audio.wav", "audio/wav", None)
            .await
            .unwrap_err();
        let _ = request_handle.await.unwrap();

        assert_eq!(
            error.category(),
            super::super::CloudFallbackReasonCategory::AuthConfig
        );
    }

    async fn spawn_one_response_server(
        status_line: &'static str,
        body: &'static str,
        content_type: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request_complete(&request) {
                    break;
                }
            }
            let response = format!(
                "{status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&request).to_string()
        });

        (format!("http://{addr}"), handle)
    }

    fn request_complete(request: &[u8]) -> bool {
        let Some(header_end) = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
        else {
            return false;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        request.len() >= header_end + content_length
    }
}
