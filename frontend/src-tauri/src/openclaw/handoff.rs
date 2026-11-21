use super::*;
use crate::database::meeting_files;
use crate::state::AppState;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::collections::HashSet;
use tauri::Emitter;

const CHANGED: &str = "openclaw-handoff-changed";
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const DEFERRED: &str =
    "Automatic handoff paused for file repair. Review the destination before sending this meeting.";
static ACTIVE: once_cell::sync::Lazy<std::sync::Mutex<HashSet<String>>> =
    once_cell::sync::Lazy::new(Default::default);

struct AttemptGuard(String);

impl AttemptGuard {
    fn acquire(meeting_id: &str) -> Result<Self, String> {
        if !ACTIVE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(meeting_id.into())
        {
            return Err("An OpenClaw handoff for this meeting is in progress".into());
        }
        Ok(Self(meeting_id.into()))
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        ACTIVE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.0);
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum State {
    Skipped,
    Deferred,
    Pending,
    Submitted,
    Failed,
    Unknown,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::Deferred => "deferred",
            Self::Pending => "pending",
            Self::Submitted => "submitted",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Attempt {
    state: State,
    message: String,
    updated_at: String,
    status_code: Option<u16>,
    destination: Option<String>,
    #[serde(default)]
    destination_label: Option<String>,
    idempotency_key: String,
    payload_hash: Option<String>,
    reconciliation_required: bool,
}

impl Attempt {
    fn new(meeting_id: &str, config: &OpenClawConfig) -> Self {
        Self {
            state: State::Skipped,
            message: String::new(),
            updated_at: Utc::now().to_rfc3339(),
            status_code: None,
            destination: delivery_destination_key(&config.endpoint),
            destination_label: display_endpoint(&config.endpoint),
            idempotency_key: format!("clawscribe-{meeting_id}-v1"),
            payload_hash: None,
            reconciliation_required: false,
        }
    }

    fn set(&mut self, state: State, message: impl Into<String>, status_code: Option<u16>) {
        self.state = state;
        self.message = message.into();
        self.status_code = status_code;
        self.updated_at = Utc::now().to_rfc3339();
    }

    fn record_result(&mut self, response: Result<reqwest::Response, reqwest::Error>) {
        match response {
            Ok(response) if response.status().is_success() => self.set(
                State::Submitted,
                "OpenClaw accepted this meeting.",
                Some(response.status().as_u16()),
            ),
            Ok(response) => {
                let code = response.status().as_u16();
                if safe_rejection(code) {
                    self.set(
                        State::Failed,
                        format!("OpenClaw rejected the handoff (HTTP {code})."),
                        Some(code),
                    );
                } else {
                    self.set(State::Unknown, format!("OpenClaw returned HTTP {code}. Verify whether it received the meeting before retrying."), Some(code));
                    self.reconciliation_required = true;
                }
            }
            Err(error) if error.is_connect() && !error.is_timeout() => self.set(
                State::Failed,
                "Could not connect to OpenClaw. Check the endpoint and retry.",
                None,
            ),
            Err(_) => {
                self.set(State::Unknown, "ClawScribe could not confirm delivery. Check OpenClaw for this meeting before retrying.", None);
                self.reconciliation_required = true;
            }
        }
    }

    fn recovered(mut self, in_flight: bool) -> Self {
        if self.state == State::Pending && !in_flight {
            self.state = State::Unknown;
            self.reconciliation_required = true;
            self.message =
                "ClawScribe closed before confirming whether OpenClaw received this meeting."
                    .into();
        }
        self
    }

    fn display_destination(&self, config: &OpenClawConfig) -> Option<String> {
        self.destination_label.clone().or_else(|| {
            (self.destination.is_some()
                && self.destination == delivery_destination_key(&config.endpoint))
            .then(|| display_endpoint(&config.endpoint))
            .flatten()
        })
    }

    fn reconciliation_token(&self, meeting_id: &str, config: &OpenClawConfig) -> Option<String> {
        if self.state != State::Unknown {
            return None;
        }
        let destination = self.destination.as_deref()?;
        self.display_destination(config)?;
        Some(hash(
            format!(
                "{meeting_id}\n{destination}\n{}\n{}",
                self.idempotency_key, self.updated_at
            )
            .as_bytes(),
        ))
    }

    fn encode_payload(
        &mut self,
        mut payload: Value,
        old: Option<&Self>,
    ) -> Result<Vec<u8>, String> {
        let encode = |value: &Value| {
            serde_json::to_vec(value).map_err(|_| "Could not encode the meeting handoff".to_owned())
        };
        let mut bytes = encode(&payload)?;
        let mut fingerprint = hash(&bytes);
        if old.is_some_and(|old| {
            old.state == State::Failed
                && old.destination == self.destination
                && match old.payload_hash.as_deref() {
                    Some(previous) => previous != fingerprint,
                    None => old.reconciliation_required,
                }
        }) {
            // A destination can cache an earlier response by key. A reviewed
            // retry with different content needs its own key in both places.
            self.idempotency_key = format!("clawscribe-{fingerprint}");
            payload["idempotency_key"] = json!(self.idempotency_key);
            bytes = encode(&payload)?;
            fingerprint = hash(&bytes);
        }
        self.payload_hash = Some(fingerprint);
        Ok(bytes)
    }
}

#[derive(Serialize)]
pub struct MeetingHandoffStatus {
    meeting_id: String,
    state: &'static str,
    message: String,
    updated_at: Option<String>,
    status_code: Option<u16>,
    destination: Option<String>,
    attempted_destination: Option<String>,
    includes_audio_path: bool,
    review_token: Option<String>,
    reconciliation_token: Option<String>,
    can_submit: bool,
    can_reconcile: bool,
}

pub(super) fn delivery_destination_key(endpoint: &str) -> Option<String> {
    let mut parsed = url::Url::parse(endpoint).ok()?;
    parsed.set_fragment(None);
    Some(hash(parsed.as_str().as_bytes()))
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn review_token(config: &OpenClawConfig, meeting_id: &str) -> Option<String> {
    let destination = delivery_destination_key(&config.endpoint)?;
    Some(hash(
        format!(
            "{meeting_id}\n{destination}\n{}\n{}\n{}",
            config.source, config.include_audio_path, config.allow_unencrypted
        )
        .as_bytes(),
    ))
}

fn display_endpoint(endpoint: &str) -> Option<String> {
    let mut parsed = url::Url::parse(endpoint).ok()?;
    parsed.set_query(None);
    parsed.set_fragment(None);
    parsed.set_username("").ok()?;
    parsed.set_password(None).ok()?;
    Some(parsed.to_string())
}

async fn previous(pool: &SqlitePool, meeting_id: &str) -> Result<Option<Attempt>, String> {
    sqlx::query_scalar::<_, String>(
        "SELECT status_json FROM openclaw_handoffs WHERE meeting_id = ?",
    )
    .bind(meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| "Could not read the OpenClaw handoff")?
    .map(|json| {
        serde_json::from_str(&json)
            .map_err(|_| "The saved OpenClaw handoff could not be read".into())
    })
    .transpose()
}

async fn persist<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting_id: &str,
    attempt: &Attempt,
) -> Result<(), String> {
    let json =
        serde_json::to_string(attempt).map_err(|_| "Could not encode the OpenClaw handoff")?;
    sqlx::query(
        "INSERT INTO openclaw_handoffs (meeting_id, destination, updated_at, status_json) VALUES (?, ?, ?, ?)
         ON CONFLICT(meeting_id) DO UPDATE SET destination = excluded.destination,
           updated_at = excluded.updated_at, status_json = excluded.status_json",
    ).bind(meeting_id).bind(&attempt.destination).bind(&attempt.updated_at).bind(json)
        .execute(pool).await.map_err(|_| "Could not save the OpenClaw handoff status")?;
    changed(app, meeting_id);
    Ok(())
}

fn changed<R: Runtime>(app: &AppHandle<R>, meeting_id: &str) {
    let _ = app.emit(CHANGED, json!({ "meeting_id": meeting_id }));
}

pub(super) async fn last_delivery<R: Runtime>(
    app: &AppHandle<R>,
    config: &OpenClawConfig,
) -> Option<OpenClawSubmissionStatus> {
    let state = app.try_state::<AppState>()?;
    let destination = delivery_destination_key(&config.endpoint)?;
    let pool = state.db_manager.pool();
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT meeting_id, status_json FROM openclaw_handoffs WHERE destination = ?
         AND json_extract(status_json, '$.state') <> 'skipped' ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(destination)
    .fetch_optional(pool)
    .await
    .ok()?;
    let Some((id, status)) = row else { return None };
    let running = ACTIVE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains(&id);
    let attempt = serde_json::from_str::<Attempt>(&status)
        .ok()?
        .recovered(running);
    Some(OpenClawSubmissionStatus {
        state: attempt.state.name().into(),
        folder_path: None,
        marker_path: None,
        message: attempt.message,
        updated_at: attempt.updated_at,
        status_code: attempt.status_code,
        endpoint: display_endpoint(&config.endpoint),
        source: None,
        idempotency_key: None,
    })
}

fn verified_folder(folder: PathBuf, meeting_id: &str) -> Result<PathBuf, String> {
    let marker = meeting_files::read_marker(&folder)
        .ok_or("The recording folder metadata is unavailable")?;
    if meeting_files::has_invalid_marker_id(&marker)
        || meeting_files::stamped_meeting_id(&marker)
            .or_else(|| meeting_files::recording_id(&marker))
            != Some(meeting_id)
    {
        return Err("The recording folder could not be verified for this meeting".into());
    }
    Ok(folder)
}

async fn folder_for<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<PathBuf, String> {
    let roots = crate::audio::recording_preferences::recording_roots(app).await;
    let folder = meeting_files::registered_meeting_folder(pool, meeting_id, &roots).await?;
    let id = meeting_id.to_owned();
    tauri::async_runtime::spawn_blocking(move || verified_folder(folder, &id))
        .await
        .map_err(|_| "Could not verify the recording folder")?
}

fn legacy_attempt(
    folder: &Path,
    meeting_id: &str,
    config: &OpenClawConfig,
) -> Result<Option<Attempt>, String> {
    for (name, state) in [
        (SUBMITTED_MARKER, State::Submitted),
        (PENDING_MARKER, State::Unknown),
        (FAILED_MARKER, State::Failed),
    ] {
        let path = folder.join(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() && meta.len() <= 128 * 1024 => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            _ => return Err("Could not verify a previous OpenClaw handoff marker".into()),
        }
        let marker = read_json_if_exists(&path)?;
        let mut attempt = Attempt::new(meeting_id, config);
        attempt.destination = marker["endpoint"]
            .as_str()
            .and_then(delivery_destination_key);
        attempt.destination_label = marker["endpoint"].as_str().and_then(display_endpoint);
        if let Some(key) = marker["idempotency_key"].as_str().filter(|key| {
            key.len() <= 256
                && key.ends_with("-transcripts-v1")
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        }) {
            attempt.idempotency_key = key.to_owned();
        }
        let code = marker["status_code"]
            .as_u64()
            .and_then(|code| u16::try_from(code).ok());
        let state = if state == State::Failed && !code.is_some_and(safe_rejection) {
            State::Unknown
        } else {
            state
        };
        let message = match state {
            State::Submitted => "OpenClaw accepted a previous handoff for this meeting.",
            State::Failed => "OpenClaw rejected a previous handoff for this meeting.",
            _ => "A previous OpenClaw delivery could not be confirmed.",
        };
        attempt.set(state, message, code);
        if let Some(date) = read_string(&marker, &["submitted_at", "pending_at", "failed_at"]) {
            attempt.updated_at = date;
        }
        attempt.reconciliation_required = state == State::Unknown;
        return Ok(Some(attempt));
    }
    Ok(None)
}

pub(super) fn safe_rejection(code: u16) -> bool {
    matches!(
        code,
        400 | 401 | 403 | 404 | 405 | 411 | 413 | 414 | 415 | 422 | 429
    )
}

async fn read_attempt<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting_id: &str,
    config: &OpenClawConfig,
) -> Result<Option<Attempt>, String> {
    if let Some(attempt) = previous(pool, meeting_id).await? {
        let in_flight = ACTIVE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(meeting_id);
        return Ok(Some(attempt.recovered(in_flight)));
    }
    let Ok(folder) = folder_for(app, pool, meeting_id).await else {
        return Ok(None);
    };
    let id = meeting_id.to_owned();
    let config = config.clone();
    tauri::async_runtime::spawn_blocking(move || legacy_attempt(&folder, &id, &config))
        .await
        .map_err(|_| "Could not review previous OpenClaw handoffs")?
}

#[tauri::command]
pub async fn get_openclaw_submission_status<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
) -> Result<MeetingHandoffStatus, String> {
    let state = app
        .try_state::<AppState>()
        .ok_or("Meeting storage is unavailable")?;
    let pool = state.db_manager.pool();
    let eligible: Option<bool> = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM transcripts WHERE meeting_id = m.id AND length(trim(transcript)) > 0)
         AND m.folder_path IS NOT NULL AND trim(m.folder_path) <> ''
         AND NOT EXISTS(SELECT 1 FROM transcript_file_sync WHERE meeting_id = m.id)
         AND NOT EXISTS(SELECT 1 FROM recording_outcomes WHERE meeting_id = m.id AND recording_files_incomplete = 1)
         FROM meetings m WHERE m.id = ?",
    ).bind(&meeting_id).fetch_optional(pool).await.map_err(|_| "Could not read this meeting")?;
    let eligible = eligible.ok_or("This meeting is unavailable")?;
    let config = load_config(&app);
    let ready = config
        .as_ref()
        .is_ok_and(|config| config.enabled && handoff_config_problem(config).is_none());
    let config = config.unwrap_or_default();
    let attempt = read_attempt(&app, pool, &meeting_id, &config).await?;
    Ok(meeting_status(
        meeting_id,
        eligible && ready,
        &config,
        attempt,
    ))
}

