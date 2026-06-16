//! Export orchestration over the Graph client and idempotency ledger.
//!
//! Ties together page/task builders, the ledger, and the retrying client.
//! Succeeded items are never re-sent; partial batch failures preserve the
//! successful resource IDs and only failed items are retried on a later run.

use crate::exports::client::{GraphClient, GraphOutcome, Sleeper};
use crate::exports::error::GraphErrorKind;
use crate::exports::ledger::ExportLedger;
use crate::exports::model::{ExportStatus, MeetingExport, MicrosoftConnectionState};
use crate::exports::onenote::{self, DEFAULT_PAGE_BUDGET_BYTES};
use crate::exports::planner::{self, PlannerDestination};
use crate::exports::transport::{GraphRequest, GraphTransport};

const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

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
    /// False when the item was satisfied from the ledger without calling Graph.
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
        .map(|s| s.to_string());
    (id, web_url)
}

/// Reduce per-item results to an overall destination status.
fn summarize(items: &[ItemResult]) -> (ExportStatus, Option<MicrosoftConnectionState>) {
    if items.iter().any(|i| i.status == ExportStatus::FailedAuth) {
        return (ExportStatus::FailedAuth, Some(MicrosoftConnectionState::Expired));
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
    match client.execute(&request, token).await {
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
            if let Some(d) = &detail {
                log::warn!("Graph export failed ({}): {d}", kind.code());
            }
            ledger.record_failure(&dedupe_key, status, Some(kind.code().to_string()), now_rfc3339());
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
                Some("network".to_string()),
                now_rfc3339(),
            );
            (
                ItemResult {
                    dedupe_key,
                    local_id,
                    status: ExportStatus::UnknownAfterSubmit,
                    resource_id: None,
                    web_url: None,
                    code: Some("network".to_string()),
                    graph_called: true,
                },
                true,
            )
        }
    }
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
    let url = format!("{GRAPH_BASE}/me/onenote/sections/{}/pages", target.section_id);
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
        };
        let (result, stop) =
            run_item(client, ledger, ctx.bearer_token, dedupe_key, local_id, request).await;
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

/// Export a meeting's reviewed action items as Planner tasks.
pub async fn export_planner<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    meeting: &MeetingExport,
    destination: &PlannerDestination,
    ctx: &ExportContext<'_>,
) -> Result<ExportReport, planner::PlannerBuildError> {
    let url = format!("{GRAPH_BASE}/planner/tasks");
    let mut items = Vec::new();

    for action in &meeting.action_items {
        let dedupe_key = planner::dedupe_key(ctx.tenant_id, ctx.user_id, destination, meeting, action);
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
        };
        let (result, stop) =
            run_item(client, ledger, ctx.bearer_token, dedupe_key, local_id, request).await;
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
                },
                ExportActionItem {
                    local_action_id: "action-2".into(),
                    task: "Book the room".into(),
                    owner: None,
                    due_date: None,
                },
                ExportActionItem {
                    local_action_id: "action-3".into(),
                    task: "Update the deck".into(),
                    owner: None,
                    due_date: None,
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

    fn client(
        transport: MockGraphTransport,
    ) -> GraphClient<MockGraphTransport, RecordingSleeper> {
        GraphClient::new(transport, RecordingSleeper::default(), RetryPolicy::default())
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
    async fn onenote_401_marks_expired_and_stops() {
        let transport = MockGraphTransport::new();
        transport.queue_for_url(&onenote_url(), [GraphResponse::failure(401, None)]);
        let client = client(transport);
        let mut ledger = ExportLedger::new("m");

        let report = export_onenote(
            &client,
            &mut ledger,
            &meeting(),
            &OneNoteTarget { section_id: "section-1".into() },
            &ctx(),
        )
        .await;

        assert_eq!(report.overall, ExportStatus::FailedAuth);
        assert_eq!(report.connection_state, Some(MicrosoftConnectionState::Expired));
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
            &OneNoteTarget { section_id: "section-1".into() },
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
            &OneNoteTarget { section_id: "section-1".into() },
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

        let report = export_planner(&client, &mut ledger, &m, &dest, &ctx())
            .await
            .unwrap();

        assert_eq!(report.overall, ExportStatus::Succeeded);
        assert_eq!(report.items[0].resource_id.as_deref(), Some("task-1"));
        // Two calls (throttled + success), one created task, attempt count == 2.
        assert_eq!(client.transport().calls_for(&planner_url()), 2);
        assert_eq!(ledger.entry(&report.items[0].dedupe_key).unwrap().status, ExportStatus::Succeeded);
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
        let key = planner::dedupe_key(ctx().tenant_id, ctx().user_id, &dest, &m, &m.action_items[0]);
        ledger.record_success(&key, Some("task-1".into()), Some("http://t".into()), None);

        let client = client(transport);
        let report = export_planner(&client, &mut ledger, &m, &dest, &ctx())
            .await
            .unwrap();

        assert_eq!(report.overall, ExportStatus::Succeeded);
        assert!(!report.items[0].graph_called, "must not call Graph for succeeded item");
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

        let report = export_planner(&client, &mut ledger, &m, &dest, &ctx())
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
        let report2 = export_planner(&client, &mut ledger, &m, &dest, &ctx())
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
                GraphResponse::failure(503, None),
                GraphResponse::failure(503, None),
                GraphResponse::failure(503, None),
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

        let report = export_planner(&client, &mut ledger, &m, &dest, &ctx())
            .await
            .unwrap();
        assert_eq!(report.overall, ExportStatus::PartialFailure);
        assert_eq!(report.items[1].status, ExportStatus::Failed);

        // Second run: only item-2 is retried (Failed is retriable); queue a
        // success for it.
        client
            .transport()
            .queue_for_url(&planner_url(), [GraphResponse::success(201, r#"{"id":"task-2"}"#)]);
        let report2 = export_planner(&client, &mut ledger, &m, &dest, &ctx())
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
        let err = export_planner(&client, &mut ledger, &m, &bad, &ctx()).await;
        assert_eq!(err, Err(planner::PlannerBuildError::MissingPlanId));
    }
}
