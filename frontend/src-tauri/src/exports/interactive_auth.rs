//! Interactive Microsoft Entra ID sign-in via the OAuth 2.0 authorization-code
//! flow with PKCE.
//!
//! Unlike the device-code flow, this opens the user's system browser straight
//! to the Microsoft sign-in page and captures the redirect on a loopback
//! listener — so the user gets a normal "Sign in with Microsoft" experience
//! (SSO, Windows Hello, MFA) with nothing to type.
//!
//! Flow:
//!   1. Generate a PKCE verifier/challenge and a random state.
//!   2. Bind a loopback TCP listener on an ephemeral port.
//!   3. Open the browser to the authorize endpoint with
//!      `redirect_uri=http://localhost:<port>`.
//!   4. Wait for Entra to redirect back with `?code=...&state=...`.
//!   5. Exchange the code (+ verifier) for access/refresh tokens.
//!
//! The app registration must list `http://localhost` as a redirect URI under
//! its "Mobile and desktop applications" (public client) platform; Entra
//! ignores the loopback port at match time.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::exports::auth::{MicrosoftAuthConfig, MsAuthError, TokenResponse};

/// How long to wait for the user to complete sign-in in the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

struct Pkce {
    verifier: String,
    challenge: String,
}

fn generate_pkce() -> Pkce {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    Pkce {
        verifier,
        challenge,
    }
}

fn random_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn authorize_url(
    config: &MicrosoftAuthConfig,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
    consent: bool,
) -> String {
    let mut url = url::Url::parse(&format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/authorize",
        config.tenant_id
    ))
    .expect("static authorize URL is valid");
    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_mode", "query")
        .append_pair("scope", &config.scopes.join(" "))
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state);
    if consent {
        url.query_pairs_mut().append_pair("prompt", "consent");
    }
    url.into()
}

/// Accept only the callback route registered with Entra and our current state.
/// Invalid or unrelated requests must leave the listener available to Entra.
fn parse_callback(request_line: &str, expected_state: &str) -> Option<Result<String, MsAuthError>> {
    let mut parts = request_line.split_ascii_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    let query = parts.next()?.strip_prefix("/?")?;
    if !matches!(parts.next()?, "HTTP/1.0" | "HTTP/1.1") || parts.next().is_some() {
        return None;
    }

    let (mut code, mut state, mut error) = (None, None, None);
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        let slot = match key.as_ref() {
            "code" => &mut code,
            "state" => &mut state,
            "error" => &mut error,
            _ => continue,
        };
        if value.is_empty() || slot.replace(value.into_owned()).is_some() {
            return None;
        }
    }
    if state.as_deref() != Some(expected_state) {
        return None;
    }
    match (code, error.as_deref()) {
        (Some(code), None) => Some(Ok(code)),
        (None, Some("access_denied")) => Some(Err(MsAuthError::AuthorizationDeclined)),
        (None, Some("consent_required" | "interaction_required")) => {
            Some(Err(MsAuthError::ConsentRequired))
        }
        (None, Some(_)) => Some(Err(MsAuthError::Unexpected(
            "Microsoft did not authorize sign-in".into(),
        ))),
        _ => None,
    }
}

/// Block on the loopback listener until Entra redirects back, then return the
/// verified authorization code. Responds to the browser with a short status page.
fn wait_for_redirect(
    listener: TcpListener,
    cancel: CancellationToken,
    deadline: Instant,
    expected_state: &str,
) -> Result<String, MsAuthError> {
    listener
        .set_nonblocking(true)
        .map_err(|e| MsAuthError::Network(e.to_string()))?;

    // Polling and per-read deadlines also cover local clients that never finish a request.
    'accept: loop {
        if cancel.is_cancelled() {
            return Err(MsAuthError::AuthorizationDeclined);
        }
        if Instant::now() >= deadline {
            return Err(MsAuthError::Unexpected("Sign-in timed out".into()));
        }
        let (mut stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(25));
                continue;
            }
            Err(_) => return Err(MsAuthError::Network("Loopback listener failed".into())),
        };
        // Windows may inherit the listener's nonblocking mode on accepted sockets.
        stream
            .set_nonblocking(false)
            .map_err(|_| MsAuthError::Network("Could not configure sign-in connection".into()))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|_| MsAuthError::Network("Could not configure sign-in listener".into()))?;
        stream
            .set_write_timeout(Some(Duration::from_millis(100)))
            .map_err(|_| MsAuthError::Network("Could not configure sign-in listener".into()))?;
        let connection_deadline = Instant::now() + Duration::from_secs(5);
        let mut request = Vec::new();
        while !request.contains(&b'\n') {
            if cancel.is_cancelled() {
                return Err(MsAuthError::AuthorizationDeclined);
            }
            if Instant::now() >= deadline {
                return Err(MsAuthError::Unexpected("Sign-in timed out".into()));
            }
            if Instant::now() >= connection_deadline {
                continue 'accept;
            }
            let mut buffer = [0u8; 256];
            match stream.read(&mut buffer) {
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue
                }
                Ok(0) | Err(_) => continue 'accept,
                Ok(count) => request.extend_from_slice(&buffer[..count]),
            }
            if request.len() > 8192 {
                continue 'accept;
            }
        }
        let result = request
            .split(|byte| *byte == b'\n')
            .next()
            .and_then(|line| std::str::from_utf8(line).ok())
            .and_then(|line| parse_callback(line, expected_state));
        let (status, title) = match &result {
            Some(Ok(_)) => ("200 OK", "Continue in ClawScribe"),
            Some(Err(_)) => ("400 Bad Request", "Sign-in failed"),
            None => ("400 Bad Request", "Invalid sign-in request"),
        };
        let body = format!(
            "<html><body style=\"font-family:sans-serif\"><h2>{title}</h2><p>You can close this window and return to ClawScribe.</p></body></html>"
        );
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();

        if let Some(result) = result {
            return result;
        }
        // Otherwise keep listening (ignored a stray request).
    }
}

