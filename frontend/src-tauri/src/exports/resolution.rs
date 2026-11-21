//! Read-only destination checks for recovering interrupted or stale exports.

use serde::Serialize;
use serde_json::Value;

use super::client::{GraphClient, GraphOutcome, Sleeper};
use super::error::GraphErrorKind;
use super::exporter::{
    graph_url_with_path_segments as graph_url, json_request, now_rfc3339, ExportContext,
};
use super::ledger::{
    artifact_key_hash, microsoft_web_url, valid_remote_id, DriveBinding, ExportLedger,
};
use super::transport::GraphTransport;

pub(super) fn drive_binding(
    account: &ExportContext<'_>,
    drive: &str,
    parent: &str,
) -> DriveBinding {
    DriveBinding {
        account_hash: artifact_key_hash(&[account.tenant_id, account.user_id]),
        drive_id: drive.into(),
        parent_id: parent.into(),
    }
}

#[derive(Clone, Copy)]
enum Target<'a> {
    Planner {
        plan: &'a str,
        bucket: &'a str,
    },
    Todo {
        list: &'a str,
    },
    Section {
        notebook: &'a str,
    },
    Page {
        section: &'a str,
    },
    Drive {
        binding: &'a DriveBinding,
        kind: &'a str,
    },
}

pub(super) fn scope_for(key: &str) -> Result<&'static str, String> {
    match key.split(':').next().unwrap_or_default() {
        "planner" | "todo" => Ok("Tasks.ReadWrite"),
        "onenote" | "onenote-section" => Ok("Notes.ReadWrite.All"),
        "onedrive" => Ok("Files.ReadWrite"),
        _ => Err("This export cannot be resolved here.".into()),
    }
}

impl<'a> Target<'a> {
    fn recorded(
        ledger: &'a ExportLedger,
        key: &'a str,
        account: &ExportContext<'_>,
    ) -> Result<(Self, Option<&'a str>), String> {
        const WRONG_ACCOUNT: &str = "Reconnect the Microsoft account that started this export.";
        if let Some(id) = key.strip_prefix("onenote-section:") {
            let section = ledger
                .onenote_sections
                .get(id)
                .ok_or("This section is no longer in export history.")?;
            if section.tenant_id != account.tenant_id || section.user_id != account.user_id {
                return Err(WRONG_ACCOUNT.into());
            }
            return Ok((
                Self::Section {
                    notebook: &section.notebook_id,
                },
                section.section_id.as_deref(),
            ));
        }
        let entry = ledger
            .entry(key)
            .filter(|entry| entry.can_resolve())
            .ok_or("This export does not need reconciliation.")?;
        let fields: Vec<_> = key.split(':').collect();
        let target = match fields.as_slice() {
            ["onedrive", _, _, kind @ ("folder" | "docx" | "pdf")] => {
                let binding = entry.drive.as_ref().ok_or("Open OneDrive export with the original destination and draft to restore the saved destination reference, then try again.")?;
                if binding.account_hash != artifact_key_hash(&[account.tenant_id, account.user_id])
                {
                    return Err(WRONG_ACCOUNT.into());
                }
                Self::Drive { binding, kind }
            }
            [provider, tenant, user, destination, tail @ ..]
                if *tenant == account.tenant_id && *user == account.user_id =>
            {
                match (*provider, tail) {
                    ("planner", [bucket, ..]) => Self::Planner {
                        plan: destination,
                        bucket,
                    },
                    ("todo", _) => Self::Todo { list: destination },
                    ("onenote", _) => Self::Page {
                        section: destination,
                    },
                    _ => return Err("This export cannot be resolved here.".into()),
                }
            }
            _ => return Err(WRONG_ACCOUNT.into()),
        };
        Ok((target, entry.resource_id.as_deref()))
    }

    fn list_url(self) -> String {
        match self {
            Self::Planner { plan, .. } => graph_url(&["planner", "plans", plan, "tasks"]),
            Self::Todo { list } => graph_url(&["me", "todo", "lists", list, "tasks"]),
            Self::Section { notebook } => {
                graph_url(&["me", "onenote", "notebooks", notebook, "sections"])
            }
            Self::Page { section } => graph_url(&["me", "onenote", "sections", section, "pages"]),
            Self::Drive { binding, .. } => graph_url(&[
                "drives",
                &binding.drive_id,
                "items",
                &binding.parent_id,
                "children",
            ]),
        }
    }

