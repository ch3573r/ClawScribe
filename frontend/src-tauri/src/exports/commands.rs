//! Tauri commands for Microsoft Graph auth and exports.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::exports::actions::{LegacyAction, ReviewedAction};
use crate::exports::auth;
use crate::exports::calendar;
use crate::exports::client::{GraphClient, RetryPolicy, TokioSleeper};
use crate::exports::discovery;
use crate::exports::exporter::{self, ExportContext, OneNoteTarget};
use crate::exports::files;
use crate::exports::ledger::{is_positional_action_id, ExportLedger, PositionalSiblings};
use crate::exports::model::{ExportActionItem, ExportStatus, MicrosoftConnectionState};
use crate::exports::ms_auth_state::MicrosoftAuthState;
use crate::exports::planner::PlannerDestination;
use crate::exports::reqwest_transport::ReqwestGraphTransport;
use crate::exports::todo::ToDoDestination;
use crate::exports::token_store;
use crate::external_url::open_url_in_default_browser;

// ── Response types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrosoftConnectionInfo {
    pub state: MicrosoftConnectionState,
    pub user_display_name: Option<String>,
    pub user_email: Option<String>,
    /// Space-separated scopes the active token was granted. Empty when not
    /// connected. Surfaced so permission problems (e.g. a token missing the
    /// OneNote scope) are diagnosable from the UI.
    pub granted_scopes: Option<String>,
    pub missing_scopes: Vec<String>,
    pub unavailable_exports: Vec<String>,
    pub session_only: bool,
    pub requesting_permissions: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportItemResult {
    pub dedupe_key: String,
    pub local_id: String,
    pub status: String,
    pub resource_id: Option<String>,
    pub web_url: Option<String>,
    pub code: Option<String>,
    pub graph_called: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReportResponse {
    pub overall: String,
    pub connection_state: Option<String>,
    pub items: Vec<ExportItemResult>,
}

impl From<exporter::ExportReport> for ExportReportResponse {
    fn from(r: exporter::ExportReport) -> Self {
        ExportReportResponse {
            overall: format!("{:?}", r.overall).to_ascii_lowercase(),
            connection_state: r
                .connection_state
                .map(|s| format!("{s:?}").to_ascii_lowercase()),
            items: r
                .items
                .into_iter()
                .map(|i| ExportItemResult {
                    dedupe_key: i.dedupe_key,
                    local_id: i.local_id,
                    status: format!("{:?}", i.status).to_ascii_lowercase(),
                    resource_id: i.resource_id,
                    web_url: i.web_url,
                    code: i.code,
                    graph_called: i.graph_called,
                })
                .collect(),
        }
    }
}

// ── Auth commands ───────────────────────────────────────────────────────

/// Begin interactive Microsoft sign-in. Opens the system browser to the Entra
/// sign-in page and captures the loopback redirect (authorization-code + PKCE).
/// Returns immediately; completion (or failure) is delivered via the
/// `microsoft-auth-complete` event so the UI can update without blocking.
#[tauri::command]
pub async fn microsoft_sign_in<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, MicrosoftAuthState>,
    request_missing_permissions: Option<bool>,
) -> Result<(), String> {
    let consent = request_missing_permissions.unwrap_or(false);
    let (config, http, cancel, generation);
    {
        let mut inner = state.inner.write().await;
        if inner.sign_in_cancel.is_some() {
            return Err("Microsoft sign-in is already in progress".into());
        }
        if consent {
            if inner.connection_state != MicrosoftConnectionState::Connected
                || inner.current_token.is_none()
            {
                return Err("Sign in before requesting missing permissions".into());
            }
            cancel = tokio_util::sync::CancellationToken::new();
        } else {
            cancel = super::ms_auth_state::begin_sign_in(&mut inner.connection_state)?;
        }
        inner.sign_in_cancel = Some(cancel.clone());
        inner.begin_auth_attempt();
        generation = inner.generation;
        config = inner.config.clone();
        http = inner.http.clone();
    }

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = crate::exports::interactive_auth::run_interactive_sign_in(
            &http,
            &config,
            |url| {
                let _ = open_url_in_default_browser(url);
            },
            consent,
            cancel.clone(),
        )
        .await;

        let state = app_handle.state::<MicrosoftAuthState>();
        match result {
            Ok(tokens) => {
                let profile = auth::fetch_user_profile(&http, &tokens.access_token)
                    .await
                    .ok();
                let user_id = profile.as_ref().map(|p| p.id.clone()).unwrap_or_default();
                let display_name = profile
                    .as_ref()
                    .map(|p| p.display_name.clone())
                    .unwrap_or_else(|| "Microsoft User".to_string());
                let email = profile.as_ref().and_then(|p| p.email.clone());

                let stored = token_store::StoredToken::from_token_response(
                    &tokens,
                    user_id.clone(),
                    display_name.clone(),
                    email.clone(),
                    config.tenant_id.clone(),
                );
                {
                    let mut inner = state.inner.write().await;
                    if cancel.is_cancelled()
                        || inner.generation != generation
                        || !matches!(
                            inner.connection_state,
                            MicrosoftConnectionState::Connecting
                                | MicrosoftConnectionState::Connected
                        )
                    {
                        return;
                    }
                    let persistence = if stored
                        .refresh_token
                        .as_ref()
                        .is_some_and(|token| !token.is_empty())
                    {
                        token_store::save_token(&stored)
                    } else {
                        // This session cannot be restored; do not leave an older account on disk.
                        token_store::delete_token()
                    };
                    if persistence.is_err() {
                        log::warn!("Could not persist Microsoft session");
                    }
                    // A refresh started while consent was open must not overwrite this grant.
                    inner.begin_auth_attempt();
                    inner.connection_state = MicrosoftConnectionState::Connected;
                    inner.sign_in_cancel = None;
                    inner.pending_device_code = None;
                    inner.user_display_name = Some(display_name.clone());
                    inner.user_email = email.clone();
                    inner.user_id = Some(user_id);
                    inner.current_token = Some(stored);
                }

                let _ = app_handle.emit(
                    "microsoft-auth-complete",
                    serde_json::json!({
                        "state": "connected",
                        "userDisplayName": display_name,
                        "userEmail": email,
                    }),
                );
            }
            Err(e) => {
                let mut inner = state.inner.write().await;
                if cancel.is_cancelled() || !inner.finish_sign_in_failure(generation) {
                    return;
                }
                let error = if consent {
                    let missing = inner
                        .current_token
                        .as_ref()
                        .map(|token| super::permissions::missing_scopes(&config, token))
                        .unwrap_or_default();
                    format!("Ask your administrator to approve: {}", missing.join(", "))
                } else {
                    e.to_string()
                };
                let _ = app_handle.emit(
                    "microsoft-auth-complete",
                    serde_json::json!({
                        "state": inner.connection_state, "error": error,
                        "userDisplayName": inner.user_display_name, "userEmail": inner.user_email,
                    }),
                );
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn microsoft_cancel_sign_in<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, MicrosoftAuthState>,
) -> Result<(), String> {
    let mut inner = state.inner.write().await;
    if let Some(cancel) = inner.sign_in_cancel.take() {
        cancel.cancel();
        let generation = inner.generation;
        inner.finish_sign_in_failure(generation);
        let _ = app.emit(
            "microsoft-auth-complete",
            serde_json::json!({"state": inner.connection_state}),
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn microsoft_sign_out(
    state: tauri::State<'_, MicrosoftAuthState>,
    app_state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    state
        .inner
        .write()
        .await
        .end_session(token_store::delete_token)?;
    crate::library::calendar::clear_microsoft(app_state.db_manager.pool()).await
}

#[tauri::command]
pub async fn microsoft_connection_status(
    state: tauri::State<'_, MicrosoftAuthState>,
) -> Result<MicrosoftConnectionInfo, String> {
    let inner = state.inner.read().await;
    Ok(MicrosoftConnectionInfo {
        missing_scopes: inner
            .current_token
            .as_ref()
            .map(|token| super::permissions::missing_scopes(&inner.config, token))
            .unwrap_or_default(),
        unavailable_exports: inner
            .current_token
            .as_ref()
            .map(super::permissions::unavailable_exports)
            .unwrap_or_default(),
        session_only: inner.current_token.as_ref().is_some_and(|token| {
            token
                .refresh_token
                .as_ref()
                .is_none_or(|value| value.is_empty())
        }),
        requesting_permissions: inner.connection_state == MicrosoftConnectionState::Connected
            && inner.sign_in_cancel.is_some(),
        state: inner.connection_state,
        user_display_name: inner.user_display_name.clone(),
        user_email: inner.user_email.clone(),
        granted_scopes: inner
            .current_token
            .as_ref()
            .map(|t| t.granted_scopes.clone()),
    })
}

// ── Export idempotency ledger persistence ────────────────────────────────
// The exporter dedupes via an ExportLedger, but that only prevents duplicates
// if the ledger is loaded before an export and saved after. We key it by
// meeting id under the app data dir (not the recording folder, which may be
// gone for imported meetings), so re-exporting the same meeting skips
// already-created OneNote pages / Planner tasks across sessions.

pub(crate) fn export_ledger_dir<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    Ok(super::ledger::meeting_dir(&base, meeting_id))
}

// The desktop app is single-instance. Serialize ledger owners across export dialogs.
pub(crate) static EXPORT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Serializes ledger owners and keeps an installer restart from interrupting
/// an outbound write or its ledger checkpoint.
fn export_guard() -> Result<
    (
        tokio::sync::RwLockReadGuard<'static, ()>,
        tokio::sync::MutexGuard<'static, ()>,
    ),
    String,
> {
    let work = crate::updates::admit_work()?;
    let export = EXPORT_LOCK
        .try_lock()
        .map_err(|_| "Another Microsoft export is in progress. Wait for it to finish and retry.")?;
    Ok((work, export))
}

/// An unreadable history must never silently disable duplicate protection.
async fn load_ledger<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
) -> Result<ExportLedger, String> {
    let directory = export_ledger_dir(app, meeting_id)?;
    let meeting_id = meeting_id.to_owned();
    tokio::task::spawn_blocking(move || ExportLedger::load_or_new(&directory, &meeting_id))
        .await
        .map_err(|_| "Export history reader failed.")?
}

// ── Export commands ──────────────────────────────────────────────────────

async fn get_token_and_context(
    state: &MicrosoftAuthState,
    required_scope: &str,
) -> Result<(String, String, String), String> {
    get_token_and_context_with(
        state,
        required_scope,
        |http, config, current, force| async move {
            let result = if force {
                token_store::refresh_session_token(&http, &config, current).await
            } else {
                token_store::ensure_valid_token(&http, &config, Some(current)).await
            };
            result.map_err(|error| error.to_string())
        },
        token_store::save_token,
    )
    .await
}

async fn get_token_and_context_with<F, Fut>(
    state: &MicrosoftAuthState,
    required_scope: &str,
    refresh: F,
    persist: impl FnOnce(&token_store::StoredToken) -> Result<(), token_store::TokenStoreError>,
) -> Result<(String, String, String), String>
where
    F: FnOnce(reqwest::Client, auth::MicrosoftAuthConfig, token_store::StoredToken, bool) -> Fut,
    Fut: std::future::Future<Output = Result<token_store::StoredToken, String>>,
{
    let (config, http, current, generation, missing_scope);
    {
        let mut inner = state.inner.write().await;
        if inner.connection_state != MicrosoftConnectionState::Connected {
            return Err("Microsoft is not connected".into());
        }
        generation = inner.generation;
        config = inner.config.clone();
        http = inner.http.clone();
        current = inner
            .current_token
            .clone()
            .ok_or("Microsoft is not connected")?;
        missing_scope = super::permissions::require_scope(&current, required_scope).err();
        if let Some(error) = missing_scope.as_ref() {
            if current.refresh_token.is_none()
                || !inner.reserve_scope_refresh(required_scope, std::time::Instant::now())
            {
                return Err(error.clone());
            }
        }
    }
    let stored = refresh(http, config, current, missing_scope.is_some())
        .await
        .map_err(|error| match missing_scope {
            Some(permission) => format!("{permission}. Could not refresh permissions: {error}"),
            None => error,
        })?;

    // Cache any refreshed token for the rest of the session.
    {
        let mut inner = state.inner.write().await;
        inner.accept_refreshed_token(generation, stored.clone(), persist)?;
    }

    super::permissions::require_scope(&stored, required_scope)?;

    Ok((stored.access_token, stored.tenant_id, stored.user_id))
}

#[tauri::command]
pub async fn export_to_onenote<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
    meeting_title: String,
    summary_json: String,
    section_id: String,
) -> Result<ExportReportResponse, String> {
    let (token, tenant_id, user_id) = get_token_and_context(&state, "Notes.Create").await?;

    let notes: crate::summary::codex_provider::MeetingNotesOutput =
        serde_json::from_str(&summary_json).map_err(|e| format!("Failed to parse summary: {e}"))?;

    let meeting_export =
        crate::exports::meeting_export_from_notes(&meeting_id, &meeting_title, None, &notes, None);

    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    let _export_guard = export_guard()?;
    let mut ledger = load_ledger(&app, &meeting_id).await?;

    let ctx = ExportContext {
        tenant_id: &tenant_id,
        user_id: &user_id,
        bearer_token: &token,
    };

    let report = exporter::export_onenote(
        &client,
        &mut ledger,
        &meeting_export,
        &OneNoteTarget { section_id },
        &ctx,
    )
    .await;
    ledger.checkpoint().await?;

    if report.connection_state == Some(MicrosoftConnectionState::Expired) {
        let mut inner = state.inner.write().await;
        inner.connection_state = MicrosoftConnectionState::Expired;
    }

    Ok(report.into())
}

#[tauri::command]
pub async fn export_to_planner<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
    meeting_title: String,
    summary_json: String,
    plan_id: String,
    bucket_id: String,
) -> Result<ExportReportResponse, String> {
    let (token, tenant_id, user_id) = get_token_and_context(&state, "Tasks.ReadWrite").await?;

    let notes: crate::summary::codex_provider::MeetingNotesOutput =
        serde_json::from_str(&summary_json).map_err(|e| format!("Failed to parse summary: {e}"))?;

    let meeting_export =
        crate::exports::meeting_export_from_notes(&meeting_id, &meeting_title, None, &notes, None);

    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    let _export_guard = export_guard()?;
    let mut ledger = load_ledger(&app, &meeting_id).await?;
    let destination = PlannerDestination { plan_id, bucket_id };

    let ctx = ExportContext {
        tenant_id: &tenant_id,
        user_id: &user_id,
        bearer_token: &token,
    };

    let result = exporter::export_planner(
        &client,
        &mut ledger,
        &meeting_export,
        &destination,
        &ctx,
        Some(&summary_positional_siblings(&meeting_export.action_items)),
    )
    .await;
    ledger.checkpoint().await?;
    let report = result.map_err(|e| e.to_string())?;

    if report.connection_state == Some(MicrosoftConnectionState::Expired) {
        let mut inner = state.inner.write().await;
        inner.connection_state = MicrosoftConnectionState::Expired;
    }

    Ok(report.into())
}

// ── Per-meeting markdown export ───────────────────────────────────────────
// These power the export buttons in a meeting's summary view. The summary is
// stored as markdown, so OneNote receives the whole summary rendered to XHTML
// and Planner receives the action items parsed out of it.

/// Whether a summary's markdown contains parseable action items (drives whether
/// the Planner export button is shown for a meeting).
#[tauri::command]
pub fn summary_has_action_items(markdown: String) -> bool {
    crate::exports::markdown_notes::has_action_items(&markdown)
}

#[tauri::command]
pub async fn export_meeting_markdown_to_onenote<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
    meeting_title: String,
    markdown: String,
    section_id: String,
) -> Result<ExportReportResponse, String> {
    let (token, tenant_id, user_id) = get_token_and_context(&state, "Notes.Create").await?;

    let meeting_export = crate::exports::markdown_notes::meeting_export_for_onenote(
        &meeting_id,
        &meeting_title,
        None,
        &markdown,
    );

    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    let _export_guard = export_guard()?;
    let mut ledger = load_ledger(&app, &meeting_id).await?;

    let ctx = ExportContext {
        tenant_id: &tenant_id,
        user_id: &user_id,
        bearer_token: &token,
    };

    let report = exporter::export_onenote(
        &client,
        &mut ledger,
        &meeting_export,
        &OneNoteTarget { section_id },
        &ctx,
    )
    .await;
    ledger.checkpoint().await?;

    if report.connection_state == Some(MicrosoftConnectionState::Expired) {
        let mut inner = state.inner.write().await;
        inner.connection_state = MicrosoftConnectionState::Expired;
    }

    Ok(report.into())
}

