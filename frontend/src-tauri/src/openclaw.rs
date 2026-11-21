use chrono::Utc;
use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, Runtime};

const CONFIG_FILE_NAME: &str = "openclaw.json";
const LAST_STATUS_FILE_NAME: &str = "openclaw-last-submission.json";
const PENDING_MARKER: &str = ".openclaw-pending.json";
const SUBMITTED_MARKER: &str = ".openclaw-submitted.json";
const FAILED_MARKER: &str = ".openclaw-failed.json";
pub mod handoff;
static CONFIG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
// No prefilled endpoints — the user supplies their own OpenClaw URLs. (A
// hardcoded dev IP must never ship as a default.)
const DEFAULT_ENDPOINT: &str = "";
const DEFAULT_MODEL_ENDPOINT: &str = "";
const DEFAULT_SOURCE: &str = "ClawScribe";

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenClawConfig {
    #[serde(skip)]
    pub destination_problem: Option<String>,
    pub allow_unencrypted: bool,
    pub enabled: bool,
    pub endpoint: String,
    pub model_endpoint: String,
    pub bearer_token: String,
    pub source: String,
    pub include_audio_path: bool,
}

crate::credentials::debug_without_secrets!(OpenClawConfig {
    destination_problem, allow_unencrypted, enabled, endpoint, model_endpoint, source, include_audio_path;
    secret bearer_token
});