    fn item_url(self, id: &str) -> String {
        match self {
            Self::Planner { .. } => graph_url(&["planner", "tasks", id]),
            Self::Todo { list } => graph_url(&["me", "todo", "lists", list, "tasks", id]),
            Self::Section { .. } => format!(
                "{}?$expand=parentNotebook",
                graph_url(&["me", "onenote", "sections", id])
            ),
            Self::Page { .. } => format!(
                "{}?$expand=parentSection",
                graph_url(&["me", "onenote", "pages", id])
            ),
            Self::Drive { binding, .. } => graph_url(&["drives", &binding.drive_id, "items", id]),
        }
    }

    fn matches(self, item: &Value, from_list: bool) -> bool {
        match self {
            Self::Planner { plan, bucket } => {
                field(item, "/planId") == Some(plan) && field(item, "/bucketId") == Some(bucket)
            }
            Self::Todo { .. } => true, // Both URLs include the signed-in user's list.
            Self::Section { notebook } => {
                from_list || field(item, "/parentNotebook/id") == Some(notebook)
            }
            Self::Page { section } => {
                from_list || field(item, "/parentSection/id") == Some(section)
            }
            Self::Drive { binding, kind } => {
                field(item, "/parentReference/driveId") == Some(binding.drive_id.as_str())
                    && field(item, "/parentReference/id") == Some(binding.parent_id.as_str())
                    && if kind == "folder" {
                        item.get("folder").is_some_and(Value::is_object)
                    } else {
                        item.get("file").is_some_and(Value::is_object)
                            && field(item, "/name")
                                .and_then(|name| name.rsplit_once('.'))
                                .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case(kind))
                    }
            }
        }
    }
}

fn field<'a>(item: &'a Value, field: &str) -> Option<&'a str> {
    item.pointer(field)?.as_str()
}

