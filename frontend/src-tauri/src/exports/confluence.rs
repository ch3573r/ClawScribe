//! Confluence Server/Data Center export.
//!
//! This targets self-hosted Confluence behind corporate network access. The PAT
//! is stored in the OS credential store and sent as a Bearer token only from the
//! Rust side; the frontend stores only non-sensitive destination settings.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Runtime};

use super::exporter::now_rfc3339;
use super::ledger::{artifact_key_hash, valid_remote_id, ConfluenceBinding, ExportLedger};
use super::model::ExportStatus;

const SERVICE_NAME: &str = "net.rismondo.openclaw.clawscribe.confluence";
const ACCOUNT_NAME: &str = "default";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfluenceConnectionStatus {
    pub base_url: Option<String>,
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
    pub outcome: PublishOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PublishOutcome {
    Published,
    AlreadyPublished,
    DraftChanged,
}

const REVIEW: &str = "Check Confluence for the page, then resolve the outcome in Export history before publishing again.";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfluenceDestination {
    base_url: String,
    space_key: String,
    parent_id: Option<String>,
}

impl ConfluenceDestination {
    fn base(&self) -> Result<String, String> {
        normalize_base_url(&self.base_url).map_err(|e| e.to_string())
    }

    fn parent(&self) -> Option<&str> {
        self.parent_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
    }

    fn key(&self, base: &str) -> Result<String, String> {
        let canonical = url::Url::parse(base).map_err(|_| "Invalid Confluence base URL.")?;
        Ok(format!(
            "confluence:{}",
            artifact_key_hash(&[
                canonical.as_str(),
                self.space_key.trim(),
                self.parent().unwrap_or("")
            ])
        ))
    }
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
struct CreatePageResponse {
    id: String,
    title: String,
}

#[derive(Debug)]
enum ConfluenceError {
    MissingToken,
    Keyring(String),
    InvalidInput(String),
}

impl std::fmt::Display for ConfluenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfluenceError::MissingToken => write!(f, "No Confluence PAT is saved."),
            ConfluenceError::Keyring(e) => write!(f, "Credential store error: {e}"),
            ConfluenceError::InvalidInput(e) => write!(f, "{e}"),
        }
    }
}

fn credential_entry() -> Result<keyring::Entry, ConfluenceError> {
    keyring::Entry::new(
        &crate::database::recovery::credential_service(SERVICE_NAME),
        ACCOUNT_NAME,
    )
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
        || parsed.query().is_some()
        || parsed.fragment().is_some()
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
        base_url: binding.map(|binding| binding.base_url.clone()),
        destination_problem: None,
        token_configured: binding.is_some(),
        allow_unencrypted: false,
        reachable: false,
        user_display_name: None,
        message: "No Confluence PAT is saved.".into(),
    };
    if let Some(binding) = binding {
        let base_url = if base_url.trim().is_empty() {
            &binding.base_url
        } else {
            base_url
        };
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
    let http = http_client(&base_url, allow_unencrypted).await?;
    let user = current_user(&http, base_url, token).await?;
    let display = user
        .display_name
        .or(user.username)
        .or(user.user_key)
        .filter(|s| !s.trim().is_empty());

    Ok(display)
}

async fn current_user(
    http: &reqwest::Client,
    base: &str,
    token: &str,
) -> Result<ConfluenceUser, String> {
    let url = api_url(base, "/rest/api/user/current").map_err(|e| e.to_string())?;
    let response = http
        .get(url)
        .bearer_auth(token)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| "Could not verify the Confluence connection.")?;
    if !response.status().is_success() {
        return Err(format!(
            "Could not verify the Confluence account (HTTP {}).",
            response.status().as_u16()
        ));
    }
    response
        .json()
        .await
        .map_err(|_| "Confluence did not return a usable account.")
        .map_err(Into::into)
}

async fn verified_account(
    http: &reqwest::Client,
    base: &str,
    token: &str,
) -> Result<String, String> {
    let user = current_user(http, base, token).await?;
    let user_key = user
        .user_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
        .ok_or("Confluence did not return a stable account ID. Check the saved PAT.")?;
    let canonical = url::Url::parse(base).map_err(|_| "Invalid Confluence base URL.")?;
    Ok(artifact_key_hash(&[canonical.as_str(), user_key]))
}

