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

/// The outcome captured from the loopback redirect.
struct Redirect {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Block on the loopback listener until Entra redirects back, then return the
/// parsed query parameters. Responds to the browser with a small "you can close
/// this window" page.
fn wait_for_redirect(
    listener: TcpListener,
    cancel: CancellationToken,
    deadline: Instant,
) -> Result<Redirect, MsAuthError> {
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
        let mut request = Vec::new();
        while !request.contains(&b'\n') {
            if cancel.is_cancelled() {
                return Err(MsAuthError::AuthorizationDeclined);
            }
            if Instant::now() >= deadline {
                return Err(MsAuthError::Unexpected("Sign-in timed out".into()));
            }
            let mut buffer = [0u8; 256];
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => continue 'accept,
                Ok(count) => request.extend_from_slice(&buffer[..count]),
            }
            if request.len() > 8192 {
                continue 'accept;
            }
        }
        let request_line = String::from_utf8_lossy(&request);

        // Request line looks like: `GET /?code=...&state=... HTTP/1.1`
        let path = request_line.split_whitespace().nth(1).unwrap_or("");
        let full = format!("http://localhost{path}");
        let parsed = url::Url::parse(&full).ok();

        let (mut code, mut state, mut error, mut error_description) = (None, None, None, None);
        if let Some(u) = parsed.as_ref() {
            for (k, v) in u.query_pairs() {
                match k.as_ref() {
                    "code" => code = Some(v.into_owned()),
                    "state" => state = Some(v.into_owned()),
                    "error" => error = Some(v.into_owned()),
                    "error_description" => error_description = Some(v.into_owned()),
                    _ => {}
                }
            }
        }

        let has_payload = code.is_some() || error.is_some();
        let body = if error.is_some() {
            "<html><body style=\"font-family:sans-serif\"><h2>Sign-in failed</h2><p>You can close this window and return to ClawScribe.</p></body></html>"
        } else {
            "<html><body style=\"font-family:sans-serif\"><h2>Signed in to ClawScribe</h2><p>You can close this window and return to the app.</p></body></html>"
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();

        if has_payload {
            return Ok(Redirect {
                code,
                state,
                error,
                error_description,
            });
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
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let code = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error")?.as_str().map(String::from))
            .unwrap_or_default();
        return Err(match code.as_str() {
            "invalid_grant" => MsAuthError::InvalidGrant(body),
            "consent_required" | "interaction_required" => MsAuthError::ConsentRequired,
            _ => MsAuthError::Unexpected(format!("Token exchange failed: {body}")),
        });
    }

    serde_json::from_str::<TokenResponse>(&body)
        .map_err(|e| MsAuthError::Unexpected(format!("Failed to parse token: {e}")))
}

pub async fn run_interactive_sign_in(
    http: &reqwest::Client,
    config: &MicrosoftAuthConfig,
    open_browser: impl Fn(&str),
    cancel: CancellationToken,
) -> Result<TokenResponse, MsAuthError> {
    tokio::select! {
        _ = cancel.cancelled() => Err(MsAuthError::AuthorizationDeclined),
        result = sign_in_with_scopes(config, |consent| run_interactive_attempt(http, config, &open_browser, consent, cancel.clone())) => result,
    }
}

async fn sign_in_with_scopes<F, Fut>(
    config: &MicrosoftAuthConfig,
    mut attempt: F,
) -> Result<TokenResponse, MsAuthError>
where
    F: FnMut(bool) -> Fut,
    Fut: std::future::Future<Output = Result<TokenResponse, MsAuthError>>,
{
    for consent in [false, true] {
        let token = attempt(consent).await?;
        let missing: Vec<_> = config
            .scopes
            .iter()
            .filter(|required| {
                // offline_access is represented by a refresh token, not a Graph permission.
                if required.as_str() == "offline_access" {
                    return token.refresh_token.as_ref().is_none_or(|t| t.is_empty());
                }
                !token.scope.split_whitespace().any(|granted| {
                    granted
                        .trim_start_matches("https://graph.microsoft.com/")
                        .eq_ignore_ascii_case(required)
                })
            })
            .cloned()
            .collect();
        if missing.is_empty() {
            return Ok(token);
        }
        if consent {
            return Err(MsAuthError::Unexpected(format!("Microsoft sign-in is missing required permissions: {}. Ask your administrator to approve these permissions.", missing.join(", "))));
        }
    }
    unreachable!()
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
    let redirect =
        tokio::task::spawn_blocking(move || wait_for_redirect(listener, cancel, deadline))
            .await
            .map_err(|_| MsAuthError::Unexpected("Sign-in listener failed".into()))??;

    if let Some(err) = redirect.error {
        let desc = redirect.error_description.unwrap_or_default();
        return Err(match err.as_str() {
            "access_denied" => MsAuthError::AuthorizationDeclined,
            "consent_required" | "interaction_required" => MsAuthError::ConsentRequired,
            _ => MsAuthError::Unexpected(format!("{err}: {desc}")),
        });
    }

    if redirect.state.as_deref() != Some(state.as_str()) {
        return Err(MsAuthError::Unexpected(
            "State mismatch in sign-in redirect (possible CSRF)".to_string(),
        ));
    }

    let code = redirect
        .code
        .ok_or_else(|| MsAuthError::Unexpected("No authorization code in redirect".to_string()))?;

    exchange_code(http, config, &code, &pkce.verifier, &redirect_uri).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_accepts_callback_after_client_connects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            wait_for_redirect(
                listener,
                CancellationToken::new(),
                Instant::now() + Duration::from_secs(2),
            )
        });
        let mut client = std::net::TcpStream::connect(address).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        client
            .write_all(b"GET /?code=test-code&state=test-state HTTP/1.1\r\n\r\n")
            .unwrap();
        let result = worker.join().unwrap().unwrap();
        assert_eq!(result.code.as_deref(), Some("test-code"));
        assert_eq!(result.state.as_deref(), Some("test-state"));
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
    async fn requests_consent_only_once_for_missing_scopes() {
        for still_missing in [false, true] {
            let config = MicrosoftAuthConfig::default();
            let mut calls = Vec::new();
            let result = sign_in_with_scopes(&config, |consent| {
                calls.push(consent);
                assert_eq!(
                    authorize_url(
                        &config,
                        "http://localhost:12345",
                        "challenge",
                        "state",
                        consent
                    )
                    .contains("prompt=consent"),
                    consent
                );
                std::future::ready(Ok(TokenResponse {
                    access_token: "test-token".into(),
                    refresh_token: Some("test-token".into()),
                    expires_in: 3600,
                    token_type: "Bearer".into(),
                    scope: if consent && !still_missing {
                        config.scopes.join(" ")
                    } else {
                        "User.Read".into()
                    },
                }))
            })
            .await;
            assert_eq!(calls, [false, true]);
            assert_eq!(result.is_err(), still_missing);
        }
        let config = MicrosoftAuthConfig::default();
        let mut calls = Vec::new();
        sign_in_with_scopes(&config, |consent| {
            calls.push(consent);
            std::future::ready(Ok(TokenResponse {
                access_token: "test-token".into(),
                refresh_token: Some("test-token".into()),
                expires_in: 3600,
                token_type: "Bearer".into(),
                scope: config.scopes.join(" "),
            }))
        })
        .await
        .unwrap();
        assert_eq!(calls, [false]);
    }

    #[test]
    fn states_and_verifiers_are_unique() {
        assert_ne!(random_state(), random_state());
        assert_ne!(generate_pkce().verifier, generate_pkce().verifier);
    }
}
