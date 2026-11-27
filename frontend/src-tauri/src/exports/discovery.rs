//! Graph API destination discovery: notebooks, sections, plans, buckets, To Do lists.

use serde::{Deserialize, Serialize};

use crate::exports::client::{GraphClient, GraphOutcome, Sleeper};
use crate::exports::exporter::graph_url_with_path_segments;
use crate::exports::ledger::require_remote_ids;
use crate::exports::transport::{GraphRequest, GraphTransport};

const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";
const MAX_DESTINATION_PAGES: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookInfo {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionInfo {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanInfo {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToDoListInfo {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub wellknown_list_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GraphListResponse<T> {
    value: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
}

fn get_request(url: String) -> GraphRequest {
    GraphRequest {
        method: "GET".into(),
        url,
        content_type: "application/json".into(),
        body: String::new(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: Vec::new(),
    }
}

async fn list_destinations<I: serde::de::DeserializeOwned, T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    mut url: String,
) -> Result<Vec<I>, String> {
    let graph_origin = url::Url::parse(GRAPH_BASE)
        .expect("Graph base URL")
        .origin();
    let mut items = Vec::new();
    for _ in 0..MAX_DESTINATION_PAGES {
        let outcome = client.execute(&get_request(url), token).await;
        let page: GraphListResponse<I> = map_response(outcome, "Graph response")?;
        items.extend(page.value);
        let Some(next_link) = page.next_link else {
            return Ok(items);
        };
        // Validate where the bearer token will be sent. Use Graph's original
        // link for the request because the continuation query is opaque.
        let next =
            url::Url::parse(&next_link).map_err(|_| "Microsoft returned an invalid next page.")?;
        if next.origin() != graph_origin
            || !next.path().starts_with("/v1.0/")
            || !next.username().is_empty()
            || next.password().is_some()
            || next.fragment().is_some()
        {
            return Err("Microsoft returned a next page outside the Graph API.".into());
        }
        url = next_link;
    }
    Err("Microsoft returned too many destination pages to load the complete list.".into())
}

fn map_response<T: serde::de::DeserializeOwned>(
    outcome: GraphOutcome,
    what: &str,
) -> Result<T, String> {
    match outcome {
        GraphOutcome::Success(resp) => serde_json::from_str::<T>(&resp.body)
            .map_err(|e| format!("Failed to parse {what}: {e}")),
        GraphOutcome::Failed(kind, detail) => Err(match detail {
            Some(d) => format!("Graph error ({}): {d}", kind.code()),
            None => format!("Graph error: {}", kind.code()),
        }),
        GraphOutcome::Unknown(_) => Err("Could not reach Microsoft Graph. Try again.".into()),
    }
}

fn map_created<T: serde::de::DeserializeOwned>(
    outcome: GraphOutcome,
    what: &str,
) -> Result<T, String> {
    match outcome {
        GraphOutcome::Unknown(_) => Err(
            "Microsoft did not confirm creation. Check the destination before trying again.".into(),
        ),
        outcome => map_response(outcome, what),
    }
}

pub fn is_onenote_large_library_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("10008")
        || lower.contains("5,000 onenote items")
        || lower.contains("5000 onenote items")
        || lower.contains("more than 5,000")
        || lower.contains("more than 5000")
}

pub async fn list_notebooks<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
) -> Result<Vec<NotebookInfo>, String> {
    // The OneNote API requires any $orderby field to also appear in $select,
    // otherwise it rejects the request — which previously surfaced as an empty
    // picker. Keep the query minimal and let the service apply its default
    // ordering rather than risk that constraint.
    let url = format!("{GRAPH_BASE}/me/onenote/notebooks?$select=id,displayName&$top=50");
    list_destinations(client, token, url).await
}

/// Create a new section in a notebook and return it.
///
/// Unlike listing sections, this is a POST (not a library enumeration), so it
/// is not subject to the OneNote 5,000-items-per-document-library limit
/// (error 10008). This is how exports target a notebook whose library is too
/// large to enumerate: a fresh dated section is created per export.
pub async fn create_section<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    notebook_id: &str,
    display_name: &str,
) -> Result<SectionInfo, String> {
    create_section_checked(client, token, notebook_id, display_name)
        .await
        .map_err(|error| error.message)
}

pub(super) struct SectionCreateError {
    pub outcome_unknown: bool,
    pub message: String,
}

pub(super) async fn create_section_checked<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    notebook_id: &str,
    display_name: &str,
) -> Result<SectionInfo, SectionCreateError> {
    require_remote_ids(&[notebook_id]).map_err(|message| SectionCreateError {
        outcome_unknown: false,
        message,
    })?;
    let request = GraphRequest {
        method: "POST".into(),
        url: graph_url_with_path_segments(&["me", "onenote", "notebooks", notebook_id, "sections"]),
        content_type: "application/json".into(),
        body: serde_json::json!({ "displayName": display_name }).to_string(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: Vec::new(),
    };
    let unknown = || {
        SectionCreateError {
        outcome_unknown: true,
        message:
              "OneNote did not confirm the new section. Check the notebook and resolve the outcome in Export history."
                .into(),
    }
    };
    match client.execute_create(&request, token).await {
        GraphOutcome::Success(response) => serde_json::from_str::<SectionInfo>(&response.body)
            .ok()
            .filter(|section| super::ledger::valid_remote_id(&section.id))
            .ok_or_else(unknown),
        GraphOutcome::Failed(kind, _) => Err(SectionCreateError {
            outcome_unknown: false,
            message: format!("OneNote could not create the section ({}).", kind.code()),
        }),
        GraphOutcome::Unknown(_) => Err(unknown()),
    }
}

/// Reuse the notebook's existing section with this display name if one exists,
/// otherwise create it. Creating a fresh section on every export would mint a
/// new section id each time, and since the OneNote dedupe key includes the
/// section id, that defeats the idempotency ledger and produces a duplicate
/// page (in a duplicate section) on every re-export. Matching the section by
/// name keeps the id — and therefore the dedupe key — stable across runs.
///
/// Returns `(section, created_now)`: `created_now` is true only when this call
/// created the section, so the caller can clean it up if the subsequent export
/// fails without touching a pre-existing section it merely reused.
pub async fn ensure_section<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    notebook_id: &str,
    display_name: &str,
) -> Result<(SectionInfo, bool), String> {
    let sections = match list_sections(client, token, notebook_id).await {
        Ok(sections) => sections,
        Err(err) if is_onenote_large_library_error(&err) => {
            log::warn!(
                "OneNote section listing hit Graph 10008; creating section without enumeration"
            );
            let created = create_section(client, token, notebook_id, display_name).await?;
            return Ok((created, true));
        }
        Err(err) => return Err(err),
    };
    if let Some(existing) = sections.into_iter().find(|s| {
        s.display_name
            .trim()
            .eq_ignore_ascii_case(display_name.trim())
    }) {
        return Ok((existing, false));
    }
    let created = create_section(client, token, notebook_id, display_name).await?;
    Ok((created, true))
}