impl Default for OpenClawConfig {
    fn default() -> Self {
        Self {
            destination_problem: None,
            allow_unencrypted: false,
            enabled: false,
            endpoint: DEFAULT_ENDPOINT.to_string(),
            model_endpoint: DEFAULT_MODEL_ENDPOINT.to_string(),
            bearer_token: String::new(),
            source: DEFAULT_SOURCE.to_string(),
            include_audio_path: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawConfigStatus {
    pub destination_problem: Option<String>,
    pub allow_unencrypted: bool,
    pub enabled: bool,
    pub configured: bool,
    pub ready: bool,
    pub delivery_state: String,
    pub model_ready: bool,
    pub bearer_token_configured: bool,
    pub endpoint: String,
    pub model_endpoint: String,
    pub source: String,
    pub status_message: String,
    pub config_path: String,
    pub last_status_path: String,
    pub include_audio_path: bool,
    pub last_submission: Option<OpenClawSubmissionStatus>,
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
    let endpoint_configured = !config.endpoint.trim().is_empty();
    let source_configured = !config.source.trim().is_empty();
    let configured = endpoint_configured && source_configured && bearer_token_configured;
    let destination_error = config.destination_problem.clone();
    let problem = handoff_config_problem(&config);
    let ready = config.enabled && configured && problem.is_none();
    let model_ready = config.enabled
        && bearer_token_configured
        && crate::openai::secret_destination::validate_secret_destination(
            &config.model_endpoint,
            config.allow_unencrypted,
        )
        .is_ok();
    let last_submission = match handoff::last_delivery(&app, &config).await {
        Some(status) => Some(status),
        None => read_last_submission_status(&last_status_path)
            .ok()
            .flatten()
            .and_then(|status| legacy_delivery_status(status, &config)),
    };
    let delivery_state = last_submission
        .as_ref()
        .map(|status| status.state.as_str())
        .unwrap_or("unverified")
        .to_owned();

    Ok(OpenClawConfigStatus {
        destination_problem: destination_error.clone(),
        allow_unencrypted: config.allow_unencrypted,
        enabled: config.enabled,
        configured,
        ready,
        delivery_state,
        model_ready,
        bearer_token_configured,
        endpoint: config.endpoint,
        model_endpoint: config.model_endpoint,
        source: config.source,
        status_message: if !config.enabled {
            "OpenClaw handoff is disabled.".into()
        } else {
            problem.unwrap_or_else(|| "OpenClaw handoff settings are complete.".into())
        },
        config_path: config_path.to_string_lossy().to_string(),
        last_status_path: last_status_path.to_string_lossy().to_string(),
        include_audio_path: config.include_audio_path,
        last_submission,
    })
}

#[tauri::command]
pub async fn save_openclaw_config<R: Runtime>(
    app: AppHandle<R>,
    config: OpenClawConfig,
) -> Result<(), String> {
    use crate::summary::{automatic, llm_client::LLMProvider};
    let _route = automatic::ROUTE_CHANGE_LOCK.lock().await;
    let selected = match app.try_state::<crate::state::AppState>() {
        Some(state) => {
            crate::database::repositories::setting::SettingsRepository::get_model_config(
                state.db_manager.pool(),
            )
            .await
            .map_err(|_| "Could not read the saved summary model settings")?
        }
        None => None,
    };
    let _guard = CONFIG_LOCK
        .lock()
        .map_err(|_| "Provider settings are unavailable")?;
    let path = config_path(&app)?;
    if selected.is_some_and(|setting| {
        LLMProvider::from_str(&setting.provider) == Ok(LLMProvider::OpenClaw)
    }) {
        let old = load_config_from_path(&path).ok();
        if old.is_none_or(|old| {
            old.enabled != config.enabled
                || old.allow_unencrypted != config.allow_unencrypted
                || !automatic::same_endpoint(&old.model_endpoint, &config.model_endpoint)
        }) {
            automatic::invalidate_external_review(&app)?;
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    save_config_at(
        &path,
        config,
        |value| crate::credentials::seal("openclaw/bearer-token", value),
        forget_bearer_reference,
    )?;
    let _ = app.emit("openclaw-config-changed", ());
    Ok(())
}

/// Removes the credential-store entry behind a replaced or cleared token.
fn forget_bearer_reference(reference: &str) {
    if crate::credentials::forget("openclaw/bearer-token", reference).is_err() {
        warn!("A replaced OpenClaw credential could not be removed from the credential store");
    }
}

/// The protected reference of the stored bearer token, exactly as it is in the
/// file: never the token itself. A token an old version stored in plaintext is
/// sealed first.
fn stored_bearer_reference(path: &Path) -> Result<Option<String>, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => content,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(
                "Provider settings could not be read. Existing settings were preserved.".into(),
            )
        }
    };
    let value: Value =
        serde_json::from_str(&content).map_err(|_| "Invalid OpenClaw configuration")?;
    let token = value
        .get("bearer_token")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if token.is_empty() {
        Ok(None)
    } else if crate::credentials::is_protected(token) {
        Ok(Some(token.to_string()))
    } else {
        crate::credentials::seal("openclaw/bearer-token", token).map(Some)
    }
}

fn save_config_at(
    path: &Path,
    config: OpenClawConfig,
    protect: impl FnOnce(&str) -> Result<String, String>,
    forget_old: impl FnOnce(&str),
) -> Result<(), String> {
    let mut config = normalize_config(config);
    validate_endpoints(&config)?;
    let old_reference = stored_bearer_reference(path)?;
    let mut new_reference: Option<String> = None;
    if config.bearer_token.is_empty() {
        // Untouched: the stored token stays exactly as it is, and only for the
        // addresses it was saved with. A changed address needs the token again.
        if old_reference.is_some() && destination_changed(path, &config)? {
            return Err(crate::credentials::KEY_REQUIRED_FOR_NEW_ADDRESS.into());
        }
        config.bearer_token = old_reference.clone().unwrap_or_default();
    } else {
        if let Some(problem) = crate::credentials::new_secret_problem(&config.bearer_token) {
            return Err(problem.into());
        }
        // Replaced: the new token is written first, the old entry removed after.
        config.bearer_token = protect(&config.bearer_token)?;
        new_reference = Some(config.bearer_token.clone());
    }
    if let Err(error) = write_config_atomically(&path, &config) {
        // The file still names the old token: the entry just written is not
        // referenced by anything and is removed again.
        if let Some(new_reference) = &new_reference {
            forget_old(new_reference);
        }
        return Err(error);
    }
    if let Some(old) = old_reference {
        if old != config.bearer_token && crate::credentials::is_protected(&old) {
            forget_old(&old);
        }
    }
    Ok(())
}

/// True when the saved configuration names a different destination for the
/// token than the stored one (either endpoint), compared by scheme, host, port
/// and path. An endpoint that was empty and now is set counts as changed; one
/// that is cleared is not a destination.
fn destination_changed(path: &Path, requested: &OpenClawConfig) -> Result<bool, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => content,
        _ => return Ok(false),
    };
    let stored: Value =
        serde_json::from_str(&content).map_err(|_| "Invalid OpenClaw configuration")?;
    let field = |name: &str| {
        stored
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    Ok([
        (field("endpoint"), &requested.endpoint),
        (field("model_endpoint"), &requested.model_endpoint),
    ]
    .iter()
    .any(|(stored, requested)| {
        !requested.is_empty()
            && !crate::openai::secret_destination::keeps_destination(stored, requested)
    }))
}

/// Removes the stored bearer token and its credential-store entry; the rest of
/// the configuration stays.
#[tauri::command]
pub async fn clear_openclaw_bearer_token<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let _guard = CONFIG_LOCK
        .lock()
        .map_err(|_| "Provider settings are unavailable")?;
    clear_bearer_at(&config_path(&app)?, forget_bearer_reference)?;
    let _ = app.emit("openclaw-config-changed", ());
    Ok(())
}

