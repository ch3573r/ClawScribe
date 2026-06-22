//! Microsoft To Do task request builder.
//!
//! Builds `POST /me/todo/lists/{listId}/tasks` request bodies from reviewed
//! action items. To Do is personal by design: no automatic owner assignment and
//! no directory lookup.

use serde::{Deserialize, Serialize};

use crate::exports::model::{ExportActionItem, MeetingExport};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToDoDestination {
    #[serde(rename = "listId")]
    pub list_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToDoBuildError {
    MissingListId,
    EmptyTitle,
}

impl ToDoBuildError {
    pub fn message(self) -> &'static str {
        match self {
            ToDoBuildError::MissingListId => "Microsoft To Do listId is required",
            ToDoBuildError::EmptyTitle => "Microsoft To Do task title must not be empty",
        }
    }
}

impl std::fmt::Display for ToDoBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ToDoBuildError {}

fn normalize_title(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn build_task_request(
    destination: &ToDoDestination,
    _meeting: &MeetingExport,
    action: &ExportActionItem,
) -> Result<serde_json::Value, ToDoBuildError> {
    if destination.list_id.trim().is_empty() {
        return Err(ToDoBuildError::MissingListId);
    }
    let title = normalize_title(&action.task);
    if title.is_empty() {
        return Err(ToDoBuildError::EmptyTitle);
    }

    Ok(serde_json::json!({ "title": title }))
}

pub fn build_task_body_patch(
    meeting: &MeetingExport,
    action: &ExportActionItem,
) -> serde_json::Value {
    serde_json::json!({
        "body": {
            "contentType": "text",
            "content": build_task_body(meeting, action),
        },
    })
}

pub fn build_task_due_date_patch(action: &ExportActionItem) -> Option<serde_json::Value> {
    if let Some(due) = action.due_date.as_deref().and_then(to_due_date_time) {
        return Some(serde_json::json!({
            "dueDateTime": {
                "dateTime": due,
                "timeZone": "UTC",
            },
        }));
    }
    None
}

fn to_due_date_time(date: &str) -> Option<String> {
    let date = date.trim();
    let bytes = date.as_bytes();
    let looks_iso = date.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && date.chars().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    looks_iso.then(|| format!("{date}T00:00:00.0000000"))
}

pub fn build_task_body(meeting: &MeetingExport, action: &ExportActionItem) -> String {
    let mut lines: Vec<String> = Vec::new();
    match action
        .details
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        Some(details) => lines.push(details.to_string()),
        None => lines.push(format!("Action item: {}", normalize_title(&action.task))),
    }
    if let Some(owner) = action.owner.as_deref().filter(|s| !s.trim().is_empty()) {
        lines.push(format!("Owner (suggested): {owner}"));
    }
    if let Some(due) = action.due_date.as_deref().filter(|s| !s.trim().is_empty()) {
        lines.push(format!("Due: {due}"));
    }
    let meeting_line = match meeting.created_at.as_deref() {
        Some(when) if !when.trim().is_empty() => {
            format!("From meeting: {} ({})", meeting.title, when)
        }
        _ => format!("From meeting: {}", meeting.title),
    };
    lines.push(meeting_line);
    let context = meeting.executive_summary.trim();
    if !context.is_empty() {
        let excerpt: String = context.chars().take(600).collect();
        lines.push(String::new());
        lines.push("Meeting context:".to_string());
        lines.push(excerpt);
    }
    lines.push(String::new());
    lines.push("Exported from ClawScribe.".to_string());
    lines.join("\n")
}

pub fn action_hash(action: &ExportActionItem) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(normalize_title(&action.task).as_bytes());
    let digest = hasher.finalize();
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

pub fn dedupe_key(
    tenant_id: &str,
    user_id: &str,
    destination: &ToDoDestination,
    meeting: &MeetingExport,
    action: &ExportActionItem,
) -> String {
    format!(
        "todo:{tenant}:{user}:{list}:{meeting_hash}:{action_id}:{action_hash}",
        tenant = tenant_id,
        user = user_id,
        list = destination.list_id,
        meeting_hash = meeting.artifact_hash(),
        action_id = action.local_action_id,
        action_hash = action_hash(action),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dest() -> ToDoDestination {
        ToDoDestination {
            list_id: "list-1".into(),
        }
    }

    fn meeting() -> MeetingExport {
        MeetingExport {
            meeting_id: "m1".into(),
            title: "Sync".into(),
            created_at: Some("2026-06-22T10:00:00Z".into()),
            executive_summary: "Discussed launch tasks.".into(),
            decisions: vec![],
            action_items: vec![],
            transcript_excerpt: None,
            summary_html: None,
        }
    }

    fn action(id: &str, task: &str) -> ExportActionItem {
        ExportActionItem {
            local_action_id: id.into(),
            task: task.into(),
            owner: Some("Sam".into()),
            due_date: Some("2026-07-01".into()),
            details: None,
        }
    }

    #[test]
    fn builds_todo_task_request() {
        let body = build_task_request(&dest(), &meeting(), &action("action-1", "Send  notes\nout"))
            .unwrap();
        assert_eq!(body["title"], "Send notes out");
        assert!(body.get("body").is_none());
        assert!(body.get("dueDateTime").is_none());
    }

    #[test]
    fn builds_todo_task_body_and_due_patches() {
        let item = action("action-1", "Send notes");
        let body_patch = build_task_body_patch(&meeting(), &item);
        assert_eq!(body_patch["body"]["contentType"], "text");
        assert!(body_patch["body"]["content"]
            .as_str()
            .unwrap()
            .contains("Owner (suggested): Sam"));

        let due_patch = build_task_due_date_patch(&item).unwrap();
        assert_eq!(
            due_patch["dueDateTime"]["dateTime"],
            "2026-07-01T00:00:00.0000000"
        );
        assert_eq!(due_patch["dueDateTime"]["timeZone"], "UTC");
    }

    #[test]
    fn rejects_missing_destination_and_title() {
        assert_eq!(
            build_task_request(
                &ToDoDestination {
                    list_id: " ".into()
                },
                &meeting(),
                &action("a", "x")
            ),
            Err(ToDoBuildError::MissingListId)
        );
        assert_eq!(
            build_task_request(&dest(), &meeting(), &action("a", " \n ")),
            Err(ToDoBuildError::EmptyTitle)
        );
    }

    #[test]
    fn dedupe_key_is_destination_scoped() {
        let action = action("action-1", "Send notes");
        let key = dedupe_key("tenant", "user", &dest(), &meeting(), &action);
        assert!(key.starts_with("todo:tenant:user:list-1:"));
    }
}