/// OneNote section names reject `?*\/:<>|&#'%~"` and must be fewer than 50
/// characters (Graph errors 20153 / 20155). Replace forbidden characters with
/// spaces, collapse whitespace, and truncate to 49 chars on a char boundary.
fn sanitize_onenote_section_name(raw: &str) -> String {
    const FORBIDDEN: &[char] = &[
        '?', '*', '\\', '/', ':', '<', '>', '|', '&', '#', '\'', '%', '~', '"',
    ];
    let replaced: String = raw
        .chars()
        .map(|c| if FORBIDDEN.contains(&c) { ' ' } else { c })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated: String = collapsed.chars().take(49).collect();
    let result = truncated.trim().to_string();
    if result.is_empty() {
        "ClawScribe Export".to_string()
    } else {
        result
    }
}

/// Create or resume a section directly by ID, without enumerating sections.
#[tauri::command]
pub async fn export_meeting_to_onenote_section<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
    meeting_title: String,
    markdown: String,
    notebook_id: String,
    section_name: String,
    options: Option<super::ledger::OneNoteOptions>,
    resume_id: Option<String>,
) -> Result<ExportReportResponse, String> {
    let (token, tenant_id, user_id) = get_token_and_context(&state, "Notes.Create").await?;
    let _export_guard = export_guard()?;
    let mut ledger = load_ledger(&app, &meeting_id).await?;
    let client = GraphClient::new(
        ReqwestGraphTransport::new(),
        TokioSleeper,
        RetryPolicy::default(),
    );
    let ctx = ExportContext {
        tenant_id: &tenant_id,
        user_id: &user_id,
        bearer_token: &token,
    };
    let report = super::onenote_resume::export_section(
        &client,
        &mut ledger,
        &ctx,
        super::onenote_resume::SectionRequest {
            notebook_id,
            section_name: sanitize_onenote_section_name(&section_name),
            page_title: meeting_title,
            markdown,
            options,
            resume_id,
        },
    )
    .await?;
    if report.connection_state == Some(MicrosoftConnectionState::Expired) {
        state.inner.write().await.connection_state = MicrosoftConnectionState::Expired;
    }
    Ok(report.into())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OneNoteResume {
    id: String,
    section_name: String,
    page_title: String,
    options: Option<super::ledger::OneNoteOptions>,
    started_at: String,
    can_resume: bool,
}

#[tauri::command]
pub async fn list_onenote_resumes<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
) -> Result<Vec<OneNoteResume>, String> {
    let account = state
        .inner
        .read()
        .await
        .current_token
        .as_ref()
        .map(|token| (token.tenant_id.clone(), token.user_id.clone()));
    let Some((tenant_id, user_id)) = account else {
        return Ok(Vec::new());
    };
    let ledger = load_ledger(&app, &meeting_id).await?;
    let mut resumes: Vec<_> = ledger
        .onenote_sections
        .into_iter()
        .filter(|(_, section)| section.tenant_id == tenant_id && section.user_id == user_id)
        .map(|(id, section)| OneNoteResume {
            id,
            section_name: section.section_name,
            page_title: section.page_title,
            options: section.options,
            started_at: section.started_at,
            can_resume: section.section_id.is_some(),
        })
        .collect();
    resumes.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(resumes)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingExportHistory {
    destination: &'static str,
    status: ExportStatus,
    at: Option<String>,
    web_url: Option<String>,
}

#[tauri::command]
pub async fn get_meeting_export_history<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
) -> Result<Vec<MeetingExportHistory>, String> {
    let ledger = load_ledger(&app, &meeting_id).await?;
    let mut history: Vec<_> = ledger
        .entries
        .into_iter()
        .filter_map(|(key, item)| {
            let destination = match key.split(':').next()? {
                "onenote" => "OneNote",
                "planner" => "Planner",
                "todo" => "Microsoft To Do",
                _ => return None,
            };
            Some(MeetingExportHistory {
                destination,
                status: item.status,
                at: item.last_attempt_at,
                web_url: item
                    .web_url
                    .as_deref()
                    .and_then(super::ledger::microsoft_web_url),
            })
        })
        .collect();
    history.extend(
        ledger
            .onenote_sections
            .into_values()
            .filter(|section| section.section_id.is_none())
            .map(|section| MeetingExportHistory {
                destination: "OneNote section",
                status: ExportStatus::UnknownAfterSubmit,
                at: Some(section.started_at),
                web_url: None,
            }),
    );
    history.sort_by(|a, b| b.at.cmp(&a.at));
    Ok(history)
}

