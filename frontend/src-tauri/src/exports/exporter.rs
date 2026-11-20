//! Export orchestration over the Graph client and idempotency ledger.
//!
//! Ties together page/task builders, the ledger, and the retrying client.
//! Succeeded items are never re-sent; partial batch failures preserve the
//! successful resource IDs and only failed items are retried on a later run.

use std::collections::HashMap;

use crate::exports::client::{GraphClient, GraphOutcome, Sleeper};
use crate::exports::error::GraphErrorKind;
use crate::exports::ledger::{task_key_for_action, ExportLedger, PositionalSiblings};
use crate::exports::model::{
    ExportActionItem, ExportStatus, MeetingExport, MicrosoftConnectionState,
};
use crate::exports::onenote::{self, DEFAULT_PAGE_BUDGET_BYTES};
use crate::exports::planner::{self, PlannerDestination};
use crate::exports::todo::{self, ToDoDestination};
use crate::exports::transport::{GraphRequest, GraphTransport};

const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

fn graph_url_with_path_segments(segments: &[&str]) -> String {
    let mut url = url::Url::parse(GRAPH_BASE).expect("Graph base URL is valid");
    {
        let mut path = url
            .path_segments_mut()
            .expect("Graph base URL supports path segments");
        for segment in segments {
            path.push(segment);
        }
    }
    url.to_string()
}

fn todo_tasks_url(list_id: &str) -> String {
    graph_url_with_path_segments(&["me", "todo", "lists", list_id, "tasks"])
}

fn todo_task_url(list_id: &str, task_id: &str) -> String {
    graph_url_with_path_segments(&["me", "todo", "lists", list_id, "tasks", task_id])
}

/// Caller identity/token for an export run. Held only for the call's duration.
pub struct ExportContext<'a> {
    pub tenant_id: &'a str,
    pub user_id: &'a str,
    pub bearer_token: &'a str,
}

/// A stored OneNote destination.
pub struct OneNoteTarget {
    pub section_id: String,
}

/// Per-item (page or task) result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemResult {
    pub dedupe_key: String,
    pub local_id: String,
    pub status: ExportStatus,
    pub resource_id: Option<String>,
    pub web_url: Option<String>,
    pub code: Option<String>,
    /// Whether a create was submitted to Graph for this item.
    pub graph_called: bool,
}

/// Aggregate result of an export run for one destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    pub overall: ExportStatus,
    /// Set when a 401 indicates the Microsoft session must be reconnected.
    pub connection_state: Option<MicrosoftConnectionState>,
    pub items: Vec<ItemResult>,
}

fn now_rfc3339() -> Option<String> {
    Some(chrono::Utc::now().to_rfc3339())
}

fn parse_resource(body: &str) -> (Option<String>, Option<String>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return (None, None);
    };
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let web_url = value
        .get("webUrl")
        .and_then(|v| v.as_str())
        .or_else(|| {
            value
                .get("links")
                .and_then(|l| l.get("oneNoteWebUrl"))
                .and_then(|w| w.get("href"))
                .and_then(|h| h.as_str())
        })
        .and_then(super::ledger::microsoft_web_url);
    (id, web_url)
}

/// Reduce per-item results to an overall destination status.
fn summarize(items: &[ItemResult]) -> (ExportStatus, Option<MicrosoftConnectionState>) {
    if items.iter().any(|i| i.status == ExportStatus::FailedAuth) {
        return (
            ExportStatus::FailedAuth,
            Some(MicrosoftConnectionState::Expired),
        );
    }
    let succeeded = items
        .iter()
        .filter(|i| i.status == ExportStatus::Succeeded)
        .count();
    let failed: Vec<&ItemResult> = items
        .iter()
        .filter(|i| i.status != ExportStatus::Succeeded)
        .collect();

    if failed.is_empty() {
        return (ExportStatus::Succeeded, None);
    }
    if succeeded > 0 {
        return (ExportStatus::PartialFailure, None);
    }
    // All failed: surface the single shared status, else generic Failed.
    let first = failed[0].status;
    if failed.iter().all(|i| i.status == first) {
        (first, None)
    } else {
        (ExportStatus::Failed, None)
    }
}

/// Run one Graph attempt for an item and translate it into an [`ItemResult`],
/// updating the ledger. Returns whether the caller should stop the batch (auth
/// failure invalidates the whole token).
#[allow(clippy::too_many_arguments)]
async fn run_item<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    token: &str,
    dedupe_key: String,
    local_id: String,
    request: GraphRequest,
) -> (ItemResult, bool) {
    ledger.begin_attempt(&dedupe_key, now_rfc3339());
    if ledger.checkpoint().await.is_err() {
        ledger.record_failure(
            &dedupe_key,
            ExportStatus::Failed,
            Some("export_history_write_failed".into()),
            now_rfc3339(),
        );
        return (from_ledger(dedupe_key, local_id, ledger), true);
    }
    let (mut result, stop) = match client.execute_create(&request, token).await {
        GraphOutcome::Success(resp) => {
            let (id, url) = parse_resource(&resp.body);
            ledger.record_success(&dedupe_key, id.clone(), url.clone(), now_rfc3339());
            (
                ItemResult {
                    dedupe_key,
                    local_id,
                    status: ExportStatus::Succeeded,
                    resource_id: id,
                    web_url: url,
                    code: None,
                    graph_called: true,
                },
                false,
            )
        }
        GraphOutcome::Failed(kind, detail) => {
            let status = kind.export_status();
            let _ = detail;
            log::warn!("Graph export failed ({})", kind.code());
            ledger.record_failure(
                &dedupe_key,
                status,
                Some(kind.code().to_string()),
                now_rfc3339(),
            );
            let stop = kind == GraphErrorKind::Unauthorized;
            (
                ItemResult {
                    dedupe_key,
                    local_id,
                    status,
                    resource_id: None,
                    web_url: None,
                    code: Some(kind.code().to_string()),
                    graph_called: true,
                },
                stop,
            )
        }
        GraphOutcome::Unknown(_) => {
            // Non-idempotent create with unknown outcome: require review.
            ledger.record_failure(
                &dedupe_key,
                ExportStatus::UnknownAfterSubmit,
                Some("submission_unconfirmed".to_string()),
                now_rfc3339(),
            );
            (
                ItemResult {
                    dedupe_key,
                    local_id,
                    status: ExportStatus::UnknownAfterSubmit,
                    resource_id: None,
                    web_url: None,
                    code: Some("submission_unconfirmed".to_string()),
                    graph_called: true,
                },
                true,
            )
        }
    };
    if ledger.checkpoint().await.is_err() {
        // The durable pending entry prevents replay even if saving success failed.
        result.status = ExportStatus::UnknownAfterSubmit;
        result.code = Some("export_history_write_failed_review_required".into());
        return (result, true);
    }
    (result, stop)
}

