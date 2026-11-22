//! OneDrive/SharePoint file export operations over Microsoft Graph.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};

use crate::exports::client::{GraphClient, GraphOutcome, Sleeper};
use crate::exports::document::build_meeting_docx;
use crate::exports::error::GraphErrorKind;
use crate::exports::exporter::{now_rfc3339, ExportContext};
use crate::exports::ledger::{
    artifact_key_hash, microsoft_web_url, valid_remote_id, DriveBinding, ExportLedger,
};
use crate::exports::model::ExportStatus;
use crate::exports::resolution::drive_binding;
use crate::exports::transport::{
    GraphBinaryRequest, GraphBinaryResponse, GraphRequest, GraphTransport, TransportError,
};

const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";
pub const DOCX_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
pub const PDF_CONTENT_TYPE: &str = "application/pdf";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriveDestination {
    pub drive_id: String,
    pub item_id: String,
    pub name: String,
    pub web_url: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriveFile {
    pub drive_id: String,
    pub item_id: String,
    pub name: String,
    pub web_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OneDriveExportRequest {
    pub meeting_id: String,
    pub meeting_title: String,
    pub markdown: String,
    #[serde(default)]
    pub transcript: Option<String>,
    #[serde(default)]
    pub destination: Option<DriveDestination>,
    #[serde(default)]
    pub folder_name: Option<String>,
    #[serde(default)]
    pub include_pdf: bool,
    #[serde(default)]
    pub create_organization_link: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OneDriveExportedFile {
    pub kind: String,
    pub drive_id: String,
    pub item_id: String,
    pub name: String,
    pub web_url: Option<String>,
    pub sharing_link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OneDriveExportResponse {
    pub destination: DriveDestination,
    pub files: Vec<OneDriveExportedFile>,
    pub issues: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphDrive {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    web_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphParentReference {
    #[serde(default)]
    drive_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphRemoteItem {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    web_url: Option<String>,
    #[serde(default)]
    parent_reference: Option<GraphParentReference>,
    #[serde(default)]
    folder: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphDriveItem {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    web_url: Option<String>,
    #[serde(default)]
    parent_reference: Option<GraphParentReference>,
    #[serde(default)]
    folder: Option<serde_json::Value>,
    #[serde(default)]
    remote_item: Option<GraphRemoteItem>,
}

#[derive(Debug, Deserialize)]
struct GraphCreateLinkResponse {
    link: Option<GraphSharingLink>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphSharingLink {
    web_url: Option<String>,
}

fn graph_error(kind: GraphErrorKind, detail: Option<String>) -> String {
    match detail {
        Some(d) => format!("Graph error ({}): {d}", kind.code()),
        None => format!("Graph error: {}", kind.code()),
    }
}

fn graph_failure_detail(error_code: Option<&str>, error_message: Option<&str>) -> Option<String> {
    match (error_code, error_message) {
        (Some(c), Some(m)) => Some(format!("{c}: {m}")),
        (Some(c), None) => Some(c.to_string()),
        (None, Some(m)) => Some(m.to_string()),
        (None, None) => None,
    }
}

fn parse_text_response<T: serde::de::DeserializeOwned>(
    outcome: GraphOutcome,
    what: &str,
) -> Result<T, String> {
    match outcome {
        GraphOutcome::Success(resp) => serde_json::from_str::<T>(&resp.body)
            .map_err(|e| format!("Failed to parse {what}: {e}")),
        GraphOutcome::Failed(kind, detail) => Err(graph_error(kind, detail)),
        GraphOutcome::Unknown(msg) => Err(format!("Network error: {msg}")),
    }
}

fn binary_bytes_response(
    outcome: Result<GraphBinaryResponse, TransportError>,
    what: &str,
) -> Result<Vec<u8>, String> {
    match outcome {
        Ok(resp) if resp.is_success() => Ok(resp.body),
        Ok(resp) => Err(graph_error(
            GraphErrorKind::from_status(resp.status, resp.error_code.as_deref()),
            graph_failure_detail(resp.error_code.as_deref(), resp.error_message.as_deref()),
        )),
        Err(TransportError::Network(msg)) => Err(format!("Network error while {what}: {msg}")),
    }
}

fn get_request(url: String) -> GraphRequest {
    GraphRequest {
        method: "GET".into(),
        url,
        content_type: "application/json".into(),
        body: String::new(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: Vec::new(),
    }
}

fn post_json_request(url: String, body: serde_json::Value) -> GraphRequest {
    GraphRequest {
        method: "POST".into(),
        url,
        content_type: "application/json".into(),
        body: body.to_string(),
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: Vec::new(),
    }
}

fn binary_request(
    method: impl Into<String>,
    url: String,
    content_type: impl Into<String>,
    body: Vec<u8>,
) -> GraphBinaryRequest {
    GraphBinaryRequest {
        method: method.into(),
        url,
        content_type: content_type.into(),
        body,
        correlation_id: uuid::Uuid::new_v4().to_string(),
        headers: Vec::new(),
    }
}

pub fn encode_sharing_url(sharing_url: &str) -> String {
    format!(
        "u!{}",
        URL_SAFE_NO_PAD.encode(sharing_url.trim().as_bytes())
    )
}

pub fn encode_graph_path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn sanitize_drive_item_name(raw: &str, fallback: &str) -> String {
    const FORBIDDEN: &[char] = &['"', '*', ':', '<', '>', '?', '/', '\\', '|'];
    let replaced: String = raw
        .chars()
        .map(|c| {
            if FORBIDDEN.contains(&c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches('.').trim();
    let name = if trimmed.is_empty() {
        fallback
    } else {
        trimmed
    };
    name.chars().take(120).collect()
}

fn drive_file_from_item(
    item: GraphDriveItem,
    fallback_drive_id: &str,
) -> Result<DriveFile, String> {
    let drive_id = item
        .parent_reference
        .and_then(|p| p.drive_id)
        .unwrap_or_else(|| fallback_drive_id.to_string());
    let name = item.name.unwrap_or_else(|| "Meeting notes".to_string());
    Ok(DriveFile {
        drive_id,
        item_id: item.id,
        name,
        web_url: item.web_url,
    })
}

fn destination_from_item(
    item: GraphDriveItem,
    fallback_drive_id: Option<&str>,
    source: &str,
) -> Result<DriveDestination, String> {
    if let Some(remote) = item.remote_item {
        if remote.folder.is_none() {
            return Err("The sharing URL must point to a folder.".to_string());
        }
        let drive_id = remote
            .parent_reference
            .and_then(|p| p.drive_id)
            .or_else(|| fallback_drive_id.map(str::to_string))
            .ok_or_else(|| "Graph did not return a drive id for the shared folder.".to_string())?;
        return Ok(DriveDestination {
            drive_id,
            item_id: remote.id,
            name: remote.name.unwrap_or_else(|| "Shared folder".to_string()),
            web_url: remote.web_url,
            source: source.to_string(),
        });
    }

    if item.folder.is_none() {
        return Err("The sharing URL must point to a folder.".to_string());
    }
    let drive_id = item
        .parent_reference
        .and_then(|p| p.drive_id)
        .or_else(|| fallback_drive_id.map(str::to_string))
        .ok_or_else(|| "Graph did not return a drive id for the folder.".to_string())?;
    Ok(DriveDestination {
        drive_id,
        item_id: item.id,
        name: item.name.unwrap_or_else(|| "Folder".to_string()),
        web_url: item.web_url,
        source: source.to_string(),
    })
}

pub async fn resolve_default_drive_root<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
) -> Result<DriveDestination, String> {
    let drive: GraphDrive = parse_text_response(
        client
            .execute(
                &get_request(format!("{GRAPH_BASE}/me/drive?$select=id,name,webUrl")),
                token,
            )
            .await,
        "default drive",
    )?;

    let root: GraphDriveItem = parse_text_response(
        client
            .execute(
                &get_request(format!(
                    "{GRAPH_BASE}/me/drive/root?$select=id,name,webUrl,parentReference,folder"
                )),
                token,
            )
            .await,
        "drive root",
    )?;

    let mut destination = destination_from_item(root, Some(&drive.id), "default_drive_root")?;
    if destination.name.eq_ignore_ascii_case("root") {
        if let Some(name) = drive.name {
            if !name.trim().is_empty() {
                destination.name = name;
            }
        }
    }
    if destination.web_url.is_none() {
        destination.web_url = drive.web_url;
    }
    Ok(destination)
}

pub async fn resolve_sharing_url<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    sharing_url: &str,
) -> Result<DriveDestination, String> {
    if sharing_url.trim().is_empty() {
        return Err("Sharing URL is required.".to_string());
    }
    let share_token = encode_sharing_url(sharing_url);
    let request = get_request(format!(
        "{GRAPH_BASE}/shares/{share_token}/driveItem?$select=id,name,webUrl,parentReference,folder,remoteItem"
    ));
    let item: GraphDriveItem =
        parse_text_response(client.execute(&request, token).await, "shared drive item")?;
    destination_from_item(item, None, "sharing_url")
}

pub async fn create_folder<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    parent: &DriveDestination,
    folder_name: &str,
) -> Result<DriveDestination, String> {
    let folder_name = sanitize_drive_item_name(folder_name, "Meeting notes");
    let request = post_json_request(
        format!(
            "{GRAPH_BASE}/drives/{}/items/{}/children",
            parent.drive_id, parent.item_id
        ),
        serde_json::json!({
            "name": folder_name,
            "folder": {},
            "@microsoft.graph.conflictBehavior": "rename",
        }),
    );
    let item: GraphDriveItem =
        parse_text_response(client.execute(&request, token).await, "created folder")?;
    let mut destination = destination_from_item(item, Some(&parent.drive_id), "created_folder")?;
    destination.source = "created_folder".to_string();
    Ok(destination)
}

#[derive(Debug)]
enum UploadFailure {
    Rejected(GraphErrorKind),
    Unconfirmed,
}

enum FileExportFailure {
    History(String),
    Remote(String),
}

async fn upload_file<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    parent: &DriveDestination,
    file_name: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> Result<DriveFile, UploadFailure> {
    let encoded_name = encode_graph_path_segment(file_name);
    let request = binary_request(
        "PUT",
        format!(
            "{GRAPH_BASE}/drives/{}/items/{}:/{encoded_name}:/content?%40microsoft.graph.conflictBehavior=fail",
            parent.drive_id, parent.item_id
        ),
        content_type,
        bytes,
    );
    let response = client
        .execute_binary(&request, token)
        .await
        .map_err(|_| UploadFailure::Unconfirmed)?;
    if !response.is_success() {
        return if response.status >= 500 || matches!(response.status, 408 | 409 | 412) {
            Err(UploadFailure::Unconfirmed)
        } else {
            Err(UploadFailure::Rejected(GraphErrorKind::from_status(
                response.status,
                response.error_code.as_deref(),
            )))
        };
    }
    let item: GraphDriveItem =
        serde_json::from_slice(&response.body).map_err(|_| UploadFailure::Unconfirmed)?;
    let file =
        drive_file_from_item(item, &parent.drive_id).map_err(|_| UploadFailure::Unconfirmed)?;
    if !valid_remote_id(&file.item_id) || file.drive_id != parent.drive_id {
        return Err(UploadFailure::Unconfirmed);
    }
    Ok(file)
}

pub async fn convert_docx_to_pdf<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    file: &DriveFile,
) -> Result<Vec<u8>, String> {
    let request = binary_request(
        "GET",
        format!(
            "{GRAPH_BASE}/drives/{}/items/{}/content?format=pdf",
            file.drive_id, file.item_id
        ),
        "",
        Vec::new(),
    );
    binary_bytes_response(
        client.execute_binary(&request, token).await,
        "converting DOCX to PDF",
    )
}

pub async fn create_organization_view_link<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    file: &DriveFile,
) -> Result<String, String> {
    let request = post_json_request(
        format!(
            "{GRAPH_BASE}/drives/{}/items/{}/createLink",
            file.drive_id, file.item_id
        ),
        serde_json::json!({
            "type": "view",
            "scope": "organization",
        }),
    );
    let response: GraphCreateLinkResponse =
        parse_text_response(client.execute(&request, token).await, "sharing link")?;
    response
        .link
        .and_then(|link| link.web_url)
        .ok_or_else(|| "Graph did not return a sharing link.".to_string())
}

pub async fn export_meeting_files<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    account: &ExportContext<'_>,
    request: OneDriveExportRequest,
) -> Result<OneDriveExportResponse, String> {
    let token = account.bearer_token;
    let base = match request.destination {
        Some(destination) => destination,
        None => resolve_default_drive_root(client, token).await?,
    };
    if !valid_remote_id(&base.drive_id) || !valid_remote_id(&base.item_id) {
        return Err("Invalid OneDrive destination.".into());
    }
    let folder = request
        .folder_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map(|name| sanitize_drive_item_name(name, "Meeting notes"));
    let scope = artifact_key_hash(&[
        account.tenant_id,
        account.user_id,
        &base.drive_id,
        &base.item_id,
        folder.as_deref().unwrap_or(""),
    ]);
    let folder_key = format!("onedrive:{scope}:{scope}:folder");
    let destination = match folder {
        Some(name) => export_folder(client, ledger, account, base, name, &folder_key).await?,
        None => base,
    };

    let revision = artifact_key_hash(&[
        &request.meeting_id,
        &request.meeting_title,
        &request.markdown,
        request.transcript.as_deref().unwrap_or(""),
    ]);
    let title: String = sanitize_drive_item_name(&request.meeting_title, "Meeting notes")
        .chars()
        .take(100)
        .collect();
    let prefix = format!("onedrive:{scope}:{revision}");
    let docx_key = format!("{prefix}:docx");
    let pdf_key = format!("{prefix}:pdf");
    let stem = revision_stem(ledger, &docx_key, &title, chrono::Utc::now());
    let binding = drive_binding(account, &destination.drive_id, &destination.item_id);
    bind_saved_drive_item(ledger, &docx_key, &binding).await?;
    bind_saved_drive_item(ledger, &pdf_key, &binding).await?;

    let mut report = OneDriveExportResponse {
        destination,
        files: Vec::new(),
        issues: Vec::new(),
    };
    let docx_name = format!("{stem}.docx");
    let docx = match saved_file(client, ledger, &docx_key, account, &binding, "docx").await {
        Ok(Some(file)) => file,
        Ok(None) => {
            let bytes = build_meeting_docx(
                &request.meeting_title,
                &request.markdown,
                request.transcript.as_deref(),
            )?;
            match export_file(
                client,
                ledger,
                token,
                &report.destination,
                &docx_name,
                DOCX_CONTENT_TYPE,
                bytes,
                &docx_key,
                &binding,
            )
            .await
            {
                Ok(file) => file,
                Err(FileExportFailure::History(issue) | FileExportFailure::Remote(issue)) => {
                    report.issues.push(format!("Word document: {issue}"));
                    return Ok(report);
                }
            }
        }
        Err(issue) => {
            report.issues.push(format!("Word document: {issue}"));
            return Ok(report);
        }
    };

    // Save each file independently. A sharing failure should not erase a file
    // or prevent attempting the other requested format.
    let pdf = if request.include_pdf {
        let pdf_name = format!("{stem}.pdf");
        let outcome = match saved_file(client, ledger, &pdf_key, account, &binding, "pdf").await {
            Ok(Some(file)) => Ok(file),
            Ok(None) => match convert_docx_to_pdf(client, token, &docx).await {
                Ok(bytes) => {
                    export_file(
                        client,
                        ledger,
                        token,
                        &report.destination,
                        &pdf_name,
                        PDF_CONTENT_TYPE,
                        bytes,
                        &pdf_key,
                        &binding,
                    )
                    .await
                }
                Err(_) => {
                    ledger.record_failure(
                        &pdf_key,
                        ExportStatus::Failed,
                        Some("pdf_conversion_failed".into()),
                        now_rfc3339(),
                    );
                    if ledger.checkpoint().await.is_err() {
                        report.files.push(exported_file("docx", &docx, None));
                        report
                            .issues
                            .push("Could not save PDF conversion status.".into());
                        return Ok(report);
                    }
                    Err(FileExportFailure::Remote(
                        "Could not convert the Word document to PDF.".into(),
                    ))
                }
            },
            Err(issue) => Err(FileExportFailure::Remote(issue)),
        };
        match outcome {
            Ok(file) => Some(file),
            Err(FileExportFailure::History(issue)) => {
                report.files.push(exported_file("docx", &docx, None));
                report.issues.push(format!("PDF: {issue}"));
                return Ok(report);
            }
            Err(FileExportFailure::Remote(issue)) => {
                report.issues.push(format!("PDF: {issue}"));
                None
            }
        }
    } else {
        None
    };

    let mut link_writes_available = request.create_organization_link;
    for (kind, file) in [("docx", Some(docx)), ("pdf", pdf)] {
        let Some(file) = file else { continue };
        let link = if link_writes_available {
            let key = format!("{prefix}:{kind}-link");
            match export_sharing_link(client, ledger, token, &file, &key).await {
                Ok(link) => link,
                Err(FileExportFailure::History(issue)) => {
                    link_writes_available = false;
                    report.issues.push(issue);
                    None
                }
                Err(FileExportFailure::Remote(issue)) => {
                    report.issues.push(format!(
                        "{} sharing link: {issue}",
                        kind.to_ascii_uppercase()
                    ));
                    None
                }
            }
        } else {
            None
        };
        report.files.push(exported_file(kind, &file, link));
    }
    Ok(report)
}

fn revision_stem(
    ledger: &ExportLedger,
    docx_key: &str,
    title: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    if let Some(saved) = ledger
        .entry(docx_key)
        .and_then(|entry| entry.display_name.as_deref())
        .and_then(|name| name.strip_suffix(".docx"))
    {
        return saved.into();
    }
    let base = format!("{title} - {}", now.format("%Y-%m-%d %H%M%S%.3f UTC"));
    let names: std::collections::HashSet<_> = ledger
        .entries
        .values()
        .filter_map(|entry| entry.display_name.as_deref())
        .filter_map(|name| name.strip_suffix(".docx"))
        .map(str::to_lowercase)
        .collect();
    if !names.contains(&base.to_lowercase()) {
        return base;
    }
    for number in 2..=names.len() + 1 {
        let stem = format!("{base} ({number})");
        if !names.contains(&stem.to_lowercase()) {
            return stem;
        }
    }
    unreachable!("one more name than reserved revisions")
}

async fn bind_saved_drive_item(
    ledger: &mut ExportLedger,
    key: &str,
    binding: &DriveBinding,
) -> Result<(), String> {
    let Some(entry) = ledger.entries.get_mut(key) else {
        return Ok(());
    };
    if let Some(previous) = &entry.drive {
        if previous == binding {
            return Ok(());
        }
        if entry.can_resolve() {
            return Err(
                "Resolve the saved OneDrive item in its original folder before exporting again."
                    .into(),
            );
        }
    }
    // The export key was derived from the original account, destination and
    // revision. Repeating that export can restore a legacy item's binding.
    entry.drive = Some(binding.clone());
    ledger.checkpoint().await
}

async fn export_folder<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    account: &ExportContext<'_>,
    base: DriveDestination,
    name: String,
    key: &str,
) -> Result<DriveDestination, String> {
    let token = account.bearer_token;
    let binding = drive_binding(account, &base.drive_id, &base.item_id);
    bind_saved_drive_item(ledger, key, &binding).await?;
    if let Some(entry) = ledger.already_succeeded(key) {
        let item_id = entry
            .resource_id
            .clone()
            .ok_or("The saved folder ID is missing.")?;
        let web_url = entry.web_url.clone();
        let scope = key.split(':').nth(1).expect("OneDrive folder scope");
        let prefix = format!("onedrive:{scope}:");
        let child_binding = drive_binding(account, &base.drive_id, &item_id);
        let mut changed = false;
        for (child_key, child) in ledger.entries.range_mut(prefix.clone()..) {
            if !child_key.starts_with(&prefix) {
                break;
            }
            if matches!(child_key.rsplit(':').next(), Some("docx" | "pdf")) && child.drive.is_none()
            {
                child.drive = Some(child_binding.clone());
                changed = true;
            }
        }
        if changed {
            ledger.checkpoint().await?;
        }
        super::resolution::check_saved_drive_item(client, account, &binding, &item_id, "folder")
            .await?;
        return Ok(DriveDestination {
            item_id,
            name,
            web_url,
            ..base
        });
    }
    if !can_retry_file(ledger, key) {
        return Err("Check the OneDrive folder in export history before trying again.".into());
    }
    let request = post_json_request(
        format!(
            "{GRAPH_BASE}/drives/{}/items/{}/children",
            base.drive_id, base.item_id
        ),
        serde_json::json!({"name": name, "folder": {}, "@microsoft.graph.conflictBehavior": "rename"}),
    );
    ledger.begin_attempt(key, now_rfc3339());
    ledger.label_attempt(key, &name);
    ledger
        .entries
        .get_mut(key)
        .expect("pending OneDrive folder")
        .drive = Some(binding);
    ledger.checkpoint().await?;
    let outcome = match client.execute_create(&request, token).await {
        GraphOutcome::Success(response) => serde_json::from_str::<GraphDriveItem>(&response.body)
            .ok()
            .and_then(|item| {
                destination_from_item(item, Some(&base.drive_id), "created_folder").ok()
            })
            .filter(|folder| valid_remote_id(&folder.item_id) && folder.drive_id == base.drive_id)
            .ok_or(UploadFailure::Unconfirmed),
        GraphOutcome::Failed(kind, _) => Err(UploadFailure::Rejected(kind)),
        GraphOutcome::Unknown(_) => Err(UploadFailure::Unconfirmed),
    };
    let (id, url) = outcome.as_ref().map_or((None, None), |folder| {
        (Some(folder.item_id.as_str()), folder.web_url.as_deref())
    });
    checkpoint_upload(ledger, key, id, url, outcome.as_ref().err()).await?;
    outcome.map_err(|_| "Could not create the OneDrive folder. Check export history.".into())
}

async fn saved_file<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &ExportLedger,
    key: &str,
    account: &ExportContext<'_>,
    binding: &DriveBinding,
    kind: &str,
) -> Result<Option<DriveFile>, String> {
    if let Some(entry) = ledger.already_succeeded(key) {
        let item_id = entry
            .resource_id
            .clone()
            .filter(|id| valid_remote_id(id))
            .ok_or("The saved file ID is missing.")?;
        let name =
            super::resolution::check_saved_drive_item(client, account, binding, &item_id, kind)
                .await?;
        return Ok(Some(DriveFile {
            drive_id: binding.drive_id.clone(),
            item_id,
            name,
            web_url: entry.web_url.clone(),
        }));
    }
    if !can_retry_file(ledger, key) {
        return Err("Check this file in export history before trying again.".into());
    }
    Ok(None)
}

fn can_retry_file(ledger: &ExportLedger, key: &str) -> bool {
    // A new export is an explicit retry and may follow a permissions repair.
    ledger.entry(key).is_none_or(|entry| {
        entry.resource_id.is_none()
            && !matches!(
                entry.status,
                ExportStatus::Pending | ExportStatus::UnknownAfterSubmit | ExportStatus::Succeeded
            )
    })
}

#[allow(clippy::too_many_arguments)]
async fn export_file<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    token: &str,
    destination: &DriveDestination,
    name: &str,
    content_type: &str,
    bytes: Vec<u8>,
    key: &str,
    binding: &DriveBinding,
) -> Result<DriveFile, FileExportFailure> {
    ledger.begin_attempt(key, now_rfc3339());
    ledger.label_attempt(key, name);
    ledger
        .entries
        .get_mut(key)
        .expect("pending OneDrive file")
        .drive = Some(binding.clone());
    ledger
        .checkpoint()
        .await
        .map_err(FileExportFailure::History)?;
    let outcome = upload_file(client, token, destination, name, content_type, bytes).await;
    let (id, url) = outcome.as_ref().map_or((None, None), |file| {
        (Some(file.item_id.as_str()), file.web_url.as_deref())
    });
    checkpoint_upload(ledger, key, id, url, outcome.as_ref().err())
        .await
        .map_err(FileExportFailure::History)?;
    outcome.map_err(|failure| {
        FileExportFailure::Remote(match failure {
            UploadFailure::Unconfirmed => {
                format!("Check for {name} in OneDrive and resolve its outcome in Export history.")
            }
            UploadFailure::Rejected(kind) => {
                format!("OneDrive declined the upload ({}).", kind.code())
            }
        })
    })
}

async fn checkpoint_upload(
    ledger: &mut ExportLedger,
    key: &str,
    id: Option<&str>,
    url: Option<&str>,
    failure: Option<&UploadFailure>,
) -> Result<(), String> {
    match failure {
        None => ledger.record_success(
            key,
            id.map(str::to_owned),
            url.and_then(microsoft_web_url),
            now_rfc3339(),
        ),
        Some(UploadFailure::Rejected(kind)) => ledger.record_failure(
            key,
            kind.export_status(),
            Some(kind.code().into()),
            now_rfc3339(),
        ),
        Some(UploadFailure::Unconfirmed) => ledger.record_failure(
            key,
            ExportStatus::UnknownAfterSubmit,
            Some("submission_unconfirmed".into()),
            now_rfc3339(),
        ),
    }
    ledger.checkpoint().await
}

async fn export_sharing_link<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    token: &str,
    file: &DriveFile,
    key: &str,
) -> Result<Option<String>, FileExportFailure> {
    if let Some(entry) = ledger.already_succeeded(key) {
        return Ok(entry.web_url.clone());
    }
    // Graph returns the existing link when the same app requests the same
    // sharing type. Resuming this step does not create another permission.
    let result = create_organization_view_link(client, token, file).await;
    match result {
        Ok(link) => {
            let link = microsoft_web_url(&link);
            ledger.record_success(key, Some(file.item_id.clone()), link.clone(), now_rfc3339());
            ledger
                .checkpoint()
                .await
                .map_err(FileExportFailure::History)?;
            Ok(link)
        }
        Err(_) => {
            ledger.record_failure(
                key,
                ExportStatus::Failed,
                Some("sharing_link_failed".into()),
                now_rfc3339(),
            );
            ledger
                .checkpoint()
                .await
                .map_err(FileExportFailure::History)?;
            Err(FileExportFailure::Remote(
                "Could not create the organization link.".into(),
            ))
        }
    }
}

fn exported_file(
    kind: &str,
    file: &DriveFile,
    sharing_link: Option<String>,
) -> OneDriveExportedFile {
    OneDriveExportedFile {
        kind: kind.to_string(),
        drive_id: file.drive_id.clone(),
        item_id: file.item_id.clone(),
        name: file.name.clone(),
        web_url: file.web_url.clone(),
        sharing_link,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::client::{RetryPolicy, Sleeper};
    use crate::exports::transport::{GraphBinaryResponse, GraphResponse, MockGraphTransport};
    use async_trait::async_trait;
    use std::time::Duration;

    #[derive(Default)]
    struct InstantSleeper;

    #[async_trait]
    impl Sleeper for InstantSleeper {
        async fn sleep(&self, _: Duration) {}
    }

    fn client(transport: MockGraphTransport) -> GraphClient<MockGraphTransport, InstantSleeper> {
        GraphClient::new(transport, InstantSleeper, RetryPolicy::default())
    }

    fn parent_destination() -> DriveDestination {
        DriveDestination {
            drive_id: "drive-1".to_string(),
            item_id: "root-id".to_string(),
            name: "OneDrive".to_string(),
            web_url: None,
            source: "default_drive_root".to_string(),
        }
    }

    fn queue_confirmed_file(transport: &MockGraphTransport, id: &str, name: &str, repeats: usize) {
        let body = serde_json::json!({ "id": id, "name": name, "file": {},
            "parentReference": { "id": "root-id", "driveId": "drive-1" } })
        .to_string();
        transport.queue_for_url(
            &format!("{GRAPH_BASE}/drives/drive-1/items/{id}"),
            (0..repeats).map(|_| GraphResponse::success(200, body.clone())),
        );
    }

    #[test]
    fn readable_names_keep_the_same_reservation_and_distinguish_revisions() {
        let mut ledger = ExportLedger::new("meeting");
        let at = chrono::DateTime::parse_from_rfc3339("2026-04-03T10:22:45.123Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let first = revision_stem(&ledger, "first:docx", "Weekly notes", at);
        assert_eq!(first, "Weekly notes - 2026-04-03 102245.123 UTC");
        ledger.begin_attempt("first:docx", None);
        ledger.label_attempt("first:docx", &format!("{first}.docx"));
        assert_eq!(
            revision_stem(
                &ledger,
                "first:docx",
                "Weekly notes",
                at + chrono::Duration::days(1)
            ),
            first
        );
        assert_eq!(
            revision_stem(&ledger, "second:docx", "Weekly notes", at),
            format!("{first} (2)")
        );
    }

    #[test]
    fn graph_path_segment_encoding_preserves_unreserved_only() {
        assert_eq!(
            encode_graph_path_segment("Meeting notes #1/next.docx"),
            "Meeting%20notes%20%231%2Fnext.docx"
        );
        assert_eq!(encode_graph_path_segment("AZaz09-._~"), "AZaz09-._~");
    }

    #[test]
    fn share_token_encoding_uses_graph_u_prefix_and_base64url() {
        assert_eq!(
            encode_sharing_url("https://example.com/shared"),
            "u!aHR0cHM6Ly9leGFtcGxlLmNvbS9zaGFyZWQ"
        );
        assert!(!encode_sharing_url("https://example.com/shared").contains('='));
    }

    #[tokio::test]
    async fn upload_file_uses_parent_path_content_endpoint() {
        let transport = MockGraphTransport::new();
        transport.queue_binary_default([GraphBinaryResponse::success(
            201,
            br#"{"id":"file-1","name":"Meeting #1.docx","webUrl":"https://example.test/file","parentReference":{"driveId":"drive-1"}}"#
                .to_vec(),
        )]);
        let c = client(transport);

        let file = upload_file(
            &c,
            "token",
            &parent_destination(),
            "Meeting #1.docx",
            DOCX_CONTENT_TYPE,
            b"docx".to_vec(),
        )
        .await
        .unwrap();

        assert_eq!(file.item_id, "file-1");
        let recorded = c.transport().recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].method, "PUT");
        assert_eq!(recorded[0].content_type, DOCX_CONTENT_TYPE);
        assert_eq!(
            recorded[0].url,
            format!("{GRAPH_BASE}/drives/drive-1/items/root-id:/Meeting%20%231.docx:/content?%40microsoft.graph.conflictBehavior=fail")
        );
    }

    #[tokio::test]
    async fn pdf_conversion_uses_format_query_on_uploaded_docx() {
        let transport = MockGraphTransport::new();
        transport.queue_binary_default([GraphBinaryResponse::success(200, b"%PDF-1.7".to_vec())]);
        let c = client(transport);
        let file = DriveFile {
            drive_id: "drive-1".to_string(),
            item_id: "docx-1".to_string(),
            name: "Meeting.docx".to_string(),
            web_url: None,
        };

        let pdf = convert_docx_to_pdf(&c, "token", &file).await.unwrap();

        assert_eq!(pdf, b"%PDF-1.7".to_vec());
        let recorded = c.transport().recorded();
        assert_eq!(recorded[0].method, "GET");
        assert_eq!(
            recorded[0].url,
            format!("{GRAPH_BASE}/drives/drive-1/items/docx-1/content?format=pdf")
        );
    }

    #[tokio::test]
    async fn pdf_retry_after_restart_reuses_the_confirmed_word_document() {
        let folder = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        let transport = MockGraphTransport::new();
        transport.queue_binary_default([
            GraphBinaryResponse::success(201, br#"{"id":"word-id","name":"Notes.docx"}"#.to_vec()),
            GraphBinaryResponse::failure(400, None),
            GraphBinaryResponse::success(200, b"%PDF-1.7".to_vec()),
            GraphBinaryResponse::success(201, br#"{"id":"pdf-id","name":"Notes.pdf"}"#.to_vec()),
        ]);
        let client = client(transport);
        let account = ExportContext {
            tenant_id: "tenant",
            user_id: "user",
            bearer_token: "<redacted>",
        };
        let request = OneDriveExportRequest {
            meeting_id: "meeting".into(),
            meeting_title: "Notes".into(),
            markdown: "Reviewed content".into(),
            transcript: None,
            destination: Some(parent_destination()),
            folder_name: None,
            include_pdf: true,
            create_organization_link: false,
        };
        let first = export_meeting_files(&client, &mut ledger, &account, request.clone())
            .await
            .unwrap();
        assert_eq!(first.files.len(), 1);
        assert_eq!(first.files[0].item_id, "word-id");
        assert_eq!(first.issues.len(), 1);

        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        queue_confirmed_file(client.transport(), "word-id", "Notes.docx", 2);
        queue_confirmed_file(client.transport(), "pdf-id", "Notes.pdf", 1);
        let second = export_meeting_files(&client, &mut ledger, &account, request.clone())
            .await
            .unwrap();
        assert!(second.issues.is_empty());
        assert_eq!(
            second
                .files
                .iter()
                .map(|file| file.item_id.as_str())
                .collect::<Vec<_>>(),
            ["word-id", "pdf-id"]
        );
        assert_eq!(
            client
                .transport()
                .recorded()
                .iter()
                .map(|request| request.method.as_str())
                .collect::<Vec<_>>(),
            ["PUT", "GET", "GET", "GET", "PUT"]
        );
        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        assert!(
            export_meeting_files(&client, &mut ledger, &account, request)
                .await
                .unwrap()
                .issues
                .is_empty()
        );
        assert_eq!(client.transport().recorded().len(), 7);
    }

    #[tokio::test]
    async fn unconfirmed_upload_reuses_the_found_file_after_reconciliation() {
        let folder = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        let transport = MockGraphTransport::new();
        transport.queue_binary_default([GraphBinaryResponse::failure(503, None)]);
        let client = client(transport);
        let account = ExportContext {
            tenant_id: "tenant",
            user_id: "user",
            bearer_token: "<redacted>",
        };
        let request = OneDriveExportRequest {
            meeting_id: "meeting".into(),
            meeting_title: "Notes".into(),
            markdown: "Reviewed content".into(),
            transcript: None,
            destination: Some(parent_destination()),
            folder_name: None,
            include_pdf: false,
            create_organization_link: false,
        };
        let first = export_meeting_files(&client, &mut ledger, &account, request.clone())
            .await
            .unwrap();
        assert!(first.files.is_empty());
        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        let key = ledger.entries.keys().next().unwrap().clone();
        assert_eq!(
            ledger.entries[&key].status,
            ExportStatus::UnknownAfterSubmit
        );
        let blocked = export_meeting_files(&client, &mut ledger, &account, request.clone())
            .await
            .unwrap();
        assert!(blocked.files.is_empty());
        queue_confirmed_file(client.transport(), "uploaded-word", "Notes.docx", 2);
        super::super::resolution::reconcile(
            &client,
            &mut ledger,
            &key,
            &account,
            Some("uploaded-word"),
        )
        .await
        .unwrap();
        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        let confirmed = export_meeting_files(
            &client,
            &mut ledger,
            &account,
            OneDriveExportRequest {
                include_pdf: true,
                create_organization_link: true,
                ..request
            },
        )
        .await
        .unwrap();
        assert_eq!(confirmed.files[0].item_id, "uploaded-word");
        assert_eq!(confirmed.files[0].name, "Notes.docx");
        let calls = client.transport().recorded();
        assert!(calls.iter().any(|call| {
            call.method == "GET" && call.url.ends_with("/uploaded-word/content?format=pdf")
        }));
        assert!(calls.iter().any(|call| {
            call.method == "POST" && call.url.ends_with("/uploaded-word/createLink")
        }));
        assert_eq!(calls.iter().filter(|call| call.method == "PUT").count(), 1);
    }
}
