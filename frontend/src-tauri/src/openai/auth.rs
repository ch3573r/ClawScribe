use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};

use crate::{database::repositories::setting::SettingsRepository, state::AppState};

const OPENAI_AUTH_REFERENCE_URL: &str =
    "https://developers.openai.com/api/reference/overview#authentication";
const OPENAI_OAUTH_UNSUPPORTED_REASON: &str =
    "Public OpenAI OAuth PKCE metadata alone cannot authenticate OpenAI API requests in ClawScribe. Use direct OpenAI API-key auth, or configure a standalone OpenAI-compatible managed endpoint that owns OAuth and accepts bearer-authenticated chat/completions requests.";
const OPENCLAW_CODEX_MANAGED_MESSAGE: &str =
    "Optional OpenClaw managed auth is configured. ClawScribe sends requests to the configured OpenClaw endpoint and does not store ChatGPT or Codex tokens locally.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenAIAuthMode {
    Disabled,
    ApiKey,
    OpenClawCodexManaged,
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
    pub openclaw_codex_managed: Option<OpenAIOpenClawCodexManagedConfig>,
    #[serde(default)]
    pub oauth_pkce: Option<OpenAIOAuthPkceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIOpenClawCodexManagedConfig {
    pub endpoint: String,
    #[serde(default)]
    pub status_endpoint: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIAuthStatus {
    pub mode: OpenAIAuthMode,
    pub configured: bool,
    pub api_key_present: bool,
    pub openclaw_codex_managed_configured: bool,
    pub openclaw_codex_endpoint_present: bool,
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
    pub openclaw_codex_managed: Option<OpenAIOpenClawCodexManagedConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_pkce: Option<OpenAIOAuthPkceConfig>,
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

pub(crate) fn validate_url_field(label: &str, value: &str) -> Result<(), String> {
    super::secret_destination::validate_secret_destination(value, false)
        .map_err(|error| format!("{label}: {error}"))
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

fn normalize_openclaw_codex_managed_config(
    config: OpenAIOpenClawCodexManagedConfig,
) -> Result<OpenAIOpenClawCodexManagedConfig, String> {
    let endpoint = config.endpoint.trim();
    if endpoint.is_empty() {
        return Err(
            "OpenClaw/Codex managed auth endpoint is required for openclaw_codex_managed mode"
                .to_string(),
        );
    }
    validate_url_field("OpenClaw/Codex managed auth endpoint", endpoint)?;

    let status_endpoint = config.status_endpoint.and_then(|endpoint| {
        let endpoint = endpoint.trim();
        if endpoint.is_empty() {
            None
        } else {
            Some(endpoint.to_string())
        }
    });
    if let Some(status_endpoint) = status_endpoint.as_deref() {
        validate_url_field(
            "OpenClaw/Codex managed auth status endpoint",
            status_endpoint,
        )?;
    }

    Ok(OpenAIOpenClawCodexManagedConfig {
        endpoint: endpoint.to_string(),
        status_endpoint,
        label: config
            .label
            .and_then(|label| (!label.trim().is_empty()).then(|| label.trim().to_string())),
    })
}

fn normalize_auth_config(config: OpenAIAuthConfig) -> Result<OpenAIAuthConfig, String> {
    match config.mode {
        OpenAIAuthMode::Disabled => Ok(OpenAIAuthConfig {
            mode: OpenAIAuthMode::Disabled,
            openclaw_codex_managed: None,
            oauth_pkce: None,
        }),
        OpenAIAuthMode::ApiKey => Ok(OpenAIAuthConfig {
            mode: OpenAIAuthMode::ApiKey,
            openclaw_codex_managed: None,
            oauth_pkce: None,
        }),
        OpenAIAuthMode::OpenClawCodexManaged => {
            let managed = config.openclaw_codex_managed.ok_or_else(|| {
                "OpenClaw/Codex managed auth configuration is required for openclaw_codex_managed mode"
                    .to_string()
            })?;

            Ok(OpenAIAuthConfig {
                mode: OpenAIAuthMode::OpenClawCodexManaged,
                openclaw_codex_managed: Some(normalize_openclaw_codex_managed_config(managed)?),
                oauth_pkce: None,
            })
        }
        OpenAIAuthMode::OauthPkce => {
            let oauth_pkce = config.oauth_pkce.ok_or_else(|| {
                "OAuth PKCE configuration is required for oauth_pkce mode".to_string()
            })?;

            Ok(OpenAIAuthConfig {
                mode: OpenAIAuthMode::OauthPkce,
                openclaw_codex_managed: None,
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
        openclaw_codex_managed_configured: false,
        openclaw_codex_endpoint_present: false,
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
        openclaw_codex_managed: None,
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
                openclaw_codex_managed_configured: false,
                openclaw_codex_endpoint_present: false,
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
                openclaw_codex_managed: None,
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
            OpenAIAuthMode::OpenClawCodexManaged => {
                let endpoint_present = config
                    .openclaw_codex_managed
                    .as_ref()
                    .map(|managed| !managed.endpoint.trim().is_empty())
                    .unwrap_or(false);
                OpenAIAuthStatus {
                    mode: OpenAIAuthMode::OpenClawCodexManaged,
                    configured: endpoint_present,
                    api_key_present,
                    openclaw_codex_managed_configured: endpoint_present,
                    openclaw_codex_endpoint_present: endpoint_present,
                    oauth_pkce_configured: false,
                    oauth_browser_launch_ready: false,
                    oauth_device_flow_configured: false,
                    can_authenticate_requests: endpoint_present,
                    requires_user_action: !endpoint_present,
                    source: "openai_auth_config".to_string(),
                    message: if endpoint_present {
                        OPENCLAW_CODEX_MANAGED_MESSAGE.to_string()
                    } else {
                        "OpenClaw/Codex managed auth mode is selected, but no endpoint is configured."
                            .to_string()
                    },
                    next_action: if endpoint_present {
                        "Use the configured OpenClaw endpoint for ChatGPT/Codex-authenticated processing; do not store ChatGPT tokens in ClawScribe."
                            .to_string()
                    } else {
                        "Configure an OpenClaw/Codex managed auth endpoint, or select API-key auth."
                            .to_string()
                    },
                    request_authentication: if endpoint_present {
                        "openclaw_codex_managed".to_string()
                    } else {
                        "missing_openclaw_codex_endpoint".to_string()
                    },
                    auth_reference_url: openai_auth_reference_url(),
                    unsupported_reason: None,
                    openclaw_codex_managed: config.openclaw_codex_managed,
                    oauth_pkce: None,
                }
            }
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
                    openclaw_codex_managed_configured: false,
                    openclaw_codex_endpoint_present: false,
                    oauth_pkce_configured,
                    oauth_browser_launch_ready: false,
                    oauth_device_flow_configured,
                    can_authenticate_requests: false,
                    requires_user_action: true,
                    source: "openai_auth_config".to_string(),
                    message: if oauth_pkce_configured {
                        "Public OpenAI OAuth PKCE metadata is configured, but it is not enough by itself to authenticate OpenAI API requests"
                            .to_string()
                    } else {
                        "Public OpenAI OAuth PKCE mode is selected, but metadata is incomplete"
                            .to_string()
                    },
                    next_action: if oauth_pkce_configured {
                        "Use direct API-key auth, or select a standalone OpenAI-compatible managed endpoint that owns OAuth."
                            .to_string()
                    } else {
                        "Select API-key auth, or configure a standalone OpenAI-compatible managed endpoint."
                            .to_string()
                    },
                    request_authentication: "unsupported_oauth_pkce".to_string(),
                    auth_reference_url: openai_auth_reference_url(),
                    unsupported_reason: unsupported_reason(),
                    openclaw_codex_managed: None,
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
            openclaw_codex_managed_configured: false,
            openclaw_codex_endpoint_present: false,
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
            openclaw_codex_managed: None,
            oauth_pkce: None,
        },
    }
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
/// ChatGPT/Codex tokens, OAuth client secrets, and OAuth tokens are intentionally
/// not accepted or stored here. OpenClaw is an optional managed endpoint, not the
/// OpenAI auth foundation for the distributable desktop app.
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

#[cfg(test)]
mod tests {
    #[test]
    fn secret_destinations_require_tls_except_loopback() {
        for url in [
            "https://example.com/api",
            "http://localhost/api",
            "http://127.0.0.1/api",
            "http://[::1]/api",
        ] {
            assert!(super::validate_url_field("Endpoint", url).is_ok());
        }
        for url in [
            "http://example.com/api",
            "http://localhost.example.com/api",
            "ftp://example.com",
        ] {
            assert!(super::validate_url_field("Endpoint", url).is_err());
        }
    }

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

    fn managed_config() -> OpenAIOpenClawCodexManagedConfig {
        OpenAIOpenClawCodexManagedConfig {
            endpoint: " http://127.0.0.1:41980/openclaw/codex/auth/process ".to_string(),
            status_endpoint: Some("https://openclaw.example.test/codex/auth/status ".to_string()),
            label: Some(" OpenClaw desktop bridge ".to_string()),
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
                openclaw_codex_managed: None,
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
    fn openclaw_codex_managed_auth_is_request_ready_without_storing_chatgpt_tokens() {
        let status = build_openai_auth_status(
            Some(OpenAIAuthConfig {
                mode: OpenAIAuthMode::OpenClawCodexManaged,
                openclaw_codex_managed: Some(managed_config()),
                oauth_pkce: None,
            }),
            None,
        );

        assert_eq!(status.mode, OpenAIAuthMode::OpenClawCodexManaged);
        assert!(status.configured);
        assert!(status.openclaw_codex_managed_configured);
        assert!(status.openclaw_codex_endpoint_present);
        assert!(status.can_authenticate_requests);
        assert_eq!(status.request_authentication, "openclaw_codex_managed");
        assert!(status.unsupported_reason.is_none());
        assert!(status
            .message
            .contains("does not store ChatGPT or Codex tokens locally"));
    }

    #[test]
    fn openclaw_codex_managed_normalization_trims_endpoint_metadata() {
        let config = normalize_auth_config(OpenAIAuthConfig {
            mode: OpenAIAuthMode::OpenClawCodexManaged,
            openclaw_codex_managed: Some(managed_config()),
            oauth_pkce: Some(oauth_config()),
        })
        .expect("valid managed auth config");

        let managed = config.openclaw_codex_managed.expect("managed auth config");
        assert_eq!(
            managed.endpoint,
            "http://127.0.0.1:41980/openclaw/codex/auth/process"
        );
        assert_eq!(
            managed.status_endpoint.as_deref(),
            Some("https://openclaw.example.test/codex/auth/status")
        );
        assert_eq!(managed.label.as_deref(), Some("OpenClaw desktop bridge"));
        assert!(config.oauth_pkce.is_none());
    }

    #[test]
    fn openclaw_codex_managed_requires_https_for_non_localhost_endpoint() {
        let mut config = managed_config();
        config.endpoint = "http://openclaw.example.test/codex/auth/process".to_string();

        let error = normalize_auth_config(OpenAIAuthConfig {
            mode: OpenAIAuthMode::OpenClawCodexManaged,
            openclaw_codex_managed: Some(config),
            oauth_pkce: None,
        })
        .expect_err("non-localhost http endpoint should fail");

        assert!(error.contains("HTTPS"));
    }

    #[test]
    fn public_oauth_pkce_metadata_is_not_reported_as_request_ready() {
        let status = build_openai_auth_status(
            Some(OpenAIAuthConfig {
                mode: OpenAIAuthMode::OauthPkce,
                openclaw_codex_managed: None,
                oauth_pkce: Some(oauth_config()),
            }),
            None,
        );

        assert_eq!(status.mode, OpenAIAuthMode::OauthPkce);
        assert!(status.configured);
        assert!(status.oauth_pkce_configured);
        assert!(!status.oauth_browser_launch_ready);
        assert!(status.oauth_device_flow_configured);
        assert!(!status.can_authenticate_requests);
        assert!(status.unsupported_reason.is_some());
        assert!(status
            .unsupported_reason
            .as_deref()
            .unwrap_or_default()
            .contains("standalone OpenAI-compatible managed endpoint"));
    }

    #[test]
    fn oauth_pkce_normalization_trims_public_metadata() {
        let config = normalize_auth_config(OpenAIAuthConfig {
            mode: OpenAIAuthMode::OauthPkce,
            openclaw_codex_managed: Some(managed_config()),
            oauth_pkce: Some(oauth_config()),
        })
        .expect("valid oauth config");

        assert!(config.openclaw_codex_managed.is_none());
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
            openclaw_codex_managed: None,
            oauth_pkce: Some(config),
        })
        .expect_err("non-localhost http endpoint should fail");

        assert!(error.contains("HTTPS"));
    }
}
