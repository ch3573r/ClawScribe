//! Real reqwest-backed [`GraphTransport`] for live Microsoft Graph calls.

use async_trait::async_trait;

use crate::exports::transport::{GraphRequest, GraphResponse, GraphTransport, TransportError};

pub struct ReqwestGraphTransport {
    client: reqwest::Client,
}

impl ReqwestGraphTransport {
    pub fn new() -> Self {
        let client = reqwest::ClientBuilder::new()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to create reqwest client");
        Self { client }
    }
}

#[async_trait]
impl GraphTransport for ReqwestGraphTransport {
    async fn send(
        &self,
        request: &GraphRequest,
        bearer_token: &str,
    ) -> Result<GraphResponse, TransportError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|e| TransportError::Network(format!("invalid method: {e}")))?;

        let mut builder = self
            .client
            .request(method, &request.url)
            .header("Authorization", format!("Bearer {bearer_token}"))
            .header("client-request-id", &request.correlation_id);

        if !request.body.is_empty() {
            builder = builder
                .header("Content-Type", &request.content_type)
                .body(request.body.clone());
        }

        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let resp = builder
            .send()
            .await
            .map_err(|e| TransportError::Network(e.to_string()))?;

        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get("Retry-After")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        let body = resp
            .text()
            .await
            .map_err(|e| TransportError::Network(e.to_string()))?;

        let parsed_error = serde_json::from_str::<serde_json::Value>(&body).ok();
        let error_code = parsed_error
            .as_ref()
            .and_then(|v| v.get("error")?.get("code")?.as_str().map(String::from));
        let error_message = parsed_error
            .as_ref()
            .and_then(|v| v.get("error")?.get("message")?.as_str().map(String::from));

        Ok(GraphResponse {
            status,
            retry_after_secs: retry_after,
            error_code,
            error_message,
            body,
        })
    }
}
