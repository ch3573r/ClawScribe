//! Confluence Server/Data Center export.
//!
//! This targets self-hosted Confluence behind corporate network access. The PAT
//! is stored in the OS credential store and sent as a Bearer token only from the
//! Rust side; the frontend stores only non-sensitive destination settings.

use serde::{Deserialize, Serialize};
use std::time::Duration;

const SERVICE_NAME: &str = "net.rismondo.openclaw.clawscribe.confluence";
const ACCOUNT_NAME: &str = "default";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfluenceConnectionStatus {
    pub token_configured: bool,
    pub reachable: bool,
    pub user_display_name: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfluenceExportResponse {
    pub page_id: String,
    pub title: String,
    pub web_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfluenceUser {
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default, rename = "userKey")]
    user_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConfluenceLinks {
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    webui: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreatePageResponse {
    id: String,
    title: String,
    #[serde(default, rename = "_links")]
    links: Option<ConfluenceLinks>,
}

#[derive(Debug)]
enum ConfluenceError {
    MissingToken,
    Keyring(String),
    InvalidInput(String),
    Network(String),
    Http(u16, String),
    Parse(String),
}

impl std::fmt::Display for ConfluenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfluenceError::MissingToken => write!(f, "No Confluence PAT is saved."),
            ConfluenceError::Keyring(e) => write!(f, "Credential store error: {e}"),
            ConfluenceError::InvalidInput(e) => write!(f, "{e}"),
            ConfluenceError::Network(e) => write!(f, "Network error: {e}"),
            ConfluenceError::Http(status, body) => {
                write!(f, "Confluence returned HTTP {status}: {body}")
            }
            ConfluenceError::Parse(e) => write!(f, "Failed to parse Confluence response: {e}"),
        }
    }
}

fn credential_entry() -> Result<keyring::Entry, ConfluenceError> {
    keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME)
        .map_err(|e| ConfluenceError::Keyring(e.to_string()))
}

fn save_pat_to_keyring(pat: &str) -> Result<(), ConfluenceError> {
    credential_entry()?
        .set_password(pat)
        .map_err(|e| ConfluenceError::Keyring(e.to_string()))
}

fn load_pat_from_keyring() -> Result<Option<String>, ConfluenceError> {
    match credential_entry()?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(ConfluenceError::Keyring(e.to_string())),
    }
}

fn delete_pat_from_keyring() -> Result<(), ConfluenceError> {
    match credential_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(ConfluenceError::Keyring(e.to_string())),
    }
}

fn normalize_base_url(raw: &str) -> Result<String, ConfluenceError> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(ConfluenceError::InvalidInput(
            "Confluence base URL is required.".into(),
        ));
    }
    if !trimmed.starts_with("https://") && !trimmed.starts_with("http://") {
        return Err(ConfluenceError::InvalidInput(
            "Confluence base URL must start with http:// or https://.".into(),
        ));
    }
    let without_rest = trimmed.trim_end_matches("/rest/api");
    Ok(without_rest.to_string())
}

fn api_url(base_url: &str, path: &str) -> Result<String, ConfluenceError> {
    Ok(format!("{}{}", normalize_base_url(base_url)?, path))
}

fn http_client() -> Result<reqwest::Client, ConfluenceError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| ConfluenceError::Network(e.to_string()))
}

fn truncate_error_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() > 800 {
        format!("{}...", trimmed.chars().take(800).collect::<String>())
    } else {
        trimmed.to_string()
    }
}

async fn response_text_or_error(resp: reqwest::Response) -> Result<String, ConfluenceError> {
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| ConfluenceError::Network(e.to_string()))?;
    if status.is_success() {
        Ok(text)
    } else {
        Err(ConfluenceError::Http(
            status.as_u16(),
            truncate_error_body(&text),
        ))
    }
}

