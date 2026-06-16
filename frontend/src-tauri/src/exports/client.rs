//! Authenticated Graph client: correlation ids, retry/backoff, sanitized errors.
//!
//! Retries only throttling/service-unavailable failures (see
//! [`GraphErrorKind::is_retriable`]). Auth, access, tenant, and not-found
//! failures stop immediately so the same token/inputs are not retried blindly.

use std::time::Duration;

use async_trait::async_trait;

use crate::exports::error::GraphErrorKind;
use crate::exports::transport::{GraphRequest, GraphResponse, GraphTransport, TransportError};

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 500,
            max_delay_ms: 30_000,
        }
    }
}

/// Compute the wait before the next attempt.
///
/// Honors `Retry-After` (seconds) when present; otherwise bounded exponential
/// backoff: `base * 2^(attempt-1)`, capped at `max_delay_ms`. `attempt` is the
/// 1-based number of the attempt that just failed.
pub fn backoff_delay(attempt: u32, retry_after_secs: Option<u64>, policy: &RetryPolicy) -> Duration {
    if let Some(secs) = retry_after_secs {
        return Duration::from_millis((secs * 1000).min(policy.max_delay_ms));
    }
    let shift = attempt.saturating_sub(1).min(20);
    let scaled = policy.base_delay_ms.saturating_mul(1u64 << shift);
    Duration::from_millis(scaled.min(policy.max_delay_ms))
}

/// Abstraction over waiting, so tests can run instantly and assert the waits.
#[async_trait]
pub trait Sleeper: Send + Sync {
    async fn sleep(&self, duration: Duration);
}

/// Production sleeper backed by tokio.
pub struct TokioSleeper;

#[async_trait]
impl Sleeper for TokioSleeper {
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// Outcome of a single logical Graph operation (after any retries).
#[derive(Debug, Clone)]
pub enum GraphOutcome {
    Success(GraphResponse),
    /// A mapped, sanitized failure. The optional detail is the Graph
    /// `error.code`/`error.message` (no token/body) for diagnostics.
    Failed(GraphErrorKind, Option<String>),
    /// Network/timeout with no HTTP status. For non-idempotent creates the
    /// caller must treat this as `unknown_after_submit`.
    Unknown(String),
}

pub struct GraphClient<T: GraphTransport, S: Sleeper> {
    transport: T,
    sleeper: S,
    policy: RetryPolicy,
}

impl<T: GraphTransport, S: Sleeper> GraphClient<T, S> {
    pub fn new(transport: T, sleeper: S, policy: RetryPolicy) -> Self {
        GraphClient {
            transport,
            sleeper,
            policy,
        }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Send a request, retrying retriable failures with backoff.
    pub async fn execute(&self, request: &GraphRequest, bearer_token: &str) -> GraphOutcome {
        let mut attempt = 1;
        loop {
            match self.transport.send(request, bearer_token).await {
                Ok(resp) if resp.is_success() => return GraphOutcome::Success(resp),
                Ok(resp) => {
                    let kind = GraphErrorKind::from_status(resp.status, resp.error_code.as_deref());
                    if kind.is_retriable() && attempt < self.policy.max_attempts {
                        let delay = backoff_delay(attempt, resp.retry_after_secs, &self.policy);
                        self.sleeper.sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    let detail = match (resp.error_code.as_deref(), resp.error_message.as_deref()) {
                        (Some(c), Some(m)) => Some(format!("{c}: {m}")),
                        (Some(c), None) => Some(c.to_string()),
                        (None, Some(m)) => Some(m.to_string()),
                        (None, None) => None,
                    };
                    return GraphOutcome::Failed(kind, detail);
                }
                Err(TransportError::Network(msg)) => return GraphOutcome::Unknown(msg),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honors_retry_after_capped() {
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay_ms: 500,
            max_delay_ms: 30_000,
        };
        assert_eq!(backoff_delay(1, Some(2), &policy), Duration::from_millis(2000));
        // Retry-After above the cap is clamped.
        assert_eq!(backoff_delay(1, Some(120), &policy), Duration::from_millis(30_000));
    }

    #[test]
    fn exponential_backoff_without_retry_after() {
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay_ms: 500,
            max_delay_ms: 30_000,
        };
        assert_eq!(backoff_delay(1, None, &policy), Duration::from_millis(500));
        assert_eq!(backoff_delay(2, None, &policy), Duration::from_millis(1000));
        assert_eq!(backoff_delay(3, None, &policy), Duration::from_millis(2000));
        // Capped.
        assert_eq!(backoff_delay(20, None, &policy), Duration::from_millis(30_000));
    }
}