/// Delete a OneNote section by id. Used to remove a section we just created when
/// the page export then failed, so a failed export doesn't leave an empty orphan
/// section behind.
pub async fn delete_section<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    section_id: &str,
) -> Result<(), String> {
    require_remote_ids(&[section_id])?;
    let request = GraphRequest {
        method: "DELETE".into(),
        url: graph_url_with_path_segments(&["me", "onenote", "sections", section_id]),
        content_type: "application/json".into(),
        body: String::new(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: Vec::new(),
    };
    match client.execute(&request, token).await {
        GraphOutcome::Success(_) => Ok(()),
        GraphOutcome::Failed(kind, detail) => Err(match detail {
            Some(d) => format!("Graph error ({}): {d}", kind.code()),
            None => format!("Graph error: {}", kind.code()),
        }),
        GraphOutcome::Unknown(msg) => Err(format!("Network error: {msg}")),
    }
}

/// Create a new OneNote notebook and return it. Requires the `Notes.Create`
/// (or `Notes.ReadWrite`) scope, which this app already requests.
pub async fn create_notebook<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    display_name: &str,
) -> Result<NotebookInfo, String> {
    let request = GraphRequest {
        method: "POST".into(),
        url: format!("{GRAPH_BASE}/me/onenote/notebooks"),
        content_type: "application/json".into(),
        body: serde_json::json!({ "displayName": display_name }).to_string(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: Vec::new(),
    };
    map_created(
        client.execute_create(&request, token).await,
        "created notebook",
    )
}

pub async fn list_sections<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    notebook_id: &str,
) -> Result<Vec<SectionInfo>, String> {
    require_remote_ids(&[notebook_id])?;
    let url =
        graph_url_with_path_segments(&["me", "onenote", "notebooks", notebook_id, "sections"]);
    list_destinations(
        client,
        token,
        format!("{url}?$select=id,displayName&$top=100"),
    )
    .await
}

