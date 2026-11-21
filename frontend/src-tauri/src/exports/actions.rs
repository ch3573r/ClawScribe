//! User-confirmed follow-ups stored with the saved meeting.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::exports::markdown_notes::parse_action_suggestions;
use crate::state::AppState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyAction {
    pub id: String,
    pub title_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewedAction {
    pub id: String,
    pub title: String,
    pub owner: Option<String>,
    pub due_date: Option<String>,
    pub done: bool,
    pub source_key: Option<String>,
    pub legacy_action: Option<LegacyAction>,
}

pub(super) async fn load(
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<Option<Vec<ReviewedAction>>, String> {
    let raw: Option<Option<String>> =
        sqlx::query_scalar("SELECT reviewed_actions FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| "Could not load meeting actions.")?;
    raw.ok_or("Meeting no longer exists.")?
        .map(|raw| decode(&raw))
        .transpose()
}

pub(crate) fn decode(raw: &str) -> Result<Vec<ReviewedAction>, String> {
    let mut actions = serde_json::from_str(raw).map_err(|_| "Invalid saved meeting actions.")?;
    validate(&mut actions)?;
    Ok(actions)
}

fn validate(actions: &mut Vec<ReviewedAction>) -> Result<(), String> {
    if actions.len() > 250 {
        return Err("Save up to 250 actions per meeting.".into());
    }
    let mut ids = HashSet::new();
    for action in actions {
        let id = uuid::Uuid::parse_str(&action.id).map_err(|_| "Invalid meeting action ID.")?;
        if action.id != id.to_string() || !ids.insert(id) {
            return Err("Invalid or duplicate meeting action ID.".into());
        }
        action.title = action.title.trim().to_owned();
        if action.title.is_empty()
            || action.title.chars().count() > 255
            || action.title.contains(['\r', '\n'])
        {
            return Err("Enter a single-line title of 1 to 255 characters for each action.".into());
        }
        action.owner = action
            .owner
            .take()
            .map(|owner| owner.trim().to_owned())
            .filter(|owner| !owner.is_empty());
        if action
            .owner
            .as_ref()
            .is_some_and(|owner| owner.chars().count() > 160 || owner.contains(['\r', '\n']))
        {
            return Err("Action owners must fit on one line of up to 160 characters.".into());
        }
        if action.due_date.as_deref().is_some_and(|due| {
            due.len() != 10 || chrono::NaiveDate::parse_from_str(due, "%Y-%m-%d").is_err()
        }) {
            return Err("Use a valid due date or leave it blank.".into());
        }
        if action.source_key.as_ref().is_some_and(|key| {
            key.is_empty() || key.len() > 64 || !key.bytes().all(|b| b.is_ascii_hexdigit())
        }) {
            return Err("Invalid action source reference.".into());
        }
        if action.legacy_action.as_ref().is_some_and(|legacy| {
            !super::ledger::is_positional_action_id(&legacy.id)
                || legacy.title_hash.len() != 16
                || !legacy.title_hash.bytes().all(|b| b.is_ascii_hexdigit())
        }) {
            return Err("Invalid action suggestion reference.".into());
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn get_reviewed_meeting_actions(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<ReviewedAction>, String> {
    Ok(load(state.db_manager.pool(), &meeting_id)
        .await?
        .unwrap_or_default())
}

pub(super) fn remaining_suggestions(
    markdown: &str,
    reviewed: &[ReviewedAction],
) -> Vec<ReviewedAction> {
    // Positions can move on regeneration. Consume each original title once so
    // a renamed or completed action stays reviewed, including repeated titles.
    let mut counts = HashMap::new();
    for legacy in reviewed
        .iter()
        .filter_map(|action| action.legacy_action.as_ref())
    {
        *counts.entry(legacy.title_hash.as_str()).or_insert(0usize) += 1;
    }
    parse_action_suggestions(markdown)
        .into_iter()
        .filter_map(|suggestion| {
            let hash = super::planner::action_hash(&suggestion.action);
            if let Some(remaining) = counts.get_mut(hash.as_str()).filter(|count| **count > 0) {
                *remaining -= 1;
                None
            } else {
                Some((suggestion, hash))
            }
        })
        .take(250)
        .map(|(suggestion, hash)| {
            let action = suggestion.action;
            let legacy_action = LegacyAction {
                title_hash: hash,
                id: action.local_action_id,
            };
            ReviewedAction {
                id: uuid::Uuid::new_v4().to_string(),
                title: action.task.chars().take(255).collect(),
                owner: action.owner.map(|owner| owner.chars().take(160).collect()),
                due_date: action
                    .due_date
                    .filter(|due| chrono::NaiveDate::parse_from_str(due, "%Y-%m-%d").is_ok()),
                done: suggestion.done,
                source_key: suggestion.source_key,
                legacy_action: Some(legacy_action),
            }
        })
        .collect()
}

#[tauri::command]
pub fn suggest_meeting_actions(
    markdown: String,
    reviewed: Option<Vec<ReviewedAction>>,
) -> Vec<ReviewedAction> {
    remaining_suggestions(&markdown, reviewed.as_deref().unwrap_or_default())
}

#[tauri::command]
pub async fn save_reviewed_meeting_actions(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    actions: Vec<ReviewedAction>,
) -> Result<(), String> {
    save(state.db_manager.pool(), &meeting_id, actions).await
}

async fn save(
    pool: &SqlitePool,
    meeting_id: &str,
    mut actions: Vec<ReviewedAction>,
) -> Result<(), String> {
    validate(&mut actions)?;
    let raw = serde_json::to_string(&actions).map_err(|_| "Could not prepare meeting actions.")?;
    let result = sqlx::query("UPDATE meetings SET reviewed_actions = ? WHERE id = ?")
        .bind(raw)
        .bind(meeting_id)
        .execute(pool)
        .await
        .map_err(|_| "Could not save meeting actions.")?;
    if result.rows_affected() != 1 {
        return Err("Meeting no longer exists.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reordered_actions_keep_their_review_and_new_suggestions_can_be_added() {
        let mut reviewed =
            remaining_suggestions("## Action items\n- Send report\n- Review plan", &[]);
        assert_eq!(reviewed.len(), 2);
        reviewed[0].title = "Send revised report".into();
        reviewed[1].done = true;
        let remaining = remaining_suggestions(
            "## Action items\n- Review plan\n- Confirm budget\n- Send report",
            &reviewed,
        );
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].title, "Confirm budget");
    }

    #[tokio::test]
    async fn empty_review_and_edited_actions_survive_reload_and_upgrade() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE meetings (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO meetings VALUES ('existing')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(include_str!(
            "../../migrations/20261120000019_reviewed_meeting_actions.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        assert!(load(&pool, "existing").await.unwrap().is_none());
        let mut actions = suggest_meeting_actions(
            "## Aufgaben\n- [x] Bericht senden [00:01:18](#clawscribe-source-abc123)".into(),
            None,
        );
        assert!(actions[0].done);
        assert_eq!(actions[0].source_key.as_deref(), Some("abc123"));
        save(&pool, "existing", actions.clone()).await.unwrap();
        actions[0].title = "Confirm the revised report".into();
        actions[0].owner = Some("Dana".into());
        actions[0].due_date = Some("2026-11-30".into());
        save(&pool, "existing", actions.clone()).await.unwrap();
        assert_eq!(load(&pool, "existing").await.unwrap(), Some(actions));
        save(&pool, "existing", vec![]).await.unwrap();
        assert_eq!(load(&pool, "existing").await.unwrap(), Some(vec![]));
    }
}
