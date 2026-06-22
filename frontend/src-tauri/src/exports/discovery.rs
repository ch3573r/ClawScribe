//! Graph API destination discovery: notebooks, sections, plans, buckets, To Do lists.

use serde::{Deserialize, Serialize};

use crate::exports::client::{GraphClient, GraphOutcome, Sleeper};
use crate::exports::transport::{GraphRequest, GraphTransport};

const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

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

fn map_outcome<T: serde::de::DeserializeOwned>(outcome: GraphOutcome) -> Result<Vec<T>, String> {
    match outcome {
        GraphOutcome::Success(resp) => {
            let list: GraphListResponse<T> = serde_json::from_str(&resp.body)
                .map_err(|e| format!("Failed to parse Graph response: {e}"))?;
            Ok(list.value)
        }
        GraphOutcome::Failed(kind, detail) => Err(match detail {
            Some(d) => format!("Graph error ({}): {d}", kind.code()),
            None => format!("Graph error: {}", kind.code()),
        }),
        GraphOutcome::Unknown(msg) => Err(format!("Network error: {msg}")),
    }
}

fn map_single<T: serde::de::DeserializeOwned>(
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
        GraphOutcome::Unknown(msg) => Err(format!("Network error: {msg}")),
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
    let request = get_request(format!(
        "{GRAPH_BASE}/me/onenote/notebooks?$select=id,displayName&$top=50"
    ));
    map_outcome(client.execute(&request, token).await)
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
    let request = GraphRequest {
        method: "POST".into(),
        url: format!("{GRAPH_BASE}/me/onenote/notebooks/{notebook_id}/sections"),
        content_type: "application/json".into(),
        body: serde_json::json!({ "displayName": display_name }).to_string(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: Vec::new(),
    };
    match client.execute(&request, token).await {
        GraphOutcome::Success(resp) => serde_json::from_str::<SectionInfo>(&resp.body)
            .map_err(|e| format!("Failed to parse created section: {e}")),
        GraphOutcome::Failed(kind, detail) => Err(match detail {
            Some(d) => format!("Graph error ({}): {d}", kind.code()),
            None => format!("Graph error: {}", kind.code()),
        }),
        GraphOutcome::Unknown(msg) => Err(format!("Network error: {msg}")),
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
    let request = GraphRequest {
        method: "DELETE".into(),
        url: format!("{GRAPH_BASE}/me/onenote/sections/{section_id}"),
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
    map_single(client.execute(&request, token).await, "created notebook")
}

pub async fn list_sections<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    notebook_id: &str,
) -> Result<Vec<SectionInfo>, String> {
    let request = get_request(format!(
        "{GRAPH_BASE}/me/onenote/notebooks/{notebook_id}/sections?$select=id,displayName&$top=100"
    ));
    map_outcome(client.execute(&request, token).await)
}

pub async fn list_plans<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
) -> Result<Vec<PlanInfo>, String> {
    let request = get_request(format!(
        "{GRAPH_BASE}/me/planner/plans?$select=id,title&$top=50"
    ));
    map_outcome(client.execute(&request, token).await)
}

pub async fn list_buckets<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    plan_id: &str,
) -> Result<Vec<BucketInfo>, String> {
    let request = get_request(format!(
        "{GRAPH_BASE}/planner/plans/{plan_id}/buckets?$select=id,name&$top=100"
    ));
    map_outcome(client.execute(&request, token).await)
}

pub async fn list_todo_lists<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
) -> Result<Vec<ToDoListInfo>, String> {
    let request = get_request(format!(
        "{GRAPH_BASE}/me/todo/lists?$select=id,displayName,wellknownListName&$top=50"
    ));
    map_outcome(client.execute(&request, token).await)
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
    map_single(client.execute(&request, token).await, "created bucket")
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