pub async fn list_plans<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
) -> Result<Vec<PlanInfo>, String> {
    let url = format!("{GRAPH_BASE}/me/planner/plans?$select=id,title&$top=50");
    list_destinations(client, token, url).await
}

pub async fn list_buckets<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    plan_id: &str,
) -> Result<Vec<BucketInfo>, String> {
    require_remote_ids(&[plan_id])?;
    let url = graph_url_with_path_segments(&["planner", "plans", plan_id, "buckets"]);
    list_destinations(client, token, format!("{url}?$select=id,name&$top=100")).await
}

pub async fn list_todo_lists<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
) -> Result<Vec<ToDoListInfo>, String> {
    // Microsoft To Do list enumeration is touchy with OData query parameters
    // on some accounts. Use the documented plain endpoint; the default payload
    // already includes id, displayName, and wellknownListName.
    list_destinations(client, token, format!("{GRAPH_BASE}/me/todo/lists")).await
}

fn normalized_todo_list_name(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Create a Microsoft To Do list and return it. Requires `Tasks.ReadWrite`,
/// which this app already requests for Planner and To Do task export.
pub async fn create_todo_list<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    display_name: &str,
) -> Result<ToDoListInfo, String> {
    let normalized_name = normalized_todo_list_name(display_name);
    if !normalized_name.is_empty() {
        if let Some(existing) = list_todo_lists(client, token)
            .await?
            .into_iter()
            .find(|list| normalized_todo_list_name(&list.display_name) == normalized_name)
        {
            return Ok(existing);
        }
    }

    let request = GraphRequest {
        method: "POST".into(),
        url: format!("{GRAPH_BASE}/me/todo/lists"),
        content_type: "application/json".into(),
        body: serde_json::json!({ "displayName": display_name }).to_string(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: Vec::new(),
    };
    map_created(
        client.execute_create(&request, token).await,
        "created To Do list",
    )
}

/// Create a new bucket within a plan and return it. Requires `Tasks.ReadWrite`,
/// which this app already requests. The `orderHint` " !" places the bucket at
/// the start of the plan (Graph's documented "beginning" hint).
pub async fn create_bucket<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    plan_id: &str,
    name: &str,
) -> Result<BucketInfo, String> {
    require_remote_ids(&[plan_id])?;
    let request = GraphRequest {
        method: "POST".into(),
        url: format!("{GRAPH_BASE}/planner/buckets"),
        content_type: "application/json".into(),
        body: serde_json::json!({
            "name": name,
            "planId": plan_id,
            "orderHint": " !",
        })
        .to_string(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: Vec::new(),
    };
    map_created(
        client.execute_create(&request, token).await,
        "created bucket",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::client::{GraphClient, RetryPolicy};
    use crate::exports::transport::{GraphResponse, MockGraphTransport};
    use async_trait::async_trait;
    use std::time::Duration;

    #[derive(Default)]
    struct InstantSleeper;

    #[async_trait]
    impl Sleeper for InstantSleeper {
        async fn sleep(&self, _: Duration) {}
    }

    fn client(transport: MockGraphTransport) -> GraphClient<MockGraphTransport, InstantSleeper> {
        GraphClient::new(transport, InstantSleeper, RetryPolicy::default())
    }

    #[tokio::test]
    async fn list_notebooks_parses_graph_response() {
        let transport = MockGraphTransport::new();
        transport.queue_default([GraphResponse::success(
            200,
            r#"{"value":[{"id":"nb-1","displayName":"Work"},{"id":"nb-2","displayName":"Personal"}]}"#,
        )]);
        let c = client(transport);
        let notebooks = list_notebooks(&c, "token").await.unwrap();
        assert_eq!(notebooks.len(), 2);
        assert_eq!(notebooks[0].display_name, "Work");
    }

    #[tokio::test]
    async fn list_plans_parses_graph_response() {
        let transport = MockGraphTransport::new();
        transport.queue_default([GraphResponse::success(
            200,
            r#"{"value":[{"id":"plan-1","title":"Sprint 42"}]}"#,
        )]);
        let c = client(transport);
        let plans = list_plans(&c, "token").await.unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].title, "Sprint 42");
    }

    #[tokio::test]
    async fn list_plans_follows_opaque_continuation_urls() {
        let transport = MockGraphTransport::new();
        let next = format!("{GRAPH_BASE}/me/planner/plans?$skiptoken=part%2Btwo%2F%3D");
        transport.queue_default([GraphResponse::success(
            200,
            serde_json::json!({
                "value": [{"id": "plan-1", "title": "First"}],
                "@odata.nextLink": &next,
            })
            .to_string(),
        )]);
        transport.queue_for_url(
            &next,
            [GraphResponse::success(
                200,
                r#"{"value":[{"id":"plan-2","title":"Second"}]}"#,
            )],
        );
        let c = client(transport);
        let plans = list_plans(&c, "token").await.unwrap();
        assert_eq!(
            plans
                .iter()
                .map(|plan| plan.id.as_str())
                .collect::<Vec<_>>(),
            ["plan-1", "plan-2"]
        );
    }

    #[tokio::test]
    async fn discovery_rejects_continuations_outside_graph() {
        let transport = MockGraphTransport::new();
        transport.queue_default([GraphResponse::success(
            200,
            r#"{"value":[],"@odata.nextLink":"https://example.com/v1.0/me/todo/lists"}"#,
        )]);
        let c = client(transport);
        assert!(list_todo_lists(&c, "token").await.is_err());
        assert_eq!(c.transport().recorded().len(), 1);
    }

    #[tokio::test]
    async fn incomplete_listing_does_not_trigger_todo_list_creation() {
        let transport = MockGraphTransport::new();
        let first = format!("{GRAPH_BASE}/me/todo/lists");
        let page = GraphResponse::success(
            200,
            serde_json::json!({"value": [], "@odata.nextLink": &first}).to_string(),
        );
        transport.queue_for_url(&first, std::iter::repeat_n(page, 20));
        let c = client(transport);
        assert!(create_todo_list(&c, "token", "Meeting actions")
            .await
            .is_err());
        let requests = c.transport().recorded();
        assert_eq!(requests.len(), 20);
        assert!(requests.iter().all(|request| request.method == "GET"));
    }

    #[tokio::test]
    async fn destination_creates_do_not_retry_an_unconfirmed_post() {
        let transport = MockGraphTransport::new();
        transport.queue_for_url(
            &format!("{GRAPH_BASE}/me/onenote/notebooks"),
            [
                GraphResponse::failure(503, None),
                GraphResponse::success(201, r#"{"id":"notebook-1","displayName":"Notes"}"#),
            ],
        );
        transport.queue_for_url(
            &format!("{GRAPH_BASE}/planner/buckets"),
            [
                GraphResponse::failure(503, None),
                GraphResponse::success(201, r#"{"id":"bucket-1","name":"Actions"}"#),
            ],
        );
        transport.queue_for_url(
            &format!("{GRAPH_BASE}/me/todo/lists"),
            [
                GraphResponse::success(200, r#"{"value":[]}"#),
                GraphResponse::failure(503, None),
                GraphResponse::success(201, r#"{"id":"list-1","displayName":"Tasks"}"#),
            ],
        );
        let c = client(transport);
        assert!(create_notebook(&c, "token", "Notes").await.is_err());
        assert!(create_bucket(&c, "token", "plan-1", "Actions")
            .await
            .is_err());
        assert!(create_todo_list(&c, "token", "Tasks").await.is_err());
        let requests = c.transport().recorded();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn create_notebook_parses_created_notebook() {
        let transport = MockGraphTransport::new();
        transport.queue_default([GraphResponse::success(
            201,
            r#"{"id":"nb-9","displayName":"Meetings"}"#,
        )]);
        let c = client(transport);
        let nb = create_notebook(&c, "token", "Meetings").await.unwrap();
        assert_eq!(nb.id, "nb-9");
        assert_eq!(nb.display_name, "Meetings");
    }

    #[tokio::test]
    async fn create_bucket_parses_created_bucket() {
        let transport = MockGraphTransport::new();
        transport.queue_default([GraphResponse::success(
            201,
            r#"{"id":"bk-3","name":"Action items"}"#,
        )]);
        let c = client(transport);
        let bucket = create_bucket(&c, "token", "plan-1", "Action items")
            .await
            .unwrap();
        assert_eq!(bucket.id, "bk-3");
        assert_eq!(bucket.name, "Action items");
    }

    #[tokio::test]
    async fn list_todo_lists_parses_graph_response() {
        let transport = MockGraphTransport::new();
        transport.queue_default([GraphResponse::success(
            200,
            r#"{"value":[{"id":"td-1","displayName":"Tasks","wellknownListName":"defaultList"}]}"#,
        )]);
        let c = client(transport);
        let lists = list_todo_lists(&c, "token").await.unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].display_name, "Tasks");
        assert_eq!(lists[0].wellknown_list_name.as_deref(), Some("defaultList"));
        assert_eq!(
            c.transport().recorded()[0].url,
            format!("{GRAPH_BASE}/me/todo/lists")
        );
    }

    #[tokio::test]
    async fn create_todo_list_parses_created_list() {
        let transport = MockGraphTransport::new();
        transport.queue_default([
            GraphResponse::success(200, r#"{"value":[]}"#),
            GraphResponse::success(
                201,
                r#"{"id":"td-9","displayName":"Meeting actions","wellknownListName":"none"}"#,
            ),
        ]);
        let c = client(transport);
        let list = create_todo_list(&c, "token", "Meeting actions")
            .await
            .unwrap();
        assert_eq!(list.id, "td-9");
        assert_eq!(list.display_name, "Meeting actions");
        assert_eq!(list.wellknown_list_name.as_deref(), Some("none"));
        assert_eq!(
            c.transport()
                .calls_for(&format!("{GRAPH_BASE}/me/todo/lists")),
            2
        );
    }

    #[tokio::test]
    async fn create_todo_list_reuses_existing_normalized_name_on_later_page() {
        let transport = MockGraphTransport::new();
        let next = format!("{GRAPH_BASE}/me/todo/lists?$skiptoken=next");
        transport.queue_default([
            GraphResponse::success(
                200,
                serde_json::json!({
                    "value": [{"id": "td-2", "displayName": "Personal"}],
                    "@odata.nextLink": &next,
                })
                .to_string(),
            ),
            GraphResponse::success(
                200,
                r#"{"value":[{"id":"td-1","displayName":"Meeting actions","wellknownListName":"none"}]}"#,
            ),
        ]);
        let c = client(transport);
        let list = create_todo_list(&c, "token", "  meeting   ACTIONS ")
            .await
            .unwrap();
        assert_eq!(list.id, "td-1");
        assert_eq!(list.display_name, "Meeting actions");
        let requests = c.transport().recorded();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].url, next);
        assert!(requests.iter().all(|request| request.method == "GET"));
    }

    #[tokio::test]
    async fn invalid_destination_ids_never_reach_graph() {
        let c = client(MockGraphTransport::new());
        for id in [".", "..", "contains whitespace"] {
            assert!(list_buckets(&c, "token", id).await.is_err());
            assert!(list_sections(&c, "token", id).await.is_err());
            assert!(delete_section(&c, "token", id).await.is_err());
        }
        assert!(c.transport().recorded().is_empty());
    }

    #[tokio::test]
    async fn list_sections_401_returns_error() {
        let transport = MockGraphTransport::new();
        transport.queue_default([GraphResponse::failure(401, None)]);
        let c = client(transport);
        let err = list_sections(&c, "token", "nb-1").await.unwrap_err();
        assert!(err.contains("unauthorized"));
    }

    #[tokio::test]
    async fn ensure_section_creates_without_listing_when_onenote_library_is_too_large() {
        let transport = MockGraphTransport::new();
        transport.queue_default([
            GraphResponse {
                status: 403,
                retry_after_secs: None,
                error_code: Some("10008".into()),
                error_message: Some(
                    "One or more document libraries contains more than 5,000 OneNote items".into(),
                ),
                body: String::new(),
            },
            GraphResponse::success(201, r#"{"id":"section-9","displayName":"Meeting notes"}"#),
        ]);
        let c = client(transport);
        let (section, created) = ensure_section(&c, "token", "nb-1", "Meeting notes")
            .await
            .unwrap();
        assert!(created);
        assert_eq!(section.id, "section-9");
    }
}