#[tauri::command]
pub async fn export_meeting_markdown_to_planner<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
    meeting_title: String,
    markdown: String,
    plan_id: String,
    bucket_id: String,
) -> Result<ExportReportResponse, String> {
    let (token, tenant_id, user_id) = get_token_and_context(&state, "Tasks.ReadWrite").await?;

    let meeting_export = crate::exports::markdown_notes::meeting_export_for_planner(
        &meeting_id,
        &meeting_title,
        None,
        &markdown,
    );

    if meeting_export.action_items.is_empty() {
        return Err("No action items were found in this meeting's summary.".to_string());
    }

    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    let _export_guard = export_guard()?;
    let mut ledger = load_ledger(&app, &meeting_id).await?;
    let destination = PlannerDestination { plan_id, bucket_id };

    let ctx = ExportContext {
        tenant_id: &tenant_id,
        user_id: &user_id,
        bearer_token: &token,
    };

    let result = exporter::export_planner(
        &client,
        &mut ledger,
        &meeting_export,
        &destination,
        &ctx,
        Some(&summary_positional_siblings(&meeting_export.action_items)),
    )
    .await;
    ledger.checkpoint().await?;
    let report = result.map_err(|e| e.to_string())?;

    if report.connection_state == Some(MicrosoftConnectionState::Expired) {
        let mut inner = state.inner.write().await;
        inner.connection_state = MicrosoftConnectionState::Expired;
    }

    Ok(report.into())
}