fn clear_bearer_at(path: &Path, forget_old: impl FnOnce(&str)) -> Result<(), String> {
    let Some(old) = stored_bearer_reference(path)? else {
        return Ok(());
    };
    // The file as it is (no environment overrides, no opened token).
    let content = fs::read_to_string(path)
        .map_err(|_| "Provider settings could not be read. Existing settings were preserved.")?;
    let mut config: OpenClawConfig =
        serde_json::from_str(&content).map_err(|_| "Invalid OpenClaw configuration")?;
    config.bearer_token = String::new();
    write_config_atomically(path, &config)?;
    if crate::credentials::is_protected(&old) {
        forget_old(&old);
    }
    Ok(())
}

pub(crate) fn submit_completed_recording<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    empty: bool,
) {
    tauri::async_runtime::spawn(async move {
        if handoff::submit_automatic(&app, &meeting_id, empty)
            .await
            .is_err()
        {
            warn!("Could not complete the OpenClaw handoff");
        }
    });
}
pub(crate) fn load_config<R: Runtime>(app: &AppHandle<R>) -> Result<OpenClawConfig, String> {
    let _guard = CONFIG_LOCK
        .lock()
        .map_err(|_| "Provider settings are unavailable")?;
    load_config_unlocked(app)
}

fn load_config_unlocked<R: Runtime>(app: &AppHandle<R>) -> Result<OpenClawConfig, String> {
    let path = config_path(app)?;
    load_config_from_path(&path)
}

