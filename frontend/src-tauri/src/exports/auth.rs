//! Microsoft Entra ID device-code OAuth 2.0 flow.
//!
//! Implements the device-code grant against the v2.0 token endpoint. The flow
//! is: request a user code, show it in the UI, open the verification URL, poll
//! until the user completes sign-in, then exchange for access + refresh tokens.

use serde::{Deserialize, Serialize};

pub const CLAWSCRIBE_CLIENT_ID: &str = "4ab2ca8f-c2f1-45f3-b4ee-8bc9a511bcc8";

/// The tenant where the app is registered. Kept for reference; sign-in uses the
/// multi-tenant authority below so accounts from any work/school tenant can
/// sign in (the registration must be set to multi-tenant in Entra).
pub const RISMONDO_TENANT_ID: &str = "d0627577-cabb-4909-8ea1-c5d86abfd204";

/// Authority used for sign-in. `organizations` accepts any Entra work/school
/// tenant but not personal Microsoft accounts. (Use a specific tenant GUID to
/// lock to one org, or `common` to also allow personal accounts — but Planner
/// is unavailable for personal accounts.)
pub const DEFAULT_AUTHORITY: &str = "organizations";

// Least-privilege: every scope here must map to an endpoint the app actually
// calls. OneNote export -> Notes.*; Planner export -> Tasks.ReadWrite; calendar
// add-on -> Calendars.Read; sign-in -> User.Read; token refresh ->
// offline_access. Teams meeting detection is local (window/process scanning),
// so it needs no Graph scope. Don't add OnlineMeetings/Presence/Files unless a
// code path calls /me/onlineMeetings, /me/presence, or /me/drive respectively.
pub const DEFAULT_SCOPES: &[&str] = &[
    "User.Read",
    "Notes.ReadWrite",
    "Notes.Create",
    // Read/write notebooks the user can access beyond their own OneDrive
    // (Teams/SharePoint/shared notebooks). User-consentable.
    "Notes.ReadWrite.All",
    "Tasks.ReadWrite",
    "Calendars.Read",
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub message: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        .map_err(|e| MsAuthError::Network(e.to_string()))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(MsAuthError::Unexpected(format!(
            "Device code request failed: {body}"
        )));
    }

    resp.json::<DeviceCodeResponse>()
        .await
        .map_err(|e| MsAuthError::Unexpected(format!("Failed to parse device code response: {e}")))
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
        .map_err(|e| MsAuthError::Network(e.to_string()))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if status.is_success() {
        let token: TokenResponse = serde_json::from_str(&body)
            .map_err(|e| MsAuthError::Unexpected(format!("Failed to parse token: {e}")))?;
        return Ok(PollResult::Completed(token));
    }

    let error_code = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("error")?.as_str().map(String::from))
        .unwrap_or_default();

    match error_code.as_str() {
        "authorization_pending" | "slow_down" => Ok(PollResult::Pending),
        "expired_token" => Ok(PollResult::Expired),
        "authorization_declined" => Ok(PollResult::Denied),
        "consent_required" | "interaction_required" => Err(MsAuthError::ConsentRequired),
        _ => Err(MsAuthError::Unexpected(format!(
            "Token poll error: {error_code}"
        ))),
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
        .map_err(|e| MsAuthError::TokenRefreshFailed(e.to_string()))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(MsAuthError::TokenRefreshFailed(body));
    }

    resp.json::<TokenResponse>()
        .await
        .map_err(|e| MsAuthError::TokenRefreshFailed(e.to_string()))
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
        .map_err(|e| MsAuthError::Network(e.to_string()))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(MsAuthError::Unexpected(format!(
            "Failed to fetch profile: {body}"
        )));
    }

    resp.json::<UserProfile>()
        .await
        .map_err(|e| MsAuthError::Unexpected(format!("Failed to parse profile: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_expected_client_and_tenant() {
        let config = MicrosoftAuthConfig::default();
        assert_eq!(config.client_id, CLAWSCRIBE_CLIENT_ID);
        // Multi-tenant: sign-in targets the organizations authority, not a
        // single tenant GUID, so any work/school tenant can sign in.
        assert_eq!(config.tenant_id, DEFAULT_AUTHORITY);
        assert_eq!(config.tenant_id, "organizations");
        assert!(config.scopes.contains(&"Notes.ReadWrite".to_string()));
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