fn saved_connection() -> Result<BoundPat, String> {
    load_pat_from_keyring()
        .and_then(|token| token.ok_or(ConfluenceError::MissingToken))
        .map_err(|error| error.to_string())
}

fn require_same_account(ledger: &ExportLedger, key: &str, account: &str) -> Result<(), String> {
    if ledger
        .entry(key)
        .and_then(|entry| entry.confluence.as_ref())
        .is_some_and(|saved| saved.account_hash != account)
    {
        return Err("This export belongs to a different Confluence account. Restore the original connection to review it in Export history.".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn confluence_export_page<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    base_url: String,
    space_key: String,
    parent_id: Option<String>,
    title: String,
    body_storage: String,
) -> Result<ConfluenceExportResponse, String> {
    let _guard = super::commands::export_guard()?;
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
        if !valid_page_id(parent) {
            return Err("Enter the numeric Confluence parent page ID.".into());
        }
        payload["ancestors"] = serde_json::json!([{ "id": parent }]);
    }

    let destination = ConfluenceDestination {
        base_url,
        space_key: space_key.into(),
        parent_id,
    };
    let base = destination.base()?;
    let key = destination.key(&base)?;
    let url = api_url(&base, "/rest/api/content").map_err(|e| e.to_string())?;
    let mut ledger = super::commands::load_ledger(&app, &meeting_id).await?;
    let credential = saved_connection()?;
    let token = credential
        .token_for(&base)
        .map_err(|error| error.to_string())?;
    let http = http_client(&base, credential.allow_unencrypted).await?;
    let account_hash = verified_account(&http, &base, token).await?;
    require_same_account(&ledger, &key, &account_hash)?;
    if let Some(entry) = ledger.already_succeeded(&key) {
        if entry.confluence.is_none() {
            return Err(REVIEW.into());
        }
        let id = entry.resource_id.as_deref().ok_or(REVIEW)?;
        let id = page_id_from_reference(&base, id)?;
        if !read_page(&http, &base, token, &id)
            .await?
            .is_some_and(|page| page_matches_destination(&page, &id, &destination))
        {
            return Err(REVIEW.into());
        }
    }
    let binding = ConfluenceBinding {
        account_hash,
        revision: Some(artifact_key_hash(&[title, body_storage])),
    };
    publish_page(&mut ledger, &key, &base, title, binding, || async move {
        let response = http
            .post(url)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|_| PublishFailure::Unconfirmed)?;
        let status = response.status().as_u16();
        if status >= 500 || status == 408 || (300..400).contains(&status) {
            return Err(PublishFailure::Unconfirmed);
        }
        if !response.status().is_success() {
            return Err(PublishFailure::Rejected(status));
        }
        let body = response
            .text()
            .await
            .map_err(|_| PublishFailure::Unconfirmed)?;
        serde_json::from_str(&body).map_err(|_| PublishFailure::Unconfirmed)
    })
    .await
}

#[derive(Debug)]
enum PublishFailure {
    Rejected(u16),
    Unconfirmed,
}

