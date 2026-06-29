pub mod mai_transcribe;
pub mod openai_whisper;

use crate::api::{TranscriptWord, TranscriptWordTimestampSource};
use crate::audio::common::TranscribedSegment;
use crate::database::repositories::setting::SettingsRepository;
use crate::state::AppState;
use anyhow::anyhow;
use async_trait::async_trait;
use serde::Serialize;
use std::fmt;
use std::path::Path;
use tauri::{AppHandle, Emitter, Manager, Runtime};

pub const PROVIDER_CLOUD_WHISPER: &str = "cloud-whisper";
pub const PROVIDER_MAI_TRANSCRIBE: &str = "mai-transcribe";
pub const DEFAULT_CLOUD_WHISPER_MODEL: &str = "whisper-1";
pub const DEFAULT_MAI_TRANSCRIBE_MODEL: &str = "mai-transcribe-1.5";

#[derive(Debug, Clone)]
pub struct CloudTranscriptWord {
    pub text: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
}

#[derive(Debug, Clone)]
pub struct CloudTranscriptSegment {
    pub text: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub words: Option<Vec<CloudTranscriptWord>>,
}

#[derive(Debug, Clone)]
pub(crate) struct CloudTranscriptionOutcome {
    pub provider: String,
    pub model: String,
    pub segments: Vec<TranscribedSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudFallbackReasonCategory {
    Transient,
    AuthConfig,
}

impl CloudFallbackReasonCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::AuthConfig => "auth_config",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CloudTranscriptionError {
    category: CloudFallbackReasonCategory,
    message: String,
}

impl CloudTranscriptionError {
    pub fn transient(message: impl Into<String>) -> Self {
        Self {
            category: CloudFallbackReasonCategory::Transient,
            message: message.into(),
        }
    }

    pub fn auth_config(message: impl Into<String>) -> Self {
        Self {
            category: CloudFallbackReasonCategory::AuthConfig,
            message: message.into(),
        }
    }

    pub fn category(&self) -> CloudFallbackReasonCategory {
        self.category
    }
}

impl fmt::Display for CloudTranscriptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CloudTranscriptionError {}

#[async_trait]
pub trait CloudTranscriptionProvider {
    async fn transcribe_file(
        &self,
        audio: Vec<u8>,
        file_name: &str,
        mime_type: &str,
        language: Option<&str>,
    ) -> Result<Vec<CloudTranscriptSegment>, CloudTranscriptionError>;
}

pub fn is_cloud_provider(provider: Option<&str>) -> bool {
    matches!(
        provider,
        Some(PROVIDER_CLOUD_WHISPER) | Some(PROVIDER_MAI_TRANSCRIBE)
    )
}

pub(crate) async fn transcribe_whole_file<R: Runtime>(
    app: &AppHandle<R>,
    provider: &str,
    requested_model: Option<&str>,
    audio_path: &Path,
    language: Option<&str>,
) -> Result<CloudTranscriptionOutcome, CloudTranscriptionError> {
    let app_state = app
        .try_state::<AppState>()
        .ok_or_else(|| CloudTranscriptionError::auth_config("App state not available"))?;
    let pool = app_state.db_manager.pool();
    let config = SettingsRepository::get_transcript_config(pool)
        .await
        .map_err(|e| CloudTranscriptionError::auth_config(format!("Cloud config error: {e}")))?
        .ok_or_else(|| {
            CloudTranscriptionError::auth_config("Cloud transcription is not configured")
        })?;
    let api_key = SettingsRepository::get_transcript_api_key(pool, provider)
        .await
        .map_err(|e| CloudTranscriptionError::auth_config(format!("Cloud credential error: {e}")))?
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| {
            CloudTranscriptionError::auth_config("Cloud transcription API key is missing")
        })?;

    let audio = tokio::fs::read(audio_path).await.map_err(|e| {
        CloudTranscriptionError::auth_config(format!("Failed to read audio file: {e}"))
    })?;
    let file_name = audio_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("audio.bin");
    let mime_type = mime_type_for_path(audio_path);

    match provider {
        PROVIDER_CLOUD_WHISPER => {
            let base_url = config
                .cloud_whisper_base_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("https://api.openai.com/v1");
            let model = requested_model
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    (config.provider == PROVIDER_CLOUD_WHISPER)
                        .then_some(config.model.as_str())
                        .filter(|value| !value.trim().is_empty())
                })
                .unwrap_or(DEFAULT_CLOUD_WHISPER_MODEL)
                .to_string();
            let client = openai_whisper::OpenAiWhisperProvider::new(
                base_url.to_string(),
                api_key,
                model.clone(),
            );
            let cloud_segments = client
                .transcribe_file(audio, file_name, mime_type, language)
                .await?;
            Ok(CloudTranscriptionOutcome {
                provider: PROVIDER_CLOUD_WHISPER.to_string(),
                model,
                segments: cloud_segments_to_transcribed_segments(
                    &cloud_segments,
                    CloudWordPolicy::Real,
                ),
            })
        }
        PROVIDER_MAI_TRANSCRIBE => {
            let endpoint = config
                .mai_transcribe_endpoint
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    CloudTranscriptionError::auth_config(
                        "Azure Speech endpoint is missing for MAI transcription",
                    )
                })?;
            let model = requested_model
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    (config.provider == PROVIDER_MAI_TRANSCRIBE)
                        .then_some(config.model.as_str())
                        .filter(|value| !value.trim().is_empty())
                })
                .unwrap_or(DEFAULT_MAI_TRANSCRIBE_MODEL)
                .to_string();
            let client = mai_transcribe::MaiTranscribeProvider::new(
                endpoint.to_string(),
                api_key,
                model.clone(),
            );
            let cloud_segments = client
                .transcribe_file(audio, file_name, mime_type, language)
                .await?;
            Ok(CloudTranscriptionOutcome {
                provider: PROVIDER_MAI_TRANSCRIBE.to_string(),
                model,
                segments: cloud_segments_to_transcribed_segments(
                    &cloud_segments,
                    CloudWordPolicy::None,
                ),
            })
        }
        _ => Err(CloudTranscriptionError::auth_config(format!(
            "Unsupported cloud transcription provider: {provider}"
        ))),
    }
}

