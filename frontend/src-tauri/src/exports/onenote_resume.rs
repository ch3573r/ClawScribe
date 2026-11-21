//! Durable section reservation and resume for the normal OneNote export flow.

use sha2::{Digest, Sha256};

use super::client::{GraphClient, GraphOutcome, Sleeper};
use super::exporter::{
    self, from_ledger, graph_url_with_path_segments, json_request, now_rfc3339, ExportContext,
    ExportReport, OneNoteTarget,
};
use super::ledger::{valid_remote_id, ExportLedger, OneNoteOptions, OneNoteSection};
use super::model::{ExportStatus, MeetingExport, MicrosoftConnectionState};
use super::onenote::{self, DEFAULT_PAGE_BUDGET_BYTES};
use super::transport::{GraphRequest, GraphTransport};

pub(super) const UNCONFIRMED_UPDATE: &str = "onenote_page_update_unconfirmed";

pub(super) struct SectionRequest {
    pub notebook_id: String,
    pub section_name: String,
    pub page_title: String,
    pub markdown: String,
    pub options: Option<OneNoteOptions>,
    pub resume_id: Option<String>,
    pub replace_existing: bool,
}

pub(super) async fn export_section<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    account: &ExportContext<'_>,
    request: SectionRequest,
) -> Result<ExportReport, String> {
    if request.page_title.trim().is_empty()
        || request.page_title.len() > 4096
        || request.page_title.chars().any(char::is_control)
        || request.markdown.trim().is_empty()
    {
        return Err("Enter a page title and choose the content you want to export.".into());
    }
    let content_hash = format!("{:x}", Sha256::digest(request.markdown.as_bytes()));
    let (attempt_id, section_id, title) = if let Some(resume_id) = &request.resume_id {
        let section = ledger
            .onenote_sections
            .get(resume_id)
            .filter(|section| {
                section.tenant_id == account.tenant_id && section.user_id == account.user_id
            })
            .ok_or("This section belongs to a different meeting or Microsoft account.")?;
        let id = section
            .section_id
            .clone()
            .ok_or("Check OneNote and resolve this section in Export history.")?;
        if !valid_remote_id(&id) {
            return Err("Invalid saved OneNote section reference.".into());
        }
        if !super::resolution::section_is_in_notebook(client, account, &id, &section.notebook_id)
            .await?
        {
            return Err("The saved OneNote section is missing or in a different notebook. Review its link in Export history.".into());
        }
        let changed =
            section.content_hash != content_hash || section.page_title != request.page_title;
        if !request.replace_existing || !changed {
            let prefix = format!("onenote:{}:{}:{id}:", account.tenant_id, account.user_id);
            if ledger
                .entries
                .range(prefix.clone()..)
                .take_while(|(key, _)| key.starts_with(&prefix))
                .any(|(_, entry)| entry.code.as_deref() == Some(UNCONFIRMED_UPDATE))
            {
                return Err("Review the unconfirmed OneNote update in Export history before resuming this export.".into());
            }
        }
        if request.replace_existing {
            let meeting = super::markdown_notes::meeting_export_for_onenote(
                &ledger.meeting_id,
                request.page_title.clone(),
                None,
                &request.markdown,
            );
            let report = if changed {
                replace_notes_page(
                    client,
                    ledger,
                    account,
                    &id,
                    section.notes_page_id.clone(),
                    &meeting,
                )
                .await?
            } else {
                export_saved_section_notes(client, ledger, &meeting, &id, account).await
            };
            if report.overall == ExportStatus::Succeeded {
                let section = ledger
                    .onenote_sections
                    .get_mut(resume_id)
                    .expect("saved section exists");
                section.content_hash = content_hash;
                section.page_title = request.page_title;
                section.options = request.options;
                section.notes_page_id = page_id_from_report(&report);
                ledger.checkpoint().await?;
            }
            return Ok(report);
        }
        if section.content_hash != content_hash {
            return Err("The meeting content has changed. Review the new content and choose Update exported page.".into());
        }
        (resume_id.clone(), id, section.page_title.clone())
    } else {
        if !valid_remote_id(&request.notebook_id) || request.replace_existing {
            return Err("Choose a notebook or a saved section.".into());
        }
        if ledger.onenote_sections.values().any(|section| {
            section.section_id.is_none()
                && section.tenant_id == account.tenant_id
                && section.user_id == account.user_id
                && section.notebook_id == request.notebook_id
                && section
                    .section_name
                    .eq_ignore_ascii_case(&request.section_name)
        }) {
            return Err(
                "Check OneNote and resolve the previous section attempt in Export history.".into(),
            );
        }
        let attempt_id = uuid::Uuid::new_v4().to_string();
        ledger.onenote_sections.insert(
            attempt_id.clone(),
            OneNoteSection {
                tenant_id: account.tenant_id.into(),
                user_id: account.user_id.into(),
                notebook_id: request.notebook_id.clone(),
                section_id: None,
                section_name: request.section_name.clone(),
                page_title: request.page_title.clone(),
                content_hash,
                options: request.options,
                started_at: chrono::Utc::now().to_rfc3339(),
                notes_page_id: None,
            },
        );
        // Persist before the remote create, then persist the returned section ID
        // before any page requests. A lost response is visible for manual review.
        ledger.checkpoint().await?;
        let section = match super::discovery::create_section_checked(
            client,
            account.bearer_token,
            &request.notebook_id,
            &request.section_name,
        )
        .await
        {
            Ok(section) => section,
            Err(error) => {
                if !error.outcome_unknown {
                    ledger.onenote_sections.remove(&attempt_id);
                    ledger.checkpoint().await?;
                }
                return Err(error.message);
            }
        };
        ledger
            .onenote_sections
            .get_mut(&attempt_id)
            .expect("reserved section attempt")
            .section_id = Some(section.id.clone());
        ledger.checkpoint().await?;
        (attempt_id, section.id, request.page_title)
    };
    if !valid_remote_id(&section_id) {
        return Err("Invalid saved OneNote section reference.".into());
    }
    let meeting = super::markdown_notes::meeting_export_for_onenote(
        &ledger.meeting_id,
        title,
        None,
        &request.markdown,
    );
    let report = export_saved_section_notes(client, ledger, &meeting, &section_id, account).await;
    if let Some(page_id) = page_id_from_report(&report) {
        ledger
            .onenote_sections
            .get_mut(&attempt_id)
            .expect("saved section exists")
            .notes_page_id = Some(page_id);
        ledger.checkpoint().await?;
    }
    Ok(report)
}