fn load_config_from_path(path: &Path) -> Result<OpenClawConfig, String> {
    let mut config = match fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => {
            let mut value: Value =
                serde_json::from_str(&content).map_err(|_| "Invalid OpenClaw configuration")?;
            let migrated = crate::openai::secret_destination::migrate_http_opt_in(
                &mut value,
                &["endpoint", "model_endpoint"],
            )?;
            let mut config: OpenClawConfig =
                serde_json::from_value(value).map_err(|_| "Invalid OpenClaw configuration")?;
            if migrated {
                if !config.bearer_token.is_empty()
                    && !crate::credentials::is_protected(&config.bearer_token)
                {
                    config.bearer_token =
                        crate::credentials::seal("openclaw/bearer-token", &config.bearer_token)?;
                }
                write_config_atomically(&path, &config)?;
            }
            config
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => OpenClawConfig::default(),
        _ => {
            return Err(
                "Provider settings could not be read. Existing settings were preserved.".into(),
            )
        }
    };

    if !config.bearer_token.is_empty() {
        if !crate::credentials::is_protected(&config.bearer_token) {
            config.bearer_token =
                crate::credentials::seal("openclaw/bearer-token", &config.bearer_token)?;
            write_config_atomically(&path, &config)?;
        }
        if env::var("MEETILY_OPENCLAW_BEARER_TOKEN").is_err() {
            config.bearer_token =
                crate::credentials::open("openclaw/bearer-token", &config.bearer_token)?;
        }
    }

    if let Ok(value) = env::var("MEETILY_OPENCLAW_ENABLED") {
        config.enabled = matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
    }
    if let Ok(value) = env::var("MEETILY_OPENCLAW_ENDPOINT") {
        config.endpoint = value;
    }
    if let Ok(value) = env::var("MEETILY_OPENCLAW_MODEL_ENDPOINT") {
        config.model_endpoint = value;
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

    let mut config = normalize_config(config);
    config.destination_problem = validate_endpoints(&config).err();
    Ok(config)
}

fn write_config_atomically(path: &Path, config: &OpenClawConfig) -> Result<(), String> {
    let content =
        serde_json::to_vec_pretty(config).map_err(|_| "Could not serialize provider settings")?;
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(&content)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result.map_err(|_| "Could not save protected provider settings".into())
}

fn validate_endpoints(config: &OpenClawConfig) -> Result<(), String> {
    for endpoint in [&config.endpoint, &config.model_endpoint] {
        if !endpoint.trim().is_empty() {
            crate::openai::secret_destination::validate_secret_destination(
                endpoint,
                config.allow_unencrypted,
            )?;
        }
    }
    Ok(())
}

fn normalize_config(mut config: OpenClawConfig) -> OpenClawConfig {
    config.endpoint = config.endpoint.trim().to_string();
    config.model_endpoint = config.model_endpoint.trim().to_string();
    config.bearer_token = config.bearer_token.trim().to_string();
    config.source = config.source.trim().to_string();

    // Endpoints stay exactly as the user left them (empty is valid — the
    // handoff is simply "not ready" until configured). Only the source label
    // falls back to a default.
    if config.source.is_empty() {
        config.source = DEFAULT_SOURCE.to_string();
    }

    config
}

fn handoff_config_problem(config: &OpenClawConfig) -> Option<String> {
    if config.endpoint.trim().is_empty() {
        return Some("OpenClaw handoff endpoint is missing.".into());
    }
    if config.bearer_token.trim().is_empty() {
        return Some("OpenClaw bearer token is missing.".into());
    }
    crate::openai::secret_destination::validate_secret_destination(
        &config.endpoint,
        config.allow_unencrypted,
    )
    .err()
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
    meeting_id: &str,
    idempotency_key: &str,
) -> Result<Option<Value>, String> {
    let transcript_path = folder.join("transcripts.json");
    if !fs::symlink_metadata(&transcript_path).is_ok_and(|meta| meta.is_file()) {
        return Err("The saved transcript file is unavailable.".into());
    }

    use crate::database::meeting_files;
    let metadata = meeting_files::read_marker(folder)
        .filter(|marker| !meeting_files::has_invalid_marker_id(marker))
        .filter(|marker| {
            meeting_files::stamped_meeting_id(marker)
                .or_else(|| meeting_files::recording_id(marker))
                == Some(meeting_id)
        })
        .ok_or("The recording metadata could not be verified for this meeting.")?;
    let transcript = read_json_if_exists(&transcript_path)?;
    let title = meeting_name
        .or_else(|| {
            read_string(
                &metadata,
                &[
                    "title",
                    "meeting_title",
                    "meeting_name",
                    "meetingTitle",
                    "name",
                ],
            )
        })
        .unwrap_or_else(|| {
            folder
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "ClawScribe recording".to_string())
        });

    let audio_path = if config.include_audio_path {
        find_audio_file(folder).map(|path| path.to_string_lossy().to_string())
    } else {
        None
    };

    let Some(markdown) = build_transcript_markdown(&title, &transcript) else {
        return Ok(None);
    };
    Ok(Some(json!({
        "type": "meeting.completed",
        "source": config.source,
        "meeting_id": meeting_id,
        "idempotency_key": idempotency_key,
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
            "metadata": "metadata.json",
            "transcript": "transcripts.json",
            "audio": audio_path,
            "checksums": {}
        },
        "transcript_markdown": markdown,
        "warnings": []
    })))
}