pub fn emit_fallback_event<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: Option<&str>,
    provider: &str,
    error: &CloudTranscriptionError,
) {
    let payload = TranscriptionFallbackEvent {
        meeting_id: meeting_id.map(str::to_string),
        provider: provider.to_string(),
        reason_category: error.category().as_str().to_string(),
    };
    let _ = app.emit("transcription-fell-back-to-local", payload);
}

pub fn local_fallback_error_context(
    error: &CloudTranscriptionError,
    local_error: anyhow::Error,
) -> anyhow::Error {
    anyhow!(
        "Cloud transcription failed ({}) and local fallback could not start. Download a local model or fix the cloud transcription settings. Local fallback error: {}",
        error.category().as_str(),
        local_error
    )
}

pub(crate) fn cloud_segments_to_transcribed_segments(
    segments: &[CloudTranscriptSegment],
    word_policy: CloudWordPolicy,
) -> Vec<TranscribedSegment> {
    segments
        .iter()
        .filter_map(|segment| {
            let text = segment.text.trim();
            if text.is_empty() {
                return None;
            }
            let start_seconds = segment.start_seconds.max(0.0);
            let end_seconds = segment.end_seconds.max(start_seconds);
            let word_timestamps = match word_policy {
                CloudWordPolicy::Real => segment.words.as_ref().map(|words| {
                    words
                        .iter()
                        .filter(|word| !word.text.trim().is_empty())
                        .map(|word| TranscriptWord {
                            text: word.text.clone(),
                            start: word.start_seconds.max(start_seconds),
                            end: word.end_seconds.max(word.start_seconds).min(end_seconds),
                            confidence: None,
                            speaker: None,
                            timestamp_source: Some(TranscriptWordTimestampSource::Real),
                        })
                        .collect::<Vec<_>>()
                }),
                CloudWordPolicy::None => None,
            };

            Some(TranscribedSegment {
                text: text.to_string(),
                start_ms: start_seconds * 1000.0,
                end_ms: end_seconds * 1000.0,
                word_timestamps,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudWordPolicy {
    Real,
    None,
}

#[derive(Debug, Clone, Serialize)]
struct TranscriptionFallbackEvent {
    meeting_id: Option<String>,
    provider: String,
    reason_category: String,
}

pub fn classify_status(status: reqwest::StatusCode, provider: &str) -> CloudTranscriptionError {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        CloudTranscriptionError::transient(format!(
            "{provider} cloud transcription returned HTTP {status}"
        ))
    } else {
        CloudTranscriptionError::auth_config(format!(
            "{provider} cloud transcription returned HTTP {status}"
        ))
    }
}

pub fn should_retry_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn mime_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("aac") => "audio/aac",
        Some("flac") => "audio/flac",
        Some("m4a") | Some("mp4") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("ogg") | Some("opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("webm") => "audio/webm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mai_word_policy_never_emits_real_word_timestamps() {
        let cloud_segments = vec![CloudTranscriptSegment {
            text: "hello there".to_string(),
            start_seconds: 1.0,
            end_seconds: 3.0,
            words: Some(vec![CloudTranscriptWord {
                text: "hello".to_string(),
                start_seconds: 1.0,
                end_seconds: 2.0,
            }]),
        }];

        let mapped = cloud_segments_to_transcribed_segments(&cloud_segments, CloudWordPolicy::None);

        assert_eq!(mapped.len(), 1);
        assert!(mapped[0].word_timestamps.is_none());
    }

    #[test]
    fn hosted_whisper_word_policy_marks_words_real() {
        let cloud_segments = vec![CloudTranscriptSegment {
            text: "hello there".to_string(),
            start_seconds: 1.0,
            end_seconds: 3.0,
            words: Some(vec![
                CloudTranscriptWord {
                    text: "hello".to_string(),
                    start_seconds: 1.0,
                    end_seconds: 2.0,
                },
                CloudTranscriptWord {
                    text: "there".to_string(),
                    start_seconds: 2.0,
                    end_seconds: 3.0,
                },
            ]),
        }];

        let mapped = cloud_segments_to_transcribed_segments(&cloud_segments, CloudWordPolicy::Real);
        let words = mapped[0].word_timestamps.as_ref().unwrap();

        assert_eq!(words.len(), 2);
        assert!(words
            .iter()
            .all(|word| word.timestamp_source == Some(TranscriptWordTimestampSource::Real)));
    }

    #[test]
    fn status_classification_separates_retryable_and_config_errors() {
        assert_eq!(
            classify_status(reqwest::StatusCode::UNAUTHORIZED, "provider").category(),
            CloudFallbackReasonCategory::AuthConfig
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::TOO_MANY_REQUESTS, "provider").category(),
            CloudFallbackReasonCategory::Transient
        );
        assert!(should_retry_status(reqwest::StatusCode::BAD_GATEWAY));
        assert!(!should_retry_status(reqwest::StatusCode::BAD_REQUEST));
    }
}
