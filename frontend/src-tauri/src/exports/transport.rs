//! Microsoft Graph HTTP transport abstraction.
//!
//! The exporter talks to Graph only through [`GraphTransport`]. A real reqwest
//! transport will be added with live sign-in; tests use [`MockGraphTransport`].
//! Recorded requests never retain the bearer token or full body — only a body
//! hash and the sanitized headers — so test fixtures cannot leak secrets.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use async_trait::async_trait;

/// A request to Graph. The `bearer_token` is supplied at send time and is never
/// stored in [`RecordedRequest`].
#[derive(Debug, Clone)]
pub struct GraphRequest {
    pub method: String,
    pub url: String,
    pub content_type: String,
    pub body: String,
    /// Caller-generated `client-request-id` for correlation in logs.
    pub correlation_id: String,
    /// Extra request headers (e.g. `If-Match` for Planner detail PATCHes). Never
    /// used to carry the bearer token, which is supplied separately at send time.
    pub headers: Vec<(String, String)>,
}

/// A response from Graph, already reduced to the fields the exporter needs.
#[derive(Debug, Clone)]
pub struct GraphResponse {
    pub status: u16,
    /// `Retry-After` in seconds, when present (429/503).
    pub retry_after_secs: Option<u64>,
    /// Graph `error.code`, when the body carried one.
    pub error_code: Option<String>,
    /// Graph `error.message`, when present. A human-readable reason (e.g. why
    /// OneNote denied a notebook) — surfaced for diagnostics, never a token.
    pub error_message: Option<String>,
    /// Response body. For successes the exporter parses `id`/`webUrl`.
    pub body: String,
}

impl GraphResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn success(status: u16, body: impl Into<String>) -> Self {
        GraphResponse {
            status,
            retry_after_secs: None,
            error_code: None,
            error_message: None,
            body: body.into(),
        }
    }

    pub fn failure(status: u16, error_code: Option<&str>) -> Self {
        GraphResponse {
            status,
            retry_after_secs: None,
            error_code: error_code.map(|c| c.to_string()),
            error_message: None,
            body: String::new(),
        }
    }

    pub fn with_retry_after(mut self, secs: u64) -> Self {
        self.retry_after_secs = Some(secs);
        self
    }
}

#[derive(Debug, Clone)]
pub enum TransportError {
    /// Network/timeout failure with no HTTP status — outcome unknown.
    Network(String),
}

#[async_trait]
pub trait GraphTransport: Send + Sync {
    /// Send `request` using `bearer_token`. The token must never be logged or
    /// stored by an implementation.
    async fn send(
        &self,
        request: &GraphRequest,
        bearer_token: &str,
    ) -> Result<GraphResponse, TransportError>;
}

/// A sanitized record of a sent request — for test assertions and audit. Holds
/// no token and no raw body.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub url: String,
    pub content_type: String,
    pub body_hash: String,
    pub correlation_id: String,
    pub attempt: u32,
}

fn hash_body(body: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    hasher.finalize()[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// In-memory transport for tests. Responses are queued per URL (with a fallback
/// default queue), and every request is recorded in sanitized form.
pub struct MockGraphTransport {
    per_url: Mutex<HashMap<String, VecDeque<GraphResponse>>>,
    default_queue: Mutex<VecDeque<GraphResponse>>,
    recorded: Mutex<Vec<RecordedRequest>>,
    attempt_counts: Mutex<HashMap<String, u32>>,
}

impl Default for MockGraphTransport {
    fn default() -> Self {
        MockGraphTransport {
            per_url: Mutex::new(HashMap::new()),
            default_queue: Mutex::new(VecDeque::new()),
            recorded: Mutex::new(Vec::new()),
            attempt_counts: Mutex::new(HashMap::new()),
        }
    }
}

impl MockGraphTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue responses returned in order regardless of URL.
    pub fn queue_default(&self, responses: impl IntoIterator<Item = GraphResponse>) {
        self.default_queue.lock().unwrap().extend(responses);
    }

    /// Queue responses returned in order for a specific URL.
    pub fn queue_for_url(&self, url: &str, responses: impl IntoIterator<Item = GraphResponse>) {
        self.per_url
            .lock()
            .unwrap()
            .entry(url.to_string())
            .or_default()
            .extend(responses);
    }

    pub fn recorded(&self) -> Vec<RecordedRequest> {
        self.recorded.lock().unwrap().clone()
    }

    /// Number of times the given URL was actually called.
    pub fn calls_for(&self, url: &str) -> usize {
        self.recorded
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.url == url)
            .count()
    }
}

#[async_trait]
impl GraphTransport for MockGraphTransport {
    async fn send(
        &self,
        request: &GraphRequest,
        bearer_token: &str,
    ) -> Result<GraphResponse, TransportError> {
        // Defensive: a real transport sends this; the mock proves we never log
        // it by simply discarding it here.
        let _ = bearer_token;

        let attempt = {
            let mut counts = self.attempt_counts.lock().unwrap();
            let n = counts.entry(request.url.clone()).or_insert(0);
            *n += 1;
            *n
        };

        self.recorded.lock().unwrap().push(RecordedRequest {
            method: request.method.clone(),
            url: request.url.clone(),
            content_type: request.content_type.clone(),
            body_hash: hash_body(&request.body),
            correlation_id: request.correlation_id.clone(),
            attempt,
        });

        if let Some(queue) = self.per_url.lock().unwrap().get_mut(&request.url) {
            if let Some(resp) = queue.pop_front() {
                return Ok(resp);
            }
        }
        if let Some(resp) = self.default_queue.lock().unwrap().pop_front() {
            return Ok(resp);
        }
        Err(TransportError::Network(
            "mock transport: no queued response".to_string(),
        ))
    }
}