async fn publish_page<F, Fut>(
    ledger: &mut ExportLedger,
    key: &str,
    base_url: &str,
    title: &str,
    binding: ConfluenceBinding,
    submit: F,
) -> Result<ConfluenceExportResponse, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<CreatePageResponse, PublishFailure>>,
{
    require_same_account(ledger, key, &binding.account_hash)?;
    let (id, title, outcome) = if let Some(entry) = ledger.already_succeeded(key) {
        let saved = entry.confluence.as_ref().ok_or(REVIEW)?;
        let outcome = if saved.revision == binding.revision {
            PublishOutcome::AlreadyPublished
        } else {
            PublishOutcome::DraftChanged
        };
        (
            entry.resource_id.clone().ok_or(REVIEW)?,
            entry.display_name.clone().unwrap_or_default(),
            outcome,
        )
    } else {
        if !ledger.may_attempt(key) {
            return Err(REVIEW.into());
        }
        ledger.begin_attempt(key, now_rfc3339());
        let entry = ledger.entries.get_mut(key).expect("pending publish");
        // Recovery looks up the exact title sent to Confluence.
        entry.display_name =
            (!title.is_empty() && title.len() <= 1024 && !title.chars().any(char::is_control))
                .then(|| title.to_owned());
        entry.confluence = Some(binding);
        ledger.checkpoint().await?;
        match submit().await {
            Ok(page) if valid_page_id(&page.id) => {
                ledger.record_success(key, Some(page.id.clone()), None, now_rfc3339());
                ledger.label_attempt(key, &page.title);
                ledger.checkpoint().await.map_err(|_| REVIEW)?;
                (page.id, page.title, PublishOutcome::Published)
            }
            Err(PublishFailure::Rejected(status)) => {
                ledger.record_failure(
                    key,
                    ExportStatus::Failed,
                    Some(format!("confluence_http_{status}")),
                    now_rfc3339(),
                );
                ledger.checkpoint().await?;
                return Err(format!("Confluence declined the request (HTTP {status})."));
            }
            _ => {
                ledger.record_failure(
                    key,
                    ExportStatus::UnknownAfterSubmit,
                    Some("submission_unconfirmed".into()),
                    now_rfc3339(),
                );
                ledger.checkpoint().await?;
                return Err(REVIEW.into());
            }
        }
    };
    Ok(ConfluenceExportResponse {
        web_url: page_url(base_url, &id),
        page_id: id,
        title,
        outcome,
    })
}

fn page_url(base_url: &str, id: &str) -> Option<String> {
    api_url(base_url, "/pages/viewpage.action")
        .ok()
        .and_then(|value| url::Url::parse(&value).ok())
        .map(|mut url| {
            url.query_pairs_mut().append_pair("pageId", id);
            url.to_string()
        })
}

fn valid_page_id(id: &str) -> bool {
    valid_remote_id(id) && id.bytes().all(|byte| byte.is_ascii_digit())
}

fn page_id_from_reference(base: &str, reference: &str) -> Result<String, String> {
    let reference = reference.trim();
    let id = if let Ok(url) = url::Url::parse(reference) {
        let expected = url::Url::parse(base).map_err(|_| "Invalid Confluence destination.")?;
        if expected.origin() != url.origin()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("Use a page link from the selected Confluence site.".into());
        }
        let path = url
            .path()
            .strip_prefix(expected.path().trim_end_matches('/'))
            .unwrap_or_default();
        match path {
            "/pages/viewpage.action" | "/pages/viewinfo.action" => url
                .query_pairs()
                .find(|(key, _)| key == "pageId")
                .map(|(_, id)| id.into_owned())
                .unwrap_or_default(),
            _ => match path.split('/').collect::<Vec<_>>().as_slice() {
                ["", "spaces", _, "pages", id, ..] => (*id).to_owned(),
                _ => String::new(),
            },
        }
    } else {
        reference.to_owned()
    };
    if !valid_page_id(&id) {
        return Err(
            "Enter a Confluence page link with pageId in the web address, or the numeric page ID."
                .into(),
        );
    }
    Ok(id)
}

fn page_is_in_space(value: &serde_json::Value, id: &str, space: &str) -> bool {
    value.get("id").and_then(|id| id.as_str()) == Some(id)
        && value.get("type").and_then(|kind| kind.as_str()) == Some("page")
        && value.get("status").and_then(|status| status.as_str()) == Some("current")
        && value.pointer("/space/key").and_then(|key| key.as_str()) == Some(space)
}

fn page_matches_destination(
    value: &serde_json::Value,
    id: &str,
    destination: &ConfluenceDestination,
) -> bool {
    let Some(ancestors) = value
        .get("ancestors")
        .and_then(|ancestors| ancestors.as_array())
    else {
        return false;
    };
    page_is_in_space(value, id, destination.space_key.trim())
        && match ancestors.last() {
            Some(parent) => parent["id"]
                .as_str()
                .filter(|id| valid_page_id(id))
                .is_some_and(|parent| Some(parent) == destination.parent()),
            None => destination.parent().is_none(),
        }
}

