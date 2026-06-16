//! Tauri commands for Microsoft Graph auth and exports.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::exports::auth;
use crate::exports::client::{GraphClient, RetryPolicy, TokioSleeper};
use crate::exports::discovery;
use crate::exports::exporter::{self, ExportContext, OneNoteTarget};
use crate::exports::ledger::ExportLedger;
use crate::exports::model::MicrosoftConnectionState;
use crate::exports::ms_auth_state::MicrosoftAuthState;
use crate::exports::planner::PlannerDestination;
use crate::exports::reqwest_transport::ReqwestGraphTransport;
use crate::exports::token_store;
use crate::summary::codex_provider::open_url_in_default_browser;

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
            connection_state: r.connection_state.map(|s| format!("{s:?}").to_ascii_lowercase()),
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
) -> Result<(), String> {
    let (config, http);
    {
        let mut inner = state.inner.write().await;
        inner.connection_state = MicrosoftConnectionState::Connecting;
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
                // Persist for future sessions, but don't gate sign-in on it —
                // the token is held in memory below so exports work this session
                // even when the platform credential store is unavailable.
                if let Err(e) = token_store::save_token(&stored) {
                    log::warn!("Failed to persist Microsoft token to keychain: {e}");
                }

                {
                    let mut inner = state.inner.write().await;
                    inner.connection_state = MicrosoftConnectionState::Connected;
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
                {
                    let mut inner = state.inner.write().await;
                    inner.connection_state = MicrosoftConnectionState::NotConnected;
                    inner.pending_device_code = None;
                }
                let _ = app_handle.emit(
                    "microsoft-auth-complete",
                    serde_json::json!({ "state": "not_connected", "error": e.to_string() }),
                );
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn microsoft_sign_out(
    state: tauri::State<'_, MicrosoftAuthState>,
) -> Result<(), String> {
    let _ = token_store::delete_token();
    let mut inner = state.inner.write().await;
    inner.connection_state = MicrosoftConnectionState::NotConnected;
    inner.pending_device_code = None;
    inner.user_display_name = None;
    inner.user_email = None;
    inner.user_id = None;
    inner.current_token = None;
    Ok(())
}

#[tauri::command]
pub async fn microsoft_connection_status(
    state: tauri::State<'_, MicrosoftAuthState>,
) -> Result<MicrosoftConnectionInfo, String> {
    let inner = state.inner.read().await;
    Ok(MicrosoftConnectionInfo {
        state: inner.connection_state,
        user_display_name: inner.user_display_name.clone(),
        user_email: inner.user_email.clone(),
        granted_scopes: inner
            .current_token
            .as_ref()
            .map(|t| t.granted_scopes.clone()),
    })
}

// ── Export commands ──────────────────────────────────────────────────────

async fn get_token_and_context(
    state: &MicrosoftAuthState,
) -> Result<(String, String, String), String> {
    let (config, http, current);
    {
        let inner = state.inner.read().await;
        config = inner.config.clone();
        http = inner.http.clone();
        current = inner.current_token.clone();
    }

    let stored = token_store::ensure_valid_token(&http, &config, current)
        .await
        .map_err(|e| e.to_string())?;

    // Cache any refreshed token for the rest of the session.
    {
        let mut inner = state.inner.write().await;
        inner.current_token = Some(stored.clone());
    }

    Ok((stored.access_token, stored.tenant_id, stored.user_id))
}

#[tauri::command]
pub async fn export_to_onenote(
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
    meeting_title: String,
    summary_json: String,
    section_id: String,
) -> Result<ExportReportResponse, String> {
    let (token, tenant_id, user_id) = get_token_and_context(&state).await?;

    let notes: crate::summary::codex_provider::MeetingNotesOutput =
        serde_json::from_str(&summary_json)
            .map_err(|e| format!("Failed to parse summary: {e}"))?;

    let meeting_export =
        crate::exports::meeting_export_from_notes(&meeting_id, &meeting_title, None, &notes, None);

    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    let mut ledger = ExportLedger::new(&meeting_id);

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

    if report.connection_state == Some(MicrosoftConnectionState::Expired) {
        let mut inner = state.inner.write().await;
        inner.connection_state = MicrosoftConnectionState::Expired;
    }

    Ok(report.into())
}

#[tauri::command]
pub async fn export_to_planner(
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
    meeting_title: String,
    summary_json: String,
    plan_id: String,
    bucket_id: String,
) -> Result<ExportReportResponse, String> {
    let (token, tenant_id, user_id) = get_token_and_context(&state).await?;

    let notes: crate::summary::codex_provider::MeetingNotesOutput =
        serde_json::from_str(&summary_json)
            .map_err(|e| format!("Failed to parse summary: {e}"))?;

    let meeting_export =
        crate::exports::meeting_export_from_notes(&meeting_id, &meeting_title, None, &notes, None);

    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    let mut ledger = ExportLedger::new(&meeting_id);
    let destination = PlannerDestination { plan_id, bucket_id };

    let ctx = ExportContext {
        tenant_id: &tenant_id,
        user_id: &user_id,
        bearer_token: &token,
    };

    let report = exporter::export_planner(&client, &mut ledger, &meeting_export, &destination, &ctx)
        .await
        .map_err(|e| e.to_string())?;

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
pub async fn export_meeting_markdown_to_onenote(
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
    meeting_title: String,
    markdown: String,
    section_id: String,
) -> Result<ExportReportResponse, String> {
    let (token, tenant_id, user_id) = get_token_and_context(&state).await?;

    let meeting_export = crate::exports::markdown_notes::meeting_export_for_onenote(
        &meeting_id,
        &meeting_title,
        None,
        &markdown,
    );

    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    let mut ledger = ExportLedger::new(&meeting_id);

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

    if report.connection_state == Some(MicrosoftConnectionState::Expired) {
        let mut inner = state.inner.write().await;
        inner.connection_state = MicrosoftConnectionState::Expired;
    }

    Ok(report.into())
}

/// Export a meeting's summary to OneNote by creating a fresh section in the
/// chosen notebook (named by the caller, e.g. `2026-06-16: Standup`) and writing
/// the notes into it. Creating a section is not subject to the 5,000-item
/// enumeration limit (error 10008), so this works for notebooks whose OneDrive
/// library is too large to list sections from.
#[tauri::command]
pub async fn export_meeting_to_onenote_section(
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
    meeting_title: String,
    markdown: String,
    notebook_id: String,
    section_name: String,
) -> Result<ExportReportResponse, String> {
    let (token, tenant_id, user_id) = get_token_and_context(&state).await?;

    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());

    let section = discovery::create_section(&client, &token, &notebook_id, &section_name).await?;

    let meeting_export = crate::exports::markdown_notes::meeting_export_for_onenote(
        &meeting_id,
        &meeting_title,
        None,
        &markdown,
    );

    let mut ledger = ExportLedger::new(&meeting_id);
    let ctx = ExportContext {
        tenant_id: &tenant_id,
        user_id: &user_id,
        bearer_token: &token,
    };

    let report = exporter::export_onenote(
        &client,
        &mut ledger,
        &meeting_export,
        &OneNoteTarget { section_id: section.id },
        &ctx,
    )
    .await;

    if report.connection_state == Some(MicrosoftConnectionState::Expired) {
        let mut inner = state.inner.write().await;
        inner.connection_state = MicrosoftConnectionState::Expired;
    }

    Ok(report.into())
}

#[tauri::command]
pub async fn export_meeting_markdown_to_planner(
    state: tauri::State<'_, MicrosoftAuthState>,
    meeting_id: String,
    meeting_title: String,
    markdown: String,
    plan_id: String,
    bucket_id: String,
) -> Result<ExportReportResponse, String> {
    let (token, tenant_id, user_id) = get_token_and_context(&state).await?;

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
    let mut ledger = ExportLedger::new(&meeting_id);
    let destination = PlannerDestination { plan_id, bucket_id };

    let ctx = ExportContext {
        tenant_id: &tenant_id,
        user_id: &user_id,
        bearer_token: &token,
    };

    let report = exporter::export_planner(&client, &mut ledger, &meeting_export, &destination, &ctx)
        .await
        .map_err(|e| e.to_string())?;

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
    let (token, _, _) = get_token_and_context(&state).await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    discovery::list_notebooks(&client, &token).await
}

#[tauri::command]
pub async fn list_onenote_sections(
    state: tauri::State<'_, MicrosoftAuthState>,
    notebook_id: String,
) -> Result<Vec<discovery::SectionInfo>, String> {
    let (token, _, _) = get_token_and_context(&state).await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    discovery::list_sections(&client, &token, &notebook_id).await
}

#[tauri::command]
pub async fn list_planner_plans(
    state: tauri::State<'_, MicrosoftAuthState>,
) -> Result<Vec<discovery::PlanInfo>, String> {
    let (token, _, _) = get_token_and_context(&state).await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    discovery::list_plans(&client, &token).await
}

#[tauri::command]
pub async fn list_planner_buckets(
    state: tauri::State<'_, MicrosoftAuthState>,
    plan_id: String,
) -> Result<Vec<discovery::BucketInfo>, String> {
    let (token, _, _) = get_token_and_context(&state).await?;
    let transport = ReqwestGraphTransport::new();
    let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
    discovery::list_buckets(&client, &token, &plan_id).await
}
