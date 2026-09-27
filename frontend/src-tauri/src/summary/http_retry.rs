//! Bounded, cancellable retries shared by summary HTTP providers.
use rand::Rng;
use reqwest::{header::HeaderMap, RequestBuilder};
use serde_json::Value;
use std::{future::Future, time::Duration};
use tokio_util::sync::CancellationToken;

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let seconds = value.parse::<u64>().ok().or_else(|| {
        chrono::DateTime::parse_from_rfc2822(value)
            .ok()
            .map(|date| {
                (date.with_timezone(&chrono::Utc) - chrono::Utc::now())
                    .num_seconds()
                    .max(0) as u64
            })
    })?;
    Some(Duration::from_secs(seconds))
}

fn safe_message(value: &Value, secret: &str, prompts: &[&str]) -> Option<String> {
    let message = value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| value.get("error").and_then(Value::as_str))?;
    // Providers sometimes quote the rejected input. Never display those echoes.
    if prompts.iter().any(|prompt| {
        let chars: Vec<_> = prompt.chars().collect();
        if chars.is_empty() {
            return false;
        }
        if chars.len() < 24 {
            return message.contains(*prompt);
        }
        chars
            .windows(24)
            .any(|part| message.contains(&part.iter().collect::<String>()))
    }) {
        return None;
    }
    let message = if secret.is_empty() {
        message.to_owned()
    } else {
        message.replace(secret, "[redacted]")
    };
    let message = super::codex_provider::redact_secrets(&message);
    let message = regex::Regex::new(r#"(?i)\bBearer\s+[^\s,;"'<>]+"#)
        .ok()?
        .replace_all(&message, "Bearer [redacted]")
        .into_owned();
    Some(message.chars().take(300).collect())
}

async fn error_message(
    mut response: reqwest::Response,
    secret: &str,
    prompts: &[&str],
) -> (String, bool) {
    let status = response.status().as_u16();
    let mut body = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if body.len() + chunk.len() > 64 * 1024 {
            break;
        }
        body.extend_from_slice(&chunk);
    }
    let value = serde_json::from_slice::<Value>(&body).ok();
    let quota_exhausted = value.as_ref().is_some_and(|value| {
        ["/error/code", "/error/type"]
            .iter()
            .any(|path| value.pointer(path).and_then(Value::as_str) == Some("insufficient_quota"))
    });
    let detail = value
        .as_ref()
        .and_then(|value| safe_message(value, secret, prompts));
    let message = match detail {
        Some(message) if !message.is_empty() => {
            format!("Summary provider HTTP {status}: {message}")
        }
        _ => format!("Summary provider HTTP {status}. Check provider settings and retry."),
    };
    (message, quota_exhausted)
}

