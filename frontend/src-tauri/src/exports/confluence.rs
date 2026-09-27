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
    pub destination_problem: Option<String>,
    pub token_configured: bool,
    pub allow_unencrypted: bool,
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

#[derive(Serialize, Deserialize)]
struct BoundPat {
    #[serde(skip)]
    destination_problem: Option<String>,
    base_url: String,
    pat: String,
    #[serde(default)]
    allow_unencrypted: bool,
}

impl BoundPat {
    fn token_for(&self, requested: &str) -> Result<&str, ConfluenceError> {
        crate::openai::secret_destination::validate_secret_destination(
            requested,
            self.allow_unencrypted,
        )
        .map_err(ConfluenceError::InvalidInput)?;
        self.check_origin(requested)?;
        Ok(&self.pat)
    }

    fn check_origin(&self, requested: &str) -> Result<(), ConfluenceError> {
        let saved = url::Url::parse(&normalize_base_url(&self.base_url)?).map_err(|_| {
            ConfluenceError::InvalidInput("Invalid saved Confluence destination".into())
        })?;
        let requested = url::Url::parse(&normalize_base_url(requested)?)
            .map_err(|_| ConfluenceError::InvalidInput("Invalid Confluence destination".into()))?;
        if saved.origin() != requested.origin() {
            return Err(ConfluenceError::InvalidInput("Confluence destination changed. Save a PAT for this destination before connecting.".into()));
        }
        Ok(())
    }
}

fn save_pat_to_keyring(
    pat: &str,
    base_url: &str,
    allow_unencrypted: bool,
) -> Result<(), ConfluenceError> {
    crate::openai::secret_destination::validate_secret_destination(base_url, allow_unencrypted)
        .map_err(ConfluenceError::InvalidInput)?;
    let credential = BoundPat {
        destination_problem: None,
        allow_unencrypted,
        base_url: normalize_base_url(base_url)?,
        pat: pat.into(),
    };
    persist_binding(&credential)
}

// Migration preserves the existing binding, including endpoints needing repair.
fn persist_binding(credential: &BoundPat) -> Result<(), ConfluenceError> {
    let value = serde_json::to_string(credential).map_err(|_| {
        ConfluenceError::InvalidInput("Could not serialize Confluence credentials".into())
    })?;
    credential_entry()?
        .set_password(&value)
        .map_err(|e| ConfluenceError::Keyring(e.to_string()))
}

