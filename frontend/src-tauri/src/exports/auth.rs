//! Microsoft Entra ID device-code OAuth 2.0 flow.
//!
//! Implements the device-code grant against the v2.0 token endpoint. The flow
//! is: request a user code, show it in the UI, open the verification URL, poll
//! until the user completes sign-in, then exchange for access + refresh tokens.

use serde::{Deserialize, Serialize};

pub const CLAWSCRIBE_CLIENT_ID: &str = "4ab2ca8f-c2f1-45f3-b4ee-8bc9a511bcc8";

/// Authority used for sign-in. `organizations` accepts any Entra work/school
/// tenant but not personal Microsoft accounts. (Use a specific tenant GUID to
/// lock to one org, or `common` to also allow personal accounts — but Planner
/// is unavailable for personal accounts.)
pub const DEFAULT_AUTHORITY: &str = "organizations";

// Least-privilege: every scope here must map to an endpoint the app actually
// calls. OneNote export -> Notes.*; Planner/To Do export -> Tasks.ReadWrite;
// calendar add-on -> Calendars.Read; OneDrive/SharePoint file export ->
// Files.ReadWrite; sign-in -> User.Read; token refresh -> offline_access.
// Teams meeting detection is local (window/process scanning), so it needs no
// Graph scope. Don't add OnlineMeetings/Presence or broader Files scopes unless
// a code path calls those endpoints.
pub const DEFAULT_SCOPES: &[&str] = &[
    "User.Read",
    "Notes.ReadWrite",
    "Notes.Create",
    // Read/write notebooks the user can access beyond their own OneDrive
    // (Teams/SharePoint/shared notebooks). User-consentable.
    "Notes.ReadWrite.All",
    "Tasks.ReadWrite",
    "Calendars.Read",
    "Files.ReadWrite",
    "offline_access",
];

#[derive(Debug, Clone)]
pub struct MicrosoftAuthConfig {
    pub client_id: String,
    pub tenant_id: String,
    pub scopes: Vec<String>,
}

impl Default for MicrosoftAuthConfig {
    fn default() -> Self {
        MicrosoftAuthConfig {
            client_id: CLAWSCRIBE_CLIENT_ID.to_string(),
            tenant_id: DEFAULT_AUTHORITY.to_string(),
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl MicrosoftAuthConfig {
    fn device_code_url(&self) -> String {
        format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/devicecode",
            self.tenant_id
        )
    }

    fn token_url(&self) -> String {
        format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.tenant_id
        )
    }

    fn scope_string(&self) -> String {
        self.scopes.join(" ")
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub message: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub expires_in: u64,
    #[serde(default)]
    pub token_type: String,
    #[serde(default)]
    pub scope: String,
}

crate::credentials::debug_without_secrets!(DeviceCodeResponse {
    user_code, verification_uri, message, expires_in, interval;
    secret device_code
});
crate::credentials::debug_without_secrets!(TokenResponse {
    expires_in, token_type, scope;
    secret access_token, refresh_token
});

#[derive(Debug, Clone)]
pub enum PollResult {
    Pending,
    Completed(TokenResponse),
    Expired,
    Denied,
}

#[derive(Debug, Clone)]
pub enum MsAuthError {
    Network(String),
    DeviceCodeExpired,
    AuthorizationDeclined,
    InvalidGrant(String),
    ConsentRequired,
    TenantBlocked,
    TokenRefreshFailed(String),
    Unexpected(String),
}

impl std::fmt::Display for MsAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MsAuthError::Network(e) => write!(f, "Network error: {e}"),
            MsAuthError::DeviceCodeExpired => write!(f, "Device code expired"),
            MsAuthError::AuthorizationDeclined => write!(f, "Authorization declined by user"),
            MsAuthError::InvalidGrant(e) => write!(f, "Invalid grant: {e}"),
            MsAuthError::ConsentRequired => write!(f, "Admin consent required"),
            MsAuthError::TenantBlocked => write!(f, "Tenant has blocked this application"),
            MsAuthError::TokenRefreshFailed(e) => write!(f, "Token refresh failed: {e}"),
            MsAuthError::Unexpected(e) => write!(f, "Unexpected auth error: {e}"),
        }
    }
}

fn auth_network_error(operation: &str, error: fn(String) -> MsAuthError) -> MsAuthError {
    log::warn!("Microsoft {operation} failed (network error)");
    error("Could not reach Microsoft. Try again.".into())
}

fn auth_http_error(
    status: reqwest::StatusCode,
    operation: &str,
    error: fn(String) -> MsAuthError,
) -> MsAuthError {
    log::warn!(
        "Microsoft {operation} failed (HTTP {}, request rejected)",
        status.as_u16()
    );
    error(format!(
        "Microsoft rejected the {operation} (HTTP {})",
        status.as_u16()
    ))
}

async fn read_auth_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    operation: &str,
    error: fn(String) -> MsAuthError,
) -> Result<T, MsAuthError> {
    let status = response.status();
    if !status.is_success() {
        return Err(auth_http_error(status, operation, error));
    }
    response.json().await.map_err(|_| {
        log::warn!(
            "Microsoft {operation} failed (HTTP {}, invalid response)",
            status.as_u16()
        );
        error(format!(
            "Microsoft returned an invalid response to the {operation}"
        ))
    })
}

pub async fn start_device_code_flow(
    http: &reqwest::Client,
    config: &MicrosoftAuthConfig,
) -> Result<DeviceCodeResponse, MsAuthError> {
    let resp = http
        .post(&config.device_code_url())
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("scope", &config.scope_string()),
        ])
        .send()
        .await
        .map_err(|_| auth_network_error("device code request", MsAuthError::Network))?;

    read_auth_response(resp, "device code request", MsAuthError::Unexpected).await
}