/// Build an item result for something resolved from the ledger without a call.
fn from_ledger(dedupe_key: String, local_id: String, ledger: &ExportLedger) -> ItemResult {
    let entry = ledger.entry(&dedupe_key);
    ItemResult {
        local_id,
        status: entry.map(|e| e.status).unwrap_or(ExportStatus::Pending),
        resource_id: entry.and_then(|e| e.resource_id.clone()),
        web_url: entry.and_then(|e| e.web_url.clone()),
        code: entry.and_then(|e| e.code.clone()),
        graph_called: false,
        dedupe_key,
    }
}

/// Export a meeting's OneNote page series.
pub async fn export_onenote<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    meeting: &MeetingExport,
    target: &OneNoteTarget,
    ctx: &ExportContext<'_>,
) -> ExportReport {
    let url = format!(
        "{GRAPH_BASE}/me/onenote/sections/{}/pages",
        target.section_id
    );
    let pages = onenote::build_pages(meeting, DEFAULT_PAGE_BUDGET_BYTES);
    let meeting_hash = meeting.artifact_hash();
    let mut items = Vec::new();

    for page in pages {
        let dedupe_key = format!(
            "onenote:{}:{}:{}:{}:{}:{}",
            ctx.tenant_id,
            ctx.user_id,
            target.section_id,
            meeting_hash,
            page.kind.as_str(),
            page.index,
        );
        let local_id = format!("{}-{}", page.kind.as_str(), page.index);

        if ledger.already_succeeded(&dedupe_key).is_some() || !ledger.may_attempt(&dedupe_key) {
            items.push(from_ledger(dedupe_key, local_id, ledger));
            continue;
        }

        let request = GraphRequest {
            method: "POST".into(),
            url: url.clone(),
            content_type: "application/xhtml+xml".into(),
            body: page.xhtml,
            correlation_id: uuid::Uuid::new_v4().to_string(),
            headers: Vec::new(),
        };
        let (result, stop) = run_item(
            client,
            ledger,
            ctx.bearer_token,
            dedupe_key,
            local_id,
            request,
        )
        .await;
        items.push(result);
        if stop {
            break;
        }
    }

    let (overall, connection_state) = summarize(&items);
    ExportReport {
        overall,
        connection_state,
        items,
    }
}

/// Resolve the selection before sending anything so one row can't claim the
/// history written for another row during the same export.
pub(super) fn resolve_task_keys(
    ledger: &ExportLedger,
    meeting: &MeetingExport,
    scope: &str,
    action_hash: fn(&ExportActionItem) -> String,
    full_positional_siblings: Option<&PositionalSiblings>,
) -> Vec<Result<String, ItemResult>> {
    let mut claims = HashMap::new();
    let keys: Vec<_> = meeting
        .action_items
        .iter()
        .map(|action| {
            let hash = action_hash(action);
            let local_id = action.local_action_id.as_str();
            let current = task_key_for_action(scope, &meeting.meeting_id, action, &hash);
            let existing = ledger.existing_task_key(
                scope,
                &meeting.meeting_id,
                local_id,
                &hash,
                action.legacy_action.as_ref(),
                full_positional_siblings,
            );
            if let Ok(Some(key)) = &existing {
                claims
                    .entry(key.clone())
                    .and_modify(|claim| {
                        if *claim != Some(local_id) {
                            *claim = None;
                        }
                    })
                    .or_insert(Some(local_id));
            }
            (local_id, current, existing)
        })
        .collect();

    keys.into_iter()
        .map(|(local_id, current, existing)| match existing {
            Ok(Some(key)) if claims[&key].is_some() => Ok(key),
            Ok(None) => Ok(current),
            _ => Err(ItemResult {
                dedupe_key: current,
                local_id: local_id.into(),
                status: ExportStatus::Failed,
                resource_id: None,
                web_url: None,
                code: Some("task_identity_review_required".into()),
                graph_called: false,
            }),
        })
        .collect()
}

/// Export a meeting's reviewed action items as Planner tasks.
///
/// Pass the full set of candidate identities across all buckets when available.
pub async fn export_planner<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    meeting: &MeetingExport,
    destination: &PlannerDestination,
    ctx: &ExportContext<'_>,
    full_positional_siblings: Option<&PositionalSiblings>,
) -> Result<ExportReport, planner::PlannerBuildError> {
    let url = format!("{GRAPH_BASE}/planner/tasks");
    let mut items = Vec::new();
    let scope = planner::task_scope(ctx.tenant_id, ctx.user_id, destination);
    let keys =
        resolve_task_keys(ledger, meeting, &scope, planner::action_hash, full_positional_siblings);

    for (action, key) in meeting.action_items.iter().zip(keys) {
        let dedupe_key = match key {
            Ok(key) => key,
            Err(review) => {
                items.push(review);
                continue;
            }
        };
        let local_id = action.local_action_id.clone();

        if ledger.already_succeeded(&dedupe_key).is_some() || !ledger.may_attempt(&dedupe_key) {
            items.push(from_ledger(dedupe_key, local_id, ledger));
            continue;
        }

        // Build first so a malformed item fails fast without a Graph call.
        let body = planner::build_task_request(destination, action)?;
        let request = GraphRequest {
            method: "POST".into(),
            url: url.clone(),
            content_type: "application/json".into(),
            body: body.to_string(),
            correlation_id: uuid::Uuid::new_v4().to_string(),
            headers: Vec::new(),
        };
        let (result, stop) = run_item(
            client,
            ledger,
            ctx.bearer_token,
            dedupe_key,
            local_id,
            request,
        )
        .await;

        // Best-effort: attach a description with context to the new task. The
        // task already exists, so a details failure must not fail the export.
        if let Some(task_id) = result.resource_id.as_deref() {
            if result.status == ExportStatus::Succeeded {
                let description = planner::build_task_details_description(meeting, action);
                set_planner_task_details(client, ctx.bearer_token, task_id, &description).await;
            }
        }

        items.push(result);
        if stop {
            break;
        }
    }

    let (overall, connection_state) = summarize(&items);
    Ok(ExportReport {
        overall,
        connection_state,
        items,
    })
}

