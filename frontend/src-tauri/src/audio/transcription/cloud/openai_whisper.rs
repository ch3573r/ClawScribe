use super::{
    classify_status, should_retry_status, CloudTranscriptSegment, CloudTranscriptWord,
    CloudTranscriptionError, CloudTranscriptionProvider,
};
use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use std::time::Duration;

const MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Clone)]
pub struct OpenAiWhisperProvider {
    base_url: String,
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiWhisperProvider {
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        Self {
            base_url,
            api_key,
            model,
            client: reqwest::Client::new(),
        }
    }

    fn endpoint(&self) -> String {
        transcription_endpoint(&self.base_url)
    }

    fn form(
        &self,
        audio: Vec<u8>,
        file_name: &str,
        mime_type: &str,
        language: Option<&str>,
    ) -> Result<Form, CloudTranscriptionError> {
        let part = Part::bytes(audio)
            .file_name(file_name.to_string())
            .mime_str(mime_type)
            .map_err(|e| {
                CloudTranscriptionError::auth_config(format!("Invalid audio MIME type: {e}"))
            })?;
        let mut form = Form::new()
            .part("file", part)
            .text("model", self.model.clone())
            .text("response_format", "verbose_json")
            .text("timestamp_granularities[]", "segment")
            .text("timestamp_granularities[]", "word");
        if let Some(language) = language.map(str::trim).filter(|value| !value.is_empty()) {
            form = form.text("language", normalize_language(language));
        }
        Ok(form)
    }
}

#[async_trait]
impl CloudTranscriptionProvider for OpenAiWhisperProvider {
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
                .bearer_auth(&self.api_key)
                .multipart(self.form(audio.clone(), file_name, mime_type, language)?)
                .send()
                .await;

            match response {
                Ok(response) if response.status().is_success() => {
                    let payload = response
                        .json::<OpenAiVerboseTranscription>()
                        .await
                        .map_err(|e| {
                            CloudTranscriptionError::auth_config(format!(
                                "OpenAI transcription response was not valid JSON: {e}"
                            ))
                        })?;
                    return parse_verbose_transcription(payload);
                }
                Ok(response) => {
                    let status = response.status();
                    let error = classify_status(status, "OpenAI-compatible");
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
                        "OpenAI-compatible transcription request failed: {error}"
                    )));
                }
            }
        }

        Err(CloudTranscriptionError::transient(
            "OpenAI-compatible transcription failed after retries",
        ))
    }
}

pub fn transcription_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/audio/transcriptions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/audio/transcriptions")
    }
}

