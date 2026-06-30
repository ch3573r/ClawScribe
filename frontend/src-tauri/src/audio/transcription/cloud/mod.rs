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
const OPENAI_HOSTED_WHISPER_MAX_UPLOAD_BYTES: u64 = 25_000_000;

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
    pub requires_local_timing_grid: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct CloudTranscriptionOutcome {
    pub provider: String,
    pub model: String,
    pub segments: Vec<TranscribedSegment>,
    pub requires_local_timing_grid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudFallbackReasonCategory {
    Transient,
    AuthConfig,
    UploadTooLarge,
    ProviderOutput,
}

impl CloudFallbackReasonCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::AuthConfig => "auth_config",
            Self::UploadTooLarge => "upload_too_large",
            Self::ProviderOutput => "provider_output",
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

    pub fn upload_too_large(message: impl Into<String>) -> Self {
        Self {
            category: CloudFallbackReasonCategory::UploadTooLarge,
            message: message.into(),
        }
    }

    pub fn provider_output(message: impl Into<String>) -> Self {
        Self {
            category: CloudFallbackReasonCategory::ProviderOutput,
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

    let audio_size_bytes = tokio::fs::metadata(audio_path)
        .await
        .map_err(|e| {
            CloudTranscriptionError::auth_config(format!("Failed to inspect audio file: {e}"))
        })?
        .len();
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
            validate_provider_upload_size(
                PROVIDER_CLOUD_WHISPER,
                Some(base_url),
                audio_size_bytes,
            )?;
            let audio = tokio::fs::read(audio_path).await.map_err(|e| {
                CloudTranscriptionError::auth_config(format!("Failed to read audio file: {e}"))
            })?;
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
                requires_local_timing_grid: cloud_segments
                    .iter()
                    .any(|segment| segment.requires_local_timing_grid),
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
            validate_provider_upload_size(
                PROVIDER_MAI_TRANSCRIBE,
                Some(endpoint),
                audio_size_bytes,
            )?;
            let audio = tokio::fs::read(audio_path).await.map_err(|e| {
                CloudTranscriptionError::auth_config(format!("Failed to read audio file: {e}"))
            })?;
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
                requires_local_timing_grid: cloud_segments
                    .iter()
                    .any(|segment| segment.requires_local_timing_grid),
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
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
        CloudTranscriptionError::upload_too_large(format!(
            "{provider} cloud transcription upload is too large (HTTP {status})"
        ))
    } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        CloudTranscriptionError::transient(format!(
            "{provider} cloud transcription returned HTTP {status}"
        ))
    } else {
        CloudTranscriptionError::auth_config(format!(
            "{provider} cloud transcription returned HTTP {status}"
        ))
    }
}

pub fn classify_status_with_body(
    status: reqwest::StatusCode,
    provider: &str,
    body: &str,
) -> CloudTranscriptionError {
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE || response_body_mentions_upload_size(body)
    {
        CloudTranscriptionError::upload_too_large(format!(
            "{provider} cloud transcription upload is too large (HTTP {status})"
        ))
    } else {
        classify_status(status, provider)
    }
}

pub fn should_retry_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

pub(crate) fn validate_provider_upload_size(
    provider: &str,
    endpoint_or_base_url: Option<&str>,
    file_size_bytes: u64,
) -> Result<(), CloudTranscriptionError> {
    if provider == PROVIDER_CLOUD_WHISPER
        && endpoint_or_base_url
            .map(is_official_openai_api_url)
            .unwrap_or(true)
        && file_size_bytes > OPENAI_HOSTED_WHISPER_MAX_UPLOAD_BYTES
    {
        return Err(CloudTranscriptionError::upload_too_large(format!(
            "OpenAI Hosted Whisper accepts audio uploads up to 25 MB; this file is {}",
            format_megabytes(file_size_bytes)
        )));
    }

    Ok(())
}

fn is_official_openai_api_url(value: &str) -> bool {
    reqwest::Url::parse(value)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host.eq_ignore_ascii_case("api.openai.com"))
        })
        .unwrap_or_else(|| {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "https://api.openai.com"
                || normalized.starts_with("https://api.openai.com/")
        })
}

fn response_body_mentions_upload_size(body: &str) -> bool {
    let normalized = body.to_ascii_lowercase();
    normalized.contains("413")
        || normalized.contains("payload too large")
        || normalized.contains("request entity too large")
        || normalized.contains("file too large")
        || normalized.contains("content too large")
        || normalized.contains("content size limit")
        || (normalized.contains("maximum")
            && normalized.contains("size")
            && (normalized.contains("file")
                || normalized.contains("content")
                || normalized.contains("upload")))
        || normalized.contains("25 mb")
        || normalized.contains("25mb")
        || normalized.contains("26214400")
        || normalized.contains("25000000")
}

fn format_megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