pub(crate) async fn send<F, Fut>(
    mut request: F,
    token: Option<&CancellationToken>,
    secret: &str,
    prompts: &[&str],
) -> Result<Value, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<RequestBuilder, String>>,
{
    super::llm_client::with_cancellation(token, async {
        for attempt in 0..3 {
            let (error, retryable, delay) = match request().await?.send().await {
                Ok(response) if response.status().is_success() => {
                    return super::llm_client::read_response_json(response).await;
                }
                Ok(response) => {
                    let retryable = matches!(
                        response.status().as_u16(),
                        408 | 429 | 500 | 502 | 503 | 504 | 529
                    );
                    let delay = retry_after(response.headers());
                    if let Some(delay) = delay.filter(|delay| delay.as_secs() > 60) {
                        return Err(format!("Summary provider is rate-limited; it asks to retry in about {} minutes.", delay.as_secs().div_ceil(60)));
                    }
                    let (error, quota_exhausted) = error_message(response, secret, prompts).await;
                    (
                        error,
                        retryable && !quota_exhausted,
                        delay,
                    )
                }
                Err(error) => {
                    // A full response timeout must not restart a costly generation.
                    let retryable = error.is_connect() && !error.is_timeout();
                    let message = if error.is_timeout() {
                        "Summary provider request timed out. For slow or local models, increase Timeout in provider settings."
                    } else {
                        "Could not connect to the summary provider"
                    };
                    (message.to_string(), retryable, None)
                }
            };
            if !retryable || attempt == 2 {
                return Err(error);
            }
            let jitter = rand::thread_rng().gen_range(0..250);
            tokio::time::sleep(
                delay.unwrap_or(Duration::from_secs(2 << attempt)) + Duration::from_millis(jitter),
            )
            .await;
        }
        unreachable!()
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn server(responses: Vec<String>) -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        tokio::spawn(async move {
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 4096];
                socket.read(&mut bytes).await.unwrap();
                calls.fetch_add(1, Ordering::SeqCst);
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (url, count)
    }

    fn response(status: u16, headers: &str, body: &str) -> String {
        format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}", body.len())
    }

    #[tokio::test]
    async fn quota_and_long_retry_delays_do_not_retry() {
        for field in ["code", "type"] {
            let body = serde_json::json!({"error": {field: "insufficient_quota", "message": "Quota exhausted"}}).to_string();
            let (url, count) =
                server(vec![response(429, "", &body), response(200, "", "{}")]).await;
            let client = reqwest::Client::new();
            assert!(send(|| async { Ok(client.get(&url)) }, None, "", &[])
                .await
                .unwrap_err()
                .contains("Quota exhausted"));
            assert_eq!(count.load(Ordering::SeqCst), 1);
        }
        let (url, count) = server(vec![
            response(429, "Retry-After: 3600\r\n", "{}"),
            response(200, "", "{}"),
        ])
        .await;
        let client = reqwest::Client::new();
        let error = send(|| async { Ok(client.get(&url)) }, None, "", &[])
            .await
            .unwrap_err();
        assert!(error.contains("60 minutes"));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn throttled_request_retries_and_honors_retry_after() {
        let (url, count) = server(vec![
            response(429, "Retry-After: 1\r\n", r#"{"error":{"message":"Busy"}}"#),
            response(200, "", "{}"),
        ])
        .await;
        let client = reqwest::Client::new();
        let start = std::time::Instant::now();
        send(|| async { Ok(client.get(&url)) }, None, "", &[])
            .await
            .unwrap();
        assert!(start.elapsed() >= Duration::from_secs(1));
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn bad_request_surfaces_redacted_message_without_retry() {
        let (url, count) = server(vec![response(
            400,
            "",
            r#"{"error":{"message":"Invalid model; key demo-secret; Bearer short-token"}}"#,
        )])
        .await;
        let client = reqwest::Client::new();
        let error = send(|| async { Ok(client.get(&url)) }, None, "demo-secret", &[])
            .await
            .unwrap_err();
        assert!(error.contains("Invalid model"));
        assert!(!error.contains("demo-secret") && !error.contains("short-token"));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_interrupts_backoff() {
        let (url, count) = server(vec![response(503, "Retry-After: 60\r\n", "{}")]).await;
        let token = CancellationToken::new();
        let cancel = token.clone();
        let client = reqwest::Client::new();
        tokio::spawn(async move {
            while count.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.cancel();
        });
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            send(|| async { Ok(client.get(&url)) }, Some(&token), "", &[]),
        )
        .await
        .unwrap();
        assert!(result.unwrap_err().contains("cancelled"));
    }

    #[test]
    fn request_echo_is_suppressed_and_dates_are_bounded() {
        let prompt = "Synthetic confidential meeting transcript content";
        assert!(safe_message(
            &serde_json::json!({"error": format!("Bad input: {prompt}")}),
            "",
            &[prompt]
        )
        .is_none());
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            (chrono::Utc::now() + chrono::Duration::days(1))
                .to_rfc2822()
                .parse()
                .unwrap(),
        );
        assert!(retry_after(&headers).unwrap() > Duration::from_secs(60));
    }
}
