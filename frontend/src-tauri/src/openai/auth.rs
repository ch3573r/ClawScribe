use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Duration, Utc};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Runtime};
use url::Url;

use crate::{database::repositories::setting::SettingsRepository, state::AppState};

const OPENAI_AUTH_REFERENCE_URL: &str =
    "https://developers.openai.com/api/reference/overview#authentication";
const OPENAI_OAUTH_UNSUPPORTED_REASON: &str =
    "OpenAI API request authentication is available here with API-key bearer credentials. OAuth PKCE metadata can be prepared for a future OpenAI OAuth app, but this build does not have official OpenAI OAuth endpoints or a token exchange/storage path for API requests.";
const PKCE_CODE_CHALLENGE_METHOD: &str = "S256";
const PKCE_AUTH_REQUEST_TTL_MINUTES: i64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenAIAuthMode {
    Disabled,
    ApiKey,
    OauthPkce,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIOAuthPkceConfig {
    pub client_id: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub redirect_uri: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub device_authorization_endpoint: Option<String>,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIAuthConfig {
    pub mode: OpenAIAuthMode,
    #[serde(default)]
    pub oauth_pkce: Option<OpenAIOAuthPkceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIAuthStatus {
    pub mode: OpenAIAuthMode,
    pub configured: bool,
    pub api_key_present: bool,
    pub oauth_pkce_configured: bool,
    pub oauth_browser_launch_ready: bool,
    pub oauth_device_flow_configured: bool,
    pub can_authenticate_requests: bool,
    pub requires_user_action: bool,
    pub source: String,
    pub message: String,
    pub next_action: String,
    pub request_authentication: String,
    pub auth_reference_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unsupported_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_pkce: Option<OpenAIOAuthPkceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIOAuthPkceAuthorizationRequest {
    pub authorization_url: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub state: String,
    pub nonce: String,
    pub code_verifier: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub expires_at: String,
    pub token_exchange_supported: bool,
    pub unsupported_reason: String,
}

fn is_present(value: Option<&str>) -> bool {
    value.map(|v| !v.trim().is_empty()).unwrap_or(false)
}

fn parse_openai_auth_config(json: Option<String>) -> Result<Option<OpenAIAuthConfig>, String> {
    json.map(|raw| {
        if raw.trim().is_empty() {
            return Ok(None);
        }

        serde_json::from_str::<OpenAIAuthConfig>(&raw)
            .map(Some)
            .map_err(|e| format!("Invalid OpenAI auth configuration JSON: {}", e))
    })
    .transpose()
    .map(|parsed| parsed.flatten())
}

fn validate_url_field(label: &str, value: &str) -> Result<(), String> {
    let parsed = Url::parse(value).map_err(|e| format!("{} must be a valid URL: {}", label, e))?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http"
            if parsed.host_str() == Some("localhost") || parsed.host_str() == Some("127.0.0.1") =>
        {
            Ok(())
        }
        "http" => Err(format!(
            "{} must use https unless it targets localhost",
            label
        )),
        _ => Err(format!("{} must use http or https", label)),
    }
}

fn normalize_oauth_pkce_config(
    config: OpenAIOAuthPkceConfig,
) -> Result<OpenAIOAuthPkceConfig, String> {
    let client_id = config.client_id.trim();
    if client_id.is_empty() {
        return Err("OAuth client ID is required for oauth_pkce mode".to_string());
    }

    let authorization_endpoint = config.authorization_endpoint.trim();
    let token_endpoint = config.token_endpoint.trim();
    let redirect_uri = config.redirect_uri.trim();
    validate_url_field("Authorization endpoint", authorization_endpoint)?;
    validate_url_field("Token endpoint", token_endpoint)?;
    validate_url_field("Redirect URI", redirect_uri)?;
    if let Some(device_authorization_endpoint) = config.device_authorization_endpoint.as_deref() {
        let endpoint = device_authorization_endpoint.trim();
        if !endpoint.is_empty() {
            validate_url_field("Device authorization endpoint", endpoint)?;
        }
    }

    let scopes = config
        .scopes
        .into_iter()
        .map(|scope| scope.trim().to_string())
        .filter(|scope| !scope.is_empty())
        .collect::<Vec<_>>();

    Ok(OpenAIOAuthPkceConfig {
        client_id: client_id.to_string(),
        authorization_endpoint: authorization_endpoint.to_string(),
        token_endpoint: token_endpoint.to_string(),
        redirect_uri: redirect_uri.to_string(),
        scopes,
        device_authorization_endpoint: config.device_authorization_endpoint.and_then(|endpoint| {
            (!endpoint.trim().is_empty()).then(|| endpoint.trim().to_string())
        }),
        issuer: config
            .issuer
            .and_then(|issuer| (!issuer.trim().is_empty()).then(|| issuer.trim().to_string())),
        audience: config.audience.and_then(|audience| {
            (!audience.trim().is_empty()).then(|| audience.trim().to_string())
        }),
    })
}

fn normalize_auth_config(config: OpenAIAuthConfig) -> Result<OpenAIAuthConfig, String> {
    match config.mode {
        OpenAIAuthMode::Disabled => Ok(OpenAIAuthConfig {
            mode: OpenAIAuthMode::Disabled,
            oauth_pkce: None,
        }),
        OpenAIAuthMode::ApiKey => Ok(OpenAIAuthConfig {
            mode: OpenAIAuthMode::ApiKey,
            oauth_pkce: None,
        }),
        OpenAIAuthMode::OauthPkce => {
            let oauth_pkce = config.oauth_pkce.ok_or_else(|| {
                "OAuth PKCE configuration is required for oauth_pkce mode".to_string()
            })?;

            Ok(OpenAIAuthConfig {
                mode: OpenAIAuthMode::OauthPkce,
                oauth_pkce: Some(normalize_oauth_pkce_config(oauth_pkce)?),
            })
        }
    }
}

fn openai_auth_reference_url() -> String {
    OPENAI_AUTH_REFERENCE_URL.to_string()
}

fn unsupported_reason() -> Option<String> {
    Some(OPENAI_OAUTH_UNSUPPORTED_REASON.to_string())
}

fn api_key_ready_status(api_key_present: bool, source: &str, message: String) -> OpenAIAuthStatus {
    OpenAIAuthStatus {
        mode: OpenAIAuthMode::ApiKey,
        configured: api_key_present,
        api_key_present,
        oauth_pkce_configured: false,
        oauth_browser_launch_ready: false,
        oauth_device_flow_configured: false,
        can_authenticate_requests: api_key_present,
        requires_user_action: !api_key_present,
        source: source.to_string(),
        message,
        next_action: if api_key_present {
            "OpenAI requests can use the stored API key.".to_string()
        } else {
            "Save an OpenAI API key before using OpenAI summaries.".to_string()
        },
        request_authentication: if api_key_present {
            "bearer_api_key".to_string()
        } else {
            "missing_api_key".to_string()
        },
        auth_reference_url: openai_auth_reference_url(),
        unsupported_reason: None,
        oauth_pkce: None,
    }
}

fn build_openai_auth_status(
    stored_config: Option<OpenAIAuthConfig>,
    legacy_api_key: Option<&str>,
) -> OpenAIAuthStatus {
    let api_key_present = is_present(legacy_api_key);

    match stored_config {
        Some(config) => match config.mode {
            OpenAIAuthMode::Disabled => OpenAIAuthStatus {
                mode: OpenAIAuthMode::Disabled,
                configured: false,
                api_key_present,
                oauth_pkce_configured: false,
                oauth_browser_launch_ready: false,
                oauth_device_flow_configured: false,
                can_authenticate_requests: false,
                requires_user_action: true,
                source: "openai_auth_config".to_string(),
                message: "OpenAI auth is disabled in auth-mode configuration".to_string(),
                next_action:
                    "Choose API-key auth and save an OpenAI API key to enable OpenAI summaries."
                        .to_string(),
                request_authentication: "disabled".to_string(),
                auth_reference_url: openai_auth_reference_url(),
                unsupported_reason: None,
                oauth_pkce: None,
            },
            OpenAIAuthMode::ApiKey => api_key_ready_status(
                api_key_present,
                "openai_auth_config",
                if api_key_present {
                    "OpenAI API key auth is configured through the existing settings path"
                        .to_string()
                } else {
                    "OpenAI API key auth is selected, but no API key is stored".to_string()
                },
            ),
            OpenAIAuthMode::OauthPkce => {
                let oauth_pkce_configured = config.oauth_pkce.is_some();
                let oauth_device_flow_configured = config
                    .oauth_pkce
                    .as_ref()
                    .and_then(|oauth| oauth.device_authorization_endpoint.as_ref())
                    .map(|endpoint| !endpoint.trim().is_empty())
                    .unwrap_or(false);
                OpenAIAuthStatus {
                    mode: OpenAIAuthMode::OauthPkce,
                    configured: oauth_pkce_configured,
                    api_key_present,
                    oauth_pkce_configured,
                    oauth_browser_launch_ready: oauth_pkce_configured,
                    oauth_device_flow_configured,
                    can_authenticate_requests: false,
                    requires_user_action: true,
                    source: "openai_auth_config".to_string(),
                    message: if oauth_pkce_configured {
                        "OpenAI OAuth PKCE metadata is configured, but token exchange is not implemented"
                            .to_string()
                    } else {
                        "OpenAI OAuth PKCE mode is selected, but metadata is incomplete".to_string()
                    },
                    next_action: if oauth_pkce_configured {
                        "Use api_prepare_openai_oauth_pkce_authorization only to prepare browser-launch metadata; API requests still require API-key auth in this build."
                            .to_string()
                    } else {
                        "Provide OpenAI OAuth PKCE client metadata, or select API-key auth."
                            .to_string()
                    },
                    request_authentication: "unsupported_oauth_pkce".to_string(),
                    auth_reference_url: openai_auth_reference_url(),
                    unsupported_reason: unsupported_reason(),
                    oauth_pkce: config.oauth_pkce,
                }
            }
        },
        None if api_key_present => api_key_ready_status(
            true,
            "legacy_api_key",
            "OpenAI API key auth is configured through the existing settings path".to_string(),
        ),
        None => OpenAIAuthStatus {
            mode: OpenAIAuthMode::Disabled,
            configured: false,
            api_key_present: false,
            oauth_pkce_configured: false,
            oauth_browser_launch_ready: false,
            oauth_device_flow_configured: false,
            can_authenticate_requests: false,
            requires_user_action: true,
            source: "not_configured".to_string(),
            message: "OpenAI auth is not configured".to_string(),
            next_action: "Save an OpenAI API key to enable OpenAI summaries.".to_string(),
            request_authentication: "not_configured".to_string(),
            auth_reference_url: openai_auth_reference_url(),
            unsupported_reason: None,
            oauth_pkce: None,
        },
    }
}

fn random_urlsafe_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn pkce_s256_challenge(code_verifier: &str) -> String {
    let digest = Sha256::digest(code_verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn build_oauth_authorization_request(
    oauth: OpenAIOAuthPkceConfig,
) -> Result<OpenAIOAuthPkceAuthorizationRequest, String> {
    let state = random_urlsafe_string(32);
    let nonce = random_urlsafe_string(32);
    let code_verifier = random_urlsafe_string(96);
    let code_challenge = pkce_s256_challenge(&code_verifier);
    let expires_at = (Utc::now() + Duration::minutes(PKCE_AUTH_REQUEST_TTL_MINUTES)).to_rfc3339();

    let mut authorization_url = Url::parse(&oauth.authorization_endpoint)
        .map_err(|e| format!("Authorization endpoint must be a valid URL: {}", e))?;
    {
        let mut pairs = authorization_url.query_pairs_mut();
        pairs
            .append_pair("response_type", "code")
            .append_pair("client_id", &oauth.client_id)
            .append_pair("redirect_uri", &oauth.redirect_uri)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", PKCE_CODE_CHALLENGE_METHOD)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce);

        if !oauth.scopes.is_empty() {
            pairs.append_pair("scope", &oauth.scopes.join(" "));
        }

        if let Some(audience) = oauth.audience.as_deref() {
            pairs.append_pair("audience", audience);
        }
    }

    Ok(OpenAIOAuthPkceAuthorizationRequest {
        authorization_url: authorization_url.to_string(),
        redirect_uri: oauth.redirect_uri,
        scopes: oauth.scopes,
        state,
        nonce,
        code_verifier,
        code_challenge,
        code_challenge_method: PKCE_CODE_CHALLENGE_METHOD.to_string(),
        expires_at,
        token_exchange_supported: false,
        unsupported_reason: OPENAI_OAUTH_UNSUPPORTED_REASON.to_string(),
    })
}

/// Reports the configured OpenAI auth mode without returning secrets.
#[tauri::command]
pub async fn api_get_openai_auth_status<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<OpenAIAuthStatus, String> {
    let pool = state.db_manager.pool();
    let stored_config = parse_openai_auth_config(
        SettingsRepository::get_openai_auth_config(pool)
            .await
            .map_err(|e| format!("Failed to read OpenAI auth configuration: {}", e))?,
    )?;
    let api_key = SettingsRepository::get_api_key(pool, "openai")
        .await
        .map_err(|e| format!("Failed to read OpenAI API key status: {}", e))?;

    Ok(build_openai_auth_status(stored_config, api_key.as_deref()))
}

/// Saves OpenAI auth-mode metadata. API keys still use the existing settings path.
/// OAuth client secrets and tokens are intentionally not accepted or stored here.
#[tauri::command]
pub async fn api_save_openai_auth_config<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    config: OpenAIAuthConfig,
) -> Result<OpenAIAuthStatus, String> {
    let config = normalize_auth_config(config)?;
    let config_json = serde_json::to_string(&config)
        .map_err(|e| format!("Failed to serialize OpenAI auth configuration: {}", e))?;
    let pool = state.db_manager.pool();

    SettingsRepository::save_openai_auth_config(pool, &config_json)
        .await
        .map_err(|e| format!("Failed to save OpenAI auth configuration: {}", e))?;

    let api_key = SettingsRepository::get_api_key(pool, "openai")
        .await
        .map_err(|e| format!("Failed to read OpenAI API key status: {}", e))?;

    Ok(build_openai_auth_status(Some(config), api_key.as_deref()))
}

/// Clears only the auth-mode metadata. Existing legacy OpenAI API keys are not removed.
#[tauri::command]
pub async fn api_clear_openai_auth_config<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<OpenAIAuthStatus, String> {
    let pool = state.db_manager.pool();
    SettingsRepository::clear_openai_auth_config(pool)
        .await
        .map_err(|e| format!("Failed to clear OpenAI auth configuration: {}", e))?;

    let api_key = SettingsRepository::get_api_key(pool, "openai")
        .await
        .map_err(|e| format!("Failed to read OpenAI API key status: {}", e))?;

    Ok(build_openai_auth_status(None, api_key.as_deref()))
}

/// Prepares a real OAuth PKCE S256 browser authorization request from stored metadata.
/// This does not exchange codes, refresh tokens, or authenticate OpenAI API requests.
#[tauri::command]
pub async fn api_prepare_openai_oauth_pkce_authorization<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<OpenAIOAuthPkceAuthorizationRequest, String> {
    let pool = state.db_manager.pool();
    let stored_config = parse_openai_auth_config(
        SettingsRepository::get_openai_auth_config(pool)
            .await
            .map_err(|e| format!("Failed to read OpenAI auth configuration: {}", e))?,
    )?;

    let config =
        stored_config.ok_or_else(|| "OpenAI OAuth PKCE metadata is not configured".to_string())?;
    if config.mode != OpenAIAuthMode::OauthPkce {
        return Err("OpenAI auth mode is not oauth_pkce".to_string());
    }

    let oauth = config
        .oauth_pkce
        .ok_or_else(|| "OpenAI OAuth PKCE metadata is incomplete".to_string())?;

    build_oauth_authorization_request(oauth)
}

/// Explicit unsupported state for future callback wiring. No fake OAuth tokens are minted.
#[tauri::command]
pub async fn api_exchange_openai_oauth_pkce_code<R: Runtime>(
    _app: AppHandle<R>,
    _state: tauri::State<'_, AppState>,
    _code: String,
    _state_param: String,
    _code_verifier: String,
) -> Result<OpenAIAuthStatus, String> {
    Err(OPENAI_OAUTH_UNSUPPORTED_REASON.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oauth_config() -> OpenAIOAuthPkceConfig {
        OpenAIOAuthPkceConfig {
            client_id: " client-123 ".to_string(),
            authorization_endpoint: "https://auth.example.test/oauth/authorize ".to_string(),
            token_endpoint: "https://auth.example.test/oauth/token".to_string(),
            redirect_uri: "http://127.0.0.1:38451/openai/oauth/callback".to_string(),
            scopes: vec![" openai ".to_string(), "".to_string()],
            device_authorization_endpoint: Some(
                "https://auth.example.test/oauth/device".to_string(),
            ),
            issuer: Some(" ".to_string()),
            audience: Some(" api ".to_string()),
        }
    }

    #[test]
    fn legacy_api_key_reports_api_key_mode_without_stored_config() {
        let status = build_openai_auth_status(None, Some("sk-test"));

        assert_eq!(status.mode, OpenAIAuthMode::ApiKey);
        assert!(status.configured);
        assert!(status.can_authenticate_requests);
        assert_eq!(status.request_authentication, "bearer_api_key");
        assert_eq!(status.source, "legacy_api_key");
    }

    #[test]
    fn disabled_mode_overrides_legacy_key_in_status() {
        let status = build_openai_auth_status(
            Some(OpenAIAuthConfig {
                mode: OpenAIAuthMode::Disabled,
                oauth_pkce: None,
            }),
            Some("sk-test"),
        );

        assert_eq!(status.mode, OpenAIAuthMode::Disabled);
        assert!(!status.configured);
        assert!(status.api_key_present);
        assert!(!status.can_authenticate_requests);
    }

    #[test]
    fn oauth_pkce_metadata_is_not_reported_as_request_ready() {
        let status = build_openai_auth_status(
            Some(OpenAIAuthConfig {
                mode: OpenAIAuthMode::OauthPkce,
                oauth_pkce: Some(oauth_config()),
            }),
            None,
        );

        assert_eq!(status.mode, OpenAIAuthMode::OauthPkce);
        assert!(status.configured);
        assert!(status.oauth_pkce_configured);
        assert!(status.oauth_browser_launch_ready);
        assert!(status.oauth_device_flow_configured);
        assert!(!status.can_authenticate_requests);
        assert!(status.unsupported_reason.is_some());
    }

    #[test]
    fn oauth_pkce_normalization_trims_public_metadata() {
        let config = normalize_auth_config(OpenAIAuthConfig {
            mode: OpenAIAuthMode::OauthPkce,
            oauth_pkce: Some(oauth_config()),
        })
        .expect("valid oauth config");

        let oauth = config.oauth_pkce.expect("oauth config");
        assert_eq!(oauth.client_id, "client-123");
        assert_eq!(oauth.scopes, vec!["openai"]);
        assert_eq!(oauth.issuer, None);
        assert_eq!(oauth.audience.as_deref(), Some("api"));
        assert_eq!(
            oauth.device_authorization_endpoint.as_deref(),
            Some("https://auth.example.test/oauth/device")
        );
    }

    #[test]
    fn oauth_pkce_requires_https_for_non_localhost_endpoints() {
        let mut config = oauth_config();
        config.authorization_endpoint = "http://auth.example.test/oauth/authorize".to_string();

        let error = normalize_auth_config(OpenAIAuthConfig {
            mode: OpenAIAuthMode::OauthPkce,
            oauth_pkce: Some(config),
        })
        .expect_err("non-localhost http endpoint should fail");

        assert!(error.contains("must use https"));
    }

    #[test]
    fn pkce_authorization_request_uses_s256_without_claiming_token_exchange() {
        let config = normalize_oauth_pkce_config(oauth_config()).expect("valid oauth config");
        let request = build_oauth_authorization_request(config).expect("authorization request");

        assert!(request
            .authorization_url
            .starts_with("https://auth.example.test/oauth/authorize?"));
        assert!(request.authorization_url.contains("response_type=code"));
        assert!(request
            .authorization_url
            .contains("code_challenge_method=S256"));
        assert!(request.authorization_url.contains("scope=openai"));
        assert_eq!(request.code_challenge_method, "S256");
        assert_eq!(request.code_verifier.len(), 96);
        assert!(!request.token_exchange_supported);
        assert!(request.unsupported_reason.contains("OAuth PKCE metadata"));
    }
}