async fn export_saved_section_notes<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    meeting: &MeetingExport,
    section_id: &str,
    account: &ExportContext<'_>,
) -> ExportReport {
    let revision = onenote::notes_revision(meeting);
    let target = OneNoteTarget {
        section_id: section_id.into(),
    };
    let prefix = format!(
        "onenote:{}:{}:{section_id}:",
        account.tenant_id, account.user_id
    );
    let legacy = meeting.artifact_hash();
    let revision = if ledger
        .entry(&format!("{prefix}{revision}:notes:0"))
        .is_none()
        && ledger.entry(&format!("{prefix}{legacy}:notes:0")).is_some()
    {
        &legacy
    } else {
        &revision
    };
    exporter::export_onenote_revision(client, ledger, meeting, &target, account, revision).await
}

fn page_id_from_report(report: &ExportReport) -> Option<String> {
    report
        .items
        .iter()
        .find(|item| item.local_id == "notes-0" && item.status == ExportStatus::Succeeded)
        .and_then(|item| item.resource_id.clone())
}

// The saved-section flow renders the reviewed summary/transcript selection into
// one notes page. Find it by local history and access Graph by page ID.
async fn replace_notes_page<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    account: &ExportContext<'_>,
    section_id: &str,
    saved_page_id: Option<String>,
    meeting: &MeetingExport,
) -> Result<ExportReport, String> {
    let prefix = format!(
        "onenote:{}:{}:{section_id}:",
        account.tenant_id, account.user_id
    );
    let revision = onenote::notes_revision(meeting);
    let key = format!("{prefix}{revision}:notes:0");
    let mut pages = std::collections::BTreeMap::new();
    for (history_key, entry) in ledger.entries.range(prefix.clone()..) {
        if !history_key.starts_with(&prefix) {
            break;
        }
        if !history_key.ends_with(":notes:0") {
            continue;
        }
        let retrying_same_update =
            history_key == &key && entry.code.as_deref() == Some(UNCONFIRMED_UPDATE);
        if (!retrying_same_update
            && matches!(
                entry.status,
                ExportStatus::UnknownAfterSubmit | ExportStatus::Pending
            ))
            || (entry.status == ExportStatus::Succeeded && entry.resource_id.is_none())
        {
            return Err(
                "Check the previous page attempt in Export history before updating this section."
                    .into(),
            );
        }
        if let Some(id) = &entry.resource_id {
            let url = pages.entry(id.clone()).or_insert(None);
            if entry.web_url.is_some() {
                *url = entry.web_url.clone();
            }
        }
    }
    // A prior reviewed re-export in this section may already have created
    // a separate replacement page for this exact revision.
    if let Some(entry) = ledger.already_succeeded(&key) {
        if entry.resource_id.is_some()
            && entry.resource_id != saved_page_id
            && (saved_page_id.is_some() || pages.len() > 1)
        {
            return Ok(ExportReport {
                overall: ExportStatus::Succeeded,
                connection_state: None,
                items: vec![from_ledger(key, "notes-0".into(), ledger)],
            });
        }
    }
    let (page_id, web_url) = if let Some(id) = saved_page_id {
        let url = pages
            .remove(&id)
            .ok_or("The saved notes page is missing from export history.")?;
        (id, url)
    } else if pages.len() == 1 {
        pages.into_iter().next().expect("one known page")
    } else if pages.is_empty() {
        return Ok(exporter::export_onenote_revision(
            client,
            ledger,
            meeting,
            &OneNoteTarget {
                section_id: section_id.into(),
            },
            account,
            &revision,
        )
        .await);
    } else {
        return Err("This section has multiple exported notes pages. Choose another section for these edits.".into());
    };
    let page = onenote::build_pages(meeting, DEFAULT_PAGE_BUDGET_BYTES)
        .into_iter()
        .next()
        .expect("notes page");
    if !super::resolution::page_is_in_section(client, account, &page_id, section_id).await? {
        return Err("The saved OneNote page is missing or in a different section. Review its link in Export history.".into());
    }
    let retrying_update = ledger
        .entry(&key)
        .is_some_and(|entry| entry.code.as_deref() == Some(UNCONFIRMED_UPDATE));
    let marker_prefix = onenote::notes_marker_prefix(&meeting.meeting_id);
    let outcome = match prepare_notes_patch(client, account.bearer_token, &page_id, &page, &marker_prefix).await {
        Ok(None) if !retrying_update => {
            return Ok(exporter::export_onenote_revision(
                client,
                ledger,
                meeting,
                &OneNoteTarget {
                    section_id: section_id.into(),
                },
                account,
                &revision,
            )
            .await)
        }
        Ok(None) => return Err("The last OneNote update is unconfirmed. Check the page and review its link in Export history.".into()),
        Ok(Some(patch)) => {
            ledger.begin_attempt(&key, now_rfc3339());
            ledger.label_attempt(&key, &page.title);
            let entry = ledger.entries.get_mut(&key).expect("pending OneNote update");
            entry.resource_id = Some(page_id.clone());
            entry.web_url.clone_from(&web_url);
            entry.status = ExportStatus::UnknownAfterSubmit;
            entry.code = Some(UNCONFIRMED_UPDATE.into());
            ledger.checkpoint().await?;
            graph_body(client.execute(&patch, account.bearer_token).await).map(|_| ())
        }
        Err(status) => Err(status),
    };
    let (status, connection_state) = match outcome {
        Ok(()) => {
            ledger.record_success(&key, Some(page_id), web_url, now_rfc3339());
            ledger.checkpoint().await?;
            (ExportStatus::Succeeded, None)
        }
        Err(status) => {
            // The original page remains associated with its stable ID. Replacing
            // the same content block is safe after a lost update response.
            let connection =
                (status == ExportStatus::FailedAuth).then_some(MicrosoftConnectionState::Expired);
            let unconfirmed = ledger
                .entry(&key)
                .is_some_and(|entry| entry.code.as_deref() == Some(UNCONFIRMED_UPDATE));
            let status = if unconfirmed && status != ExportStatus::FailedAuth {
                ExportStatus::UnknownAfterSubmit
            } else {
                status
            };
            return Ok(ExportReport {
                overall: status,
                connection_state: connection,
                items: vec![exporter::ItemResult {
                    dedupe_key: key,
                    local_id: "notes-0".into(),
                    status,
                    resource_id: Some(page_id),
                    web_url,
                    code: Some(
                        if unconfirmed {
                            UNCONFIRMED_UPDATE
                        } else {
                            "onenote_page_update_failed"
                        }
                        .into(),
                    ),
                    graph_called: false,
                }],
            });
        }
    };
    let report = ExportReport {
        overall: status,
        connection_state,
        items: vec![from_ledger(key, "notes-0".into(), ledger)],
    };
    Ok(report)
}