pub(crate) fn mime_type_for_path(path: &Path) -> &'static str {
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
            requires_local_timing_grid: false,
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
            requires_local_timing_grid: false,
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
        assert_eq!(
            classify_status(reqwest::StatusCode::PAYLOAD_TOO_LARGE, "provider").category(),
            CloudFallbackReasonCategory::UploadTooLarge
        );
        assert!(should_retry_status(reqwest::StatusCode::BAD_GATEWAY));
        assert!(!should_retry_status(reqwest::StatusCode::BAD_REQUEST));
        assert!(!should_retry_status(reqwest::StatusCode::PAYLOAD_TOO_LARGE));
    }

    #[test]
    fn status_body_classification_catches_size_errors_without_413() {
        let error = classify_status_with_body(
            reqwest::StatusCode::BAD_REQUEST,
            "OpenAI-compatible",
            r#"{"error":{"message":"Maximum content size limit (26214400) exceeded"}}"#,
        );

        assert_eq!(
            error.category(),
            CloudFallbackReasonCategory::UploadTooLarge
        );
    }

    #[test]
    fn openai_hosted_upload_preflight_rejects_files_over_25_mb() {
        let error = validate_provider_upload_size(
            PROVIDER_CLOUD_WHISPER,
            Some("https://api.openai.com/v1"),
            25_000_001,
        )
        .unwrap_err();

        assert_eq!(
            error.category(),
            CloudFallbackReasonCategory::UploadTooLarge
        );
        assert!(validate_provider_upload_size(
            PROVIDER_CLOUD_WHISPER,
            Some("https://example.test/v1"),
            250_000_000,
        )
        .is_ok());
        assert!(validate_provider_upload_size(
            PROVIDER_MAI_TRANSCRIBE,
            Some("https://example.cognitiveservices.azure.com"),
            250_000_000,
        )
        .is_ok());
    }
}

#[cfg(test)]
pub(crate) mod live_smoke {
    use super::{
        cloud_segments_to_transcribed_segments, mai_transcribe::MaiTranscribeProvider,
        mime_type_for_path, openai_whisper::OpenAiWhisperProvider, CloudTranscriptSegment,
        CloudTranscriptionProvider, CloudWordPolicy, DEFAULT_CLOUD_WHISPER_MODEL,
        DEFAULT_MAI_TRANSCRIBE_MODEL,
    };
    use crate::api::TranscriptWordTimestampSource;
    use std::{
        env,
        path::{Path, PathBuf},
    };

    const ENV_AUDIO: &str = "CLAWSCRIBE_SMOKE_AUDIO";
    const ENV_LANGUAGE: &str = "CLAWSCRIBE_SMOKE_LANGUAGE";
    const ENV_OPENAI_API_KEY: &str = "CLAWSCRIBE_SMOKE_OPENAI_API_KEY";
    const ENV_OPENAI_BASE_URL: &str = "CLAWSCRIBE_SMOKE_OPENAI_BASE_URL";
    const ENV_OPENAI_MODEL: &str = "CLAWSCRIBE_SMOKE_OPENAI_MODEL";
    const ENV_MAI_API_KEY: &str = "CLAWSCRIBE_SMOKE_MAI_API_KEY";
    const ENV_MAI_ENDPOINT: &str = "CLAWSCRIBE_SMOKE_MAI_ENDPOINT";
    const ENV_MAI_MODEL: &str = "CLAWSCRIBE_SMOKE_MAI_MODEL";