fn normalize_language(language: &str) -> String {
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
pub(crate) struct OpenAiVerboseTranscription {
    text: Option<String>,
    segments: Option<Vec<OpenAiSegment>>,
    words: Option<Vec<OpenAiWord>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiSegment {
    text: String,
    start: f64,
    end: f64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiWord {
    #[serde(rename = "word", alias = "text")]
    text: String,
    start: f64,
    end: f64,
}

pub(crate) fn parse_verbose_transcription(
    payload: OpenAiVerboseTranscription,
) -> Result<Vec<CloudTranscriptSegment>, CloudTranscriptionError> {
    let words = payload.words.unwrap_or_default();
    let mut segments = Vec::new();

    if let Some(api_segments) = payload.segments {
        for segment in api_segments {
            let segment_words = words
                .iter()
                .filter(|word| {
                    word_overlaps_segment(word.start, word.end, segment.start, segment.end)
                })
                .map(|word| CloudTranscriptWord {
                    text: word.text.clone(),
                    start_seconds: word.start,
                    end_seconds: word.end,
                })
                .collect::<Vec<_>>();
            segments.push(CloudTranscriptSegment {
                text: segment.text,
                start_seconds: segment.start,
                end_seconds: segment.end,
                words: (!segment_words.is_empty()).then_some(segment_words),
            });
        }
    } else if !words.is_empty() {
        let start_seconds = words.first().map(|word| word.start).unwrap_or(0.0);
        let end_seconds = words.last().map(|word| word.end).unwrap_or(start_seconds);
        let text = payload.text.unwrap_or_else(|| {
            words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        });
        segments.push(CloudTranscriptSegment {
            text,
            start_seconds,
            end_seconds,
            words: Some(
                words
                    .into_iter()
                    .map(|word| CloudTranscriptWord {
                        text: word.text,
                        start_seconds: word.start,
                        end_seconds: word.end,
                    })
                    .collect(),
            ),
        });
    } else if let Some(text) = payload.text.filter(|text| !text.trim().is_empty()) {
        segments.push(CloudTranscriptSegment {
            text,
            start_seconds: 0.0,
            end_seconds: 0.0,
            words: None,
        });
    }

    if segments.is_empty() {
        return Err(CloudTranscriptionError::auth_config(
            "OpenAI-compatible transcription response did not contain transcript segments",
        ));
    }

    Ok(segments)
}

fn word_overlaps_segment(
    word_start: f64,
    word_end: f64,
    segment_start: f64,
    segment_end: f64,
) -> bool {
    let midpoint = (word_start + word_end) / 2.0;
    midpoint >= segment_start - 0.001 && midpoint <= segment_end + 0.001
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn appends_transcription_endpoint_to_v1_base_url() {
        assert_eq!(
            transcription_endpoint("https://api.example.com/v1/"),
            "https://api.example.com/v1/audio/transcriptions"
        );
        assert_eq!(
            transcription_endpoint("https://api.example.com/v1/audio/transcriptions"),
            "https://api.example.com/v1/audio/transcriptions"
        );
    }

    #[test]
    fn parses_verbose_json_segments_and_words_in_order() {
        let payload = OpenAiVerboseTranscription {
            text: Some("hello there goodbye".to_string()),
            segments: Some(vec![
                OpenAiSegment {
                    text: "hello there".to_string(),
                    start: 0.0,
                    end: 1.2,
                },
                OpenAiSegment {
                    text: "goodbye".to_string(),
                    start: 1.2,
                    end: 2.0,
                },
            ]),
            words: Some(vec![
                OpenAiWord {
                    text: "hello".to_string(),
                    start: 0.0,
                    end: 0.4,
                },
                OpenAiWord {
                    text: "there".to_string(),
                    start: 0.4,
                    end: 1.1,
                },
                OpenAiWord {
                    text: "goodbye".to_string(),
                    start: 1.3,
                    end: 1.9,
                },
            ]),
        };

        let parsed = parse_verbose_transcription(payload).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].text, "hello there");
        assert_eq!(parsed[0].words.as_ref().unwrap()[0].text, "hello");
        assert_eq!(parsed[1].words.as_ref().unwrap()[0].text, "goodbye");
    }

    #[tokio::test]
    async fn mocked_http_request_uses_verbose_json_word_and_segment_timestamps() {
        let body = r#"{
            "text": "hello",
            "segments": [{"text": "hello", "start": 0.0, "end": 0.5}],
            "words": [{"word": "hello", "start": 0.0, "end": 0.5}]
        }"#;
        let (base_url, request_handle) =
            spawn_one_response_server("HTTP/1.1 200 OK", body, "application/json").await;
        let provider =
            OpenAiWhisperProvider::new(base_url, "test-key".to_string(), "whisper-1".to_string());

        let segments = provider
            .transcribe_file(b"abc".to_vec(), "audio.wav", "audio/wav", Some("en-US"))
            .await
            .unwrap();
        let request = request_handle.await.unwrap();

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].words.as_ref().unwrap()[0].text, "hello");
        assert!(request.starts_with("POST /audio/transcriptions "));
        assert!(request.contains(r#"name="file""#));
        assert!(request.contains(r#"name="model""#));
        assert!(request.contains("whisper-1"));
        assert!(request.contains(r#"name="response_format""#));
        assert!(request.contains("verbose_json"));
        assert!(request.contains(r#"name="timestamp_granularities[]""#));
        assert!(request.contains("segment"));
        assert!(request.contains("word"));
        assert!(request.contains(r#"name="language""#));
        assert!(request.contains("en"));
    }

    #[tokio::test]
    async fn mocked_http_auth_failure_is_not_transient() {
        let (base_url, request_handle) =
            spawn_one_response_server("HTTP/1.1 401 Unauthorized", "{}", "application/json").await;
        let provider =
            OpenAiWhisperProvider::new(base_url, "test-key".to_string(), "whisper-1".to_string());

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