fn read_json_if_exists(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(Value::Null);
    }

    let content =
        fs::read_to_string(path).map_err(|_| "A saved handoff artifact is unavailable.")?;
    serde_json::from_str(&content).map_err(|_| "A saved handoff artifact could not be read.".into())
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

fn build_transcript_markdown(title: &str, transcript: &Value) -> Option<String> {
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

            let speaker = segment
                .get("speaker")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| format!("{s}: "))
                .unwrap_or_default();

            markdown.push_str(&format!("{} {}{}\n", timestamp, speaker, text));
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

    wrote_transcript_line.then_some(markdown)
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

fn legacy_delivery_status(
    mut status: OpenClawSubmissionStatus,
    config: &OpenClawConfig,
) -> Option<OpenClawSubmissionStatus> {
    let endpoint = handoff::delivery_destination_key(status.endpoint.as_deref()?)?;
    if Some(endpoint) != handoff::delivery_destination_key(&config.endpoint) {
        return None;
    }
    let (state, message) = match status.state.as_str() {
        "submitted" => ("submitted", "OpenClaw accepted a previous meeting."),
        "pending" => (
            "unknown",
            "A previous OpenClaw delivery could not be confirmed.",
        ),
        "failed" if status.status_code.is_some_and(handoff::safe_rejection) => {
            ("failed", "OpenClaw rejected a previous meeting.")
        }
        "failed" => (
            "unknown",
            "A previous OpenClaw delivery could not be confirmed.",
        ),
        _ => return None,
    };
    status.state = state.into();
    status.message = message.into();
    status.endpoint = None;
    status.folder_path = None;
    status.marker_path = None;
    status.source = None;
    status.idempotency_key = None;
    Some(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn rejected_legacy_config_loads_and_https_repair_preserves_token() {
        use base64::Engine;
        let protect = |value: &str| -> Result<String, String> {
            let encrypted = crate::exports::token_store::protect_bytes(
                format!("openclaw/bearer-token\0{value}").as_bytes(),
            )
            .map_err(|error| error.to_string())?;
            Ok(format!(
                "clawscribe:secret:v1:dpapi:{}",
                base64::engine::general_purpose::STANDARD.encode(encrypted)
            ))
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("openclaw.json");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "endpoint": "http://openclaw.example.com", "enabled": true,
                "bearer_token": protect("test-token").unwrap()
            }))
            .unwrap(),
        )
        .unwrap();
        let loaded = load_config_from_path(&path).unwrap();
        assert!(loaded
            .destination_problem
            .as_deref()
            .unwrap()
            .contains("HTTPS"));
        assert_eq!(loaded.bearer_token, "test-token");
        assert!(crate::openai::secret_destination::secret_client(
            reqwest::Client::builder(),
            &loaded.endpoint,
            loaded.allow_unencrypted,
        )
        .await
        .is_err());
        let repaired = OpenClawConfig {
            endpoint: "https://openclaw.example.com".into(),
            bearer_token: String::new(),
            ..loaded
        };
        save_config_at(&path, repaired, protect, |_| {}).unwrap();
        let loaded = load_config_from_path(&path).unwrap();
        assert!(loaded.destination_problem.is_none());
        assert_eq!(loaded.bearer_token, "test-token");
        let persisted: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted["allow_unencrypted"], false);
        assert!(persisted.get("destination_problem").is_none());
        assert!(crate::credentials::is_protected(
            persisted["bearer_token"].as_str().unwrap()
        ));
    }
    #[test]
    fn empty_transcript_artifacts_have_no_handoff_payload() {
        for transcript in [json!({"segments":[{"text":"  "}]}), json!([]), json!({})] {
            assert!(build_transcript_markdown("Synthetic meeting", &transcript).is_none());
        }
        assert!(build_transcript_markdown("Synthetic meeting", &json!([{"text":"Ja"}])).is_some());
    }
}

