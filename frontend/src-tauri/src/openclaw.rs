use chrono::Utc;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};

const CONFIG_FILE_NAME: &str = "openclaw.json";
const SUBMITTED_MARKER: &str = ".openclaw-submitted.json";
const FAILED_MARKER: &str = ".openclaw-failed.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub bearer_token: String,
    pub source: String,
    pub include_audio_path: bool,
}

impl Default for OpenClawConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://127.0.0.1:8765/meetings/completed".to_string(),
            bearer_token: String::new(),
            source: "meetily-openclaw".to_string(),
            include_audio_path: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawConfigStatus {
    pub enabled: bool,
    pub configured: bool,
    pub endpoint: String,
    pub source: String,
    pub config_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawSubmissionResult {
    pub submitted: bool,
    pub status_code: Option<u16>,
    pub message: String,
}

#[tauri::command]
pub async fn get_openclaw_config_status<R: Runtime>(
    app: AppHandle<R>,
) -> Result<OpenClawConfigStatus, String> {
    let config = load_config(&app)?;
    let config_path = config_path(&app)?;

    Ok(OpenClawConfigStatus {
        enabled: config.enabled,
        configured: !config.endpoint.trim().is_empty() && !config.bearer_token.trim().is_empty(),
        endpoint: config.endpoint,
        source: config.source,
        config_path: config_path.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub async fn save_openclaw_config<R: Runtime>(
    app: AppHandle<R>,
    config: OpenClawConfig,
) -> Result<(), String> {
    let path = config_path(&app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn submit_meeting_folder_to_openclaw<R: Runtime>(
    app: AppHandle<R>,
    folder_path: String,
) -> Result<OpenClawSubmissionResult, String> {
    submit_folder(&app, PathBuf::from(folder_path), None).await
}

pub fn submit_completed_recording<R: Runtime>(
    app: AppHandle<R>,
    folder_path: Option<String>,
    meeting_name: Option<String>,
) {
    let Some(folder_path) = folder_path else {
        warn!("OpenClaw handoff skipped: Meetily did not provide a recording folder path");
        return;
    };

    let config = match load_config(&app) {
        Ok(config) => config,
        Err(e) => {
            error!("OpenClaw handoff config could not be loaded: {}", e);
            return;
        }
    };

    if !config.enabled {
        info!("OpenClaw handoff skipped: disabled");
        return;
    }

    tauri::async_runtime::spawn(async move {
        match submit_folder_with_config(PathBuf::from(folder_path), meeting_name, config).await {
            Ok(result) => {
                if result.submitted {
                    info!("OpenClaw handoff complete: {}", result.message);
                } else {
                    warn!("OpenClaw handoff skipped: {}", result.message);
                }
            }
            Err(e) => error!("OpenClaw handoff failed: {}", e),
        }
    });
}

async fn submit_folder<R: Runtime>(
    app: &AppHandle<R>,
    folder: PathBuf,
    meeting_name: Option<String>,
) -> Result<OpenClawSubmissionResult, String> {
    let config = load_config(app)?;
    submit_folder_with_config(folder, meeting_name, config).await
}

async fn submit_folder_with_config(
    folder: PathBuf,
    meeting_name: Option<String>,
    config: OpenClawConfig,
) -> Result<OpenClawSubmissionResult, String> {
    if !config.enabled {
        return Ok(OpenClawSubmissionResult {
            submitted: false,
            status_code: None,
            message: "OpenClaw handoff is disabled".to_string(),
        });
    }

    if config.endpoint.trim().is_empty() || config.bearer_token.trim().is_empty() {
        return Err("OpenClaw handoff requires endpoint and bearer_token".to_string());
    }

    let payload = build_payload(&folder, meeting_name, &config)?;
    let client = reqwest::Client::new();
    let response = client
        .post(config.endpoint.trim())
        .bearer_auth(config.bearer_token.trim())
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            let _ = write_failed_marker(&folder, e.to_string(), None);
            e.to_string()
        })?;

    let status = response.status();
    let response_text = response.text().await.unwrap_or_default();

    if status.is_success() {
        write_submitted_marker(&folder, status.as_u16(), &response_text)?;
        Ok(OpenClawSubmissionResult {
            submitted: true,
            status_code: Some(status.as_u16()),
            message: format!("OpenClaw endpoint returned HTTP {}", status.as_u16()),
        })
    } else {
        write_failed_marker(&folder, response_text.clone(), Some(status.as_u16()))?;
        Err(format!(
            "OpenClaw endpoint returned HTTP {}: {}",
            status.as_u16(),
            response_text
        ))
    }
}

fn load_config<R: Runtime>(app: &AppHandle<R>) -> Result<OpenClawConfig, String> {
    let mut config = match fs::read_to_string(config_path(app)?) {
        Ok(content) if !content.trim().is_empty() => {
            serde_json::from_str::<OpenClawConfig>(&content).map_err(|e| e.to_string())?
        }
        _ => OpenClawConfig::default(),
    };

    if let Ok(value) = env::var("MEETILY_OPENCLAW_ENABLED") {
        config.enabled = matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
    }
    if let Ok(value) = env::var("MEETILY_OPENCLAW_ENDPOINT") {
        config.endpoint = value;
    }
    if let Ok(value) = env::var("MEETILY_OPENCLAW_BEARER_TOKEN") {
        config.bearer_token = value;
    }
    if let Ok(value) = env::var("MEETILY_OPENCLAW_SOURCE") {
        config.source = value;
    }

    Ok(config)
}

fn config_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join(CONFIG_FILE_NAME))
}

fn build_payload(
    folder: &Path,
    meeting_name: Option<String>,
    config: &OpenClawConfig,
) -> Result<Value, String> {
    let transcript_path = folder.join("transcripts.json");
    if !transcript_path.exists() {
        return Err(format!(
            "Missing transcript artifact: {}",
            transcript_path.display()
        ));
    }

    let metadata_path = folder.join("metadata.json");
    let metadata = read_json_if_exists(&metadata_path)?;
    let transcript = read_json_if_exists(&transcript_path)?;
    let title = read_string(
        &metadata,
        &[
            "title",
            "meeting_title",
            "meeting_name",
            "meetingTitle",
            "name",
        ],
    )
    .or(meeting_name)
    .unwrap_or_else(|| {
        folder
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "Meetily recording".to_string())
    });

    let audio_path = if config.include_audio_path {
        find_audio_file(folder).map(|path| path.to_string_lossy().to_string())
    } else {
        None
    };

    Ok(json!({
        "type": "meeting.completed",
        "source": config.source,
        "meeting_id": build_meeting_id(folder, &title),
        "idempotency_key": build_idempotency_key(folder),
        "meeting": {
            "title": title,
            "started_at": read_string(&metadata, &["started_at", "startedAt", "start_time", "startTime", "created_at", "createdAt"]),
            "completed_at": read_string(&metadata, &["completed_at", "completedAt", "ended_at", "endedAt"]),
            "duration_seconds": read_number(&metadata, &["duration_seconds", "durationSeconds"]),
            "platform": read_string(&metadata, &["platform"])
        },
        "artifacts": {
            "layout": "meetily-json-v1",
            "folder": folder.to_string_lossy().to_string(),
            "metadata": if metadata_path.exists() { Some("metadata.json") } else { None },
            "transcript": "transcripts.json",
            "audio": audio_path,
            "checksums": {}
        },
        "transcript_markdown": build_transcript_markdown(&title, &transcript),
        "warnings": []
    }))
}

fn read_json_if_exists(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(Value::Null);
    }

    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&content).map_err(|e| format!("{}: {}", path.display(), e))
}