    #[tokio::test]
    #[ignore = "requires hosted transcription credentials and a short real audio file"]
    async fn hosted_api_transcription_live_smoke() {
        let audio_path = required_path(ENV_AUDIO);
        let audio = tokio::fs::read(&audio_path)
            .await
            .unwrap_or_else(|error| panic!("failed to read {ENV_AUDIO}: {error}"));
        assert!(
            !audio.is_empty(),
            "{ENV_AUDIO} must point to a non-empty audio file"
        );

        let file_name = audio_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("smoke-audio.wav");
        let mime_type = mime_type_for_path(&audio_path);
        let language = optional_env(ENV_LANGUAGE);
        let language = language.as_deref();
        let mut providers_run = 0;

        if let Some(api_key) = optional_env(ENV_OPENAI_API_KEY) {
            providers_run += 1;
            let base_url = optional_env(ENV_OPENAI_BASE_URL)
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
            let model = optional_env(ENV_OPENAI_MODEL)
                .unwrap_or_else(|| DEFAULT_CLOUD_WHISPER_MODEL.into());
            let provider = OpenAiWhisperProvider::new(base_url, api_key, model);
            let segments = transcribe_or_panic(
                "Hosted Whisper",
                &provider,
                audio.clone(),
                file_name,
                mime_type,
                language,
            )
            .await;

            assert_basic_segments("Hosted Whisper", &segments);
            assert_hosted_whisper_word_timestamps(&segments);
            print_summary("Hosted Whisper", &segments);
        }

        match (
            optional_env(ENV_MAI_ENDPOINT),
            optional_env(ENV_MAI_API_KEY),
        ) {
            (Some(endpoint), Some(api_key)) => {
                providers_run += 1;
                let model = optional_env(ENV_MAI_MODEL)
                    .unwrap_or_else(|| DEFAULT_MAI_TRANSCRIBE_MODEL.into());
                let provider = MaiTranscribeProvider::new(endpoint, api_key, model);
                let segments = transcribe_or_panic(
                    "MAI-Transcribe",
                    &provider,
                    audio,
                    file_name,
                    mime_type,
                    language,
                )
                .await;

                assert_basic_segments("MAI-Transcribe", &segments);
                assert_mai_never_emits_word_timestamps(&segments);
                print_summary("MAI-Transcribe", &segments);
            }
            (Some(_), None) => panic!("{ENV_MAI_ENDPOINT} is set but {ENV_MAI_API_KEY} is missing"),
            (None, Some(_)) => panic!("{ENV_MAI_API_KEY} is set but {ENV_MAI_ENDPOINT} is missing"),
            (None, None) => {}
        }

        assert!(
            providers_run > 0,
            "set {ENV_OPENAI_API_KEY} and/or both {ENV_MAI_ENDPOINT} + {ENV_MAI_API_KEY}"
        );
    }

    async fn transcribe_or_panic<P: CloudTranscriptionProvider>(
        label: &str,
        provider: &P,
        audio: Vec<u8>,
        file_name: &str,
        mime_type: &str,
        language: Option<&str>,
    ) -> Vec<CloudTranscriptSegment> {
        provider
            .transcribe_file(audio, file_name, mime_type, language)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{label} smoke transcription failed: category={}, error={error}",
                    error.category().as_str()
                )
            })
    }

    fn assert_basic_segments(label: &str, segments: &[CloudTranscriptSegment]) {
        assert!(
            !segments.is_empty(),
            "{label} returned no transcript segments"
        );
        assert!(
            segments
                .iter()
                .any(|segment| !segment.text.trim().is_empty()),
            "{label} returned only empty transcript text"
        );
        for segment in segments {
            assert!(
                segment.start_seconds >= 0.0,
                "{label} returned a negative segment start"
            );
            assert!(
                segment.end_seconds >= segment.start_seconds,
                "{label} returned an inverted segment time range"
            );
        }
    }

    fn assert_hosted_whisper_word_timestamps(segments: &[CloudTranscriptSegment]) {
        let mapped = cloud_segments_to_transcribed_segments(segments, CloudWordPolicy::Real);
        let word_count: usize = mapped
            .iter()
            .map(|segment| segment.word_timestamps.as_ref().map_or(0, Vec::len))
            .sum();
        assert!(
            word_count > 0,
            "Hosted Whisper returned no word timestamps; diarization would lose real word timing"
        );
        for segment in mapped {
            if let Some(words) = segment.word_timestamps {
                for word in words {
                    assert_eq!(
                        word.timestamp_source,
                        Some(TranscriptWordTimestampSource::Real),
                        "Hosted Whisper words must be marked as real acoustic timing"
                    );
                    assert!(
                        word.end >= word.start,
                        "Hosted Whisper returned an inverted word time range"
                    );
                }
            }
        }
    }

    fn assert_mai_never_emits_word_timestamps(segments: &[CloudTranscriptSegment]) {
        assert!(
            segments.iter().all(|segment| segment.words.is_none()),
            "MAI-Transcribe returned word timestamps; mapping must not promote them to Real"
        );
        let mapped = cloud_segments_to_transcribed_segments(segments, CloudWordPolicy::None);
        assert!(
            mapped
                .iter()
                .all(|segment| segment.word_timestamps.is_none()),
            "MAI-Transcribe mapped output must keep word_timestamps empty"
        );
    }

    fn print_summary(label: &str, segments: &[CloudTranscriptSegment]) {
        let collapsed = segments
            .iter()
            .any(|segment| segment.requires_local_timing_grid);
        let words: usize = segments
            .iter()
            .map(|segment| segment.words.as_ref().map_or(0, Vec::len))
            .sum();
        eprintln!(
            "{label}: segments={}, word_timestamps={}, requires_local_timing_grid={collapsed}",
            segments.len(),
            words
        );
    }

    fn required_path(name: &str) -> PathBuf {
        let value = optional_env(name).unwrap_or_else(|| panic!("{name} is required"));
        let path = Path::new(&value);
        assert!(path.exists(), "{name} does not exist: {value}");
        path.to_path_buf()
    }

    fn optional_env(name: &str) -> Option<String> {
        env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }
}