// ── Planner export preview / selected export ─────────────────────────────

const MAX_TASK_PREVIEW_CANDIDATES: usize = 250;
const REFRESH_TASK_PREVIEW: &str =
    "Meeting actions or the export preview have changed. Reopen the preview and review your selection again.";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerTaskPreview {
    pub local_id: String,
    pub title: String,
    pub original_title_hash: Option<String>,
    pub details: String,
    pub owner: Option<String>,
    pub due_date: Option<String>,
}

/// Parse a meeting summary's action items for the Planner export preview, so the
/// user can review, edit, deselect, and route them to buckets before anything is
/// created in Planner.
#[tauri::command]
pub async fn preview_planner_tasks(
    state: tauri::State<'_, crate::state::AppState>,
    meeting_id: String,
    meeting_title: String,
    markdown: String,
) -> Result<Vec<PlannerTaskPreview>, String> {
    let mut meeting_export = crate::exports::markdown_notes::meeting_export_for_planner(
        &meeting_id,
        &meeting_title,
        None,
        &markdown,
    );
    if let Some(actions) = super::actions::load(state.db_manager.pool(), &meeting_id).await? {
        meeting_export.action_items = actions
            .into_iter()
            .filter(|action| !action.done)
            .map(|action| super::model::ExportActionItem {
                local_action_id: action.id,
                task: action.title,
                owner: action.owner,
                due_date: action.due_date,
                details: None,
                legacy_action: action.legacy_action,
            })
            .collect();
    } else if meeting_export.action_items.len() > MAX_TASK_PREVIEW_CANDIDATES {
        return Err("This summary has more than 250 suggestions. Review and save up to 250 items in Meeting actions before exporting.".into());
    }
    let previews = meeting_export
        .action_items
        .iter()
        .map(|action| PlannerTaskPreview {
            local_id: action.local_action_id.clone(),
            title: action.task.clone(),
            original_title_hash: super::ledger::is_positional_action_id(&action.local_action_id)
                .then(|| super::planner::action_hash(action)),
            details: crate::exports::planner::build_task_details_description(
                &meeting_export,
                action,
            ),
            owner: action.owner.clone(),
            due_date: action.due_date.clone(),
        })
        .collect();
    Ok(previews)
}

