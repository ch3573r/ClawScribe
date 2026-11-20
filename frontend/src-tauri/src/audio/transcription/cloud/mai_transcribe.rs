use super::{
    classify_status_with_body, should_retry_status, validate_provider_upload_size,
    CloudTranscriptSegment, CloudTranscriptionError, PROVIDER_MAI_TRANSCRIBE,
};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

const MAX_ATTEMPTS: usize = 3;
const API_VERSION: &str = "2025-10-15";
const MAX_SINGLE_PHRASE_SECONDS: f64 = 90.0;
const MAX_SINGLE_PHRASE_WORDS: usize = 120;
const UPLOAD_BUFFER_BYTES: usize = 64 * 1024;

async fn open_audio_body(path: &Path) -> Result<(reqwest::Body, u64), CloudTranscriptionError> {
    let file = tokio::fs::File::open(path).await.map_err(|_| {
        CloudTranscriptionError::auth_config("Could not open audio for MAI-Transcribe")
    })?;
    let metadata = file.metadata().await.map_err(|_| {
        CloudTranscriptionError::auth_config("Could not inspect audio for MAI-Transcribe")
    })?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(CloudTranscriptionError::unsupported_media(
            "MAI-Transcribe needs a non-empty audio file",
        ));
    }
    let length = metadata.len();
    validate_provider_upload_size(PROVIDER_MAI_TRANSCRIBE, None, length)?;
    let stream = ReaderStream::with_capacity(file.take(length), UPLOAD_BUFFER_BYTES);
    Ok((reqwest::Body::wrap_stream(stream), length))
}

#[derive(Clone)]
pub struct MaiTranscribeProvider {
    endpoint: String,
    api_key: String,
    model: String,
    allow_unencrypted: bool,
}

crate::credentials::debug_without_secrets!(MaiTranscribeProvider { endpoint, model, allow_unencrypted; secret api_key });

impl MaiTranscribeProvider {
    pub fn new(endpoint: String, api_key: String, model: String) -> Self {
        Self {
            endpoint,
            api_key,
            model,
            allow_unencrypted: false,
        }
    }

    pub fn with_http_opt_in(mut self, allow: bool) -> Self {
        self.allow_unencrypted = allow;
        self
    }

    fn endpoint(&self) -> String {
        fast_transcription_endpoint(&self.endpoint)
    }

    fn form(
        &self,
        body: reqwest::Body,
        length: u64,
        file_name: &str,
        mime_type: &str,
        language: Option<&str>,
    ) -> Result<Form, CloudTranscriptionError> {
        // MAI expects Content-Length on the multipart request.
        let audio = Part::stream_with_length(body, length)
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

    pub(super) async fn transcribe_path(
        &self,
        audio: &Path,
        file_name: &str,
        mime_type: &str,
        language: Option<&str>,
    ) -> Result<Vec<CloudTranscriptSegment>, CloudTranscriptionError> {
        for attempt in 1..=MAX_ATTEMPTS {
            // Reopen for each attempt. A failed request consumes its stream.
            let (body, length) = open_audio_body(audio).await?;
            let client = crate::openai::secret_destination::secret_client(
                reqwest::Client::builder(),
                &self.endpoint(),
                self.allow_unencrypted,
            )
            .await
            .map_err(CloudTranscriptionError::auth_config)?;
            let response = client
                .post(self.endpoint())
                .timeout(Duration::from_secs(30 * 60))
                .header("Ocp-Apim-Subscription-Key", &self.api_key)
                .multipart(self.form(body, length, file_name, mime_type, language)?)
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

    #[test]
    fn debug_output_never_contains_the_api_key() {
        let provider = MaiTranscribeProvider::new("https://speech.example.com".into(), "fake-provider-key-000000".into(), "model-a".into());
        let shown = format!("{provider:?}");
        assert!(!shown.contains("fake-provider-key-000000"));
        assert!(shown.contains("model-a"));
    }
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
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

        let directory = tempfile::tempdir().unwrap();
        let audio = directory.path().join("synthetic.wav");
        tokio::fs::write(&audio, b"synthetic-audio-payload")
            .await
            .unwrap();
        let segments = provider
            .transcribe_path(&audio, "synthetic.wav", "audio/wav", Some("en-US"))
            .await
            .unwrap();
        let request = request_handle.await.unwrap();

        assert_eq!(segments.len(), 1);
        assert!(segments[0].words.is_none());
        assert!(request
            .starts_with("POST /speechtotext/transcriptions:transcribe?api-version=2025-10-15 "));
        assert!(request.contains(r#"name="audio""#));
        assert!(request.contains(r#"filename="synthetic.wav""#));
        assert!(request.contains("synthetic-audio-payload"));
        assert!(request.to_ascii_lowercase().contains("content-length: "));
        assert!(!request
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked"));
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

        let directory = tempfile::tempdir().unwrap();
        let audio = directory.path().join("synthetic.wav");
        tokio::fs::write(&audio, b"synthetic-audio").await.unwrap();
        let error = provider
            .transcribe_path(&audio, "synthetic.wav", "audio/wav", None)
            .await
            .unwrap_err();
        let _ = request_handle.await.unwrap();

        assert_eq!(
            error.category(),
            super::super::CloudFallbackReasonCategory::AuthConfig
        );
    }

    #[tokio::test]
    async fn file_upload_reopens_the_same_audio_for_a_retry() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, body) in [
                ("503 Service Unavailable", "{}"),
                (
                    "200 OK",
                    r#"{"phrases":[{"text":"synthetic speech","offsetMilliseconds":0,"durationMilliseconds":1000}]}"#,
                ),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                requests.push(read_request(&mut socket).await);
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        let directory = tempfile::tempdir().unwrap();
        let audio = directory.path().join("synthetic.mp3");
        let payload = b"synthetic audio content to replay".repeat(5_000);
        tokio::fs::write(&audio, &payload).await.unwrap();
        let provider = MaiTranscribeProvider::new(endpoint, "test-key".into(), "test-model".into());
        let segments = tokio::time::timeout(
            Duration::from_secs(10),
            provider.transcribe_path(&audio, "synthetic.mp3", "audio/mpeg", None),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(segments[0].text, "synthetic speech");
        for request in server.await.unwrap() {
            assert!(request
                .windows(payload.len())
                .any(|bytes| bytes == payload.as_slice()));
        }
    }

    #[tokio::test]
    async fn cancelling_a_file_upload_drops_the_in_flight_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            signal.store(true, Ordering::Release);
            let read = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut [0_u8; 1]))
                .await
                .expect("request connection closed when cancelled");
            assert!(matches!(read, Ok(0) | Err(_)));
            assert!(request_complete(&request));
        });
        let directory = tempfile::tempdir().unwrap();
        let audio = directory.path().join("synthetic.wav");
        tokio::fs::write(&audio, b"synthetic audio").await.unwrap();
        let provider = MaiTranscribeProvider::new(endpoint, "test-key".into(), "test-model".into());
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            super::super::transcribe_mai_file(&provider, &audio, None, &cancelled),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        server.await.unwrap();
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
            let request = read_request(&mut socket).await;
            let response = format!(
                "{status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&request).to_string()
        });

        (format!("http://{addr}"), handle)
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
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
        request
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