fn load_pat_from_keyring() -> Result<Option<BoundPat>, ConfluenceError> {
    match credential_entry()?.get_password() {
        Ok(value) => {
            let (credential, migrated) = parse_binding(&value)?;
            if migrated {
                persist_binding(&credential)?;
            }
            Ok(Some(credential))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(ConfluenceError::Keyring(e.to_string())),
    }
}

fn parse_binding(value: &str) -> Result<(BoundPat, bool), ConfluenceError> {
    let mut parsed: serde_json::Value = serde_json::from_str(value).map_err(|_| {
        ConfluenceError::InvalidInput(
            "Save your Confluence PAT again to bind it to this destination.".into(),
        )
    })?;
    let migrated =
        crate::openai::secret_destination::migrate_http_opt_in(&mut parsed, &["base_url"])
            .map_err(ConfluenceError::InvalidInput)?;
    let mut credential: BoundPat = serde_json::from_value(parsed)
        .map_err(|_| ConfluenceError::InvalidInput("Invalid saved Confluence binding".into()))?;
    credential.destination_problem =
        crate::openai::secret_destination::validate_secret_destination(
            &credential.base_url,
            credential.allow_unencrypted,
        )
        .err();
    Ok((credential, migrated))
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
    let parsed = url::Url::parse(trimmed)
        .map_err(|_| ConfluenceError::InvalidInput("Enter a valid Confluence base URL.".into()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(ConfluenceError::InvalidInput(
            "Use an HTTP or HTTPS URL without embedded credentials.".into(),
        ));
    }
    let without_rest = trimmed.trim_end_matches("/rest/api");
    Ok(without_rest.to_string())
}

fn api_url(base_url: &str, path: &str) -> Result<String, ConfluenceError> {
    Ok(format!("{}{}", normalize_base_url(base_url)?, path))
}

async fn http_client(base_url: &str, allow_unencrypted: bool) -> Result<reqwest::Client, String> {
    crate::openai::secret_destination::secret_client(
        reqwest::Client::builder().timeout(Duration::from_secs(30)),
        base_url,
        allow_unencrypted,
    )
    .await
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
pub fn confluence_save_pat(
    pat: String,
    base_url: String,
    allow_unencrypted: Option<bool>,
) -> Result<(), String> {
    let pat = pat.trim();
    if pat.is_empty() {
        return Err("Confluence PAT must not be empty.".to_string());
    }
    save_pat_to_keyring(pat, &base_url, allow_unencrypted.unwrap_or(false))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn confluence_settings_status(base_url: String) -> Result<ConfluenceConnectionStatus, String> {
    let binding = load_pat_from_keyring().map_err(|e| e.to_string())?;
    Ok(settings_status(binding.as_ref(), &base_url))
}

fn settings_status(binding: Option<&BoundPat>, base_url: &str) -> ConfluenceConnectionStatus {
    let mut status = ConfluenceConnectionStatus {
        destination_problem: None,
        token_configured: binding.is_some(),
        allow_unencrypted: false,
        reachable: false,
        user_display_name: None,
        message: "No Confluence PAT is saved.".into(),
    };
    if let Some(binding) = binding {
        status.destination_problem = binding
            .token_for(base_url)
            .err()
            .map(|error| error.to_string());
        // A changed destination must not inherit another server's opt-in.
        status.allow_unencrypted =
            binding.check_origin(base_url).is_ok() && binding.allow_unencrypted;
        status.message = match binding.token_for(base_url) {
            Ok(_) => "PAT saved. Test the connection to check availability.".into(),
            Err(error) => format!("Settings need attention: {error}"),
        };
    }
    status
}

#[tauri::command]
pub fn confluence_clear_pat() -> Result<(), String> {
    delete_pat_from_keyring().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn confluence_connection_status(
    base_url: String,
) -> Result<ConfluenceConnectionStatus, String> {
    let binding = load_pat_from_keyring().map_err(|e| e.to_string())?;
    let mut status = settings_status(binding.as_ref(), &base_url);
    let Some(token) = binding else {
        return Ok(status);
    };
    match check_connection(&token, &base_url).await {
        Ok(display) => {
            status.reachable = true;
            status.user_display_name = display.clone();
            status.message = display
                .map(|name| format!("Connected as {name}."))
                .unwrap_or_else(|| "Connected to Confluence.".into());
        }
        Err(error) => status.message = error,
    }
    Ok(status)
}

async fn check_connection(token: &BoundPat, base_url: &str) -> Result<Option<String>, String> {
    let allow_unencrypted = token.allow_unencrypted;
    let token = token.token_for(&base_url).map_err(|e| e.to_string())?;
    let url = api_url(&base_url, "/rest/api/user/current").map_err(|e| e.to_string())?;
    let http = http_client(&base_url, allow_unencrypted).await?;
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

    Ok(display)
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

    let allow_unencrypted = token.allow_unencrypted;
    let token = token.token_for(&base_url).map_err(|e| e.to_string())?;
    let url = api_url(&base_url, "/rest/api/content").map_err(|e| e.to_string())?;
    let http = http_client(&base_url, allow_unencrypted).await?;
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
    fn pat_is_bound_to_saved_origin() {
        let saved = BoundPat {
            destination_problem: None,
            base_url: "https://confluence.example.com/wiki".into(),
            pat: "test-pat".into(),
            allow_unencrypted: false,
        };
        assert!(saved
            .token_for("https://confluence.example.com/wiki")
            .is_ok());
        for destination in [
            "https://other.example.com",
            "https://confluence.example.com:444",
            "http://confluence.example.com",
        ] {
            assert!(saved.token_for(destination).is_err());
        }
        assert!(normalize_base_url("http://127.0.0.1/wiki").is_ok());
        assert!(normalize_base_url("http://[::1]/wiki").is_ok());
    }

    #[test]
    fn settings_remain_readable_when_destination_needs_repair() {
        let legacy =
            serde_json::json!({"base_url": "http://confluence.example.com", "pat": "test-pat"});
        let (saved, migrated) = parse_binding(&legacy.to_string()).unwrap();
        assert!(migrated);
        assert!(saved
            .destination_problem
            .as_deref()
            .unwrap()
            .contains("HTTPS"));
        let status = settings_status(Some(&saved), &saved.base_url);
        assert!(status.token_configured);
        assert!(!status.reachable);
        assert!(status.message.contains("Settings need attention"));
        assert!(status.destination_problem.is_some());
        assert!(saved.token_for(&saved.base_url).is_err());
        assert!(save_pat_to_keyring("test-pat", &saved.base_url, true).is_err());
    }

    #[test]
    fn settings_return_saved_opt_in_only_for_the_bound_origin() {
        let saved = BoundPat {
            destination_problem: None,
            base_url: "http://wiki.local".into(),
            pat: "test-pat".into(),
            allow_unencrypted: true,
        };
        let status = settings_status(Some(&saved), "http://wiki.local/rest/api");
        assert!(status.allow_unencrypted);
        assert!(!status.reachable);
        let json = serde_json::to_value(status).unwrap();
        assert_eq!(json["allowUnencrypted"], true);
        assert!(!settings_status(Some(&saved), "http://other.local").allow_unencrypted);
        assert!(!settings_status(None, "http://wiki.local").allow_unencrypted);
    }

    #[test]
    fn normalize_base_url_strips_rest_suffix_and_slashes() {
        assert_eq!(
            normalize_base_url("https://example.test/confluence/rest/api/").unwrap(),
            "https://example.test/confluence"
        );
        assert!(normalize_base_url("example.test/confluence").is_err());
    }
}