#[tauri::command]
pub async fn preview_todo_tasks(
    state: tauri::State<'_, crate::state::AppState>,
    meeting_id: String,
    meeting_title: String,
    markdown: String,
) -> Result<Vec<PlannerTaskPreview>, String> {
    preview_planner_tasks(state, meeting_id, meeting_title, markdown).await
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerTaskInput {
    /// The preview's stable `local_id` for this task, carried back from
    /// `preview_planner_tasks`. It feeds the Planner dedupe key, so it must
    /// survive reordering/deselection; regenerating it per bucket would shift
    /// the key and create duplicate tasks on re-export.
    #[serde(default)]
    pub local_id: String,
    #[serde(default)]
    pub original_title_hash: Option<String>,
    pub title: String,
    pub owner: Option<String>,
    pub due_date: Option<String>,
    pub bucket_id: String,
    #[serde(default)]
    pub details: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToDoTaskInput {
    #[serde(default)]
    pub local_id: String,
    #[serde(default)]
    pub original_title_hash: Option<String>,
    pub title: String,
    pub owner: Option<String>,
    pub due_date: Option<String>,
    #[serde(default)]
    pub details: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskPreviewCandidate {
    local_id: String,
    original_title_hash: String,
}

struct SelectedTaskHistory<'a> {
    reviewed: Option<&'a [ReviewedAction]>,
    preview: Option<Vec<TaskPreviewCandidate>>,
    siblings: Option<PositionalSiblings>,
}

impl<'a> SelectedTaskHistory<'a> {
    fn new(
        reviewed: Option<&'a [ReviewedAction]>,
        preview: Option<Vec<TaskPreviewCandidate>>,
    ) -> Result<Self, String> {
        if preview
            .as_ref()
            .is_some_and(|candidates| candidates.len() > MAX_TASK_PREVIEW_CANDIDATES)
        {
            return Err(REFRESH_TASK_PREVIEW.into());
        }
        let mut siblings = PositionalSiblings::new();
        if let Some(actions) = reviewed {
            // Saved actions supersede the preview. Completed and unselected rows
            // still identify distinct suggestions that can own older history.
            for legacy in actions.iter().filter_map(|action| action.legacy_action.as_ref()) {
                siblings
                    .entry(legacy.title_hash.clone())
                    .or_default()
                    .insert(legacy.id.clone());
            }
            return Ok(Self {
                reviewed,
                preview: None,
                siblings: Some(siblings),
            });
        }
        if let Some(candidates) = &preview {
            for (index, candidate) in candidates.iter().enumerate() {
                if candidate.local_id != format!("action-{}", index + 1)
                    || !valid_task_title_hash(&candidate.original_title_hash)
                {
                    return Err(REFRESH_TASK_PREVIEW.into());
                }
                siblings
                    .entry(candidate.original_title_hash.clone())
                    .or_default()
                    .insert(candidate.local_id.clone());
            }
        }
        let siblings = preview.as_ref().map(|_| siblings);
        Ok(Self {
            reviewed,
            preview,
            siblings,
        })
    }

    fn legacy_action(
        &self,
        local_id: &str,
        original_title_hash: Option<String>,
    ) -> Result<Option<LegacyAction>, String> {
        if let Some(actions) = self.reviewed {
            return actions
                .iter()
                .find(|action| action.id == local_id && !action.done)
                .map(|action| action.legacy_action.clone())
                .ok_or_else(|| REFRESH_TASK_PREVIEW.into());
        }

        let title_hash = if let Some(candidates) = &self.preview {
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.local_id == local_id)
                .ok_or(REFRESH_TASK_PREVIEW)?;
            if original_title_hash
                .as_ref()
                .is_some_and(|hash| hash != &candidate.original_title_hash)
            {
                return Err(REFRESH_TASK_PREVIEW.into());
            }
            Some(candidate.original_title_hash.clone())
        } else {
            // Older IPC clients may carry only a selected row's original hash.
            original_title_hash
                .filter(|hash| is_positional_action_id(local_id) && valid_task_title_hash(hash))
        };
        Ok(title_hash.map(|title_hash| LegacyAction {
            id: local_id.into(),
            title_hash,
        }))
    }
}

fn valid_task_title_hash(hash: &str) -> bool {
    hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn summary_positional_siblings(actions: &[ExportActionItem]) -> PositionalSiblings {
    let mut siblings = PositionalSiblings::new();
    for action in actions {
        if is_positional_action_id(&action.local_action_id) {
            siblings
                .entry(super::planner::action_hash(action))
                .or_default()
                .insert(action.local_action_id.clone());
        }
    }
    siblings
}

fn planner_action_items(
    tasks: Vec<PlannerTaskInput>,
    history: &SelectedTaskHistory<'_>,
) -> Result<BTreeMap<String, Vec<ExportActionItem>>, String> {
    let mut by_bucket: BTreeMap<String, Vec<ExportActionItem>> = BTreeMap::new();
    for task in tasks {
        let legacy_action = history.legacy_action(&task.local_id, task.original_title_hash)?;
        by_bucket
            .entry(task.bucket_id)
            .or_default()
            .push(ExportActionItem {
                local_action_id: task.local_id,
                task: task.title,
                owner: task.owner,
                due_date: task.due_date,
                details: task.details,
                legacy_action,
            });
    }
    // Fill missing IDs for older clients while retaining IDs from the preview.
    for actions in by_bucket.values_mut() {
        super::planner::ensure_local_action_ids(actions);
    }
    Ok(by_bucket)
}

fn todo_action_items(
    tasks: Vec<ToDoTaskInput>,
    history: &SelectedTaskHistory<'_>,
) -> Result<Vec<ExportActionItem>, String> {
    let mut actions = Vec::with_capacity(tasks.len());
    for task in tasks {
        let legacy_action = history.legacy_action(&task.local_id, task.original_title_hash)?;
        actions.push(ExportActionItem {
            local_action_id: task.local_id,
            task: task.title,
            owner: task.owner,
            due_date: task.due_date,
            details: task.details,
            legacy_action,
        });
    }
    super::planner::ensure_local_action_ids(&mut actions);
    Ok(actions)
}

/// Export a user-reviewed set of Planner tasks. Each task carries its own bucket,
/// so action items from one meeting can be routed to different buckets. Tasks are
/// grouped by bucket and created through the normal Planner exporter (dedupe +
/// retries per bucket), then the per-bucket reports are merged.
#[tauri::command]
pub async fn export_selected_planner_tasks<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
    meeting_title: String,
    plan_id: String,
    tasks: Vec<PlannerTaskInput>,
    preview_candidates: Option<Vec<TaskPreviewCandidate>>,
) -> Result<ExportReportResponse, String> {
    let tasks: Vec<PlannerTaskInput> = tasks
        .into_iter()
        .filter(|t| !t.title.trim().is_empty() && !t.bucket_id.trim().is_empty())
        .collect();
    if tasks.is_empty() {
        return Err("No tasks selected to export.".to_string());
    }

    let reviewed = super::actions::load(
        app.state::<crate::state::AppState>().db_manager.pool(),
        &meeting_id,
    )
    .await?;
    let history = SelectedTaskHistory::new(reviewed.as_deref(), preview_candidates)?;
    let by_bucket = planner_action_items(tasks, &history)?;
    let (token, tenant_id, user_id) = get_token_and_context(&state, "Tasks.ReadWrite").await?;

    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    let ctx = ExportContext {
        tenant_id: &tenant_id,
        user_id: &user_id,
        bearer_token: &token,
    };

    let mut merged_items: Vec<ExportItemResult> = Vec::new();
    let mut expired = false;
    // One ledger for the whole meeting, shared across buckets, so re-exporting
    // the same reviewed tasks skips ones already created in Planner.
    let _export_guard = export_guard()?;
    let mut ledger = load_ledger(&app, &meeting_id).await?;

    for (bucket_id, action_items) in by_bucket {
        let meeting_export = crate::exports::model::MeetingExport {
            meeting_id: meeting_id.clone(),
            title: meeting_title.clone(),
            created_at: None,
            executive_summary: String::new(),
            decisions: Vec::new(),
            action_items,
            transcript_excerpt: None,
            summary_html: None,
        };

        let destination = PlannerDestination {
            plan_id: plan_id.clone(),
            bucket_id,
        };

        let result = exporter::export_planner(
            &client,
            &mut ledger,
            &meeting_export,
            &destination,
            &ctx,
            history.siblings.as_ref(),
        )
        .await;
        let report = match result {
            Ok(report) => report,
            Err(e) => {
                ledger.checkpoint().await?;
                return Err(e.to_string());
            }
        };
        if report.connection_state == Some(MicrosoftConnectionState::Expired) {
            expired = true;
        }
        let resp: ExportReportResponse = report.into();
        merged_items.extend(resp.items);
    }
    ledger.checkpoint().await?;

    if expired {
        let mut inner = state.inner.write().await;
        inner.connection_state = MicrosoftConnectionState::Expired;
    }

    let overall = if merged_items.is_empty() {
        "failed".to_string()
    } else if merged_items
        .iter()
        .all(|i| i.status == "succeeded" || i.status == "skipped")
    {
        "succeeded".to_string()
    } else {
        "partial".to_string()
    };

    Ok(ExportReportResponse {
        overall,
        connection_state: if expired {
            Some("expired".to_string())
        } else {
            None
        },
        items: merged_items,
    })
}

#[tauri::command]
pub async fn export_selected_todo_tasks<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
    meeting_title: String,
    list_id: String,
    tasks: Vec<ToDoTaskInput>,
    preview_candidates: Option<Vec<TaskPreviewCandidate>>,
) -> Result<ExportReportResponse, String> {
    let tasks: Vec<ToDoTaskInput> = tasks
        .into_iter()
        .filter(|t| !t.title.trim().is_empty())
        .collect();
    if tasks.is_empty() {
        return Err("No tasks selected to export.".to_string());
    }

    let reviewed = super::actions::load(
        app.state::<crate::state::AppState>().db_manager.pool(),
        &meeting_id,
    )
    .await?;
    let history = SelectedTaskHistory::new(reviewed.as_deref(), preview_candidates)?;
    let action_items = todo_action_items(tasks, &history)?;
    let (token, tenant_id, user_id) = get_token_and_context(&state, "Tasks.ReadWrite").await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    let ctx = ExportContext {
        tenant_id: &tenant_id,
        user_id: &user_id,
        bearer_token: &token,
    };
    let meeting_export = crate::exports::model::MeetingExport {
        meeting_id: meeting_id.clone(),
        title: meeting_title,
        created_at: None,
        executive_summary: String::new(),
        decisions: Vec::new(),
        action_items,
        transcript_excerpt: None,
        summary_html: None,
    };
    let destination = ToDoDestination { list_id };
    let _export_guard = export_guard()?;
    let mut ledger = load_ledger(&app, &meeting_id).await?;
    let report = exporter::export_todo(
        &client,
        &mut ledger,
        &meeting_export,
        &destination,
        &ctx,
        history.siblings.as_ref(),
    )
    .await
    .map_err(|e| e.to_string())?;
    ledger.checkpoint().await?;

    if report.connection_state == Some(MicrosoftConnectionState::Expired) {
        let mut inner = state.inner.write().await;
        inner.connection_state = MicrosoftConnectionState::Expired;
    }

    Ok(report.into())
}