fn meeting_status(
    meeting_id: String,
    eligible: bool,
    config: &OpenClawConfig,
    attempt: Option<Attempt>,
) -> MeetingHandoffStatus {
    let state = attempt.as_ref().map(|attempt| attempt.state);
    let destination = delivery_destination_key(&config.endpoint);
    let same_destination = attempt.as_ref().is_none_or(|attempt| {
        !attempt.reconciliation_required
            || attempt.destination.is_some() && attempt.destination == destination
    });
    let reconciliation_token = attempt
        .as_ref()
        .and_then(|attempt| attempt.reconciliation_token(&meeting_id, config));
    MeetingHandoffStatus {
        review_token: review_token(config, &meeting_id),
        meeting_id,
        state: state.map_or("not_sent", State::name),
        message: if state == Some(State::Unknown) && reconciliation_token.is_none() {
            "The original OpenClaw destination is unavailable. Check the delivery directly in OpenClaw.".into()
        } else if !same_destination && state == Some(State::Failed) {
            "Restore the original OpenClaw destination before retrying this delivery.".into()
        } else if eligible && state == Some(State::Deferred) {
            "Automatic handoff was paused. Review the destination before sending this meeting."
                .into()
        } else {
            attempt
                .as_ref()
                .map(|attempt| attempt.message.clone())
                .unwrap_or_default()
        },
        updated_at: attempt.as_ref().map(|attempt| attempt.updated_at.clone()),
        status_code: attempt.as_ref().and_then(|attempt| attempt.status_code),
        attempted_destination: attempt.and_then(|attempt| attempt.display_destination(config)),
        destination: display_endpoint(&config.endpoint),
        includes_audio_path: config.include_audio_path,
        can_submit: eligible
            && same_destination
            && matches!(
                state,
                None | Some(State::Skipped | State::Deferred | State::Failed)
            ),
        can_reconcile: reconciliation_token.is_some(),
        reconciliation_token,
    }
}