async fn read_page(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    id: &str,
) -> Result<Option<serde_json::Value>, String> {
    let url = api_url(base, &format!("/rest/api/content/{id}")).map_err(|err| err.to_string())?;
    let response = http
        .get(url)
        .query(&[("expand", "space,ancestors,body.storage")])
        .bearer_auth(token)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| "Could not verify this Confluence page. Try again.")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!(
            "Could not verify this Confluence page (HTTP {}).",
            response.status().as_u16()
        ));
    }
    response
        .json()
        .await
        .map(Some)
        .map_err(|_| "Confluence did not return usable page details.".into())
}

pub(super) async fn reconcile_page(
    ledger: &mut ExportLedger,
    key: &str,
    destination: ConfluenceDestination,
    existing_reference: Option<&str>,
) -> Result<(), String> {
    let base = destination.base()?;
    if destination.key(&base)? != key {
        return Err("Select the original Confluence site, space and parent in Settings before resolving this export.".into());
    }
    let credential = saved_connection()?;
    let token = credential
        .token_for(&base)
        .map_err(|error| error.to_string())?;
    let http = http_client(&base, credential.allow_unencrypted).await?;
    reconcile_page_with_client(
        &http,
        &base,
        token,
        ledger,
        key,
        &destination,
        existing_reference,
    )
    .await
}

async fn reconcile_page_with_client(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    ledger: &mut ExportLedger,
    key: &str,
    destination: &ConfluenceDestination,
    existing_reference: Option<&str>,
) -> Result<(), String> {
    let entry = ledger.entry(key).ok_or("Export history entry missing.")?;
    let account_hash = verified_account(http, base, token).await?;
    require_same_account(ledger, key, &account_hash)?;
    if let Some(reference) = existing_reference {
        let id = page_id_from_reference(base, reference)?;
        let value = read_page(http, base, token, &id)
            .await?
            .filter(|value| page_matches_destination(value, &id, destination))
            .ok_or(
                "No current page at that link was found in the original Confluence destination.",
            )?;
        let title = value
            .get("title")
            .and_then(|title| title.as_str())
            .ok_or("Confluence did not return the page title.")?;
        let remote_revision = value
            .pointer("/body/storage/value")
            .and_then(|body| body.as_str())
            .map(|body| artifact_key_hash(&[title, body]));
        // Confluence may normalize the submitted HTML. Only claim that a
        // reconciled page matches the draft when the saved revision is provable.
        let revision = entry
            .confluence
            .as_ref()
            .and_then(|binding| binding.revision.clone())
            .filter(|revision| remote_revision.as_ref() == Some(revision));
        let previous = entry.resource_id.clone();
        ledger.apply_resolution(key, previous.as_deref(), Some(&id), None, now_rfc3339())?;
        ledger.label_attempt(key, title);
        ledger
            .entries
            .get_mut(key)
            .expect("reconciled export")
            .confluence = Some(ConfluenceBinding {
            account_hash,
            revision,
        });
    } else {
        // Confluence may return 404 when page visibility changed. The app
        // cannot safely re-publish a previously identified page on that basis.
        if entry.resource_id.is_some() || entry.confluence.is_none() {
            return Err("Link an existing page from the original destination. Use a reviewed browser draft to replace a deleted Confluence page.".into());
        }
        let title = entry.display_name.as_deref().filter(|title| !title.trim().is_empty())
            .ok_or("This export has no saved page title. Link the existing page or review a browser draft before publishing manually.")?;
        verify_page_absence(http, base, token, destination, title).await?;
        ledger.reconcile(key, None, now_rfc3339())?;
    }
    ledger.checkpoint().await
}