async fn exchange_code(
    http: &reqwest::Client,
    config: &MicrosoftAuthConfig,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, MsAuthError> {
    let token_url = format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
        config.tenant_id
    );
    let resp = http
        .post(&token_url)
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
            ("scope", &config.scopes.join(" ")),
        ])
        .send()
        .await
        .map_err(|e| MsAuthError::Network(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(token_exchange_error(status, &body));
    }

    resp.json::<TokenResponse>().await.map_err(|_| {
        log::warn!("Microsoft token exchange returned an invalid success response");
        MsAuthError::Unexpected("Microsoft returned an invalid token response".into())
    })
}

fn token_exchange_error(status: reqwest::StatusCode, body: &str) -> MsAuthError {
    let payload = serde_json::from_str::<serde_json::Value>(body).ok();
    let (kind, error) = match payload.as_ref().and_then(|value| value["error"].as_str()) {
        Some("invalid_grant") => (
            "invalid grant",
            MsAuthError::InvalidGrant("Sign in to Microsoft again".into()),
        ),
        Some("consent_required" | "interaction_required") => {
            ("consent required", MsAuthError::ConsentRequired)
        }
        _ => (
            "other error",
            MsAuthError::Unexpected(format!(
                "Microsoft token exchange failed (HTTP {})",
                status.as_u16()
            )),
        ),
    };
    log::warn!(
        "Microsoft token exchange failed (HTTP {}, {kind})",
        status.as_u16()
    );
    error
}

pub async fn run_interactive_sign_in(
    http: &reqwest::Client,
    config: &MicrosoftAuthConfig,
    open_browser: impl Fn(&str),
    consent: bool,
    cancel: CancellationToken,
) -> Result<TokenResponse, MsAuthError> {
    tokio::select! {
        _ = cancel.cancelled() => Err(MsAuthError::AuthorizationDeclined),
        result = sign_in_with_scopes(consent, |consent| run_interactive_attempt(http, config, &open_browser, consent, cancel.clone())) => result,
    }
}

async fn sign_in_with_scopes<F, Fut>(
    consent: bool,
    attempt: F,
) -> Result<TokenResponse, MsAuthError>
where
    F: FnOnce(bool) -> Fut,
    Fut: std::future::Future<Output = Result<TokenResponse, MsAuthError>>,
{
    // Partial grants are a usable session. Only an explicit UI action requests consent.
    attempt(consent).await
}