#[tauri::command]
pub async fn submit_meeting_folder_to_openclaw<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    review_token: String,
) -> Result<MeetingHandoffStatus, String> {
    submit(&app, &meeting_id, Some(&review_token), false).await?;
    get_openclaw_submission_status(app, meeting_id).await
}

pub(super) async fn submit_automatic<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    files_needed_repair: bool,
) -> Result<(), String> {
    submit(app, meeting_id, None, files_needed_repair).await
}

async fn submit<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    reviewed: Option<&str>,
    files_needed_repair: bool,
) -> Result<(), String> {
    let _work = crate::updates::admit_work()?;
    let guard = AttemptGuard::acquire(meeting_id)?;
    let result = submit_guarded(app, meeting_id, reviewed, files_needed_repair).await;
    drop(guard);
    changed(app, meeting_id);
    result
}

async fn submit_guarded<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    reviewed: Option<&str>,
    files_needed_repair: bool,
) -> Result<(), String> {
    let state = app
        .try_state::<AppState>()
        .ok_or("Meeting storage is unavailable")?;
    let pool = state.db_manager.pool();
    let config = load_config(app);
    let config_problem = config
        .as_ref()
        .err()
        .map(|_| "Check the OpenClaw settings.".to_owned());
    let config = config.unwrap_or_default();
    if reviewed.is_none() && config_problem.is_none() && !config.enabled {
        return Ok(());
    }
    let existing = match previous(pool, meeting_id).await? {
        // The current guard is acquired before reading a persisted attempt.
        Some(attempt) => Some(attempt.recovered(false)),
        None => read_attempt(app, pool, meeting_id, &config).await?,
    };
    if let Some(attempt) = &existing {
        if attempt.state == State::Submitted {
            return Ok(());
        }
        if attempt.state == State::Unknown {
            return Err("Check whether OpenClaw received this meeting before retrying".into());
        }
        if reviewed.is_none() {
            return Ok(());
        }
    }
    let Some((title, pending_files, has_text)) = sqlx::query_as::<_, (String, bool, bool)>(
        "SELECT m.title, EXISTS(SELECT 1 FROM transcript_file_sync WHERE meeting_id = m.id)
           OR EXISTS(SELECT 1 FROM recording_outcomes WHERE meeting_id = m.id AND recording_files_incomplete = 1),
         EXISTS(SELECT 1 FROM transcripts WHERE meeting_id = m.id AND length(trim(transcript)) > 0)
         FROM meetings m WHERE id = ?",
    )
    .bind(meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| "Could not read the saved meeting")?
    else {
        return Err("This meeting is unavailable".into());
    };
    let mut attempt = Attempt::new(meeting_id, &config);
    if !has_text {
        if reviewed.is_some() {
            return Err("There is no saved transcript text to send".into());
        }
        attempt.set(
            State::Skipped,
            "No transcript text was captured, so this meeting was skipped.",
            None,
        );
        return persist(app, pool, meeting_id, &attempt).await;
    }
    if reviewed.is_none() && config.enabled && (files_needed_repair || pending_files) {
        attempt.set(State::Deferred, DEFERRED, None);
        return persist(app, pool, meeting_id, &attempt).await;
    }
    let problem = config_problem.or_else(|| handoff_config_problem(&config));
    if !config.enabled || problem.is_some() {
        let message = problem.unwrap_or_else(|| "OpenClaw handoff is disabled.".into());
        if existing.is_none() && reviewed.is_none() {
            attempt.set(State::Failed, &message, None);
            persist(app, pool, meeting_id, &attempt).await?;
        }
        return Err(message);
    }
    if reviewed.is_some() && reviewed != review_token(&config, meeting_id).as_deref() {
        return Err("The OpenClaw destination changed. Review it before sending.".into());
    }
    if pending_files {
        return Err("Repair the recording-folder files before sending this meeting".into());
    }
    if let Some(old) = &existing {
        if old.reconciliation_required && old.destination != attempt.destination {
            return Err("Restore the original OpenClaw destination before retrying".into());
        }
        if old.destination == attempt.destination {
            attempt.idempotency_key = old.idempotency_key.clone();
        }
        attempt.reconciliation_required = old.reconciliation_required;
    }
    let payload = match prepare_payload(
        app,
        pool,
        meeting_id,
        title,
        &config,
        &attempt.idempotency_key,
    )
    .await
    {
        Ok(Some(payload)) => payload,
        Ok(None) if reviewed.is_none() => {
            attempt.set(
                State::Skipped,
                "No transcript text was available, so this meeting was skipped.",
                None,
            );
            return persist(app, pool, meeting_id, &attempt).await;
        }
        Ok(None) => return Err("There is no saved transcript text to send".into()),
        Err(message) => {
            // Keep earlier evidence about an uncertain delivery intact.
            if existing.is_none() {
                attempt.set(State::Failed, &message, None);
                persist(app, pool, meeting_id, &attempt).await?;
            }
            return Err(message);
        }
    };
    let payload = attempt.encode_payload(payload, existing.as_ref())?;

    let client = crate::openai::secret_destination::secret_client(
        reqwest::Client::builder().timeout(SEND_TIMEOUT),
        &config.endpoint,
        config.allow_unencrypted,
    )
    .await;
    let request = client.and_then(|client| {
        client
            .post(&config.endpoint)
            .bearer_auth(&config.bearer_token)
            .header("Idempotency-Key", attempt.idempotency_key.as_str())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payload)
            .build()
            .map(|request| (client, request))
            .map_err(|_| "Could not prepare the OpenClaw request".to_owned())
    });
    let (client, request) = match request {
        Ok(prepared) => prepared,
        Err(_) => {
            // No POST happened. Preserve the most recent actual attempt and
            // its key so a later reviewed retry can use that history.
            if let Some(old) = existing.filter(|old| old.state == State::Failed) {
                attempt = old;
            }
            attempt.set(
                State::Failed,
                "OpenClaw could not be reached. Check the endpoint and network settings.",
                None,
            );
            return persist(app, pool, meeting_id, &attempt).await;
        }
    };
    attempt.set(State::Pending, "Sending the meeting to OpenClaw.", None);
    persist(app, pool, meeting_id, &attempt).await?;
    attempt.record_result(client.execute(request).await);
    persist(app, pool, meeting_id, &attempt).await
}