async fn prepare_notes_patch<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    page_id: &str,
    page: &onenote::OneNotePage,
    marker_prefix: &str,
) -> Result<Option<GraphRequest>, ExportStatus> {
    if !valid_remote_id(page_id) {
        return Err(ExportStatus::DestinationNotFound);
    }
    let endpoint = graph_url_with_path_segments(&["me", "onenote", "pages", page_id, "content"]);
    let html = read_page_html(client, token, &endpoint).await?;
    let Some((target, _)) = editable_container(&html, marker_prefix) else {
        return Ok(None);
    };
    let body = page
        .xhtml
        .split_once("<body><div>")
        .and_then(|(_, body)| body.strip_suffix("</div></body></html>"))
        .ok_or(ExportStatus::Failed)?;
    Ok(Some(json_request(
        "PATCH",
        endpoint,
        serde_json::json!([
            {"target":"title", "action":"replace", "content": page.title},
            {"target": target, "action":"replace", "content": body},
        ]),
    )))
}

async fn read_page_html<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    endpoint: &str,
) -> Result<String, ExportStatus> {
    let read = json_request(
        "GET",
        format!("{endpoint}?includeIDs=true"),
        serde_json::Value::Null,
    );
    graph_body(client.execute(&read, token).await)
}

pub(super) async fn verify_updated_notes<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    token: &str,
    page: &str,
    meeting: &str,
    revision: &str,
) -> Result<(), String> {
    let endpoint = graph_url_with_path_segments(&["me", "onenote", "pages", page, "content"]);
    let html = read_page_html(client, token, &endpoint)
        .await
        .map_err(|_| "Could not verify the OneNote revision. Try again.")?;
    let prefix = onenote::notes_marker_prefix(meeting);
    if editable_container(&html, &prefix)
        .is_some_and(|(_, marker)| marker == format!("{prefix}{revision}"))
    {
        Ok(())
    } else {
        Err("This page does not contain the pending revision. Review the draft and retry the OneNote update.".into())
    }
}

