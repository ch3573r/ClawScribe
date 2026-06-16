//! Graph API destination discovery: notebooks, sections, plans, buckets.

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
    }
}

fn map_outcome<T: serde::de::DeserializeOwned>(
    outcome: GraphOutcome,
) -> Result<Vec<T>, String> {
    match outcome {
        GraphOutcome::Success(resp) => {
            let list: GraphListResponse<T> = serde_json::from_str(&resp.body)
                .map_err(|e| format!("Failed to parse Graph response: {e}"))?;
            Ok(list.value)
        }
        GraphOutcome::Failed(kind) => Err(format!("Graph error: {}", kind.code())),
        GraphOutcome::Unknown(msg) => Err(format!("Network error: {msg}")),
    }
}

pub async fn list_notebooks<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
) -> Result<Vec<NotebookInfo>, String> {
    let request = get_request(format!(
        "{GRAPH_BASE}/me/onenote/notebooks?$select=id,displayName&$orderby=lastModifiedDateTime desc&$top=50"
    ));
    map_outcome(client.execute(&request, token).await)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::client::{GraphClient, RetryPolicy};
    use crate::exports::transport::{GraphResponse, MockGraphTransport};
    use async_trait::async_trait;
    use std::sync::Mutex;
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
    async fn list_sections_401_returns_error() {
        let transport = MockGraphTransport::new();
        transport.queue_default([GraphResponse::failure(401, None)]);
        let c = client(transport);
        let err = list_sections(&c, "token", "nb-1").await.unwrap_err();
        assert!(err.contains("unauthorized"));
    }
}