async fn read<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    account: &ExportContext<'_>,
    url: String,
) -> Result<Option<Value>, String> {
    let request = json_request("GET", url, Value::Null);
    match client.execute(&request, account.bearer_token).await {
        GraphOutcome::Success(response) => serde_json::from_str(&response.body)
            .map(Some)
            .map_err(|_| "Microsoft returned invalid details for this export.".into()),
        GraphOutcome::Failed(GraphErrorKind::NotFound, _) => Ok(None),
        GraphOutcome::Failed(kind, _) => Err(format!(
            "Could not check the export destination ({}). Try again.",
            kind.code()
        )),
        GraphOutcome::Unknown(_) => {
            Err("Could not check the export destination. Try again.".into())
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCandidate {
    id: String,
    title: String,
    created_at: Option<String>,
    web_url: Option<String>,
}

impl ExportCandidate {
    fn from_item(item: &Value) -> Option<Self> {
        let id = field(item, "/id").filter(|id| valid_remote_id(id))?;
        let title = ["/title", "/displayName", "/name"]
            .into_iter()
            .find_map(|path| field(item, path))?
            .trim();
        if title.is_empty() {
            return None;
        }
        Some(Self {
            id: id.into(),
            title: title.chars().take(240).collect(),
            created_at: field(item, "/createdDateTime")
                .filter(|time| chrono::DateTime::parse_from_rfc3339(time).is_ok())
                .map(str::to_owned),
            web_url: ["/webUrl", "/links/oneNoteWebUrl/href"]
                .into_iter()
                .find_map(|path| field(item, path).and_then(microsoft_web_url)),
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCandidates {
    items: Vec<ExportCandidate>,
    truncated: bool,
}

pub(super) async fn candidates<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &ExportLedger,
    key: &str,
    account: &ExportContext<'_>,
) -> Result<ExportCandidates, String> {
    let (target, _) = Target::recorded(ledger, key, account)?;
    let list = target.list_url();
    let endpoint = url::Url::parse(&list).expect("Graph collection URL");
    // Graph may return the same /me collection under the signed-in profile ID.
    let signed_in_user_path = endpoint
        .path()
        .strip_prefix("/v1.0/me/")
        .filter(|_| valid_remote_id(account.user_id))
        .map(|collection| format!("/v1.0/users/{}/{collection}", account.user_id));
    let mut next = Some(list);
    let mut items = Vec::new();
    for _ in 0..8 {
        let Some(url) = next.take() else { break };
        let body = read(client, account, url)
            .await?
            .ok_or("The original destination is missing or inaccessible.")?;
        let page = body
            .get("value")
            .and_then(Value::as_array)
            .ok_or("Microsoft did not return a usable list of items.")?;
        items.extend(
            page.iter()
                .filter(|item| target.matches(item, true))
                .filter_map(ExportCandidate::from_item),
        );
        next = field(&body, "/@odata.nextLink")
            .map(|link| {
                let url = url::Url::parse(link)
                    .map_err(|_| "Microsoft returned an invalid next page.")?;
                if url.origin() != endpoint.origin()
                    || (url.path() != endpoint.path()
                        && signed_in_user_path.as_deref() != Some(url.path()))
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.fragment().is_some()
                {
                    return Err("Microsoft returned a next page outside the export destination.");
                }
                Ok(url.to_string())
            })
            .transpose()?;
    }
    let name = ledger
        .entry(key)
        .and_then(|entry| entry.display_name.as_deref())
        .unwrap_or_default();
    items.sort_by(|a, b| {
        b.title
            .eq_ignore_ascii_case(name)
            .cmp(&a.title.eq_ignore_ascii_case(name))
            .then_with(|| b.created_at.cmp(&a.created_at))
            .then_with(|| a.title.cmp(&b.title))
    });
    let truncated = next.is_some() || items.len() > 100;
    items.truncate(100);
    Ok(ExportCandidates { items, truncated })
}

async fn check_item<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    account: &ExportContext<'_>,
    target: Target<'_>,
    id: &str,
) -> Result<Option<ExportCandidate>, String> {
    if !valid_remote_id(id) {
        return Ok(None);
    }
    let Some(item) = read(client, account, target.item_url(id)).await? else {
        return Ok(None);
    };
    if field(&item, "/id") != Some(id) {
        return Err("Microsoft returned an unexpected item. Export history was kept.".into());
    }
    let scope_fields: &[&str] = match target {
        Target::Planner { .. } => &["/planId", "/bucketId"],
        Target::Section { .. } => &["/parentNotebook/id"],
        Target::Page { .. } => &["/parentSection/id"],
        Target::Drive { .. } => &["/parentReference/driveId", "/parentReference/id", "/name"],
        Target::Todo { .. } => &[],
    };
    if scope_fields.iter().any(|path| field(&item, path).is_none()) {
        return Err(
            "Microsoft did not confirm the item's location. Export history was kept.".into(),
        );
    }
    if !target.matches(&item, false) {
        return Err("This item is outside the original destination. Use Review link in Export history to choose the correct item.".into());
    }
    ExportCandidate::from_item(&item)
        .map(Some)
        .ok_or_else(|| "Microsoft did not return usable item details.".into())
}

pub(super) async fn reconcile<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    ledger: &mut ExportLedger,
    key: &str,
    account: &ExportContext<'_>,
    candidate: Option<&str>,
) -> Result<(), String> {
    let (target, previous) = Target::recorded(ledger, key, account)?;
    let previous = previous.map(str::to_owned);
    let verified = if let Some(id) = candidate {
        let item = check_item(client, account, target, id).await?
            .ok_or("That item was not found in the original destination. Choose another item or check the destination again.")?;
        if matches!(target, Target::Page { .. })
            && ledger.entry(key).is_some_and(|entry| {
                entry.code.as_deref() == Some(super::onenote_resume::UNCONFIRMED_UPDATE)
            })
        {
            let revision = key
                .split(':')
                .nth(4)
                .ok_or("Invalid OneNote revision in export history.")?;
            super::onenote_resume::verify_updated_notes(
                client,
                account.bearer_token,
                &item.id,
                &ledger.meeting_id,
                revision,
            )
            .await?;
        }
        Some(item)
    } else {
        if let Some(ref id) = previous {
            if check_item(client, account, target, id).await?.is_some() {
                return Err("The saved item is still in the original destination. Open it or choose the correct item instead.".into());
            }
        }
        // A 404 on the destination itself can mean access changed. Keep the
        // original protection until the account can read that collection.
        let list = read(client, account, target.list_url()).await?.ok_or(
            "The original destination is missing or inaccessible. Export history was kept.",
        )?;
        if !list.get("value").is_some_and(Value::is_array) {
            return Err("Microsoft did not confirm access to the original destination. Export history was kept.".into());
        }
        None
    };
    let replacement = verified.as_ref().map(|item| item.id.as_str());
    if previous.as_deref() != replacement {
        protect_children(ledger, key, account, target, previous.as_deref())?;
    }
    if let (Target::Planner { .. } | Target::Todo { .. }, Some(id)) = (target, replacement) {
        let parts = if key.starts_with("planner:") { 5 } else { 4 };
        if ledger.entries.iter().any(|(other_key, entry)| {
            other_key != key
                && other_key
                    .split(':')
                    .take(parts)
                    .eq(key.split(':').take(parts))
                && entry.resource_id.as_deref() == Some(id)
        }) {
            return Err("That task is already linked to another export in this meeting.".into());
        }
    }
    ledger.apply_resolution(
        key,
        previous.as_deref(),
        replacement,
        verified.as_ref().and_then(|item| item.web_url.clone()),
        now_rfc3339(),
    )?;
    ledger.checkpoint().await
}

fn protect_children(
    ledger: &ExportLedger,
    entry_key: &str,
    account: &ExportContext<'_>,
    target: Target<'_>,
    previous: Option<&str>,
) -> Result<(), String> {
    let Some(previous) = previous else {
        return Ok(());
    };
    let have_children = match target {
        Target::Drive {
            binding,
            kind: "folder",
        } => ledger.entries.iter().any(|(key, entry)| {
            entry.can_resolve()
                && matches!(key.rsplit(':').next(), Some("docx" | "pdf"))
                && entry.drive.as_ref().map_or_else(
                    || key.split(':').take(2).eq(entry_key.split(':').take(2)),
                    |child| {
                        child.account_hash == binding.account_hash
                            && child.drive_id == binding.drive_id
                            && child.parent_id == previous
                    },
                )
        }),
        Target::Section { .. } => ledger.entries.iter().any(|(key, entry)| {
            key.split(':')
                .take(4)
                .eq(["onenote", account.tenant_id, account.user_id, previous])
                && entry.can_resolve()
        }),
        _ => false,
    };
    if have_children {
        return Err(
            "Resolve the exported items in this folder or section before replacing it.".into(),
        );
    }
    Ok(())
}

pub(super) async fn page_is_in_section<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    account: &ExportContext<'_>,
    page: &str,
    section: &str,
) -> Result<bool, String> {
    Ok(check_item(client, account, Target::Page { section }, page)
        .await?
        .is_some())
}

pub(super) async fn section_is_in_notebook<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    account: &ExportContext<'_>,
    section: &str,
    notebook: &str,
) -> Result<bool, String> {
    Ok(
        check_item(client, account, Target::Section { notebook }, section)
            .await?
            .is_some(),
    )
}

pub(super) async fn check_saved_drive_item<T: GraphTransport, S: Sleeper>(
    client: &GraphClient<T, S>,
    account: &ExportContext<'_>,
    binding: &DriveBinding,
    id: &str,
    kind: &str,
) -> Result<String, String> {
    let item = check_item(client, account, Target::Drive { binding, kind }, id)
        .await?
        .ok_or_else(|| {
            "The saved OneDrive item is missing or has moved. Review its link in Export history."
                .to_owned()
        })?;
    Ok(item.title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::client::{RetryPolicy, TokioSleeper};
    use crate::exports::ledger::task_key;
    use crate::exports::model::ExportStatus;
    use crate::exports::transport::{GraphResponse, MockGraphTransport};

    fn account(user: &str) -> ExportContext<'_> {
        ExportContext {
            tenant_id: "tenant",
            user_id: user,
            bearer_token: "<redacted>",
        }
    }

    fn client(transport: MockGraphTransport) -> GraphClient<MockGraphTransport, TokioSleeper> {
        GraphClient::new(
            transport,
            TokioSleeper,
            RetryPolicy {
                max_attempts: 1,
                ..RetryPolicy::default()
            },
        )
    }

    #[tokio::test]
    async fn candidate_paging_accepts_the_authenticated_user_alias_for_the_same_collection() {
        let key = "todo:tenant:user:list:v2:meeting:action";
        let mut ledger = ExportLedger::new("meeting");
        ledger.record_failure(key, ExportStatus::UnknownAfterSubmit, None, None);
        let transport = MockGraphTransport::new();
        let list = graph_url(&["me", "todo", "lists", "list", "tasks"]);
        let next = format!(
            "{}?$skiptoken=A%2BB%3D",
            graph_url(&["users", "user", "todo", "lists", "list", "tasks"])
        );
        transport.queue_for_url(
            &list,
            [GraphResponse::success(
                200,
                serde_json::json!({"value": [], "@odata.nextLink": next}).to_string(),
            )],
        );
        transport.queue_for_url(
            &next,
            [GraphResponse::success(
                200,
                r#"{"value":[{"id":"task","title":"Review notes"}]}"#,
            )],
        );
        let client = client(transport);
        let choices = candidates(&client, &ledger, key, &account("user"))
            .await
            .unwrap();
        assert_eq!(choices.items[0].id, "task");
        assert!(!choices.truncated);
        assert_eq!(client.transport().calls_for(&next), 1);
    }

    #[tokio::test]
    async fn candidate_paging_never_requests_a_link_outside_the_recorded_collection() {
        let key = "onenote:tenant:user:section:revision";
        let mut ledger = ExportLedger::new("meeting");
        ledger.record_failure(key, ExportStatus::UnknownAfterSubmit, None, None);
        let list = graph_url(&["me", "onenote", "sections", "section", "pages"]);
        let alias = "/v1.0/users/user/onenote/sections/section/pages";
        for next in [
            format!("https://other.example.com{alias}?$skip=20"),
            format!("http://graph.microsoft.com{alias}?$skip=20"),
            format!("https://graph.microsoft.com:444{alias}?$skip=20"),
            format!("https://login@graph.microsoft.com{alias}?$skip=20"),
            format!("https://graph.microsoft.com{alias}?$skip=20#fragment"),
            "https://graph.microsoft.com/v1.0/users/other/onenote/sections/section/pages".into(),
            "https://graph.microsoft.com/v1.0/users/user/onenote/sections/other/pages".into(),
            "https://graph.microsoft.com/beta/users/user/onenote/sections/section/pages".into(),
        ] {
            let transport = MockGraphTransport::new();
            transport.queue_for_url(
                &list,
                [GraphResponse::success(
                    200,
                    serde_json::json!({"value": [], "@odata.nextLink": next}).to_string(),
                )],
            );
            let client = client(transport);
            let wrong_tenant = ExportContext {
                tenant_id: "other",
                ..account("user")
            };
            assert!(candidates(&client, &ledger, key, &wrong_tenant)
                .await
                .is_err());
            assert!(client.transport().recorded().is_empty());
            assert!(candidates(&client, &ledger, key, &account("user"))
                .await
                .unwrap_err()
                .contains("outside the export destination"));
            assert_eq!(client.transport().recorded().len(), 1);
        }
    }

    #[tokio::test]
    async fn planned_candidates_and_reconciliation_are_account_plan_and_bucket_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let key = task_key(
            "planner:tenant:user:plan:bucket",
            "meeting",
            "action-1",
            "0123456789abcdef",
        );
        let mut ledger = ExportLedger::load_or_new(dir.path(), "meeting").unwrap();
        ledger.begin_attempt(&key, now_rfc3339());
        ledger.checkpoint().await.unwrap();
        let mut ledger = ExportLedger::load_or_new(dir.path(), "meeting").unwrap();
        let transport = MockGraphTransport::new();
        let plan = graph_url(&["planner", "plans", "plan", "tasks"]);
        let get_candidate = graph_url(&["planner", "tasks", "task"]);
        transport.queue_for_url(
            &plan,
            [GraphResponse::success(
                200,
                r#"{"value":[
            {"id":"task","title":"Review notes","planId":"plan","bucketId":"bucket"},
            {"id":"sibling","title":"Another task","planId":"plan","bucketId":"other"}
        ]}"#,
            )],
        );
        transport.queue_for_url(
            &get_candidate,
            [
                GraphResponse::success(
                    200,
                    r#"{"id":"task","title":"Review notes","planId":"other","bucketId":"bucket"}"#,
                ),
                GraphResponse::success(
                    200,
                    r#"{"id":"task","title":"Review notes","planId":"plan","bucketId":"other"}"#,
                ),
                GraphResponse::failure(404, None),
                GraphResponse::failure(503, None),
                GraphResponse::success(
                    200,
                    r#"{"id":"task","title":"Review notes","planId":"plan","bucketId":"bucket"}"#,
                ),
            ],
        );
        let client = client(transport);
        assert!(
            reconcile(&client, &mut ledger, &key, &account("other"), Some("task"))
                .await
                .unwrap_err()
                .contains("Reconnect")
        );
        assert!(client.transport().recorded().is_empty());
        let choices = candidates(&client, &ledger, &key, &account("user"))
            .await
            .unwrap();
        assert_eq!(
            choices
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["task"]
        );
        for _ in 0..4 {
            assert!(
                reconcile(&client, &mut ledger, &key, &account("user"), Some("task"))
                    .await
                    .is_err()
            );
            assert_eq!(
                ledger.entry(&key).unwrap().status,
                ExportStatus::UnknownAfterSubmit
            );
            assert!(!ledger.may_attempt(&key));
        }
        reconcile(&client, &mut ledger, &key, &account("user"), Some("task"))
            .await
            .unwrap();
        let saved = ExportLedger::load_or_new(dir.path(), "meeting").unwrap();
        assert_eq!(
            saved
                .already_succeeded(&key)
                .unwrap()
                .resource_id
                .as_deref(),
            Some("task")
        );
        assert!(client
            .transport()
            .recorded()
            .iter()
            .all(|call| call.method == "GET"));
    }

    #[tokio::test]
    async fn confirmed_todo_link_can_be_repaired_or_unlinked_after_deletion_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let key = task_key(
            "todo:tenant:user:list",
            "meeting",
            "action-1",
            "0123456789abcdef",
        );
        let mut ledger = ExportLedger::load_or_new(dir.path(), "meeting").unwrap();
        ledger.record_success(&key, Some("deleted".into()), None, now_rfc3339());
        ledger.checkpoint().await.unwrap();
        let mut ledger = ExportLedger::load_or_new(dir.path(), "meeting").unwrap();
        let transport = MockGraphTransport::new();
        let list = graph_url(&["me", "todo", "lists", "list", "tasks"]);
        transport.queue_for_url(
            &format!("{list}/deleted"),
            [GraphResponse::failure(404, None)],
        );
        transport.queue_for_url(
            &format!("{list}/correct"),
            [
                GraphResponse::success(200, r#"{"id":"correct","title":"Review notes"}"#),
                GraphResponse::success(200, r#"{"id":"correct","title":"Review notes"}"#),
                GraphResponse::failure(503, None),
                GraphResponse::failure(404, None),
                GraphResponse::failure(404, None),
            ],
        );
        transport.queue_for_url(
            &list,
            [
                GraphResponse::failure(404, None),
                GraphResponse::success(200, r#"{"value":[]}"#),
            ],
        );
        let client = client(transport);
        assert!(reconcile(
            &client,
            &mut ledger,
            &key,
            &account("user"),
            Some("deleted")
        )
        .await
        .is_err());
        reconcile(
            &client,
            &mut ledger,
            &key,
            &account("user"),
            Some("correct"),
        )
        .await
        .unwrap();
        let mut ledger = ExportLedger::load_or_new(dir.path(), "meeting").unwrap();
        assert!(
            reconcile(&client, &mut ledger, &key, &account("user"), None)
                .await
                .unwrap_err()
                .contains("still in")
        );
        assert!(
            reconcile(&client, &mut ledger, &key, &account("user"), None)
                .await
                .is_err()
        );
        assert!(!ledger.may_attempt(&key));
        assert!(
            reconcile(&client, &mut ledger, &key, &account("user"), None)
                .await
                .unwrap_err()
                .contains("original destination is missing")
        );
        assert!(!ledger.may_attempt(&key));
        reconcile(&client, &mut ledger, &key, &account("user"), None)
            .await
            .unwrap();
        let ledger = ExportLedger::load_or_new(dir.path(), "meeting").unwrap();
        assert!(ledger.may_attempt(&key));
        assert!(ledger.entry(&key).unwrap().resource_id.is_none());
        assert!(client
            .transport()
            .recorded()
            .iter()
            .all(|call| call.method == "GET"));
    }
}