/// Run the full interactive sign-in: open the browser, capture the loopback
/// redirect, and exchange the code for tokens. `open_browser` is injected so
/// callers reuse the app's existing URL opener.
async fn run_interactive_attempt(
    http: &reqwest::Client,
    config: &MicrosoftAuthConfig,
    open_browser: impl Fn(&str),
    consent: bool,
    cancel: CancellationToken,
) -> Result<TokenResponse, MsAuthError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| MsAuthError::Network(format!("could not bind loopback listener: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| MsAuthError::Network(e.to_string()))?
        .port();
    let redirect_uri = format!("http://localhost:{port}");

    let pkce = generate_pkce();
    let state = random_state();
    let auth_url = authorize_url(config, &redirect_uri, &pkce.challenge, &state, consent);

    open_browser(&auth_url);

    let deadline = Instant::now() + SIGN_IN_TIMEOUT;
    let code =
        tokio::task::spawn_blocking(move || wait_for_redirect(listener, cancel, deadline, &state))
            .await
            .map_err(|_| MsAuthError::Unexpected("Sign-in listener failed".into()))??;

    exchange_code(http, config, &code, &pkce.verifier, &redirect_uri).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_rejects_unrelated_paths_and_state_before_showing_success() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            wait_for_redirect(
                listener,
                CancellationToken::new(),
                Instant::now() + Duration::from_secs(3),
                "test-state",
            )
        });
        for target in [
            "/favicon.ico",
            "/other?code=test-code&state=test-state",
            "/?code=test-code&state=wrong-state",
            "/?error=access_denied&state=wrong-state",
        ] {
            let mut client = std::net::TcpStream::connect(address).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            write!(client, "GET {target} HTTP/1.1\r\n\r\n").unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            assert!(response.starts_with("HTTP/1.1 400"), "{target}");
            assert!(!response.contains("Continue in ClawScribe"), "{target}");
        }
        let mut client = std::net::TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client
            .write_all(b"GET /?code=test-code&state=test-state HTTP/1.1\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("Continue in ClawScribe"));
        assert_eq!(worker.join().unwrap().unwrap(), "test-code");
    }

    #[test]
    fn provider_errors_keep_their_type_without_exposing_provider_details() {
        let error = token_exchange_error(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":"invalid_grant","error_description":"synthetic provider diagnostic"}"#,
        );
        assert!(matches!(error, MsAuthError::InvalidGrant(_)));
        assert_eq!(
            error.to_string(),
            "Invalid grant: Sign in to Microsoft again"
        );

        let error = token_exchange_error(
            reqwest::StatusCode::BAD_GATEWAY,
            "synthetic provider diagnostic",
        );
        assert_eq!(
            error.to_string(),
            "Unexpected auth error: Microsoft token exchange failed (HTTP 502)"
        );
        let callback = parse_callback(
            "GET /?error=access_denied&state=test-state HTTP/1.1",
            "test-state",
        );
        assert!(matches!(
            callback,
            Some(Err(MsAuthError::AuthorizationDeclined))
        ));
    }

    #[test]
    fn listener_accepts_callback_after_client_connects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            wait_for_redirect(
                listener,
                CancellationToken::new(),
                Instant::now() + Duration::from_secs(2),
                "test-state",
            )
        });
        let mut client = std::net::TcpStream::connect(address).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        client
            .write_all(b"GET /?code=test-code&state=test-state HTTP/1.1\r\n\r\n")
            .unwrap();
        assert_eq!(worker.join().unwrap().unwrap(), "test-code");
    }

    #[test]
    fn idle_callback_connection_expires_and_next_client_succeeds() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            wait_for_redirect(
                listener,
                CancellationToken::new(),
                Instant::now() + Duration::from_secs(9),
                "test-state",
            )
        });
        let _idle = std::net::TcpStream::connect(address).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        let mut next = std::net::TcpStream::connect(address).unwrap();
        next.write_all(b"GET /?code=test-code&state=test-state HTTP/1.1\r\n\r\n")
            .unwrap();
        assert_eq!(worker.join().unwrap().unwrap(), "test-code");
        assert!(started.elapsed() >= Duration::from_secs(4));
        assert!(started.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn listener_exits_and_releases_port_after_timeout_or_cancel() {
        for cancelled in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let cancel = CancellationToken::new();
            let worker_cancel = cancel.clone();
            let worker = std::thread::spawn(move || {
                wait_for_redirect(
                    listener,
                    worker_cancel,
                    Instant::now() + Duration::from_millis(150),
                    "test-state",
                )
                .map(|_| ())
            });
            let stalled = std::net::TcpStream::connect(address).unwrap();
            if cancelled {
                cancel.cancel();
            }
            assert!(worker.join().unwrap().is_err());
            drop(stalled);
            assert!(TcpListener::bind(address).is_ok());
        }
    }

    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        let pkce = generate_pkce();
        // Recompute the challenge from the verifier and confirm it matches.
        let mut hasher = Sha256::new();
        hasher.update(pkce.verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(hasher.finalize());
        assert_eq!(pkce.challenge, expected);
        // base64url, no padding.
        assert!(!pkce.challenge.contains('='));
        assert!(!pkce.challenge.contains('+'));
        assert!(!pkce.challenge.contains('/'));
    }

    #[test]
    fn authorize_url_has_pkce_and_loopback() {
        let config = MicrosoftAuthConfig::default();
        let u = authorize_url(
            &config,
            "http://localhost:12345",
            "challenge123",
            "state456",
            false,
        );
        assert!(u.contains("code_challenge=challenge123"));
        assert!(u.contains("code_challenge_method=S256"));
        assert!(u.contains("response_type=code"));
        assert!(u.contains(&format!("client_id={}", config.client_id)));
        assert!(u.contains("redirect_uri=http%3A%2F%2Flocalhost%3A12345"));
        assert!(u.contains("state=state456"));
        assert!(!u.contains("prompt=consent"));
    }

    #[tokio::test]
    async fn partial_scopes_connect_without_automatic_consent() {
        let mut calls = Vec::new();
        let token = sign_in_with_scopes(false, |consent| {
            calls.push(consent);
            std::future::ready(Ok(TokenResponse {
                access_token: "test-token".into(),
                refresh_token: None,
                expires_in: 3600,
                token_type: "Bearer".into(),
                scope: "User.Read Notes.Create".into(),
            }))
        })
        .await
        .unwrap();
        assert_eq!(calls, [false]);
        assert_eq!(token.scope, "User.Read Notes.Create");
        assert!(token.refresh_token.is_none());
        let config = MicrosoftAuthConfig::default();
        assert!(authorize_url(
            &config,
            "http://localhost:12345",
            "challenge",
            "state",
            true
        )
        .contains("prompt=consent"));
    }

    #[test]
    fn states_and_verifiers_are_unique() {
        assert_ne!(random_state(), random_state());
        assert_ne!(generate_pkce().verifier, generate_pkce().verifier);
    }
}