pub async fn poll_device_code_token(
    http: &reqwest::Client,
    config: &MicrosoftAuthConfig,
    device_code: &str,
) -> Result<PollResult, MsAuthError> {
    let resp = http
        .post(&config.token_url())
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
        ])
        .send()
        .await
        .map_err(|_| auth_network_error("token poll", MsAuthError::Network))?;

    let status = resp.status();
    if status.is_success() {
        return read_auth_response(resp, "token poll", MsAuthError::Unexpected)
            .await
            .map(PollResult::Completed);
    }

    let payload = resp.json::<serde_json::Value>().await.ok();
    token_poll_error(
        status,
        payload.as_ref().and_then(|value| value["error"].as_str()),
    )
}

fn token_poll_error(
    status: reqwest::StatusCode,
    error_code: Option<&str>,
) -> Result<PollResult, MsAuthError> {
    match error_code {
        Some("authorization_pending" | "slow_down") => Ok(PollResult::Pending),
        Some("expired_token") => Ok(PollResult::Expired),
        Some("authorization_declined") => Ok(PollResult::Denied),
        Some("consent_required" | "interaction_required") => {
            log::warn!(
                "Microsoft token poll failed (HTTP {}, consent required)",
                status.as_u16()
            );
            Err(MsAuthError::ConsentRequired)
        }
        _ => Err(auth_http_error(
            status,
            "token poll",
            MsAuthError::Unexpected,
        )),
    }
}

pub async fn refresh_access_token(
    http: &reqwest::Client,
    config: &MicrosoftAuthConfig,
    refresh_token: &str,
) -> Result<TokenResponse, MsAuthError> {
    let resp = http
        .post(&config.token_url())
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", &config.scope_string()),
        ])
        .send()
        .await
        .map_err(|_| auth_network_error("token refresh", MsAuthError::TokenRefreshFailed))?;

    read_auth_response(resp, "token refresh", MsAuthError::TokenRefreshFailed).await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "userPrincipalName", default)]
    pub email: Option<String>,
}

pub async fn fetch_user_profile(
    http: &reqwest::Client,
    access_token: &str,
) -> Result<UserProfile, MsAuthError> {
    let resp = http
        .get("https://graph.microsoft.com/v1.0/me?$select=id,displayName,userPrincipalName")
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(|_| auth_network_error("profile request", MsAuthError::Network))?;

    read_auth_response(resp, "profile request", MsAuthError::Unexpected).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn synthetic_response(status: &str, body: &str) -> reqwest::Response {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            stream.write_all(reply.as_bytes()).await.unwrap();
        });
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}/"))
            .send()
            .await
            .unwrap();
        server.await.unwrap();
        response
    }

    #[tokio::test]
    async fn auth_errors_exclude_provider_details_from_rejections_and_malformed_successes() {
        let body = r#"{"error":"synthetic-provider-detail","error_description":"synthetic trace"}"#;
        let response = synthetic_response("400 Bad Request", body).await;
        let error = read_auth_response::<TokenResponse>(
            response,
            "token refresh",
            MsAuthError::TokenRefreshFailed,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Token refresh failed: Microsoft rejected the token refresh (HTTP 400)"
        );
        let response =
            synthetic_response("200 OK", r#"{"id":["synthetic-provider-detail"]}"#).await;
        let error =
            read_auth_response::<UserProfile>(response, "profile request", MsAuthError::Unexpected)
                .await
                .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Unexpected auth error: Microsoft returned an invalid response to the profile request"
        );
    }

    #[test]
    fn token_poll_errors_preserve_known_outcomes_and_hide_unknown_codes() {
        let status = reqwest::StatusCode::BAD_REQUEST;
        for code in ["authorization_pending", "slow_down"] {
            assert!(matches!(
                token_poll_error(status, Some(code)),
                Ok(PollResult::Pending)
            ));
        }
        assert!(matches!(
            token_poll_error(status, Some("expired_token")),
            Ok(PollResult::Expired)
        ));
        assert!(matches!(
            token_poll_error(status, Some("authorization_declined")),
            Ok(PollResult::Denied)
        ));
        assert!(matches!(
            token_poll_error(status, Some("consent_required")),
            Err(MsAuthError::ConsentRequired)
        ));
        let error = token_poll_error(status, Some("synthetic-provider-detail")).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Unexpected auth error: Microsoft rejected the token poll (HTTP 400)"
        );
    }

    #[test]
    fn default_config_has_expected_client_and_tenant() {
        let config = MicrosoftAuthConfig::default();
        assert_eq!(config.client_id, CLAWSCRIBE_CLIENT_ID);
        // Multi-tenant: sign-in targets the organizations authority, not a
        // single tenant GUID, so any work/school tenant can sign in.
        assert_eq!(config.tenant_id, DEFAULT_AUTHORITY);
        assert_eq!(config.tenant_id, "organizations");
        assert!(config.scopes.contains(&"Notes.ReadWrite".to_string()));
        assert!(config.scopes.contains(&"Files.ReadWrite".to_string()));
        assert!(config.scopes.contains(&"offline_access".to_string()));
    }

    #[test]
    fn scope_string_is_space_separated() {
        let config = MicrosoftAuthConfig::default();
        let s = config.scope_string();
        assert!(s.contains("User.Read"));
        assert!(s.contains(' '));
        assert!(!s.contains(','));
    }

    #[test]
    fn device_code_url_contains_tenant() {
        let config = MicrosoftAuthConfig::default();
        assert!(config.device_code_url().contains(DEFAULT_AUTHORITY));
        assert!(config.device_code_url().ends_with("/devicecode"));
    }
}