fn read_string(value: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        value
            .get(*name)
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

fn read_number(value: &Value, names: &[&str]) -> Option<f64> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_f64))
}

fn build_meeting_id(folder: &Path, title: &str) -> String {
    let candidate = folder
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| title.to_string());

    let normalized: String = candidate
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    let collapsed = normalized
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if collapsed.is_empty() {
        "meetily-recording".to_string()
    } else {
        collapsed
    }
}

fn build_idempotency_key(folder: &Path) -> String {
    format!(
        "{}-transcripts-v1",
        build_meeting_id(folder, "Meetily recording")
    )
}

fn build_transcript_markdown(title: &str, transcript: &Value) -> String {
    let mut markdown = format!("# Transcript\n\nMeeting: {}\n\n", title);
    let mut wrote_transcript_line = false;

    if let Some(segments) = transcript.get("segments").and_then(Value::as_array) {
        markdown.push_str("## Transcript with timestamps\n\n");
        for segment in segments {
            let text = segment
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if text.is_empty() {
                continue;
            }

            let timestamp = segment
                .get("display_time")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| {
                    segment
                        .get("audio_start_time")
                        .and_then(Value::as_f64)
                        .map(format_timestamp)
                })
                .unwrap_or_else(|| "[00:00]".to_string());

            markdown.push_str(&format!("{} {}\n", timestamp, text));
            wrote_transcript_line = true;
        }
    } else if let Some(segments) = transcript.as_array() {
        markdown.push_str("## Transcript\n\n");
        for segment in segments {
            let text = segment
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if !text.is_empty() {
                markdown.push_str(text);
                markdown.push('\n');
                wrote_transcript_line = true;
            }
        }
    }

    if !wrote_transcript_line {
        markdown.push_str("Transcript artifact: transcripts.json\n");
    }

    markdown
}

fn format_timestamp(seconds: f64) -> String {
    let total_seconds = seconds.max(0.0).floor() as u64;
    format!("[{:02}:{:02}]", total_seconds / 60, total_seconds % 60)
}

fn find_audio_file(folder: &Path) -> Option<PathBuf> {
    ["audio.mp4", "audio.wav", "recording.wav", "audio.m4a"]
        .iter()
        .map(|name| folder.join(name))
        .find(|path| path.exists())
}

fn write_submitted_marker(folder: &Path, status_code: u16, response: &str) -> Result<(), String> {
    let marker = json!({
        "schema": "openclaw.meetily-submission.v1",
        "submitted_at": Utc::now().to_rfc3339(),
        "status_code": status_code,
        "response": response
    });
    write_marker(folder.join(SUBMITTED_MARKER), &marker)
}

fn write_failed_marker(
    folder: &Path,
    error: String,
    status_code: Option<u16>,
) -> Result<(), String> {
    let marker = json!({
        "schema": "openclaw.meetily-submission-failed.v1",
        "failed_at": Utc::now().to_rfc3339(),
        "status_code": status_code,
        "error": error
    });
    write_marker(folder.join(FAILED_MARKER), &marker)
}

fn write_marker(path: PathBuf, marker: &Value) -> Result<(), String> {
    let content = serde_json::to_string_pretty(marker).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())
}
