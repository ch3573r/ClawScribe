use chrono::{DateTime, Utc};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};

const CONFIG_FILE_NAME: &str = "openclaw.json";
const LAST_STATUS_FILE_NAME: &str = "openclaw-last-submission.json";
const PENDING_MARKER: &str = ".openclaw-pending.json";
const SUBMITTED_MARKER: &str = ".openclaw-submitted.json";
const FAILED_MARKER: &str = ".openclaw-failed.json";
const PENDING_STALE_SECONDS: i64 = 15 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
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
            source: "ClawScribe".to_string(),
            include_audio_path: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawConfigStatus {
    pub enabled: bool,
    pub configured: bool,
    pub bearer_token_configured: bool,
    pub endpoint: String,
    pub source: String,
    pub config_path: String,
    pub last_status_path: String,
    pub include_audio_path: bool,
    pub last_submission: Option<OpenClawSubmissionStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawSubmissionResult {
    pub submitted: bool,
    pub state: String,
    pub status_code: Option<u16>,
    pub message: String,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawSubmissionStatus {
    pub state: String,
    pub folder_path: Option<String>,
    pub marker_path: Option<String>,
    pub updated_at: String,
    pub status_code: Option<u16>,
    pub message: String,
    pub endpoint: Option<String>,
    pub source: Option<String>,
    pub idempotency_key: Option<String>,
}

#[tauri::command]
pub async fn get_openclaw_config_status<R: Runtime>(
    app: AppHandle<R>,
) -> Result<OpenClawConfigStatus, String> {
    let config = load_config(&app)?;
    let config_path = config_path(&app)?;
    let last_status_path = last_status_path(&app)?;
    let bearer_token_configured = !config.bearer_token.trim().is_empty();

    Ok(OpenClawConfigStatus {
        enabled: config.enabled,
        configured: !config.endpoint.trim().is_empty() && bearer_token_configured,
        bearer_token_configured,
        endpoint: config.endpoint,
        source: config.source,
        config_path: config_path.to_string_lossy().to_string(),
        last_status_path: last_status_path.to_string_lossy().to_string(),
        include_audio_path: config.include_audio_path,
        last_submission: read_last_submission_status(&last_status_path)?,
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

#[tauri::command]
pub async fn get_openclaw_submission_status(
    folder_path: String,
) -> Result<Option<OpenClawSubmissionStatus>, String> {
    read_submission_status_from_folder(&PathBuf::from(folder_path))
}

pub fn submit_completed_recording<R: Runtime>(
    app: AppHandle<R>,
    folder_path: Option<String>,
    meeting_name: Option<String>,
) {
    let last_status_path = match last_status_path(&app) {
        Ok(path) => Some(path),
        Err(e) => {
            warn!("OpenClaw handoff status path could not be resolved: {}", e);
            None
        }
    };

    let Some(folder_path) = folder_path else {
        warn!("OpenClaw handoff skipped: Meetily did not provide a recording folder path");
        let status = submission_status(
            "skipped",
            None,
            None,
            "Meetily did not provide a recording folder path".to_string(),
            None,
            None,
            None,
            None,
        );
        write_last_submission_status(last_status_path.as_deref(), &status);
        return;
    };

    let config = match load_config(&app) {
        Ok(config) => config,
        Err(e) => {
            error!("OpenClaw handoff config could not be loaded: {}", e);
            let folder = PathBuf::from(&folder_path);
            let status = submission_status(
                "failed",
                Some(&folder),
                None,
                format!("OpenClaw handoff config could not be loaded: {}", e),
                None,
                None,
                None,
                None,
            );
            write_last_submission_status(last_status_path.as_deref(), &status);
            return;
        }
    };

    if !config.enabled {
        info!("OpenClaw handoff skipped: disabled");
        let folder = PathBuf::from(&folder_path);
        let status = submission_status(
            "disabled",
            Some(&folder),
            None,
            "OpenClaw handoff is disabled".to_string(),
            None,
            Some(&config),
            None,
            None,
        );
        write_last_submission_status(last_status_path.as_deref(), &status);
        return;
    }

    tauri::async_runtime::spawn(async move {
        match submit_folder_with_config(
            PathBuf::from(folder_path),
            meeting_name,
            config,
            last_status_path,
        )
        .await
        {
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
    let last_status_path = last_status_path(app).ok();
    submit_folder_with_config(folder, meeting_name, config, last_status_path).await
}

async fn submit_folder_with_config(
    folder: PathBuf,
    meeting_name: Option<String>,
    config: OpenClawConfig,
    last_status_path: Option<PathBuf>,
) -> Result<OpenClawSubmissionResult, String> {
    if !config.enabled {
        let status = submission_status(
            "disabled",
            Some(&folder),
            None,
            "OpenClaw handoff is disabled".to_string(),
            None,
            Some(&config),
            None,
            None,
        );
        write_last_submission_status(last_status_path.as_deref(), &status);
        return Ok(OpenClawSubmissionResult {
            submitted: false,
            state: "disabled".to_string(),
            status_code: None,
            message: "OpenClaw handoff is disabled".to_string(),
            idempotency_key: None,
        });
    }

    if config.endpoint.trim().is_empty() || config.bearer_token.trim().is_empty() {
        let status = submission_status(
            "not_configured",
            Some(&folder),
            None,
            "OpenClaw handoff requires endpoint and bearer_token".to_string(),
            None,
            Some(&config),
            None,
            None,
        );
        write_last_submission_status(last_status_path.as_deref(), &status);
        return Ok(OpenClawSubmissionResult {
            submitted: false,
            state: "not_configured".to_string(),
            status_code: None,
            message: "OpenClaw handoff requires endpoint and bearer_token".to_string(),
            idempotency_key: None,
        });
    }

    if folder.join(SUBMITTED_MARKER).exists() {
        let status = read_submission_status_from_folder(&folder)?.unwrap_or_else(|| {
            submission_status(
                "submitted",
                Some(&folder),
                Some(&folder.join(SUBMITTED_MARKER)),
                "Recording folder already has an OpenClaw submitted marker".to_string(),
                None,
                Some(&config),
                Some(&build_idempotency_key(&folder)),
                None,
            )
        });
        let status_code = status.status_code;
        let idempotency_key = status.idempotency_key.clone();
        write_last_submission_status(last_status_path.as_deref(), &status);
        return Ok(OpenClawSubmissionResult {
            submitted: false,
            state: "already_submitted".to_string(),
            status_code,
            message: "Recording folder already has an OpenClaw submitted marker".to_string(),
            idempotency_key,
        });
    }

    let idempotency_key = build_idempotency_key(&folder);
    let payload = match build_payload(&folder, meeting_name, &config) {
        Ok(payload) => payload,
        Err(e) => {
            let _ = write_failed_marker(&folder, e.clone(), None, &config, Some(&idempotency_key));
            let status = submission_status(
                "failed",
                Some(&folder),
                Some(&folder.join(FAILED_MARKER)),
                e.clone(),
                None,
                Some(&config),
                Some(&idempotency_key),
                None,
            );
            write_last_submission_status(last_status_path.as_deref(), &status);
            return Err(e);
        }
    };

    if !write_pending_marker(&folder, &config, &idempotency_key)? {
        let status = read_submission_status_from_folder(&folder)?.unwrap_or_else(|| {
            submission_status(
                "pending",
                Some(&folder),
                Some(&folder.join(PENDING_MARKER)),
                "OpenClaw handoff is already in progress".to_string(),
                None,
                Some(&config),
                Some(&idempotency_key),
                None,
            )
        });
        write_last_submission_status(last_status_path.as_deref(), &status);
        return Ok(OpenClawSubmissionResult {
            submitted: false,
            state: "pending".to_string(),
            status_code: None,
            message: "OpenClaw handoff is already in progress".to_string(),
            idempotency_key: Some(idempotency_key),
        });
    }

    let client = reqwest::Client::new();
    let response = match client
        .post(config.endpoint.trim())
        .bearer_auth(config.bearer_token.trim())
        .json(&payload)
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => {
            let message = e.to_string();
            let _ = write_failed_marker(
                &folder,
                message.clone(),
                None,
                &config,
                Some(&idempotency_key),
            );
            let _ = remove_marker(&folder.join(PENDING_MARKER));
            let status = submission_status(
                "failed",
                Some(&folder),
                Some(&folder.join(FAILED_MARKER)),
                message.clone(),
                None,
                Some(&config),
                Some(&idempotency_key),
                None,
            );
            write_last_submission_status(last_status_path.as_deref(), &status);
            return Err(message);
        }
    };

    let status = response.status();
    let response_text = response.text().await.unwrap_or_default();

    if status.is_success() {
        write_submitted_marker(
            &folder,
            status.as_u16(),
            &response_text,
            &config,
            Some(&idempotency_key),
        )?;
        let _ = remove_marker(&folder.join(PENDING_MARKER));
        let _ = remove_marker(&folder.join(FAILED_MARKER));
        let last_status = submission_status(
            "submitted",
            Some(&folder),
            Some(&folder.join(SUBMITTED_MARKER)),
            format!("OpenClaw endpoint returned HTTP {}", status.as_u16()),
            Some(status.as_u16()),
            Some(&config),
            Some(&idempotency_key),
            None,
        );
        write_last_submission_status(last_status_path.as_deref(), &last_status);
        Ok(OpenClawSubmissionResult {
            submitted: true,
            state: "submitted".to_string(),
            status_code: Some(status.as_u16()),
            message: format!("OpenClaw endpoint returned HTTP {}", status.as_u16()),
            idempotency_key: Some(idempotency_key),
        })
    } else {
        write_failed_marker(
            &folder,
            response_text.clone(),
            Some(status.as_u16()),
            &config,
            Some(&idempotency_key),
        )?;
        let _ = remove_marker(&folder.join(PENDING_MARKER));
        let message = format!(
            "OpenClaw endpoint returned HTTP {}: {}",
            status.as_u16(),
            response_text
        );
        let last_status = submission_status(
            "failed",
            Some(&folder),
            Some(&folder.join(FAILED_MARKER)),
            message.clone(),
            Some(status.as_u16()),
            Some(&config),
            Some(&idempotency_key),
            None,
        );
        write_last_submission_status(last_status_path.as_deref(), &last_status);
        Err(message)
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
    if let Ok(value) = env::var("MEETILY_OPENCLAW_INCLUDE_AUDIO_PATH") {
        config.include_audio_path = matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
    }

    Ok(config)
}

fn config_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join(CONFIG_FILE_NAME))
}

fn last_status_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join(LAST_STATUS_FILE_NAME))
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

fn read_last_submission_status(path: &Path) -> Result<Option<OpenClawSubmissionStatus>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|e| e.to_string())
}

fn read_submission_status_from_folder(
    folder: &Path,
) -> Result<Option<OpenClawSubmissionStatus>, String> {
    let submitted_marker = folder.join(SUBMITTED_MARKER);
    if submitted_marker.exists() {
        return marker_to_status(folder, &submitted_marker, "submitted").map(Some);
    }

    let failed_marker = folder.join(FAILED_MARKER);
    if failed_marker.exists() {
        return marker_to_status(folder, &failed_marker, "failed").map(Some);
    }

    let pending_marker = folder.join(PENDING_MARKER);
    if pending_marker.exists() {
        return marker_to_status(folder, &pending_marker, "pending").map(Some);
    }

    Ok(None)
}

fn marker_to_status(
    folder: &Path,
    marker_path: &Path,
    state: &str,
) -> Result<OpenClawSubmissionStatus, String> {
    let marker = read_json_if_exists(marker_path)?;
    let updated_at = read_string(
        &marker,
        &["submitted_at", "failed_at", "pending_at", "updated_at"],
    )
    .unwrap_or_else(|| Utc::now().to_rfc3339());
    let message = marker_message(state, &marker);
    let status_code = marker
        .get("status_code")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok());

    Ok(OpenClawSubmissionStatus {
        state: state.to_string(),
        folder_path: Some(folder.to_string_lossy().to_string()),
        marker_path: Some(marker_path.to_string_lossy().to_string()),
        updated_at,
        status_code,
        message,
        endpoint: read_string(&marker, &["endpoint"]),
        source: read_string(&marker, &["source"]),
        idempotency_key: read_string(&marker, &["idempotency_key"]),
    })
}

fn marker_message(state: &str, marker: &Value) -> String {
    if let Some(error) = marker.get("error").and_then(Value::as_str) {
        return error.to_string();
    }

    if let Some(response) = marker.get("response").and_then(Value::as_str) {
        if !response.trim().is_empty() {
            return response.to_string();
        }
    }

    match state {
        "submitted" => "OpenClaw handoff submitted".to_string(),
        "failed" => "OpenClaw handoff failed".to_string(),
        "pending" => "OpenClaw handoff is in progress".to_string(),
        _ => "OpenClaw handoff status is unknown".to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn submission_status(
    state: &str,
    folder: Option<&Path>,
    marker_path: Option<&Path>,
    message: String,
    status_code: Option<u16>,
    config: Option<&OpenClawConfig>,
    idempotency_key: Option<&str>,
    updated_at: Option<String>,
) -> OpenClawSubmissionStatus {
    OpenClawSubmissionStatus {
        state: state.to_string(),
        folder_path: folder.map(|path| path.to_string_lossy().to_string()),
        marker_path: marker_path.map(|path| path.to_string_lossy().to_string()),
        updated_at: updated_at.unwrap_or_else(|| Utc::now().to_rfc3339()),
        status_code,
        message,
        endpoint: config.map(|config| config.endpoint.clone()),
        source: config.map(|config| config.source.clone()),
        idempotency_key: idempotency_key.map(ToString::to_string),
    }
}

fn write_last_submission_status(path: Option<&Path>, status: &OpenClawSubmissionStatus) {
    let Some(path) = path else {
        return;
    };

    match serde_json::to_value(status) {
        Ok(marker) => {
            if let Err(e) = write_marker(path.to_path_buf(), &marker) {
                warn!("OpenClaw handoff status could not be written: {}", e);
            }
        }
        Err(e) => warn!("OpenClaw handoff status could not be serialized: {}", e),
    }
}

fn write_pending_marker(
    folder: &Path,
    config: &OpenClawConfig,
    idempotency_key: &str,
) -> Result<bool, String> {
    let marker = json!({
        "schema": "openclaw.meetily-submission-pending.v1",
        "pending_at": Utc::now().to_rfc3339(),
        "endpoint": config.endpoint,
        "source": config.source,
        "idempotency_key": idempotency_key
    });
    let path = folder.join(PENDING_MARKER);
    let content = serde_json::to_string_pretty(&marker).map_err(|e| e.to_string())?;

    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(content.as_bytes())
                .map_err(|e| e.to_string())?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if pending_marker_is_stale(&path)? {
                remove_marker(&path)?;
                write_marker(path, &marker)?;
                Ok(true)
            } else {
                Ok(false)
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

fn pending_marker_is_stale(path: &Path) -> Result<bool, String> {
    let marker = read_json_if_exists(path)?;
    let Some(pending_at) = marker.get("pending_at").and_then(Value::as_str) else {
        return Ok(true);
    };

    let parsed = DateTime::parse_from_rfc3339(pending_at)
        .map_err(|e| format!("{}: {}", path.display(), e))?;
    Ok(Utc::now()
        .signed_duration_since(parsed.with_timezone(&Utc))
        .num_seconds()
        > PENDING_STALE_SECONDS)
}

fn write_submitted_marker(
    folder: &Path,
    status_code: u16,
    response: &str,
    config: &OpenClawConfig,
    idempotency_key: Option<&str>,
) -> Result<(), String> {
    let marker = json!({
        "schema": "openclaw.meetily-submission.v1",
        "submitted_at": Utc::now().to_rfc3339(),
        "status_code": status_code,
        "response": response,
        "endpoint": config.endpoint,
        "source": config.source,
        "idempotency_key": idempotency_key
    });
    write_marker(folder.join(SUBMITTED_MARKER), &marker)
}

fn write_failed_marker(
    folder: &Path,
    error: String,
    status_code: Option<u16>,
    config: &OpenClawConfig,
    idempotency_key: Option<&str>,
) -> Result<(), String> {
    let marker = json!({
        "schema": "openclaw.meetily-submission-failed.v1",
        "failed_at": Utc::now().to_rfc3339(),
        "status_code": status_code,
        "error": error,
        "endpoint": config.endpoint,
        "source": config.source,
        "idempotency_key": idempotency_key
    });
    write_marker(folder.join(FAILED_MARKER), &marker)
}

fn remove_marker(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn write_marker(path: PathBuf, marker: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(marker).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())
}