#[cfg(test)]
mod secret_tests {
    use super::*;
    use std::cell::RefCell;

    const OLD_REFERENCE: &str = "clawscribe:secret:v1:keyring:00000000-0000-4000-8000-000000000001";
    const NEW_REFERENCE: &str = "clawscribe:secret:v1:keyring:00000000-0000-4000-8000-000000000002";

    fn write_stored(dir: &Path) -> PathBuf {
        let path = dir.join("openclaw.json");
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "endpoint": "https://openclaw.example.com", "enabled": true,
                "source": "ClawScribe", "bearer_token": OLD_REFERENCE
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    fn untouched() -> OpenClawConfig {
        OpenClawConfig {
            endpoint: "https://openclaw.example.com".into(),
            enabled: true,
            bearer_token: String::new(),
            ..OpenClawConfig::default()
        }
    }

    fn stored_token(path: &Path) -> String {
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        value["bearer_token"].as_str().unwrap().to_string()
    }

    #[test]
    fn saving_without_a_token_keeps_the_stored_one_and_removes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_stored(dir.path());
        let forgotten = RefCell::new(Vec::new());
        save_config_at(
            &path,
            untouched(),
            |_| panic!("an untouched token is not sealed again"),
            |old| forgotten.borrow_mut().push(old.to_string()),
        )
        .unwrap();
        assert_eq!(stored_token(&path), OLD_REFERENCE);
        assert!(forgotten.borrow().is_empty());
    }

    #[test]
    fn a_failed_file_write_removes_the_token_entry_that_was_just_stored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing-folder").join("openclaw.json");
        let forgotten = RefCell::new(Vec::new());
        let config = OpenClawConfig {
            bearer_token: "fake-new-token-0000".into(),
            ..untouched()
        };
        let error = save_config_at(
            &path,
            config,
            |_| Ok(NEW_REFERENCE.to_string()),
            |reference| forgotten.borrow_mut().push(reference.to_string()),
        )
        .unwrap_err();
        assert!(!error.is_empty());
        assert_eq!(
            *forgotten.borrow(),
            vec![NEW_REFERENCE.to_string()],
            "the new entry is removed, nothing else"
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_stored_token_does_not_follow_a_changed_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_stored(dir.path());
        let before = fs::read(&path).unwrap();
        for changed in [
            OpenClawConfig {
                endpoint: "https://other.example.com".into(),
                ..untouched()
            },
            OpenClawConfig {
                model_endpoint: "https://models.example.com".into(),
                ..untouched()
            },
            OpenClawConfig {
                endpoint: "https://openclaw.example.com:8443".into(),
                ..untouched()
            },
        ] {
            let error = save_config_at(
                &path,
                changed,
                |_| panic!("nothing is sealed"),
                |_| panic!("nothing is removed"),
            )
            .unwrap_err();
            assert!(error.contains("Enter the key again"));
            assert_eq!(fs::read(&path).unwrap(), before, "the file is unchanged");
        }
    }