fn graph_body(outcome: GraphOutcome) -> Result<String, ExportStatus> {
    match outcome {
        GraphOutcome::Success(response) => Ok(response.body),
        GraphOutcome::Failed(kind, _) => Err(kind.export_status()),
        GraphOutcome::Unknown(_) => Err(ExportStatus::Failed),
    }
}

fn editable_container(html: &str, prefix: &str) -> Option<(String, String)> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(html);
    let mut parents = Vec::new();
    let mut target = None;
    loop {
        match reader.read_event().ok()? {
            Event::Start(tag) => {
                let is_div = tag.name().as_ref() == b"div";
                if is_div {
                    let marker = tag.try_get_attribute("data-id").ok()?.and_then(|attr| {
                        attr.decode_and_unescape_value(reader.decoder())
                            .ok()
                            .map(|value| value.into_owned())
                    });
                    if let Some(marker) = marker.filter(|marker| {
                        marker.strip_prefix(prefix).is_some_and(|revision| {
                            revision.len() == 64
                                && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                    }) {
                        if target.is_some() || parents.last() != Some(&true) {
                            return None;
                        }
                        let id = tag
                            .try_get_attribute("id")
                            .ok()??
                            .decode_and_unescape_value(reader.decoder())
                            .ok()?
                            .into_owned();
                        if !id.strip_prefix("div:{").is_some_and(|rest| {
                            rest.ends_with('}')
                                && rest.contains("}{")
                                && rest.bytes().all(|byte| {
                                    byte.is_ascii_alphanumeric()
                                        || matches!(byte, b'{' | b'}' | b'-')
                                })
                        }) {
                            return None;
                        }
                        target = Some((id, marker));
                    }
                }
                parents.push(is_div);
            }
            Event::End(_) => {
                parents.pop()?;
            }
            Event::Eof => return if parents.is_empty() { target } else { None },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::client::{RetryPolicy, TokioSleeper};
    use crate::exports::model::ExportStatus;
    use crate::exports::transport::{GraphResponse, MockGraphTransport};

    const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";
    const SECTION_DETAILS: &str =
        "https://graph.microsoft.com/v1.0/me/onenote/sections/section-a?$expand=parentNotebook";
    const PAGE_DETAILS: &str =
        "https://graph.microsoft.com/v1.0/me/onenote/pages/page-a?$expand=parentSection";

    fn queue_section_details(transport: &MockGraphTransport, repeats: usize) {
        transport.queue_for_url(SECTION_DETAILS, (0..repeats).map(|_| GraphResponse::success(200,
            r#"{"id":"section-a","displayName":"Weekly sync","parentNotebook":{"id":"notebook"}}"#)));
    }

    fn page_details() -> GraphResponse {
        GraphResponse::success(
            200,
            r#"{"id":"page-a","title":"Release follow-ups","parentSection":{"id":"section-a"}}"#,
        )
    }

    #[test]
    fn only_one_owned_nested_one_note_container_is_editable() {
        let prefix = onenote::notes_marker_prefix("meeting");
        let marker = format!("{prefix}{}", "a".repeat(64));
        let owned = format!(
            r#"<div id="div:{{abc}}{{14}}" data-id="{marker}"><p>Recorded notes</p></div>"#
        );
        assert_eq!(
            editable_container(&format!("<div>{owned}</div>"), &prefix),
            Some(("div:{abc}{14}".into(), marker))
        );
        for html in [
            format!("<body>{owned}</body>"),
            format!("<div>{owned}{owned}</div>"),
            format!("<div><!-- {owned} --></div>"),
            format!("<div>{owned}</div>").replace(&prefix, "other-meeting-"),
            format!("<div>{owned}</div>").replace("div:{abc}{14}", "custom-id"),
        ] {
            assert!(editable_container(&html, &prefix).is_none());
        }
    }

    #[tokio::test]
    async fn one_note_creates_one_separate_revision_when_the_saved_block_is_uneditable() {
        let temp = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::load_or_new(temp.path(), "meeting").unwrap();
        let transport = MockGraphTransport::new();
        let sections = format!("{GRAPH_BASE}/me/onenote/notebooks/notebook/sections");
        let pages = format!("{GRAPH_BASE}/me/onenote/sections/section-a/pages");
        let content = format!("{GRAPH_BASE}/me/onenote/pages/page-a/content?includeIDs=true");
        let marker = format!(
            "{}{}",
            onenote::notes_marker_prefix("meeting"),
            "a".repeat(64)
        );
        let html = format!(
            r#"<body><div data-id="{marker}" id="div:{{abc}}{{14}}"><p>Old</p></div></body>"#
        );
        transport.queue_for_url(
            &sections,
            [GraphResponse::success(
                201,
                r#"{"id":"section-a","displayName":"Weekly sync"}"#,
            )],
        );
        transport.queue_for_url(
            &pages,
            [
                GraphResponse::success(201, r#"{"id":"page-a"}"#),
                GraphResponse::success(201, r#"{"id":"page-b"}"#),
            ],
        );
        transport.queue_for_url(&content, [GraphResponse::success(200, html.as_str())]);
        queue_section_details(&transport, 2);
        transport.queue_for_url(PAGE_DETAILS, [page_details()]);
        let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
        let account = ExportContext {
            tenant_id: "tenant",
            user_id: "user",
            bearer_token: "<redacted>",
        };
        export_section(&client, &mut ledger, &account, request(None))
            .await
            .unwrap();
        let id = ledger.onenote_sections.keys().next().unwrap().clone();
        for _ in 0..2 {
            let mut ledger = ExportLedger::load_or_new(temp.path(), "meeting").unwrap();
            let mut revision = request(Some(id.clone()));
            revision.replace_existing = true;
            revision.markdown.push_str("\n- Update the reviewed draft");
            let report = export_section(&client, &mut ledger, &account, revision)
                .await
                .unwrap();
            assert_eq!(report.items[0].resource_id.as_deref(), Some("page-b"));
        }
        assert_eq!(client.transport().calls_for(&pages), 2);
        assert!(client
            .transport()
            .recorded()
            .iter()
            .all(|call| call.method != "PATCH"));
    }

    #[tokio::test]
    async fn losing_the_owned_block_after_an_unconfirmed_update_never_creates_a_new_page() {
        let temp = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::load_or_new(temp.path(), "meeting").unwrap();
        let transport = MockGraphTransport::new();
        let sections = format!("{GRAPH_BASE}/me/onenote/notebooks/notebook/sections");
        let pages = format!("{GRAPH_BASE}/me/onenote/sections/section-a/pages");
        let content = format!("{GRAPH_BASE}/me/onenote/pages/page-a/content");
        let marker = format!(
            "{}{}",
            onenote::notes_marker_prefix("meeting"),
            "a".repeat(64)
        );
        let html = format!(
            r#"<div><div data-id="{marker}" id="div:{{abc}}{{14}}">Old content</div></div>"#
        );
        transport.queue_for_url(
            &sections,
            [GraphResponse::success(
                201,
                r#"{"id":"section-a","displayName":"Weekly sync"}"#,
            )],
        );
        transport.queue_for_url(&pages, [GraphResponse::success(201, r#"{"id":"page-a"}"#)]);
        transport.queue_for_url(
            &format!("{content}?includeIDs=true"),
            [
                GraphResponse::success(200, html),
                GraphResponse::success(200, "<div><p>Missing marker</p></div>"),
            ],
        );
        transport.queue_for_url(&content, [GraphResponse::failure(503, None)]);
        queue_section_details(&transport, 2);
        transport.queue_for_url(PAGE_DETAILS, [page_details(), page_details()]);
        let client = GraphClient::new(
            transport,
            TokioSleeper,
            RetryPolicy {
                max_attempts: 1,
                ..RetryPolicy::default()
            },
        );
        let account = ExportContext {
            tenant_id: "tenant",
            user_id: "user",
            bearer_token: "<redacted>",
        };
        export_section(&client, &mut ledger, &account, request(None))
            .await
            .unwrap();
        let id = ledger.onenote_sections.keys().next().unwrap().clone();
        let revised = || {
            let mut revision = request(Some(id.clone()));
            revision.replace_existing = true;
            revision.markdown.push_str("\n- Reviewed edits");
            revision
        };
        let report = export_section(&client, &mut ledger, &account, revised())
            .await
            .unwrap();
        assert_eq!(report.overall, ExportStatus::UnknownAfterSubmit);
        let mut ledger = ExportLedger::load_or_new(temp.path(), "meeting").unwrap();
        assert!(export_section(&client, &mut ledger, &account, revised())
            .await
            .unwrap_err()
            .contains("unconfirmed"));
        assert_eq!(client.transport().calls_for(&pages), 1);
    }

    fn request(resume_id: Option<String>) -> SectionRequest {
        SectionRequest {
            notebook_id: "notebook".into(),
            section_name: "Weekly sync".into(),
            page_title: "Release follow-ups".into(),
            markdown: "## Decisions\n- Send the draft".into(),
            options: None,
            resume_id,
            replace_existing: false,
        }
    }

    #[tokio::test]
    async fn ambiguous_section_create_is_kept_and_never_replayed() {
        let temp = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::load_or_new(temp.path(), "meeting").unwrap();
        let transport = MockGraphTransport::new();
        let sections = format!("{GRAPH_BASE}/me/onenote/notebooks/notebook/sections");
        transport.queue_for_url(&sections, [GraphResponse::failure(503, None)]);
        let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
        let account = ExportContext {
            tenant_id: "tenant",
            user_id: "user",
            bearer_token: "<redacted>",
        };
        assert!(
            export_section(&client, &mut ledger, &account, request(None))
                .await
                .is_err()
        );
        let mut restored = ExportLedger::load_or_new(temp.path(), "meeting").unwrap();
        let (id, section) = restored.onenote_sections.iter().next().unwrap();
        let id = id.clone();
        assert!(section.section_id.is_none());
        assert!(
            export_section(&client, &mut restored, &account, request(Some(id)))
                .await
                .is_err()
        );
        assert_eq!(client.transport().calls_for(&sections), 1);
    }

    #[tokio::test]
    async fn resumes_by_saved_id_after_restart_without_creating_or_listing_sections() {
        let temp = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::load_or_new(temp.path(), "meeting").unwrap();
        let transport = MockGraphTransport::new();
        let sections = format!("{GRAPH_BASE}/me/onenote/notebooks/notebook/sections");
        let pages = format!("{GRAPH_BASE}/me/onenote/sections/section-a/pages");
        queue_section_details(&transport, 3);
        transport.queue_for_url(
            &sections,
            [GraphResponse::success(
                201,
                r#"{"id":"section-a","displayName":"Weekly sync"}"#,
            )],
        );
        transport.queue_for_url(
            &pages,
            [
                GraphResponse::success(400, "{}"),
                GraphResponse::success(201, r#"{"id":"page-a"}"#),
            ],
        );
        let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
        let account = ExportContext {
            tenant_id: "tenant",
            user_id: "user",
            bearer_token: "<redacted>",
        };
        let failed = export_section(&client, &mut ledger, &account, request(None))
            .await
            .unwrap();
        assert_eq!(failed.overall, ExportStatus::Failed);

        let mut ledger = ExportLedger::load_or_new(temp.path(), "meeting").unwrap();
        let id = ledger.onenote_sections.keys().next().unwrap().clone();
        let mut changed = request(Some(id.clone()));
        changed.markdown.push_str("\n- Changed after export");
        assert!(export_section(&client, &mut ledger, &account, changed)
            .await
            .is_err());
        let resumed = export_section(&client, &mut ledger, &account, request(Some(id.clone())))
            .await
            .unwrap();
        assert_eq!(resumed.overall, ExportStatus::Succeeded);
        let repeated = export_section(&client, &mut ledger, &account, request(Some(id)))
            .await
            .unwrap();
        assert!(!repeated.items[0].graph_called);
        assert_eq!(client.transport().calls_for(&sections), 1);
        assert_eq!(client.transport().calls_for(&pages), 2);
        assert_eq!(client.transport().recorded().len(), 6);
    }

    #[tokio::test]
    async fn reviewed_revisions_replace_the_saved_page_by_id_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::load_or_new(temp.path(), "meeting").unwrap();
        let transport = MockGraphTransport::new();
        let sections = format!("{GRAPH_BASE}/me/onenote/notebooks/notebook/sections");
        let pages = format!("{GRAPH_BASE}/me/onenote/sections/section-a/pages");
        let content = format!("{GRAPH_BASE}/me/onenote/pages/page-a/content");
        let marker = format!(
            "{}{}",
            onenote::notes_marker_prefix("meeting"),
            "a".repeat(64)
        );
        let html = format!(
            r#"<div data-id="_default"><p>User-owned text</p><div data-id="{marker}" id="div:{{abc}}{{14}}"><p>Old</p></div></div>"#
        );
        queue_section_details(&transport, 1);
        transport.queue_for_url(PAGE_DETAILS, [page_details()]);
        transport.queue_for_url(
            &sections,
            [GraphResponse::success(
                201,
                r#"{"id":"section-a","displayName":"Weekly sync"}"#,
            )],
        );
        transport.queue_for_url(&pages, [GraphResponse::success(201, r#"{"id":"page-a"}"#)]);
        transport.queue_for_url(
            &format!("{content}?includeIDs=true"),
            [
                GraphResponse::success(200, html.as_str()),
                GraphResponse::success(200, html.as_str()),
            ],
        );
        transport.queue_for_url(&content, [GraphResponse::success(204, "")]);
        let client = GraphClient::new(transport, TokioSleeper, RetryPolicy::default());
        let account = ExportContext {
            tenant_id: "tenant",
            user_id: "user",
            bearer_token: "<redacted>",
        };
        export_section(&client, &mut ledger, &account, request(None))
            .await
            .unwrap();
        let mut ledger = ExportLedger::load_or_new(temp.path(), "meeting").unwrap();
        let id = ledger.onenote_sections.keys().next().unwrap().clone();
        let mut revision = request(Some(id.clone()));
        revision.replace_existing = true;
        revision.page_title = "Budget & plans <draft>".into();
        revision.markdown = "## Decisions\n- Send the reviewed draft".into();
        let meeting = super::super::markdown_notes::meeting_export_for_onenote(
            "meeting",
            &revision.page_title,
            None,
            &revision.markdown,
        );
        let page = onenote::build_pages(&meeting, DEFAULT_PAGE_BUDGET_BYTES).remove(0);
        let patch = prepare_notes_patch(
            &client,
            account.bearer_token,
            "page-a",
            &page,
            &onenote::notes_marker_prefix("meeting"),
        )
        .await
        .unwrap()
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&patch.body).unwrap();
        assert_eq!(body[0]["content"], "Budget & plans <draft>");
        assert_eq!(body[1]["target"], "div:{abc}{14}");
        assert!(!body[1]["content"]
            .as_str()
            .unwrap()
            .contains("User-owned text"));
        let report = export_section(&client, &mut ledger, &account, revision)
            .await
            .unwrap();
        assert_eq!(report.overall, ExportStatus::Succeeded);
        assert_eq!(report.items[0].resource_id.as_deref(), Some("page-a"));
        let ledger = ExportLedger::load_or_new(temp.path(), "meeting").unwrap();
        assert_eq!(
            ledger.onenote_sections[&id].page_title,
            "Budget & plans <draft>"
        );
        assert_eq!(client.transport().calls_for(&sections), 1);
        assert_eq!(client.transport().calls_for(&pages), 1);
        assert_eq!(
            client
                .transport()
                .recorded()
                .iter()
                .find(|request| request.url == content)
                .unwrap()
                .method,
            "PATCH"
        );
    }
}