/// Export a meeting's reviewed action items as personal Microsoft To Do tasks.
///
/// Pass the full set of candidate identities when available.
pub async fn export_todo<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    meeting: &MeetingExport,
    destination: &ToDoDestination,
    ctx: &ExportContext<'_>,
    full_positional_siblings: Option<&PositionalSiblings>,
) -> Result<ExportReport, todo::ToDoBuildError> {
    let url = todo_tasks_url(&destination.list_id);
    let mut items = Vec::new();
    let scope = todo::task_scope(ctx.tenant_id, ctx.user_id, destination);
    let keys =
        resolve_task_keys(ledger, meeting, &scope, todo::action_hash, full_positional_siblings);

    for (action, key) in meeting.action_items.iter().zip(keys) {
        let dedupe_key = match key {
            Ok(key) => key,
            Err(review) => {
                items.push(review);
                continue;
            }
        };
        let local_id = action.local_action_id.clone();

        if ledger.already_succeeded(&dedupe_key).is_some() || !ledger.may_attempt(&dedupe_key) {
            items.push(from_ledger(dedupe_key, local_id, ledger));
            continue;
        }

        let body = todo::build_task_request(destination, meeting, action)?;
        let request = GraphRequest {
            method: "POST".into(),
            url: url.clone(),
            content_type: "application/json".into(),
            body: body.to_string(),
            correlation_id: uuid::Uuid::new_v4().to_string(),
            headers: Vec::new(),
        };
        let (mut result, stop) = run_item(
            client,
            ledger,
            ctx.bearer_token,
            dedupe_key,
            local_id,
            request,
        )
        .await;
        if result.status == ExportStatus::Succeeded {
            if let Some(task_id) = result.resource_id.as_deref() {
                let mut patch_codes = Vec::new();
                if let Some(code) = patch_todo_task(
                    client,
                    ctx.bearer_token,
                    &destination.list_id,
                    task_id,
                    todo::build_task_body_patch(meeting, action),
                    "body",
                )
                .await
                {
                    patch_codes.push(code);
                }
                if let Some(due_patch) = todo::build_task_due_date_patch(action) {
                    if let Some(code) = patch_todo_task(
                        client,
                        ctx.bearer_token,
                        &destination.list_id,
                        task_id,
                        due_patch,
                        "due date",
                    )
                    .await
                    {
                        patch_codes.push(code);
                    }
                }

                if !patch_codes.is_empty() {
                    result.status = ExportStatus::PartialFailure;
                    result.code = Some(patch_codes.join(","));
                }
            }
        }
        items.push(result);
        if stop {
            break;
        }
    }

    let (overall, connection_state) = summarize(&items);
    Ok(ExportReport {
        overall,
        connection_state,
        items,
    })
}