    #[test]
    fn the_same_endpoint_in_another_spelling_keeps_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_stored(dir.path());
        let spelled = OpenClawConfig {
            endpoint: " HTTPS://OpenClaw.Example.com/ ".into(),
            ..untouched()
        };
        save_config_at(
            &path,
            spelled,
            |_| panic!("not sealed"),
            |_| panic!("not removed"),
        )
        .unwrap();
        assert_eq!(stored_token(&path), OLD_REFERENCE);
    }

    #[test]
    fn a_new_token_may_come_with_a_new_endpoint_and_a_short_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_stored(dir.path());
        let forgotten = RefCell::new(Vec::new());
        let moved = OpenClawConfig {
            endpoint: "https://other.example.com".into(),
            bearer_token: "fake-new-token-0000".into(),
            ..untouched()
        };
        save_config_at(
            &path,
            moved,
            |_| Ok(NEW_REFERENCE.to_string()),
            |old| forgotten.borrow_mut().push(old.to_string()),
        )
        .unwrap();
        assert_eq!(stored_token(&path), NEW_REFERENCE);
        let short = OpenClawConfig {
            bearer_token: "abcd".into(),
            ..untouched()
        };
        let error = save_config_at(
            &path,
            short,
            |_| panic!("not sealed"),
            |_| panic!("not removed"),
        )
        .unwrap_err();
        assert!(error.contains("too short"));
        assert_eq!(stored_token(&path), NEW_REFERENCE);
    }

    #[test]
    fn without_a_stored_token_the_endpoint_changes_freely() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("openclaw.json");
        let config = OpenClawConfig {
            endpoint: "https://one.example.com".into(),
            ..OpenClawConfig::default()
        };
        save_config_at(
            &path,
            config,
            |_| panic!("no token"),
            |_| panic!("nothing stored"),
        )
        .unwrap();
        let config = OpenClawConfig {
            endpoint: "https://two.example.com".into(),
            ..OpenClawConfig::default()
        };
        save_config_at(
            &path,
            config,
            |_| panic!("no token"),
            |_| panic!("nothing stored"),
        )
        .unwrap();
    }

    #[test]
    fn replacing_the_token_stores_the_new_one_and_removes_the_old_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_stored(dir.path());
        let forgotten = RefCell::new(Vec::new());
        let config = OpenClawConfig {
            bearer_token: "fake-new-token-0000".into(),
            ..untouched()
        };
        save_config_at(
            &path,
            config,
            |value| {
                assert_eq!(value, "fake-new-token-0000");
                Ok(NEW_REFERENCE.to_string())
            },
            |old| forgotten.borrow_mut().push(old.to_string()),
        )
        .unwrap();
        assert_eq!(stored_token(&path), NEW_REFERENCE);
        assert_eq!(*forgotten.borrow(), vec![OLD_REFERENCE.to_string()]);
    }

    #[test]
    fn clearing_removes_the_token_and_its_entry_and_keeps_the_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_stored(dir.path());
        let forgotten = RefCell::new(Vec::new());
        clear_bearer_at(&path, |old| forgotten.borrow_mut().push(old.to_string())).unwrap();
        assert_eq!(stored_token(&path), "");
        assert_eq!(*forgotten.borrow(), vec![OLD_REFERENCE.to_string()]);
        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["endpoint"], "https://openclaw.example.com");
        // Clearing again, or with nothing stored, is harmless.
        clear_bearer_at(&path, |_| panic!("nothing left to remove")).unwrap();
        clear_bearer_at(&dir.path().join("missing.json"), |_| panic!("no file")).unwrap();
    }

    #[test]
    fn the_status_shown_to_the_webview_has_no_token() {
        let status = OpenClawConfigStatus {
            destination_problem: None,
            allow_unencrypted: false,
            enabled: true,
            configured: true,
            ready: true,
            delivery_state: "unverified".into(),
            model_ready: false,
            bearer_token_configured: true,
            endpoint: "https://openclaw.example.com".into(),
            model_endpoint: String::new(),
            source: "ClawScribe".into(),
            status_message: String::new(),
            config_path: String::new(),
            last_status_path: String::new(),
            include_audio_path: false,
            last_submission: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"bearer_token_configured\":true"));
        assert!(!json.contains("\"bearer_token\""));
    }

    #[test]
    fn debug_output_of_the_configuration_never_contains_the_token() {
        let config = OpenClawConfig {
            bearer_token: "fake-debug-token-1111".into(),
            ..OpenClawConfig::default()
        };
        assert!(!format!("{config:?}").contains("fake-debug-token-1111"));
    }
}