// ── Discovery commands ──────────────────────────────────────────────────

#[tauri::command]
pub async fn list_onenote_notebooks(
    state: tauri::State<'_, MicrosoftAuthState>,
) -> Result<Vec<discovery::NotebookInfo>, String> {
    let (token, _, _) = get_token_and_context(&state, "Notes.Read").await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    discovery::list_notebooks(&client, &token).await
}

/// Calendar events within an explicit ISO-8601 UTC window (recurrences expanded).
#[tauri::command]
pub async fn list_calendar_events(
    state: tauri::State<'_, MicrosoftAuthState>,
    start_iso: String,
    end_iso: String,
    require_complete: Option<bool>,
) -> Result<Vec<calendar::CalendarEvent>, String> {
    let (token, authority, user_id) = get_token_and_context(&state, "Calendars.Read").await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    calendar::list_calendar_events(
        &client,
        &token,
        calendar::CalendarAccount {
            authority: &authority,
            user_id: &user_id,
        },
        &start_iso,
        &end_iso,
        require_complete.unwrap_or(false),
    )
    .await
}

/// The meeting happening now, else the next one within ~12h (with invited attendees).
#[tauri::command]
pub async fn current_or_next_meeting(
    state: tauri::State<'_, MicrosoftAuthState>,
) -> Result<Option<calendar::CalendarEvent>, String> {
    let (token, authority, user_id) = get_token_and_context(&state, "Calendars.Read").await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    calendar::current_or_next_meeting(
        &client,
        &token,
        calendar::CalendarAccount {
            authority: &authority,
            user_id: &user_id,
        },
    )
    .await
}

/// OneNote notebook names reject `?*\/:<>|'#` and must be 128 characters or
/// fewer. Replace forbidden characters with spaces, collapse whitespace, and
/// truncate on a char boundary.
fn sanitize_onenote_notebook_name(raw: &str) -> String {
    const FORBIDDEN: &[char] = &['?', '*', '\\', '/', ':', '<', '>', '|', '\'', '#'];
    let replaced: String = raw
        .chars()
        .map(|c| if FORBIDDEN.contains(&c) { ' ' } else { c })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated: String = collapsed.chars().take(128).collect();
    let result = truncated.trim().to_string();
    if result.is_empty() {
        "ClawScribe".to_string()
    } else {
        result
    }
}

/// Create a new OneNote notebook and return it, so the picker can offer a "New
/// notebook" action when an account has none.
#[tauri::command]
pub async fn create_onenote_notebook(
    state: tauri::State<'_, MicrosoftAuthState>,
    display_name: String,
) -> Result<discovery::NotebookInfo, String> {
    let (token, _, _) = get_token_and_context(&state, "Notes.Create").await?;
    let name = sanitize_onenote_notebook_name(&display_name);
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    discovery::create_notebook(&client, &token, &name).await
}

/// Create a new bucket within a plan and return it.
#[tauri::command]
pub async fn create_planner_bucket(
    state: tauri::State<'_, MicrosoftAuthState>,
    plan_id: String,
    name: String,
) -> Result<discovery::BucketInfo, String> {
    let (token, _, _) = get_token_and_context(&state, "Tasks.ReadWrite").await?;
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = if name.is_empty() {
        "Action items".to_string()
    } else {
        name.chars().take(255).collect()
    };
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    discovery::create_bucket(&client, &token, &plan_id, &name).await
}

fn sanitize_todo_list_name(raw: &str) -> String {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = name.chars().take(255).collect();
    if name.is_empty() {
        "ClawScribe".to_string()
    } else {
        name
    }
}

/// Create a Microsoft To Do list and return it, so accounts without an existing
/// task list can configure export without leaving ClawScribe.
#[tauri::command]
pub async fn create_todo_list(
    state: tauri::State<'_, MicrosoftAuthState>,
    display_name: String,
) -> Result<discovery::ToDoListInfo, String> {
    let (token, _, _) = get_token_and_context(&state, "Tasks.ReadWrite").await?;
    let name = sanitize_todo_list_name(&display_name);
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    discovery::create_todo_list(&client, &token, &name).await
}

// ── OneDrive/SharePoint file export commands ───────────────────────────

#[tauri::command]
pub async fn list_onedrive_destinations(
    state: tauri::State<'_, MicrosoftAuthState>,
) -> Result<Vec<files::DriveDestination>, String> {
    let (token, _, _) = get_token_and_context(&state, "Files.ReadWrite").await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    Ok(vec![
        files::resolve_default_drive_root(&client, &token).await?,
    ])
}

#[tauri::command]
pub async fn resolve_onedrive_destination_url(
    state: tauri::State<'_, MicrosoftAuthState>,
    sharing_url: String,
) -> Result<files::DriveDestination, String> {
    let (token, _, _) = get_token_and_context(&state, "Files.ReadWrite").await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    files::resolve_sharing_url(&client, &token, &sharing_url).await
}

#[tauri::command]
pub async fn create_onedrive_destination_folder(
    state: tauri::State<'_, MicrosoftAuthState>,
    parent: files::DriveDestination,
    folder_name: String,
) -> Result<files::DriveDestination, String> {
    let (token, _, _) = get_token_and_context(&state, "Files.ReadWrite").await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    files::create_folder(&client, &token, &parent, &folder_name).await
}

#[tauri::command]
pub async fn export_meeting_to_onedrive_files(
    state: tauri::State<'_, MicrosoftAuthState>,
    request: files::OneDriveExportRequest,
) -> Result<files::OneDriveExportResponse, String> {
    let _work = crate::updates::admit_work()?;
    let (token, _, _) = get_token_and_context(&state, "Files.ReadWrite").await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    files::export_meeting_files(&client, &token, request).await
}

#[tauri::command]
pub async fn list_planner_plans(
    state: tauri::State<'_, MicrosoftAuthState>,
) -> Result<Vec<discovery::PlanInfo>, String> {
    let (token, _, _) = get_token_and_context(&state, "Tasks.ReadWrite").await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    discovery::list_plans(&client, &token).await
}

#[tauri::command]
pub async fn list_planner_buckets(
    state: tauri::State<'_, MicrosoftAuthState>,
    plan_id: String,
) -> Result<Vec<discovery::BucketInfo>, String> {
    let (token, _, _) = get_token_and_context(&state, "Tasks.ReadWrite").await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    discovery::list_buckets(&client, &token, &plan_id).await
}

#[tauri::command]
pub async fn list_todo_lists(
    state: tauri::State<'_, MicrosoftAuthState>,
) -> Result<Vec<discovery::ToDoListInfo>, String> {
    let (token, _, _) = get_token_and_context(&state, "Tasks.ReadWrite").await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    discovery::list_todo_lists(&client, &token).await
}

#[cfg(test)]
mod export_history_tests {
    use super::*;