/// Best-effort: To Do accepts title-only creates reliably. Patch optional notes
/// and due-date metadata after the task exists so optional-field validation
/// quirks cannot prevent task creation.
async fn patch_todo_task<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    list_id: &str,
    task_id: &str,
    body: serde_json::Value,
    label: &str,
) -> Option<String> {
    let request = GraphRequest {
        method: "PATCH".into(),
        url: todo_task_url(list_id, task_id),
        content_type: "application/json".into(),
        body: body.to_string(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: Vec::new(),
    };
    match client.execute(&request, token).await {
        GraphOutcome::Success(_) => None,
        GraphOutcome::Failed(kind, detail) => {
            let detail = detail.unwrap_or_else(|| kind.code().to_string());
            log::warn!(
                "Microsoft To Do {label} patch failed ({}): {detail}",
                kind.code()
            );
            Some(todo_patch_failure_code(label, kind.code()))
        }
        GraphOutcome::Unknown(msg) => {
            log::warn!("Microsoft To Do {label} patch network failure: {msg}");
            Some(todo_patch_failure_code(label, "network"))
        }
    }
}

fn todo_patch_failure_code(label: &str, reason: &str) -> String {
    let label = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    format!("todo_{}_patch_{}", label, reason)
}

/// Best-effort: set a Planner task's description (its "notes"). Planner keeps
/// the description on a separate `/details` resource that requires the current
/// ETag via `If-Match`, so this is GET-then-PATCH. Any failure is logged and
/// ignored — the task itself already exists.
async fn set_planner_task_details<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    task_id: &str,
    description: &str,
) {
    let details_url = format!("{GRAPH_BASE}/planner/tasks/{task_id}/details");

    let get = GraphRequest {
        method: "GET".into(),
        url: details_url.clone(),
        content_type: "application/json".into(),
        body: String::new(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: Vec::new(),
    };
    let etag = match client.execute(&get, token).await {
        GraphOutcome::Success(resp) => serde_json::from_str::<serde_json::Value>(&resp.body)
            .ok()
            .and_then(|v| v.get("@odata.etag")?.as_str().map(String::from)),
        GraphOutcome::Failed(kind, _) => {
            log::warn!("Planner task details GET failed ({})", kind.code());
            None
        }
        GraphOutcome::Unknown(_) => {
            log::warn!("Planner task details GET failed (network)");
            None
        }
    };
    let Some(etag) = etag else {
        log::warn!("No ETag for Planner task {task_id}; skipping description");
        return;
    };

    let patch = GraphRequest {
        method: "PATCH".into(),
        url: details_url,
        content_type: "application/json".into(),
        body: serde_json::json!({ "description": description }).to_string(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: vec![("If-Match".to_string(), etag)],
    };
    match client.execute(&patch, token).await {
        GraphOutcome::Failed(kind, _) => {
            log::warn!("Planner task details PATCH failed ({})", kind.code());
        }
        GraphOutcome::Unknown(_) => {
            log::warn!("Planner task details PATCH failed (network)");
        }
        GraphOutcome::Success(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::client::{GraphClient, RetryPolicy, Sleeper};
    use crate::exports::model::{ExportActionItem, ExportDecision};
    use crate::exports::transport::{GraphResponse, MockGraphTransport};
    use async_trait::async_trait;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Records sleep durations instead of actually waiting.
    #[derive(Default)]
    struct RecordingSleeper {
        waits: Mutex<Vec<Duration>>,
    }

    #[async_trait]
    impl Sleeper for RecordingSleeper {
        async fn sleep(&self, duration: Duration) {
            self.waits.lock().unwrap().push(duration);
        }
    }

    fn meeting() -> MeetingExport {
        MeetingExport {
            meeting_id: "2026-06-15-sync".into(),
            title: "Weekly sync".into(),
            created_at: Some("2026-06-15T10:00:00Z".into()),
            executive_summary: "We discussed the roadmap.".into(),
            decisions: vec![ExportDecision {
                decision: "Adopt plan A".into(),
                owner: None,
            }],
            action_items: vec![
                ExportActionItem {
                    local_action_id: "action-1".into(),
                    task: "Send proposal".into(),
                    owner: None,
                    due_date: None,
                    details: None,
                    legacy_action: None,
                },
                ExportActionItem {
                    local_action_id: "action-2".into(),
                    task: "Book the room".into(),
                    owner: None,
                    due_date: None,
                    details: None,
                    legacy_action: None,
                },
                ExportActionItem {
                    local_action_id: "action-3".into(),
                    task: "Update the deck".into(),
                    owner: None,
                    due_date: None,
                    details: None,
                    legacy_action: None,
                },
            ],
            transcript_excerpt: None,
            summary_html: None,
        }
    }

    fn ctx() -> ExportContext<'static> {
        ExportContext {
            tenant_id: "tenant",
            user_id: "user",
            bearer_token: "SECRET-TOKEN-do-not-log",
        }
    }

    fn client(transport: MockGraphTransport) -> GraphClient<MockGraphTransport, RecordingSleeper> {
        GraphClient::new(
            transport,
            RecordingSleeper::default(),
            RetryPolicy::default(),
        )
    }

    fn onenote_url() -> String {
        format!("{GRAPH_BASE}/me/onenote/sections/section-1/pages")
    }
    fn planner_url() -> String {
        format!("{GRAPH_BASE}/planner/tasks")
    }

    fn assert_no_token_logged(transport: &MockGraphTransport, token: &str) {
        for rec in transport.recorded() {
            assert!(!rec.url.contains(token));
            assert!(!rec.body_hash.contains(token));
            assert!(!rec.correlation_id.contains(token));
        }
    }

    #[tokio::test]
    async fn history_write_failure_stops_before_any_remote_create() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        let mut ledger = ExportLedger::load_or_new(&blocked, "m").unwrap();
        std::fs::write(&blocked, "storage is unavailable").unwrap();
        let client = client(MockGraphTransport::new());
        let report = export_onenote(
            &client,
            &mut ledger,
            &meeting(),
            &OneNoteTarget {
                section_id: "section-1".into(),
            },
            &ctx(),
        )
        .await;
        assert_eq!(report.overall, ExportStatus::Failed);
        assert_eq!(
            report.items[0].code.as_deref(),
            Some("export_history_write_failed")
        );
        assert!(client.transport().recorded().is_empty());
    }

    #[tokio::test]
    async fn each_success_is_durable_without_waiting_for_the_end_of_the_batch() {
        let dir = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::load_or_new(dir.path(), "m").unwrap();
        let transport = MockGraphTransport::new();
        transport.queue_for_url(
            &onenote_url(),
            [GraphResponse::success(201, r#"{"id":"page-1"}"#)],
        );
        let client = client(transport);
        let target = OneNoteTarget {
            section_id: "section-1".into(),
        };
        let report = export_onenote(&client, &mut ledger, &meeting(), &target, &ctx()).await;
        assert_eq!(report.overall, ExportStatus::Succeeded);
        let mut restored = ExportLedger::load_or_new(dir.path(), "m").unwrap();
        let repeated = export_onenote(&client, &mut restored, &meeting(), &target, &ctx()).await;
        assert_eq!(repeated.items[0].resource_id.as_deref(), Some("page-1"));
        assert!(!repeated.items[0].graph_called);
        assert_eq!(client.transport().calls_for(&onenote_url()), 1);
    }

    #[tokio::test]
    async fn onenote_401_marks_expired_and_stops() {
        let transport = MockGraphTransport::new();
        transport.queue_for_url(&onenote_url(), [GraphResponse::failure(401, None)]);
        let client = client(transport);
        let mut ledger = ExportLedger::new("m");

        let report = export_onenote(
            &client,
            &mut ledger,
            &meeting(),
            &OneNoteTarget {
                section_id: "section-1".into(),
            },
            &ctx(),
        )
        .await;

        assert_eq!(report.overall, ExportStatus::FailedAuth);
        assert_eq!(
            report.connection_state,
            Some(MicrosoftConnectionState::Expired)
        );
        // Notes-only meeting => single page => single call, no blind retry.
        assert_eq!(client.transport().calls_for(&onenote_url()), 1);
        assert_no_token_logged(client.transport(), ctx().bearer_token);
    }

    #[tokio::test]
    async fn onenote_403_tenant_blocked_vs_access_denied() {
        let transport = MockGraphTransport::new();
        transport.queue_for_url(
            &onenote_url(),
            [GraphResponse::failure(403, Some("consent_required"))],
        );
        let client = client(transport);
        let mut ledger = ExportLedger::new("m");
        let report = export_onenote(
            &client,
            &mut ledger,
            &meeting(),
            &OneNoteTarget {
                section_id: "section-1".into(),
            },
            &ctx(),
        )
        .await;
        assert_eq!(report.overall, ExportStatus::TenantBlocked);
        // ledger holds sanitized code only.
        let entry = ledger.entries.values().next().unwrap();
        assert_eq!(entry.code.as_deref(), Some("tenant_blocked"));
    }

    #[tokio::test]
    async fn onenote_404_destination_not_found() {
        let transport = MockGraphTransport::new();
        transport.queue_for_url(&onenote_url(), [GraphResponse::failure(404, None)]);
        let client = client(transport);
        let mut ledger = ExportLedger::new("m");
        let report = export_onenote(
            &client,
            &mut ledger,
            &meeting(),
            &OneNoteTarget {
                section_id: "section-1".into(),
            },
            &ctx(),
        )
        .await;
        assert_eq!(report.overall, ExportStatus::DestinationNotFound);
        // Not retriable without review.
        let key = &report.items[0].dedupe_key;
        assert!(!ledger.may_attempt(key));
    }

    #[tokio::test]
    async fn planner_429_then_success_no_duplicate() {
        let transport = MockGraphTransport::new();
        // Single action item meeting for a clean throttle test.
        let mut m = meeting();
        m.action_items.truncate(1);
        transport.queue_for_url(
            &planner_url(),
            [
                GraphResponse::failure(429, None).with_retry_after(2),
                GraphResponse::success(201, r#"{"id":"task-1"}"#),
            ],
        );
        let client = client(transport);
        let mut ledger = ExportLedger::new("m");
        let dest = PlannerDestination {
            plan_id: "plan-1".into(),
            bucket_id: "bucket-1".into(),
        };

        let report = export_planner(&client, &mut ledger, &m, &dest, &ctx(), None)
            .await
            .unwrap();

        assert_eq!(report.overall, ExportStatus::Succeeded);
        assert_eq!(report.items[0].resource_id.as_deref(), Some("task-1"));
        // Two calls (throttled + success), one created task, attempt count == 2.
        assert_eq!(client.transport().calls_for(&planner_url()), 2);
        assert_eq!(
            ledger.entry(&report.items[0].dedupe_key).unwrap().status,
            ExportStatus::Succeeded
        );
    }

    #[tokio::test]
    async fn planner_and_todo_reconcile_legacy_success_across_selections() {
        let mut first = meeting();
        first.action_items.truncate(1);
        let mut next = meeting();
        next.action_items.truncate(2);
        let action = &first.action_items[0];
        let account = ctx();
        let planner = PlannerDestination {
            plan_id: "plan-1".into(),
            bucket_id: "bucket-1".into(),
        };
        let todo = ToDoDestination {
            list_id: "list-1".into(),
        };
        let mut history = ExportLedger::new(&first.meeting_id);
        for (scope, hash) in [
            (
                "planner:tenant:user:plan-1:bucket-1",
                planner::action_hash(action),
            ),
            ("todo:tenant:user:list-1", todo::action_hash(action)),
        ] {
            history.record_success(
                &format!(
                    "{scope}:{}:{}:{hash}",
                    first.artifact_hash(),
                    action.local_action_id
                ),
                Some("old-task".into()),
                None,
                None,
            );
        }
        let transport = MockGraphTransport::new();
        transport.queue_for_url(
            &planner_url(),
            [GraphResponse::success(201, r#"{"id":"new-task"}"#)],
        );
        transport.queue_for_url(
            &todo_tasks_url(&todo.list_id),
            [GraphResponse::success(201, r#"{"id":"new-task"}"#)],
        );
        let client = client(transport);
        for report in [
            export_planner(&client, &mut history, &next, &planner, &account, None)
                .await
                .unwrap(),
            export_todo(&client, &mut history, &next, &todo, &account, None)
                .await
                .unwrap(),
        ] {
            assert_eq!(report.items.len(), 2);
            assert_eq!(report.items[0].resource_id.as_deref(), Some("old-task"));
            assert!(!report.items[0].graph_called);
            assert!(report.items[1].graph_called);
            assert_eq!(report.items[1].resource_id.as_deref(), Some("new-task"));
        }
    }

    async fn export_tasks_for_test(
        client: &GraphClient<MockGraphTransport, RecordingSleeper>,
        ledger: &mut ExportLedger,
        meeting: &MeetingExport,
        planner: bool,
    ) -> ExportReport {
        if planner {
            let destination = PlannerDestination {
                plan_id: "plan-1".into(),
                bucket_id: "bucket-1".into(),
            };
            export_planner(client, ledger, meeting, &destination, &ctx(), None)
                .await
                .unwrap()
        } else {
            let destination = ToDoDestination {
                list_id: "list-1".into(),
            };
            export_todo(client, ledger, meeting, &destination, &ctx(), None)
                .await
                .unwrap()
        }
    }

    fn assert_task_identity_review(item: &ItemResult) {
        assert_eq!(item.status, ExportStatus::Failed);
        assert_eq!(item.code.as_deref(), Some("task_identity_review_required"));
        assert!(item.resource_id.is_none());
        assert!(!item.graph_called);
    }

    #[tokio::test]
    async fn same_title_suggestions_create_separately_and_block_ambiguous_replays() {
        for (planner, url) in [(true, planner_url()), (false, todo_tasks_url("list-1"))] {
            let transport = MockGraphTransport::new();
            transport.queue_for_url(
                &url,
                [
                    GraphResponse::success(201, r#"{"id":"task-first"}"#),
                    GraphResponse::success(201, r#"{"id":"task-second"}"#),
                ],
            );
            let client = client(transport);
            let mut meeting = meeting();
            meeting.action_items.truncate(2);
            meeting.action_items[1].task = meeting.action_items[0].task.clone();
            let mut ledger = ExportLedger::new(&meeting.meeting_id);
            let first = export_tasks_for_test(&client, &mut ledger, &meeting, planner).await;
            assert_eq!(first.items[0].resource_id.as_deref(), Some("task-first"));
            assert_eq!(first.items[1].resource_id.as_deref(), Some("task-second"));
            assert!(first.items.iter().all(|item| item.graph_called));

            let history = ledger.entries.clone();
            let repeated = export_tasks_for_test(&client, &mut ledger, &meeting, planner).await;
            repeated.items.iter().for_each(assert_task_identity_review);
            meeting.action_items.reverse();
            meeting.action_items[0].local_action_id = "action-5".into();
            let reordered = export_tasks_for_test(&client, &mut ledger, &meeting, planner).await;
            assert_eq!(reordered.items[0].local_id, "action-5");
            reordered.items.iter().for_each(assert_task_identity_review);

            meeting.action_items.truncate(1);
            let action = &mut meeting.action_items[0];
            action.legacy_action = Some(crate::exports::actions::LegacyAction {
                id: action.local_action_id.clone(),
                title_hash: planner::action_hash(action),
            });
            action.task = "Edited task".into();
            let edited = export_tasks_for_test(&client, &mut ledger, &meeting, planner).await;
            assert_task_identity_review(&edited.items[0]);
            meeting.action_items[0].local_action_id = "00000000-0000-4000-8000-000000000001".into();
            let linked = export_tasks_for_test(&client, &mut ledger, &meeting, planner).await;
            assert_task_identity_review(&linked.items[0]);
            assert_eq!(client.transport().calls_for(&url), 2);
            assert_eq!(ledger.entries, history);
        }
    }

    #[tokio::test]
    async fn shared_legacy_match_requires_review_but_independent_saved_actions_can_export() {
        for (planner, url) in [(true, planner_url()), (false, todo_tasks_url("list-1"))] {
            let transport = MockGraphTransport::new();
            transport.queue_for_url(
                &url,
                [
                    GraphResponse::success(201, r#"{"id":"old-task"}"#),
                    GraphResponse::success(201, r#"{"id":"reviewed-first"}"#),
                    GraphResponse::success(201, r#"{"id":"reviewed-second"}"#),
                ],
            );
            let client = client(transport);
            let mut meeting = meeting();
            let mut ledger = ExportLedger::new(&meeting.meeting_id);
            let mut first = meeting.clone();
            first.action_items.truncate(1);
            let first = export_tasks_for_test(&client, &mut ledger, &first, planner).await;
            assert_eq!(first.items[0].resource_id.as_deref(), Some("old-task"));

            meeting.action_items.truncate(2);
            meeting.action_items[1].task = meeting.action_items[0].task.clone();
            let repeated = export_tasks_for_test(&client, &mut ledger, &meeting, planner).await;
            assert_eq!(repeated.items[0].resource_id.as_deref(), Some("old-task"));
            assert!(!repeated.items[0].graph_called);
            assert_task_identity_review(&repeated.items[1]);
            assert_eq!(client.transport().calls_for(&url), 1);

            for (action, id) in meeting.action_items.iter_mut().zip([
                "00000000-0000-4000-8000-000000000001",
                "00000000-0000-4000-8000-000000000002",
            ]) {
                action.local_action_id = id.into();
            }
            let mut first_saved = meeting.clone();
            first_saved.action_items.truncate(1);
            let first_saved =
                export_tasks_for_test(&client, &mut ledger, &first_saved, planner).await;
            assert!(first_saved.items[0].graph_called);
            let saved = export_tasks_for_test(&client, &mut ledger, &meeting, planner).await;
            assert_eq!(
                saved.items[0].resource_id.as_deref(),
                Some("reviewed-first")
            );
            assert_eq!(
                saved.items[1].resource_id.as_deref(),
                Some("reviewed-second")
            );
            assert!(!saved.items[0].graph_called);
            assert!(saved.items[1].graph_called);

            meeting.action_items.reverse();
            let action = &mut meeting.action_items[0];
            action.legacy_action = Some(crate::exports::actions::LegacyAction {
                id: "action-2".into(),
                title_hash: planner::action_hash(action),
            });
            action.task = "Updated reviewed task".into();
            let saved_retry = export_tasks_for_test(&client, &mut ledger, &meeting, planner).await;
            assert_eq!(
                saved_retry.items[0].resource_id.as_deref(),
                Some("reviewed-second")
            );
            assert_eq!(
                saved_retry.items[1].resource_id.as_deref(),
                Some("reviewed-first")
            );
            assert!(saved_retry.items.iter().all(|item| !item.graph_called));
            assert_eq!(client.transport().calls_for(&url), 3);
        }
    }

    #[tokio::test]
    async fn duplicate_retry_skips_graph() {
        let transport = MockGraphTransport::new();
        let mut m = meeting();
        m.action_items.truncate(1);
        let dest = PlannerDestination {
            plan_id: "plan-1".into(),
            bucket_id: "bucket-1".into(),
        };
        let mut ledger = ExportLedger::new("m");
        // Pre-seed a succeeded entry for the only action's dedupe key.
        let key = planner::dedupe_key(
            ctx().tenant_id,
            ctx().user_id,
            &dest,
            &m,
            &m.action_items[0],
        );
        ledger.record_success(&key, Some("task-1".into()), Some("http://t".into()), None);

        let client = client(transport);
        let report = export_planner(&client, &mut ledger, &m, &dest, &ctx(), None)
            .await
            .unwrap();

        assert_eq!(report.overall, ExportStatus::Succeeded);
        assert!(
            !report.items[0].graph_called,
            "must not call Graph for succeeded item"
        );
        assert_eq!(report.items[0].resource_id.as_deref(), Some("task-1"));
        assert_eq!(client.transport().calls_for(&planner_url()), 0);
    }

    #[tokio::test]
    async fn planner_partial_failure_preserves_successes_and_retries_only_failed() {
        let transport = MockGraphTransport::new();
        transport.queue_for_url(
            &planner_url(),
            [
                GraphResponse::success(201, r#"{"id":"task-1"}"#),
                GraphResponse::failure(403, Some("AccessDenied")),
                GraphResponse::success(201, r#"{"id":"task-3"}"#),
            ],
        );
        let client = client(transport);
        let mut ledger = ExportLedger::new("m");
        let m = meeting();
        let dest = PlannerDestination {
            plan_id: "plan-1".into(),
            bucket_id: "bucket-1".into(),
        };

        let report = export_planner(&client, &mut ledger, &m, &dest, &ctx(), None)
            .await
            .unwrap();

        assert_eq!(report.overall, ExportStatus::PartialFailure);
        assert_eq!(report.items[0].status, ExportStatus::Succeeded);
        assert_eq!(report.items[0].resource_id.as_deref(), Some("task-1"));
        assert_eq!(report.items[1].status, ExportStatus::AccessDenied);
        assert_eq!(report.items[2].status, ExportStatus::Succeeded);
        assert_eq!(report.items[2].resource_id.as_deref(), Some("task-3"));
        assert_eq!(client.transport().calls_for(&planner_url()), 3);

        // 403 (access denied) is not auto-retriable, so a second run does not
        // re-attempt any item, and successes are not duplicated.
        let report2 = export_planner(&client, &mut ledger, &m, &dest, &ctx(), None)
            .await
            .unwrap();
        assert!(report2.items.iter().all(|i| !i.graph_called));
        assert_eq!(client.transport().calls_for(&planner_url()), 3);

        assert_no_token_logged(client.transport(), ctx().bearer_token);
    }

    #[tokio::test]
    async fn planner_partial_failure_retries_retriable_item_on_second_run() {
        let transport = MockGraphTransport::new();
        // item1 ok, item2 throttled-exhausted (failed), item3 ok; then on retry
        // item2 succeeds.
        transport.queue_for_url(
            &planner_url(),
            [
                GraphResponse::success(201, r#"{"id":"task-1"}"#),
                GraphResponse::failure(429, None),
                GraphResponse::failure(429, None),
                GraphResponse::failure(429, None),
                GraphResponse::success(201, r#"{"id":"task-3"}"#),
            ],
        );
        let client = client(transport);
        let mut ledger = ExportLedger::new("m");
        let m = meeting();
        let dest = PlannerDestination {
            plan_id: "plan-1".into(),
            bucket_id: "bucket-1".into(),
        };

        let report = export_planner(&client, &mut ledger, &m, &dest, &ctx(), None)
            .await
            .unwrap();
        assert_eq!(report.overall, ExportStatus::PartialFailure);
        assert_eq!(report.items[1].status, ExportStatus::Failed);

        // Second run: only item-2 is retried (Failed is retriable); queue a
        // success for it.
        client.transport().queue_for_url(
            &planner_url(),
            [GraphResponse::success(201, r#"{"id":"task-2"}"#)],
        );
        let report2 = export_planner(&client, &mut ledger, &m, &dest, &ctx(), None)
            .await
            .unwrap();
        assert_eq!(report2.overall, ExportStatus::Succeeded);
        assert!(!report2.items[0].graph_called);
        assert!(report2.items[1].graph_called);
        assert!(!report2.items[2].graph_called);
        assert_eq!(report2.items[1].resource_id.as_deref(), Some("task-2"));
    }

    #[tokio::test]
    async fn planner_rejects_missing_destination() {
        let client = client(MockGraphTransport::new());
        let mut ledger = ExportLedger::new("m");
        let mut m = meeting();
        m.action_items.truncate(1);
        let bad = PlannerDestination {
            plan_id: "".into(),
            bucket_id: "bucket-1".into(),
        };
        let err = export_planner(&client, &mut ledger, &m, &bad, &ctx(), None).await;
        assert_eq!(err, Err(planner::PlannerBuildError::MissingPlanId));
    }

    #[tokio::test]
    async fn todo_exports_action_item() {
        let transport = MockGraphTransport::new();
        transport.queue_default([
            GraphResponse::success(201, r#"{"id":"todo-1"}"#),
            GraphResponse::success(200, r#"{"id":"todo-1"}"#),
            GraphResponse::success(200, r#"{"id":"todo-1"}"#),
        ]);
        let client = client(transport);
        let mut ledger = ExportLedger::new("m1");
        let mut m = meeting();
        m.action_items.truncate(1);
        m.action_items[0].due_date = Some("2026-07-01".into());
        let dest = ToDoDestination {
            list_id: "list-1".into(),
        };
        let report = export_todo(&client, &mut ledger, &m, &dest, &ctx(), None)
            .await
            .unwrap();
        assert_eq!(report.overall, ExportStatus::Succeeded);
        assert_eq!(report.items[0].resource_id.as_deref(), Some("todo-1"));
        assert_eq!(
            client
                .transport()
                .calls_for(&format!("{GRAPH_BASE}/me/todo/lists/list-1/tasks")),
            1
        );
        assert_eq!(
            client
                .transport()
                .calls_for(&format!("{GRAPH_BASE}/me/todo/lists/list-1/tasks/todo-1")),
            2
        );
    }

    #[tokio::test]
    async fn todo_patch_failure_surfaces_partial_export_without_recreating_task() {
        let transport = MockGraphTransport::new();
        transport.queue_default([
            GraphResponse::success(201, r#"{"id":"todo-1"}"#),
            GraphResponse::failure(403, Some("AccessDenied")),
            GraphResponse::success(200, r#"{"id":"todo-1"}"#),
        ]);
        let client = client(transport);
        let mut ledger = ExportLedger::new("m1");
        let mut m = meeting();
        m.action_items.truncate(1);
        m.action_items[0].due_date = Some("2026-07-01".into());
        let dest = ToDoDestination {
            list_id: "list-1".into(),
        };

        let report = export_todo(&client, &mut ledger, &m, &dest, &ctx(), None)
            .await
            .unwrap();

        assert_eq!(report.overall, ExportStatus::PartialFailure);
        assert_eq!(report.items[0].status, ExportStatus::PartialFailure);
        assert_eq!(report.items[0].resource_id.as_deref(), Some("todo-1"));
        assert_eq!(
            report.items[0].code.as_deref(),
            Some("todo_body_patch_access_denied")
        );

        let ledger_entry = ledger.entry(&report.items[0].dedupe_key).unwrap();
        assert_eq!(ledger_entry.status, ExportStatus::Succeeded);
        assert_eq!(ledger_entry.resource_id.as_deref(), Some("todo-1"));

        let report2 = export_todo(&client, &mut ledger, &m, &dest, &ctx(), None)
            .await
            .unwrap();
        assert_eq!(report2.overall, ExportStatus::Succeeded);
        assert!(!report2.items[0].graph_called);
        assert_eq!(
            client
                .transport()
                .calls_for(&format!("{GRAPH_BASE}/me/todo/lists/list-1/tasks")),
            1
        );
    }

    #[test]
    fn todo_urls_encode_graph_ids_as_path_segments() {
        assert_eq!(
            todo_tasks_url("list/with spaces"),
            format!("{GRAPH_BASE}/me/todo/lists/list%2Fwith%20spaces/tasks")
        );
        assert_eq!(
            todo_task_url("list/with/slash", "task/with/slash"),
            format!("{GRAPH_BASE}/me/todo/lists/list%2Fwith%2Fslash/tasks/task%2Fwith%2Fslash")
        );
    }

    // ── Regression: sibling-aware cross-positional dedup ──────────────────

    /// action-1 was exported as a positional suggestion (old-style key). The
    /// user then saves both action-1 and action-2 as reviewed UUID actions
    /// (same title). Exporting only UUID-B (legacy→action-2) must NOT inherit
    /// action-1's history — they are distinct actions.
    #[tokio::test]
    async fn selected_action_with_same_title_as_completed_sibling_is_held_not_matched() {
        for (is_planner, url) in [(true, planner_url()), (false, todo_tasks_url("list-1"))] {
            let transport = MockGraphTransport::new();
            // No responses queued: must not make any Graph call.
            let client = client(transport);
            let mut ledger = ExportLedger::new("meeting");

            let (scope, hash) = if is_planner {
                (
                    "planner:tenant:user:plan-1:bucket-1",
                    planner::action_hash(&meeting().action_items[0]),
                )
            } else {
                (
                    "todo:tenant:user:list-1",
                    todo::action_hash(&meeting().action_items[0]),
                )
            };
            // Seed action-1's positional key as already succeeded (old-style export).
            let key_a1 = crate::exports::ledger::task_key(scope, "meeting", "action-1", &hash);
            ledger.record_success(&key_a1, Some("task-old".into()), None, None);

            // Export only UUID-B (legacy→action-2, same title hash as action-1).
            let uuid_b = uuid::Uuid::new_v4().to_string();
            let m = crate::exports::model::MeetingExport {
                meeting_id: "meeting".into(),
                title: "T".into(),
                created_at: None,
                executive_summary: String::new(),
                decisions: Vec::new(),
                action_items: vec![crate::exports::model::ExportActionItem {
                    local_action_id: uuid_b.clone(),
                    task: "Same title".into(),
                    owner: None,
                    due_date: None,
                    details: None,
                    legacy_action: Some(crate::exports::actions::LegacyAction {
                        id: "action-2".into(),
                        title_hash: hash.clone(),
                    }),
                }],
                transcript_excerpt: None,
                summary_html: None,
            };

            // Full set: both action-1 and action-2 are known siblings.
            let mut siblings: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
            siblings
                .entry(hash.clone())
                .or_default()
                .insert("action-1".into());
            siblings
                .entry(hash.clone())
                .or_default()
                .insert("action-2".into());

            let report = if is_planner {
                export_planner(
                    &client,
                    &mut ledger,
                    &m,
                    &PlannerDestination {
                        plan_id: "plan-1".into(),
                        bucket_id: "bucket-1".into(),
                    },
                    &ctx(),
                    Some(&siblings),
                )
                .await
                .unwrap()
            } else {
                export_todo(
                    &client,
                    &mut ledger,
                    &m,
                    &ToDoDestination {
                        list_id: "list-1".into(),
                    },
                    &ctx(),
                    Some(&siblings),
                )
                .await
                .unwrap()
            };

            assert_task_identity_review(&report.items[0]);
            assert_eq!(
                client.transport().calls_for(&url),
                0,
                "must not call Graph when held"
            );
        }
    }

    /// Same scenario but the full set has only action-2 (action-1 was removed
    /// from reviewed actions). The shift is legitimate and should match.
    #[tokio::test]
    async fn uuid_action_inherits_shifted_positional_when_no_known_sibling() {
        for is_planner in [true, false] {
            let transport = MockGraphTransport::new();
            let client = client(transport);
            let mut ledger = ExportLedger::new("meeting");

            let (scope, hash) = if is_planner {
                (
                    "planner:tenant:user:plan-1:bucket-1",
                    planner::action_hash(&meeting().action_items[0]),
                )
            } else {
                (
                    "todo:tenant:user:list-1",
                    todo::action_hash(&meeting().action_items[0]),
                )
            };
            let key_a1 = crate::exports::ledger::task_key(scope, "meeting", "action-1", &hash);
            ledger.record_success(&key_a1, Some("task-shifted".into()), None, None);

            let uuid_b = uuid::Uuid::new_v4().to_string();
            let m = crate::exports::model::MeetingExport {
                meeting_id: "meeting".into(),
                title: "T".into(),
                created_at: None,
                executive_summary: String::new(),
                decisions: Vec::new(),
                action_items: vec![crate::exports::model::ExportActionItem {
                    local_action_id: uuid_b.clone(),
                    task: "Same title".into(),
                    owner: None,
                    due_date: None,
                    details: None,
                    legacy_action: Some(crate::exports::actions::LegacyAction {
                        id: "action-2".into(),
                        title_hash: hash.clone(),
                    }),
                }],
                transcript_excerpt: None,
                summary_html: None,
            };

            // Full set has only action-2 — no cross-positional sibling to block.
            let mut siblings: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
            siblings
                .entry(hash.clone())
                .or_default()
                .insert("action-2".into());

            let report = if is_planner {
                export_planner(
                    &client,
                    &mut ledger,
                    &m,
                    &PlannerDestination {
                        plan_id: "plan-1".into(),
                        bucket_id: "bucket-1".into(),
                    },
                    &ctx(),
                    Some(&siblings),
                )
                .await
                .unwrap()
            } else {
                export_todo(
                    &client,
                    &mut ledger,
                    &m,
                    &ToDoDestination {
                        list_id: "list-1".into(),
                    },
                    &ctx(),
                    Some(&siblings),
                )
                .await
                .unwrap()
            };

            // action-1's key was found as a legitimate shift — already succeeded.
            assert_eq!(
                report.items[0].resource_id.as_deref(),
                Some("task-shifted"),
                "must inherit action-1 history when no blocking sibling"
            );
            assert!(!report.items[0].graph_called);
        }
    }
}