async fn verify_page_absence(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    destination: &ConfluenceDestination,
    title: &str,
) -> Result<(), String> {
    let space_key = destination.space_key.trim();
    let mut space =
        url::Url::parse(&api_url(base, "/rest/api/space").map_err(|error| error.to_string())?)
            .map_err(|_| "Invalid Confluence destination.")?;
    space
        .path_segments_mut()
        .map_err(|_| "Invalid Confluence destination.")?
        .push(space_key);
    let response = http
        .get(space)
        .header("Accept", "application/json")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "Could not check the original Confluence space. Try again.")?;
    let space = if response.status().is_success() {
        response.json::<serde_json::Value>().await.ok()
    } else {
        None
    };
    if space.as_ref().and_then(|space| space["key"].as_str()) != Some(space_key) {
        return Err(
            "The original Confluence space is unavailable. Export history was kept.".into(),
        );
    }
    if let Some(parent) = destination.parent() {
        if !valid_page_id(parent)
            || !read_page(http, base, token, parent)
                .await?
                .is_some_and(|page| page_is_in_space(&page, parent, space_key))
        {
            return Err("The original parent page is unavailable. Export history was kept.".into());
        }
    }

    const INCOMPLETE: &str =
        "Confluence did not return a complete title search. Check the original destination and try again.";
    let response = http
        .get(api_url(base, "/rest/api/content").map_err(|error| error.to_string())?)
        .query(&[
            ("spaceKey", space_key),
            ("title", title),
            ("type", "page"),
            ("status", "current"),
            ("expand", "space,ancestors"),
            ("limit", "1"),
        ])
        .header("Accept", "application/json")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| INCOMPLETE)?;
    if !response.status().is_success() {
        return Err(INCOMPLETE.into());
    }
    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| INCOMPLETE)?;
    let pages = body["results"].as_array().ok_or(INCOMPLETE)?;
    if pages.iter().any(|page| {
        page["title"].as_str() == Some(title)
            && page["id"]
                .as_str()
                .filter(|id| valid_page_id(id))
                .is_some_and(|id| page_matches_destination(page, id, destination))
    }) {
        return Err("Confluence found a page with the saved title in the original destination. Check it in Confluence and link it from Export history.".into());
    }
    // A title match elsewhere in the space may be the original page after a move.
    if !pages.is_empty() {
        return Err("Confluence returned a possible matching page. Check the saved title in the original space. Export history was kept.".into());
    }
    if body
        .get("_links")
        .is_some_and(|links| !links.is_object() || !links["next"].is_null())
        || ["start", "size", "totalCount"].iter().any(|field| {
            body.get(*field)
                .is_some_and(|value| value.as_u64() != Some(0))
        })
    {
        return Err(INCOMPLETE.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(account: &str, revision: &str) -> ConfluenceBinding {
        ConfluenceBinding {
            account_hash: artifact_key_hash(&[account]),
            revision: Some(artifact_key_hash(&[revision])),
        }
    }

    #[tokio::test]
    async fn confluence_retry_requires_the_original_destination_and_a_complete_empty_search() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let space = r#"{"key":"TEST"}"#;
        let parent = r#"{"id":"40","type":"page","status":"current","space":{"key":"TEST"}}"#;
        let empty = r#"{"results":[],"start":0,"size":0,"_links":{}}"#;
        let title = "Review & follow-up";
        for (saved_title, space_body, parent_body, search, expected) in [
            (Some(title), space, parent, Some(empty), None),
            (
                Some(title),
                r#"{"key":"OTHER"}"#,
                parent,
                None,
                Some("space is unavailable"),
            ),
            (
                Some(title),
                space,
                r#"{"id":"40","type":"page","status":"current","space":{"key":"OTHER"}}"#,
                None,
                Some("parent page is unavailable"),
            ),
            (
                Some(title),
                space,
                parent,
                Some(
                    r#"{"results":[{"id":"318","title":"Review & follow-up","type":"page","status":"current","space":{"key":"TEST"},"ancestors":[{"id":"40"}]}]}"#,
                ),
                Some("a page with the saved title in the original destination"),
            ),
            (
                Some(title),
                space,
                parent,
                Some(
                    r#"{"results":[{"id":"318","title":"Review & follow-up","type":"page","status":"current","space":{"key":"TEST"},"ancestors":[{"id":"41"}]}]}"#,
                ),
                Some("possible matching page"),
            ),
            (
                Some(title),
                space,
                parent,
                Some(r#"{"results":[{"title":"Review & follow-up"}]}"#),
                Some("possible matching page"),
            ),
            (
                Some(title),
                space,
                parent,
                Some(r#"{"results":[],"_links":{"next":"https://other.example.com"}}"#),
                Some("complete title search"),
            ),
            (
                Some(title),
                space,
                parent,
                Some(r#"{"results":[],"totalCount":1}"#),
                Some("complete title search"),
            ),
            (None, space, parent, None, Some("no saved page title")),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}/wiki", listener.local_addr().unwrap());
            let destination = ConfluenceDestination {
                base_url: base.clone(),
                space_key: "TEST".into(),
                parent_id: Some("40".into()),
            };
            let key = destination.key(&base).unwrap();
            let mut ledger = ExportLedger::new("meeting");
            ledger.record_failure(&key, ExportStatus::UnknownAfterSubmit, None, None);
            if let Some(title) = saved_title {
                ledger.label_attempt(&key, title);
            }
            ledger.entries.get_mut(&key).unwrap().confluence = Some(ConfluenceBinding {
                account_hash: artifact_key_hash(&[&base, "test-user"]),
                revision: None,
            });

            let mut responses = vec![("/wiki/rest/api/user/current", r#"{"userKey":"test-user"}"#)];
            if saved_title.is_some() {
                responses.push(("/wiki/rest/api/space/TEST", space_body));
                if space_body == space {
                    responses.push(("/wiki/rest/api/content/40", parent_body));
                    if let Some(search) = search {
                        responses.push(("/wiki/rest/api/content", search));
                    }
                }
            }
            let server = tokio::spawn(async move {
                for (path, body) in responses {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        request.push(stream.read_u8().await.unwrap());
                    }
                    let request = std::str::from_utf8(&request).unwrap();
                    let mut words = request.split_whitespace();
                    assert_eq!(words.next(), Some("GET"));
                    let target =
                        url::Url::parse(&format!("http://example.com{}", words.next().unwrap()))
                            .unwrap();
                    assert_eq!(target.path(), path);
                    if path == "/wiki/rest/api/content" {
                        let query: std::collections::HashMap<_, _> = target.query_pairs().collect();
                        assert_eq!(query["title"], title);
                        assert_eq!(query["spaceKey"], "TEST");
                        assert_eq!(query["type"], "page");
                        assert_eq!(query["status"], "current");
                        assert_eq!(query["expand"], "space,ancestors");
                    }
                    stream.write_all(format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ).as_bytes()).await.unwrap();
                }
            });
            let http = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap();
            let result = reconcile_page_with_client(
                &http,
                &base,
                "<redacted>",
                &mut ledger,
                &key,
                &destination,
                None,
            )
            .await;
            tokio::time::timeout(Duration::from_secs(2), server)
                .await
                .unwrap()
                .unwrap();
            if let Some(message) = expected {
                assert!(result.unwrap_err().contains(message), "{message}");
                assert!(!ledger.may_attempt(&key));
            } else {
                result.unwrap();
                assert!(ledger.may_attempt(&key));
            }
        }
    }

    #[tokio::test]
    async fn lost_confluence_response_is_not_replayed_until_the_user_reconciles_it() {
        let folder = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        let base = "https://confluence.example.com/wiki";
        let key = format!("confluence:{}", artifact_key_hash(&[base, "TEST", ""]));
        let title = "N".repeat(241);
        assert!(publish_page(
            &mut ledger,
            &key,
            base,
            &title,
            binding("user", "original"),
            || async { Err(PublishFailure::Unconfirmed) }
        )
        .await
        .is_err());

        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        assert_eq!(
            ledger.entries[&key].display_name.as_deref(),
            Some(title.as_str())
        );
        assert!(publish_page(
            &mut ledger,
            &key,
            base,
            "Edited notes",
            binding("user", "edited"),
            || async { panic!("uncertain publish was re-submitted") }
        )
        .await
        .is_err());
        ledger
            .reconcile(&key, Some("318".into()), now_rfc3339())
            .unwrap();
        ledger.checkpoint().await.unwrap();
        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        let published = publish_page(
            &mut ledger,
            &key,
            base,
            "Edited notes",
            binding("user", "edited"),
            || async { panic!("confirmed publish was re-submitted") },
        )
        .await
        .unwrap();
        assert_eq!(published.page_id, "318");
        assert_eq!(published.title, title);
        assert_eq!(published.outcome, PublishOutcome::DraftChanged);
        assert_eq!(
            published.web_url.as_deref(),
            Some("https://confluence.example.com/wiki/pages/viewpage.action?pageId=318")
        );
    }

    #[tokio::test]
    async fn same_revision_is_opened_and_edited_revisions_or_changed_users_do_not_publish() {
        let folder = tempfile::tempdir().unwrap();
        let base = "https://confluence.example.com/wiki";
        let key = format!("confluence:{}", artifact_key_hash(&[base, "TEST", ""]));
        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        let created = publish_page(
            &mut ledger,
            &key,
            base,
            "Original notes",
            binding("original-user", "original"),
            || async {
                let pending = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
                assert_eq!(
                    pending.entries[&key].status,
                    ExportStatus::UnknownAfterSubmit
                );
                Ok(CreatePageResponse {
                    id: "318".into(),
                    title: "Original notes".into(),
                })
            },
        )
        .await
        .unwrap();
        assert_eq!(created.outcome, PublishOutcome::Published);

        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        for (title, revision, expected) in [
            (
                "Original notes",
                "original",
                PublishOutcome::AlreadyPublished,
            ),
            (
                "Original notes",
                "body edited",
                PublishOutcome::DraftChanged,
            ),
            ("Edited title", "title edited", PublishOutcome::DraftChanged),
        ] {
            let result = publish_page(
                &mut ledger,
                &key,
                base,
                title,
                binding("original-user", revision),
                || async { panic!("the reviewed page must be opened rather than written") },
            )
            .await
            .unwrap();
            assert_eq!(result.outcome, expected);
            assert_eq!(result.title, "Original notes");
            assert!(result.web_url.is_some());
        }
        assert!(publish_page(
            &mut ledger,
            &key,
            base,
            "Original notes",
            binding("other-user", "original"),
            || async { panic!("a different account must not write to prior history") }
        )
        .await
        .unwrap_err()
        .contains("different Confluence account"));
    }

    #[test]
    fn confluence_page_links_and_locations_are_checked_against_the_original_site() {
        let base = "https://confluence.example.com/wiki";
        let destination = ConfluenceDestination {
            base_url: base.into(),
            space_key: "TEST".into(),
            parent_id: Some("40".into()),
        };
        assert_eq!(
            page_id_from_reference(base, &page_url(base, "318").unwrap()).unwrap(),
            "318"
        );
        assert_eq!(
            page_id_from_reference(base, &format!("{base}/pages/viewinfo.action?pageId=318"))
                .unwrap(),
            "318"
        );
        assert_eq!(
            page_id_from_reference(base, &format!("{base}/spaces/TEST/pages/318/Notes")).unwrap(),
            "318"
        );
        assert!(page_id_from_reference(
            base,
            "https://confluence.example.com/other/pages/viewinfo.action?pageId=318"
        )
        .is_err());
        assert!(page_id_from_reference(
            base,
            "https://other.example.com/wiki/pages/viewpage.action?pageId=318"
        )
        .is_err());
        let mut page = serde_json::json!({"id":"318", "type":"page", "status":"current", "space":{"key":"TEST"}, "ancestors":[{"id":"40"}]});
        assert!(page_matches_destination(&page, "318", &destination));
        page["ancestors"][0]["id"] = "41".into();
        assert!(!page_matches_destination(&page, "318", &destination));
    }

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
        assert_eq!(json["baseUrl"], "http://wiki.local");
        let reopened = settings_status(Some(&saved), "");
        assert_eq!(reopened.base_url.as_deref(), Some("http://wiki.local"));
        assert!(reopened.allow_unencrypted);
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