    #[test]
    fn selected_exports_use_full_preview_or_saved_action_history() {
        let mut meeting = super::super::markdown_notes::meeting_export_for_planner(
            "meeting",
            "Test meeting",
            None,
            "## Action items\n- Send the update\n- Send the update",
        );
        assert_eq!(meeting.action_items.len(), 2);
        let hash = super::super::planner::action_hash(&meeting.action_items[0]);
        let preview = || {
            serde_json::from_value(serde_json::json!([
                {"localId": "action-1", "originalTitleHash": hash},
                {"localId": "action-2", "originalTitleHash": hash}
            ]))
            .unwrap()
        };
        let persisted = super::super::actions::decode(
            &serde_json::json!([
                {"id": "00000000-0000-4000-8000-000000000001", "title": "Edited first",
                 "done": true, "legacyAction": {"id": "action-1", "titleHash": hash}},
                {"id": "00000000-0000-4000-8000-000000000002", "title": "Edited second",
                 "done": false, "legacyAction": {"id": "action-2", "titleHash": hash}},
                {"id": "00000000-0000-4000-8000-000000000003", "title": "Send the update",
                 "done": false}
            ])
            .to_string(),
        )
        .unwrap();
        for planner in [true, false] {
            let (scope, action_hash) = if planner {
                (
                    "planner:tenant:user:plan:bucket",
                    super::super::planner::action_hash as fn(&ExportActionItem) -> String,
                )
            } else {
                ("todo:tenant:user:list", super::super::todo::action_hash)
            };
            let mut ledger = ExportLedger::new("meeting");
            let known = super::super::ledger::task_key(scope, "meeting", "action-1", &hash);
            ledger.record_success(&known, Some("existing-task".into()), None, None);
            let select = |history: &SelectedTaskHistory<'_>, local_id, original_hash, title| {
                let mut task = serde_json::json!({
                    "localId": local_id, "originalTitleHash": original_hash, "title": title,
                });
                if planner {
                    task["bucketId"] = "bucket".into();
                    planner_action_items(vec![serde_json::from_value(task).unwrap()], history)
                        .unwrap()
                        .remove("bucket")
                        .unwrap()
                } else {
                    todo_action_items(vec![serde_json::from_value(task).unwrap()], history)
                        .unwrap()
                }
            };
            let resolve = |meeting: &mut super::super::model::MeetingExport,
                           history: &SelectedTaskHistory<'_>, id, original_hash, title| {
                meeting.action_items = select(history, id, original_hash, title);
                exporter::resolve_task_keys(
                    &ledger,
                    meeting,
                    scope,
                    action_hash,
                    history.siblings.as_ref(),
                )
                .pop()
                .unwrap()
            };
            let assert_held = |result: Result<String, exporter::ItemResult>, id| {
                let item = result.unwrap_err();
                assert_eq!(item.local_id, id);
                assert_eq!(item.code.as_deref(), Some("task_identity_review_required"));
                assert!(item.resource_id.is_none());
                assert!(!item.graph_called);
            };

            let initial = SelectedTaskHistory::new(None, Some(preview())).unwrap();
            let old_ipc = SelectedTaskHistory::new(None, None).unwrap();
            for history in [&initial, &old_ipc] {
                assert_held(
                    resolve(&mut meeting, history, "action-2", Some(hash.as_str()), "Edited second"),
                    "action-2",
                );
                assert_eq!(
                    resolve(&mut meeting, history, "action-1", Some(hash.as_str()), "Edited first")
                        .unwrap(),
                    known,
                );
            }

            if planner {
                let tasks = serde_json::from_value(serde_json::json!([
                    {"localId": "action-1", "title": "Edited first", "bucketId": "other"},
                    {"localId": "action-2", "title": "Edited second", "bucketId": "bucket"}
                ]))
                .unwrap();
                let mut groups = planner_action_items(tasks, &initial).unwrap();
                assert_eq!(groups["other"][0].local_action_id, "action-1");
                meeting.action_items = groups.remove("bucket").unwrap();
                assert_held(
                    exporter::resolve_task_keys(
                        &ledger,
                        &meeting,
                        scope,
                        action_hash,
                        initial.siblings.as_ref(),
                    )
                    .pop()
                    .unwrap(),
                    "action-2",
                );
            }