#[tauri::command]
pub fn confluence_save_pat(pat: String) -> Result<(), String> {
    let pat = pat.trim();
    if pat.is_empty() {
        return Err("Confluence PAT must not be empty.".to_string());
    }
    save_pat_to_keyring(pat).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn confluence_clear_pat() -> Result<(), String> {
    delete_pat_from_keyring().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn confluence_connection_status(
    base_url: String,
) -> Result<ConfluenceConnectionStatus, String> {
    let token = match load_pat_from_keyring().map_err(|e| e.to_string())? {
        Some(t) => t,
        None => {
            return Ok(ConfluenceConnectionStatus {
                token_configured: false,
                reachable: false,
                user_display_name: None,
                message: "No Confluence PAT is saved.".to_string(),
            });
        }
    };

    let url = api_url(&base_url, "/rest/api/user/current").map_err(|e| e.to_string())?;
    let http = http_client().map_err(|e| e.to_string())?;
    let text = response_text_or_error(
        http.get(url)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ConfluenceError::Network(e.to_string()))
            .map_err(|e| e.to_string())?,
    )
    .await
    .map_err(|e| e.to_string())?;

    let user: ConfluenceUser = serde_json::from_str(&text)
        .map_err(|e| ConfluenceError::Parse(e.to_string()).to_string())?;
    let display = user
        .display_name
        .or(user.username)
        .or(user.user_key)
        .filter(|s| !s.trim().is_empty());

    Ok(ConfluenceConnectionStatus {
        token_configured: true,
        reachable: true,
        user_display_name: display.clone(),
        message: display
            .map(|name| format!("Connected as {name}."))
            .unwrap_or_else(|| "Connected to Confluence.".to_string()),
    })
}

#[tauri::command]
pub async fn confluence_export_page(
    base_url: String,
    space_key: String,
    parent_id: Option<String>,
    title: String,
    body_storage: String,
) -> Result<ConfluenceExportResponse, String> {
    let token = load_pat_from_keyring()
        .map_err(|e| e.to_string())?
        .ok_or(ConfluenceError::MissingToken)
        .map_err(|e| e.to_string())?;
    let space_key = space_key.trim();
    let title = title.trim();
    let body_storage = body_storage.trim();
    if space_key.is_empty() {
        return Err("Confluence space key is required.".to_string());
    }
    if title.is_empty() {
        return Err("Confluence page title is required.".to_string());
    }
    if body_storage.is_empty() {
        return Err("Confluence page body is empty.".to_string());
    }

    let mut payload = serde_json::json!({
        "type": "page",
        "title": title,
        "space": { "key": space_key },
        "body": {
            "storage": {
                "value": body_storage,
                "representation": "storage"
            }
        }
    });

    if let Some(parent) = parent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        payload["ancestors"] = serde_json::json!([{ "id": parent }]);
    }

    let url = api_url(&base_url, "/rest/api/content").map_err(|e| e.to_string())?;
    let http = http_client().map_err(|e| e.to_string())?;
    let text = response_text_or_error(
        http.post(url)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ConfluenceError::Network(e.to_string()))
            .map_err(|e| e.to_string())?,
    )
    .await
    .map_err(|e| e.to_string())?;

    let created: CreatePageResponse = serde_json::from_str(&text)
        .map_err(|e| ConfluenceError::Parse(e.to_string()).to_string())?;

    let base = normalize_base_url(&base_url).map_err(|e| e.to_string())?;
    let web_url = created.links.and_then(|links| {
        let webui = links.webui?;
        let response_base = links.base.unwrap_or_else(|| base.clone());
        Some(format!("{}{}", response_base.trim_end_matches('/'), webui))
    });

    Ok(ConfluenceExportResponse {
        page_id: created.id,
        title: created.title,
        web_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_base_url_strips_rest_suffix_and_slashes() {
        assert_eq!(
            normalize_base_url("https://example.test/confluence/rest/api/").unwrap(),
            "https://example.test/confluence"
        );
        assert!(normalize_base_url("example.test/confluence").is_err());
    }
}
