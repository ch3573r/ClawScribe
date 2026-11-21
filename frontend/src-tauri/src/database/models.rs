use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteRow, FromRow, Row};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingModel {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub folder_path: Option<String>,
}

impl FromRow<'_, SqliteRow> for MeetingModel {
    fn from_row(row: &SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            title: row.try_get("title")?,
            created_at: meeting_timestamp(row, "created_at")?,
            updated_at: meeting_timestamp(row, "updated_at")?,
            folder_path: row.try_get("folder_path")?,
        })
    }
}

pub(crate) fn calendar_date(value: &str) -> Option<NaiveDate> {
    if value.len() != 10 {
        return None;
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    (date.format("%Y-%m-%d").to_string() == value).then_some(date)
}

pub(crate) fn local_meeting_date(row: &SqliteRow, column: &str) -> Option<NaiveDate> {
    row.try_get::<DateTime<Utc>, _>(column)
        .map(|date| date.with_timezone(&Local).date_naive())
        .ok()
        .or_else(|| calendar_date(row.try_get::<&str, _>(column).ok()?))
}

fn meeting_timestamp(row: &SqliteRow, column: &str) -> Result<String, sqlx::Error> {
    match row.try_get::<DateTime<Utc>, _>(column) {
        Ok(date) => Ok(date.to_rfc3339()),
        Err(error) => match row.try_get::<String, _>(column) {
            Ok(value) if calendar_date(&value).is_some() => Ok(value),
            _ => Err(error),
        },
    }
}

// Renamed from TranscriptSegment to Transcript to match the table name
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Transcript {
    #[serde(default)]
    #[sqlx(default)]
    pub original_transcript: Option<String>,
    pub id: String,
    pub meeting_id: String,
    pub transcript: String,
    pub timestamp: String,
    pub summary: Option<String>,
    pub action_items: Option<String>,
    pub key_points: Option<String>,
    // Recording-relative timestamps for audio-transcript synchronization
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
    pub duration: Option<f64>,
    // Audio source label: "Me" (microphone) or "Participants" (system audio).
    pub speaker: Option<String>,
    pub word_timestamps_json: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SummaryProcess {
    #[sqlx(default)]
    #[serde(skip)]
    pub current_content_revision: Option<i64>,
    pub meeting_id: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub error: Option<String>,
    pub result: Option<String>, // JSON
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    pub chunk_count: i64,
    pub processing_time: f64,
    pub metadata: Option<String>,      // JSON
    pub result_backup: Option<String>, // Backup of result before regeneration
    pub result_backup_timestamp: Option<chrono::DateTime<chrono::Utc>>, // When backup was created
    pub previous_result: Option<String>,
    pub previous_result_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct TranscriptChunk {
    pub meeting_id: String,
    pub meeting_name: Option<String>,
    pub transcript_text: String,
    pub model: String,
    pub model_name: String,
    pub chunk_size: Option<i64>,
    pub overlap: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Setting {
    pub id: String,
    pub provider: String,
    pub model: String,
    #[sqlx(rename = "whisperModel")]
    #[serde(rename = "whisperModel")]
    pub whisper_model: String,
    #[sqlx(rename = "groqApiKey")]
    #[serde(rename = "groqApiKey")]
    pub groq_api_key: Option<String>,
    #[sqlx(rename = "openaiApiKey")]
    #[serde(rename = "openaiApiKey")]
    pub openai_api_key: Option<String>,
    #[sqlx(rename = "anthropicApiKey")]
    #[serde(rename = "anthropicApiKey")]
    pub anthropic_api_key: Option<String>,
    #[sqlx(rename = "ollamaApiKey")]
    #[serde(rename = "ollamaApiKey")]
    pub ollama_api_key: Option<String>,
    #[sqlx(rename = "openRouterApiKey")]
    #[serde(rename = "openRouterApiKey")]
    pub open_router_api_key: Option<String>,
    #[sqlx(rename = "ollamaEndpoint")]
    #[serde(rename = "ollamaEndpoint")]
    pub ollama_endpoint: Option<String>,
    /// Custom OpenAI-compatible endpoint configuration stored as JSON
    #[sqlx(rename = "customOpenAIConfig")]
    #[serde(rename = "customOpenAIConfig")]
    pub custom_openai_config: Option<String>,
}

impl Setting {
    /// Parse the custom OpenAI config from JSON string
    pub fn get_custom_openai_config(&self) -> Option<crate::summary::CustomOpenAIConfig> {
        self.custom_openai_config
            .as_ref()
            .and_then(|json| serde_json::from_str(json).ok())
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct TranscriptSetting {
    #[sqlx(rename = "cloudWhisperAllowUnencrypted")]
    pub cloud_whisper_allow_unencrypted: Option<bool>,
    #[sqlx(rename = "maiTranscribeAllowUnencrypted")]
    pub mai_transcribe_allow_unencrypted: Option<bool>,
    pub id: String,
    pub provider: String,
    pub model: String,
    #[sqlx(rename = "whisperApiKey")]
    #[serde(rename = "whisperApiKey")]
    pub whisper_api_key: Option<String>,
    #[sqlx(rename = "deepgramApiKey")]
    #[serde(rename = "deepgramApiKey")]
    pub deepgram_api_key: Option<String>,
    #[sqlx(rename = "elevenLabsApiKey")]
    #[serde(rename = "elevenLabsApiKey")]
    pub eleven_labs_api_key: Option<String>,
    #[sqlx(rename = "groqApiKey")]
    #[serde(rename = "groqApiKey")]
    pub groq_api_key: Option<String>,
    #[sqlx(rename = "openaiApiKey")]
    #[serde(rename = "openaiApiKey")]
    pub openai_api_key: Option<String>,
    #[sqlx(rename = "cloudWhisperApiKey")]
    #[serde(rename = "cloudWhisperApiKey")]
    pub cloud_whisper_api_key: Option<String>,
    #[sqlx(rename = "cloudWhisperBaseUrl")]
    #[serde(rename = "cloudWhisperBaseUrl")]
    pub cloud_whisper_base_url: Option<String>,
    #[sqlx(rename = "maiTranscribeApiKey")]
    #[serde(rename = "maiTranscribeApiKey")]
    pub mai_transcribe_api_key: Option<String>,
    #[sqlx(rename = "maiTranscribeEndpoint")]
    #[serde(rename = "maiTranscribeEndpoint")]
    pub mai_transcribe_endpoint: Option<String>,
    #[sqlx(rename = "maiTranscribeRegion")]
    #[serde(rename = "maiTranscribeRegion")]
    pub mai_transcribe_region: Option<String>,
}

impl TranscriptSetting {
    pub fn cloud_http_opt_in(&self, provider: &str) -> bool {
        let (saved, endpoint) = match provider {
            "cloud-whisper" => (
                self.cloud_whisper_allow_unencrypted,
                self.cloud_whisper_base_url.as_deref(),
            ),
            "mai-transcribe" => (
                self.mai_transcribe_allow_unencrypted,
                self.mai_transcribe_endpoint.as_deref(),
            ),
            _ => return false,
        };
        saved.unwrap_or_else(|| {
            endpoint.is_some_and(crate::openai::secret_destination::needs_http_opt_in)
        })
    }
}