            let saved = SelectedTaskHistory::new(Some(&persisted), Some(Vec::new())).unwrap();
            assert_held(
                resolve(&mut meeting, &saved, persisted[1].id.as_str(), None, "Edited second"),
                persisted[1].id.as_str(),
            );
            let new_key = resolve(
                &mut meeting, &saved, persisted[2].id.as_str(), None, "Send the update",
            )
            .unwrap();
            assert!(new_key.ends_with(persisted[2].id.as_str()));
            assert!(ledger.may_attempt(&new_key));
        }
    }

    #[test]
    fn selected_exports_reject_invalid_or_inconsistent_preview_context() {
        let candidate = |local_id: &str| TaskPreviewCandidate {
            local_id: local_id.into(),
            original_title_hash: "0123456789abcdef".into(),
        };
        for invalid in [
            vec![candidate("action-2")],
            vec![candidate("action-1"), candidate("action-1")],
            (1..=MAX_TASK_PREVIEW_CANDIDATES + 1)
                .map(|i| candidate(&format!("action-{i}")))
                .collect(),
            vec![TaskPreviewCandidate {
                original_title_hash: "bad".into(),
                ..candidate("action-1")
            }],
        ] {
            assert!(SelectedTaskHistory::new(None, Some(invalid)).is_err());
        }
        let history = SelectedTaskHistory::new(None, Some(vec![candidate("action-1")])).unwrap();
        assert!(history.legacy_action("action-2", None).is_err());
        assert!(history
            .legacy_action("action-1", Some("ffffffffffffffff".into()))
            .is_err());
    }

    fn partial_session(http: reqwest::Client) -> MicrosoftAuthState {
        let mut token = token_store::StoredToken::from_token_response(
            &auth::TokenResponse {
                access_token: "test-token".into(),
                refresh_token: Some("test-refresh".into()),
                expires_in: 3600,
                token_type: "Bearer".into(),
                scope: "User.Read Notes.Create".into(),
            },
            "test-user".into(),
            "Test User".into(),
            None,
            "organizations".into(),
        );
        token.expires_at = chrono::Utc::now() - chrono::Duration::hours(1);
        MicrosoftAuthState {
            inner: tokio::sync::RwLock::new(super::super::ms_auth_state::MicrosoftAuthInner {
                config: auth::MicrosoftAuthConfig::default(),
                generation: 1,
                scope_refreshes: Default::default(),
                http,
                connection_state: MicrosoftConnectionState::Connected,
                pending_device_code: None,
                sign_in_cancel: Some(tokio_util::sync::CancellationToken::new()),
                user_display_name: Some("Test User".into()),
                user_email: None,
                user_id: Some("test-user".into()),
                current_token: Some(token),
            }),
        }
    }

    #[tokio::test]
    async fn missing_export_scope_without_refresh_token_fails_before_network() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let http = reqwest::Client::builder()
            .proxy(
                reqwest::Proxy::all(format!("http://{}", listener.local_addr().unwrap())).unwrap(),
            )
            .timeout(std::time::Duration::from_millis(50))
            .build()
            .unwrap();
        let state = partial_session(http);
        state
            .inner
            .write()
            .await
            .current_token
            .as_mut()
            .unwrap()
            .refresh_token = None;
        let error = get_token_and_context(&state, "Tasks.ReadWrite")
            .await
            .unwrap_err();
        assert_eq!(error, "Permission not granted: Tasks.ReadWrite");
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[tokio::test]
    async fn missing_scope_refreshes_once_even_when_unexpired_and_caches_rotation() {
        for approved in [true, false] {
            let state = partial_session(reqwest::Client::new());
            state
                .inner
                .write()
                .await
                .current_token
                .as_mut()
                .unwrap()
                .expires_at = chrono::Utc::now() + chrono::Duration::hours(1);
            let refreshes = std::cell::Cell::new(0);
            let saves = std::cell::Cell::new(0);
            let result = get_token_and_context_with(
                &state,
                "Tasks.ReadWrite",
                |_, _, mut token, force| {
                    assert!(force);
                    assert!(token.is_access_token_valid());
                    refreshes.set(refreshes.get() + 1);
                    token.access_token = "test-refreshed-access".into();
                    token.refresh_token = Some("test-rotated".into());
                    if approved {
                        token.granted_scopes.push_str(" Tasks.ReadWrite");
                    }
                    std::future::ready(Ok(token))
                },
                |token| {
                    saves.set(saves.get() + 1);
                    assert_eq!(
                        super::super::permissions::has_scope(
                            &token.granted_scopes,
                            "Tasks.ReadWrite"
                        ),
                        approved
                    );
                    Ok(())
                },
            )
            .await;
            assert_eq!(refreshes.get(), 1);
            assert_eq!(saves.get(), 1);
            if approved {
                assert_eq!(
                    result.unwrap(),
                    (
                        "test-refreshed-access".into(),
                        "organizations".into(),
                        "test-user".into()
                    )
                );
            } else {
                assert_eq!(
                    result.unwrap_err(),
                    "Permission not granted: Tasks.ReadWrite"
                );
            }
            assert_eq!(
                state
                    .inner
                    .read()
                    .await
                    .current_token
                    .as_ref()
                    .unwrap()
                    .refresh_token
                    .as_deref(),
                Some("test-rotated")
            );
        }
    }

    #[tokio::test]
    async fn missing_calendar_scope_refreshes_once_until_consent_or_cooldown() {
        let state = partial_session(reqwest::Client::new());
        let refreshes = std::cell::Cell::new(0);
        for attempt in 0..7 {
            if attempt == 3 {
                // Both sign-in and Request missing permissions use this reset.
                state.inner.write().await.begin_auth_attempt();
            }
            if attempt == 6 {
                state.inner.write().await.scope_refreshes.insert(
                    "calendars.read".into(),
                    std::time::Instant::now() - std::time::Duration::from_secs(3600),
                );
            }
            let error = get_token_and_context_with(
                &state,
                "Calendars.Read",
                |_, _, token, force| {
                    assert!(force);
                    refreshes.set(refreshes.get() + 1);
                    std::future::ready(Ok(token))
                },
                |_| panic!("unchanged credentials must not persist"),
            )
            .await
            .unwrap_err();
            assert_eq!(error, "Permission not granted: Calendars.Read");
            assert_eq!(
                refreshes.get(),
                if attempt < 3 {
                    1
                } else if attempt < 6 {
                    2
                } else {
                    3
                }
            );
        }
        // A different missing scope can still be checked in the same session.
        get_token_and_context_with(
            &state,
            "Tasks.ReadWrite",
            |_, _, mut token, force| {
                assert!(force);
                token.granted_scopes.push_str(" Tasks.ReadWrite");
                std::future::ready(Ok(token))
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn concurrent_missing_scope_calls_share_one_refresh_reservation() {
        let state = partial_session(reqwest::Client::new());
        let error = get_token_and_context_with(
            &state,
            "Calendars.Read",
            |_, _, token, _| async {
                let second = get_token_and_context_with(
                    &state,
                    "Calendars.Read",
                    |_, _, _, _| async { panic!("concurrent call must not refresh") },
                    |_| panic!("concurrent call must not persist"),
                )
                .await
                .unwrap_err();
                assert_eq!(second, "Permission not granted: Calendars.Read");
                Ok(token)
            },
            |_| panic!("unchanged credentials must not persist"),
        )
        .await
        .unwrap_err();
        assert_eq!(error, "Permission not granted: Calendars.Read");
    }

    #[tokio::test]
    async fn permission_refresh_failure_keeps_existing_session() {
        let state = partial_session(reqwest::Client::new());
        let error = get_token_and_context_with(
            &state,
            "Tasks.ReadWrite",
            |_, _, _, force| {
                assert!(force);
                std::future::ready(Err("Refresh unavailable".into()))
            },
            |_| panic!("failed refresh must not persist"),
        )
        .await
        .unwrap_err();
        assert!(error.contains("Permission not granted: Tasks.ReadWrite"));
        let repeated = get_token_and_context_with(
            &state,
            "Tasks.ReadWrite",
            |_, _, _, _| async { panic!("failed network refresh must also respect the cooldown") },
            |_| panic!("failed refresh must not persist"),
        )
        .await
        .unwrap_err();
        assert_eq!(repeated, "Permission not granted: Tasks.ReadWrite");
        let inner = state.inner.read().await;
        assert_eq!(inner.connection_state, MicrosoftConnectionState::Connected);
        assert_eq!(
            inner
                .current_token
                .as_ref()
                .unwrap()
                .refresh_token
                .as_deref(),
            Some("test-refresh")
        );
    }

    #[tokio::test]
    async fn permission_refresh_does_not_cross_session_generations() {
        let state = partial_session(reqwest::Client::new());
        let error = get_token_and_context_with(
            &state,
            "Tasks.ReadWrite",
            |_, _, mut token, _| async {
                state.inner.write().await.generation += 1;
                token.granted_scopes.push_str(" Tasks.ReadWrite");
                Ok(token)
            },
            |_| panic!("stale refresh must not persist"),
        )
        .await
        .unwrap_err();
        assert!(error.contains("session changed"));
    }

    #[tokio::test]
    async fn approved_scope_uses_normal_expiry_check_without_forcing_refresh() {
        let state = partial_session(reqwest::Client::new());
        get_token_and_context_with(
            &state,
            "Notes.Create",
            |_, _, token, force| {
                assert!(!force);
                std::future::ready(Ok(token))
            },
            |_| panic!("unchanged credentials must not persist"),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn declined_explicit_consent_preserves_existing_session() {
        let state = partial_session(reqwest::Client::new());
        let mut inner = state.inner.write().await;
        assert!(inner.finish_sign_in_failure(1));
        assert_eq!(inner.connection_state, MicrosoftConnectionState::Connected);
        let token = inner.current_token.as_ref().unwrap();
        assert_eq!(token.refresh_token.as_deref(), Some("test-refresh"));
        assert_eq!(token.granted_scopes, "User.Read Notes.Create");
        assert!(inner.sign_in_cancel.is_none());
        assert!(!inner.finish_sign_in_failure(0));
    }
}
