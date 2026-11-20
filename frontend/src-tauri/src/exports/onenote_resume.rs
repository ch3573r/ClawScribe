//! Durable section reservation and resume for the normal OneNote export flow.

use sha2::{Digest, Sha256};

use super::client::{GraphClient, Sleeper};
use super::exporter::{self, ExportContext, ExportReport, OneNoteTarget};
use super::ledger::{valid_remote_id, ExportLedger, OneNoteOptions, OneNoteSection};
use super::transport::GraphTransport;

pub(super) struct SectionRequest {
    pub notebook_id: String,
    pub section_name: String,
    pub page_title: String,
    pub markdown: String,
    pub options: Option<OneNoteOptions>,
    pub resume_id: Option<String>,
}

pub(super) async fn export_section<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    account: &ExportContext<'_>,
    request: SectionRequest,
) -> Result<ExportReport, String> {
    let content_hash = format!("{:x}", Sha256::digest(request.markdown.as_bytes()));
    let (section_id, title) = if let Some(resume_id) = request.resume_id {
        let section = ledger
            .onenote_sections
            .get(&resume_id)
            .filter(|section| {
                section.tenant_id == account.tenant_id && section.user_id == account.user_id
            })
            .ok_or("This section belongs to a different meeting or Microsoft account.")?;
        if section.content_hash != content_hash {
            return Err("Meeting content has changed since this export. Restore the original content or choose Create another section.".into());
        }
        let id = section.section_id.clone().ok_or("The section creation outcome is unknown. Check OneNote before creating another section.")?;
        (id, section.page_title.clone())
    } else {
        if !valid_remote_id(&request.notebook_id)
            || request.page_title.trim().is_empty()
            || request.page_title.len() > 4096
            || request.page_title.chars().any(char::is_control)
            || request.markdown.trim().is_empty()
        {
            return Err(
                "Choose a notebook and enter a page title for the content you want to export."
                    .into(),
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
        (section.id, request.page_title)
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
    let report = exporter::export_onenote(
        client,
        ledger,
        &meeting,
        &OneNoteTarget { section_id },
        account,
    )
    .await;
    ledger.checkpoint().await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::client::{RetryPolicy, TokioSleeper};
    use crate::exports::model::ExportStatus;
    use crate::exports::transport::{GraphResponse, MockGraphTransport};

    const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

    fn request(resume_id: Option<String>) -> SectionRequest {
        SectionRequest {
            notebook_id: "notebook".into(),
            section_name: "Weekly sync".into(),
            page_title: "Release follow-ups".into(),
            markdown: "## Decisions\n- Send the draft".into(),
            options: None,
            resume_id,
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
        assert_eq!(client.transport().recorded().len(), 3);
    }
}