async fn prepare_payload<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting_id: &str,
    title: String,
    config: &OpenClawConfig,
    key: &str,
) -> Result<Option<Value>, String> {
    let folder = folder_for(app, pool, meeting_id).await?;
    let id = meeting_id.to_owned();
    let key = key.to_owned();
    let config = config.clone();
    tauri::async_runtime::spawn_blocking(move || {
        build_payload(&folder, Some(title), &config, &id, &key)
    })
    .await
    .map_err(|_| "Could not prepare the meeting handoff")?
}

#[tauri::command]
pub async fn reconcile_openclaw_handoff<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    received: bool,
    reconciliation_token: String,
) -> Result<MeetingHandoffStatus, String> {
    let _work = crate::updates::admit_work()?;
    let guard = AttemptGuard::acquire(&meeting_id)?;
    let state = app
        .try_state::<AppState>()
        .ok_or("Meeting storage is unavailable")?;
    let pool = state.db_manager.pool();
    let config = load_config(&app).unwrap_or_default();
    let attempt = match previous(pool, &meeting_id).await? {
        Some(attempt) => Some(attempt.recovered(false)),
        None => read_attempt(&app, pool, &meeting_id, &config).await?,
    };
    let Some(mut attempt) = attempt.filter(|attempt| attempt.state == State::Unknown) else {
        return Err("There is no uncertain delivery to reconcile".into());
    };
    if attempt
        .reconciliation_token(&meeting_id, &config)
        .as_deref()
        != Some(reconciliation_token.as_str())
    {
        return Err("The handoff to review changed. Reopen the delivery review and check the original OpenClaw destination.".into());
    }
    if received {
        attempt.set(
            State::Submitted,
            "You confirmed that OpenClaw received this meeting.",
            None,
        );
    } else {
        attempt.reconciliation_required = true;
        attempt.set(State::Failed, "You confirmed the meeting was absent in OpenClaw. You can review and retry the handoff.", None);
    }
    persist(&app, pool, &meeting_id, &attempt).await?;
    drop(state);
    drop(guard);
    get_openclaw_submission_status(app, meeting_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_stalled_reply_preserves_the_key_and_requires_reconciliation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = OpenClawConfig {
            endpoint: format!(
                "http://{}/meetings/completed",
                listener.local_addr().unwrap()
            ),
            ..OpenClawConfig::default()
        };
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let client = crate::openai::secret_destination::secret_client(
            reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)),
            &config.endpoint,
            false,
        )
        .await
        .unwrap();
        let mut attempt = Attempt::new("meeting-timeout", &config);
        let key = attempt.idempotency_key.clone();
        attempt.set(State::Pending, "Sending", None);
        attempt.record_result(
            client
                .post(&config.endpoint)
                .header("Idempotency-Key", key.as_str())
                .send()
                .await,
        );
        assert_eq!(attempt.state, State::Unknown);
        assert!(attempt.reconciliation_required);
        assert_eq!(attempt.idempotency_key, key);
        server.abort();
    }

    #[test]
    fn interrupted_post_is_unknown_and_needs_reconciliation() {
        let config = OpenClawConfig {
            endpoint: "https://openclaw.example.com/meetings/completed".into(),
            ..OpenClawConfig::default()
        };
        let mut attempt = Attempt::new("meeting-example", &config);
        attempt.set(State::Pending, "Sending", None);
        assert_eq!(attempt.clone().recovered(true).state, State::Pending);
        let status = meeting_status(
            "meeting-example".into(),
            true,
            &config,
            Some(attempt.recovered(false)),
        );
        assert_eq!(status.state, "unknown");
        assert!(!status.can_submit);
        assert!(status.can_reconcile);
        assert!(!safe_rejection(409));
        assert!(!safe_rejection(503));
        assert!(safe_rejection(401));
    }

    #[test]
    fn explicit_absence_releases_retry_only_at_the_original_destination() {
        let mut config = OpenClawConfig {
            endpoint: "https://openclaw.example.com/meetings/completed".into(),
            ..OpenClawConfig::default()
        };
        let mut attempt = Attempt::new("meeting-example", &config);
        attempt.set(State::Failed, "Confirmed absent", None);
        attempt.reconciliation_required = true;
        assert!(
            meeting_status(
                "meeting-example".into(),
                true,
                &config,
                Some(attempt.clone())
            )
            .can_submit
        );
        config.endpoint = "https://elsewhere.example.com/meetings/completed".into();
        assert!(!meeting_status("meeting-example".into(), true, &config, Some(attempt)).can_submit);
    }

    #[test]
    fn a_delivery_review_is_bound_to_the_original_attempt_even_after_settings_change() {
        let mut config = OpenClawConfig {
            endpoint: "https://openclaw.example.com/meetings/completed".into(),
            ..OpenClawConfig::default()
        };
        let id = "meeting-review";
        let mut attempt = Attempt::new(id, &config);
        attempt.set(State::Unknown, "Check the original destination", None);
        let review = attempt.reconciliation_token(id, &config).unwrap();
        config.endpoint = "https://elsewhere.example.com/meetings/completed".into();
        let status = meeting_status(id.into(), true, &config, Some(attempt.clone()));
        assert_eq!(
            status.reconciliation_token.as_deref(),
            Some(review.as_str())
        );
        assert_eq!(
            status.attempted_destination.as_deref(),
            Some("https://openclaw.example.com/meetings/completed")
        );
        attempt.idempotency_key = "another-attempt".into();
        assert_ne!(
            attempt.reconciliation_token(id, &config).as_deref(),
            Some(review.as_str())
        );
    }

    #[test]
    fn file_repair_defers_delivery_until_the_destination_can_be_reviewed() {
        let mut config = OpenClawConfig {
            endpoint: "https://openclaw.example.com/meetings/completed".into(),
            ..OpenClawConfig::default()
        };
        let id = "meeting-repair";
        let mut attempt = Attempt::new(id, &config);
        attempt.set(State::Deferred, DEFERRED, None);
        let waiting = meeting_status(id.into(), false, &config, Some(attempt.clone()));
        assert_eq!(waiting.state, "deferred");
        assert!(!waiting.can_submit);
        config.endpoint = "https://elsewhere.example.com/meetings/completed".into();
        let repaired = meeting_status(id.into(), true, &config, Some(attempt));
        assert_eq!(repaired.state, "deferred");
        assert!(repaired.can_submit);
        assert!(repaired.review_token.is_some());
    }

    #[test]
    fn reviewed_retry_of_edited_content_uses_a_new_key_then_reuses_it_for_identical_content() {
        let config = OpenClawConfig {
            endpoint: "https://openclaw.example.com/meetings/completed".into(),
            ..OpenClawConfig::default()
        };
        let mut old = Attempt::new("meeting-example", &config);
        let original = json!({ "title": "Original", "idempotency_key": old.idempotency_key });
        old.payload_hash = Some(hash(&serde_json::to_vec(&original).unwrap()));
        old.set(State::Failed, "Confirmed absent", None);
        old.reconciliation_required = true;
        let mut retry = old.clone();
        let edited = json!({ "title": "Renamed", "idempotency_key": old.idempotency_key });
        let body = retry.encode_payload(edited, Some(&old)).unwrap();
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_ne!(retry.idempotency_key, old.idempotency_key);
        assert_eq!(
            sent["idempotency_key"].as_str(),
            Some(retry.idempotency_key.as_str())
        );
        assert!(retry.reconciliation_required);
        let mut repeated = retry.clone();
        assert_eq!(repeated.encode_payload(sent, Some(&retry)).unwrap(), body);
        assert_eq!(repeated.idempotency_key, retry.idempotency_key);
    }
}
