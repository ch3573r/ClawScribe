//! User-confirmed follow-ups stored with the saved meeting.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::exports::markdown_notes::{
    action_suggestion, parse_action_suggestions, source_key, ActionSuggestion,
};
use crate::state::AppState;
use crate::summary::sources::{self, ResolvedSource, SummarySource};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyAction {
    pub id: String,
    pub title_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewedAction {
    pub id: String,
    pub title: String,
    pub owner: Option<String>,
    pub due_date: Option<String>,
    pub done: bool,
    pub source_key: Option<String>,
    pub legacy_action: Option<LegacyAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<SummarySource>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DismissedSuggestion {
    id: String,
    legacy_action: LegacyAction,
}

#[derive(Default)]
pub(super) struct SavedActions {
    pub actions: Vec<ReviewedAction>,
    pub dismissed: Vec<DismissedSuggestion>,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum StoredAction {
    Reviewed(ReviewedAction),
    Dismissed(DismissedSuggestion),
}

pub(super) async fn load(
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<Option<Vec<ReviewedAction>>, String> {
    Ok(load_saved(pool, meeting_id)
        .await?
        .map(|saved| saved.actions))
}

pub(super) async fn load_saved(
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<Option<SavedActions>, String> {
    let raw: Option<Option<String>> =
        sqlx::query_scalar("SELECT reviewed_actions FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| "Could not load meeting actions.")?;
    raw.ok_or("Meeting no longer exists.")?
        .map(|raw| decode_saved(&raw))
        .transpose()
}

pub(crate) fn decode(raw: &str) -> Result<Vec<ReviewedAction>, String> {
    Ok(decode_saved(raw)?.actions)
}

fn decode_saved(raw: &str) -> Result<SavedActions, String> {
    let stored: Vec<StoredAction> =
        serde_json::from_str(raw).map_err(|_| "Invalid saved meeting actions.")?;
    let mut saved = SavedActions::default();
    for row in stored {
        match row {
            StoredAction::Reviewed(action) => saved.actions.push(action),
            StoredAction::Dismissed(dismissed) => saved.dismissed.push(dismissed),
        }
    }
    validate(&mut saved.actions)?;
    validate_dismissals(&saved.dismissed)?;
    Ok(saved)
}

fn valid_suggestion(reference: &LegacyAction) -> bool {
    super::ledger::is_positional_action_id(&reference.id)
        && reference.title_hash.len() == 16
        && reference.title_hash.bytes().all(|b| b.is_ascii_hexdigit())
}

fn validate_dismissals(dismissed: &[DismissedSuggestion]) -> Result<(), String> {
    let mut ids = HashSet::new();
    if dismissed.len() > 4096
        || dismissed.iter().any(|row| {
            uuid::Uuid::parse_str(&row.id).is_err()
                || !ids.insert(&row.id)
                || !valid_suggestion(&row.legacy_action)
        })
    {
        return Err("Invalid saved suggestion dismissals.".into());
    }
    Ok(())
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
        if action
            .legacy_action
            .as_ref()
            .is_some_and(|legacy| !valid_suggestion(legacy))
        {
            return Err("Invalid action suggestion reference.".into());
        }
        if action.source.as_ref().is_some_and(|source| {
            action.source_key.as_deref() != Some(source.key.as_str()) || !source.valid_reference()
        }) {
            return Err("Invalid saved action source reference.".into());
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
    suggestions: Vec<ActionSuggestion>,
    reviewed: &[ReviewedAction],
    dismissed: &[DismissedSuggestion],
) -> Vec<ReviewedAction> {
    // Positions can move on regeneration. Consume each original title once so
    // a renamed or completed action stays reviewed, including repeated titles.
    let mut counts = HashMap::new();
    for legacy in reviewed
        .iter()
        .filter_map(|action| action.legacy_action.as_ref())
        .chain(dismissed.iter().map(|row| &row.legacy_action))
    {
        *counts.entry(legacy.title_hash.as_str()).or_insert(0usize) += 1;
    }
    suggestions
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
                source: None,
            }
        })
        .collect()
}

pub(super) fn summary_suggestions(
    markdown: &str,
    summary: Option<&serde_json::Value>,
) -> Vec<ActionSuggestion> {
    let mut suggestions = parse_action_suggestions(markdown);
    let mut seen = HashMap::<String, Vec<usize>>::new();
    for (index, suggestion) in suggestions.iter().enumerate().rev() {
        seen.entry(super::planner::action_hash(&suggestion.action))
            .or_default()
            .push(index);
    }
    let Some(actions) = summary
        .and_then(|summary| summary.pointer("/structured_meeting_output/action_items"))
        .and_then(|items| items.as_array())
    else {
        return suggestions;
    };
    let previously_rendered = summary
        .and_then(|summary| summary.get("markdown"))
        .and_then(serde_json::Value::as_str)
        .filter(|previous| *previous != markdown)
        .map(rendered_action_hashes)
        .unwrap_or_default();
    for item in actions.iter().take(250) {
        let text = |field| item.get(field).and_then(|value| value.as_str());
        let Some(task) = text("task") else { continue };
        let Some(mut suggestion) = action_suggestion(task, text("owner"), text("due_date"), None)
        else {
            continue;
        };
        suggestion.source_key = suggestion
            .source_key
            .or_else(|| text("source_timestamp").and_then(source_key));
        let hash = super::planner::action_hash(&suggestion.action);
        if let Some(index) = seen.get_mut(&hash).and_then(Vec::pop) {
            let rendered = &mut suggestions[index];
            rendered.source_key = rendered.source_key.take().or(suggestion.source_key);
            rendered.action.owner = rendered.action.owner.take().or(suggestion.action.owner);
            rendered.action.due_date = rendered
                .action
                .due_date
                .take()
                .or(suggestion.action.due_date);
        } else if !seen.contains_key(&hash) && !previously_rendered.contains(&hash) {
            suggestion.action.local_action_id = format!("action-{}", suggestions.len() + 1);
            suggestions.push(suggestion);
            seen.insert(hash, Vec::new());
        }
    }
    suggestions
}

fn rendered_action_hashes(markdown: &str) -> HashSet<String> {
    parse_action_suggestions(markdown)
        .into_iter()
        .map(|suggestion| super::planner::action_hash(&suggestion.action))
        .collect()
}

pub(crate) fn prune_removed_summary_actions(
    previous: &serde_json::Value,
    edited: &mut serde_json::Value,
) {
    let (Some(before), Some(after)) = (
        previous.get("markdown").and_then(serde_json::Value::as_str),
        edited.get("markdown").and_then(serde_json::Value::as_str),
    ) else {
        return;
    };
    if before == after {
        return;
    }
    let before = rendered_action_hashes(before);
    let after = rendered_action_hashes(after);
    if let Some(rows) = edited
        .pointer_mut("/structured_meeting_output/action_items")
        .and_then(serde_json::Value::as_array_mut)
    {
        rows.retain(|row| {
            row.get("task")
                .and_then(serde_json::Value::as_str)
                .and_then(|task| action_suggestion(task, None, None, None))
                .is_none_or(|suggestion| {
                    let hash = super::planner::action_hash(&suggestion.action);
                    !before.contains(&hash) || after.contains(&hash)
                })
        });
    }
}

#[tauri::command]
pub async fn suggest_meeting_actions(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    markdown: String,
    reviewed: Option<Vec<ReviewedAction>>,
    dismissed: Option<Vec<DismissedSuggestion>>,
) -> Result<Vec<ReviewedAction>, String> {
    let pool = state.db_manager.pool();
    let summary = sources::saved_result(pool, &meeting_id).await?;
    let mut saved = load_saved(pool, &meeting_id).await?.unwrap_or_default();
    let dismissed = dismissed.unwrap_or_default();
    validate_dismissals(&dismissed)?;
    if let Some(reviewed) = reviewed {
        merge_dismissals(&mut saved, &reviewed, dismissed);
        saved.actions = reviewed;
    }
    Ok(remaining_suggestions(
        summary_suggestions(&markdown, summary.as_ref()),
        &saved.actions,
        &saved.dismissed,
    ))
}

#[tauri::command]
pub async fn resolve_reviewed_action_source(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    action_id: String,
    key: String,
) -> Result<ResolvedSource, String> {
    resolve_action_source(state.db_manager.pool(), &meeting_id, &action_id, &key).await
}

async fn resolve_action_source(
    pool: &SqlitePool,
    meeting_id: &str,
    action_id: &str,
    key: &str,
) -> Result<ResolvedSource, String> {
    let saved = load(pool, meeting_id)
        .await?
        .unwrap_or_default()
        .into_iter()
        .find(|action| action.id == action_id && action.source_key.as_deref() == Some(key));
    if let Some(source) = saved.and_then(|action| action.source) {
        return sources::resolve_passage(pool, meeting_id, source).await;
    }
    let summary = sources::saved_result(pool, meeting_id)
        .await?
        .unwrap_or_default();
    let source = sources::sources_in_result(&summary)?
        .into_iter()
        .find(|source| source.key == key)
        .ok_or("This action's original source is no longer available.")?;
    sources::resolve_passage(pool, meeting_id, source).await
}

#[tauri::command]
pub async fn save_reviewed_meeting_actions(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    actions: Vec<ReviewedAction>,
    dismissed: Option<Vec<DismissedSuggestion>>,
) -> Result<(), String> {
    save(
        state.db_manager.pool(),
        &meeting_id,
        actions,
        dismissed.unwrap_or_default(),
    )
    .await
}

fn merge_dismissals(
    saved: &mut SavedActions,
    actions: &[ReviewedAction],
    dismissed: Vec<DismissedSuggestion>,
) {
    let active: HashSet<_> = actions.iter().map(|action| action.id.as_str()).collect();
    saved
        .dismissed
        .retain(|row| !active.contains(row.id.as_str()));
    let mut known: HashSet<_> = saved.dismissed.iter().map(|row| row.id.clone()).collect();
    for row in saved
        .actions
        .iter()
        .filter(|row| !active.contains(row.id.as_str()))
    {
        if let Some(legacy_action) = &row.legacy_action {
            if known.insert(row.id.clone()) {
                saved.dismissed.push(DismissedSuggestion {
                    id: row.id.clone(),
                    legacy_action: legacy_action.clone(),
                });
            }
        }
    }
    saved.dismissed.extend(
        dismissed
            .into_iter()
            .filter(|row| !active.contains(row.id.as_str()) && known.insert(row.id.clone())),
    );
}

async fn save(
    pool: &SqlitePool,
    meeting_id: &str,
    mut actions: Vec<ReviewedAction>,
    dismissed: Vec<DismissedSuggestion>,
) -> Result<(), String> {
    validate_dismissals(&dismissed)?;
    // Snapshot the existing evidence and the current generation in the same
    // transaction. Fields supplied by the editor never replace saved evidence.
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| "Could not save meeting actions.")?;
    let previous: Option<String> =
        sqlx::query_scalar("SELECT reviewed_actions FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| "Could not load meeting actions.")?
            .ok_or("Meeting no longer exists.")?;
    let mut saved = previous
        .as_deref()
        .map(decode_saved)
        .transpose()?
        .unwrap_or_default();
    merge_dismissals(&mut saved, &actions, dismissed);
    validate_dismissals(&saved.dismissed)?;
    let mut previous: HashMap<_, _> = saved
        .actions
        .into_iter()
        .map(|action| (action.id.clone(), action))
        .collect();
    for action in &mut actions {
        action.source = None;
        if let Some(original) = previous.remove(&action.id) {
            action.source_key = original.source_key;
            action.source = original.source;
            action.legacy_action = original.legacy_action;
        }
    }
    validate(&mut actions)?;
    if actions
        .iter()
        .any(|action| action.source_key.is_some() && action.source.is_none())
    {
        let raw: Option<String> =
            sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = ?")
                .bind(meeting_id)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(|_| "Could not read action sources.")?
                .flatten();
        let result = raw
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default();
        let manifest = sources::sources_in_result(&result)?;
        for action in &mut actions {
            if action.source.is_none() {
                action.source = manifest
                    .iter()
                    .find(|source| {
                        Some(source.key.as_str()) == action.source_key.as_deref()
                            && source.valid_reference()
                    })
                    .cloned();
            }
        }
    }
    // Keep the existing array shape required by the SQLite column and backups.
    let stored: Vec<_> = actions
        .into_iter()
        .map(StoredAction::Reviewed)
        .chain(saved.dismissed.into_iter().map(StoredAction::Dismissed))
        .collect();
    let raw = serde_json::to_string(&stored).map_err(|_| "Could not prepare meeting actions.")?;
    let result = sqlx::query("UPDATE meetings SET reviewed_actions = ? WHERE id = ?")
        .bind(raw)
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| "Could not save meeting actions.")?;
    if result.rows_affected() != 1 {
        return Err("Meeting no longer exists.".into());
    }
    transaction
        .commit()
        .await
        .map_err(|_| "Could not save meeting actions.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn structured_suggestion_retains_its_transcript_source_after_summary_replacement() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let meeting = "review-test";
        let snapshot = sources::prepare_snapshot(&pool, meeting, String::new(), false)
            .await
            .unwrap();
        let key = snapshot.sources[0].key.as_str();
        let mut summary = serde_json::json!({
            "markdown":"## Decisions\nProceed with the project.",
            "structured_meeting_output": {"action_items":[{
                "task":"Send the project notes", "owner":"Dana", "due_date":"2026-12-04", "source_timestamp":"[S1]"
            }]}
        });
        sources::attach(&mut summary, &snapshot.sources);
        sqlx::query("INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result) VALUES (?, 'completed', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?)")
            .bind(meeting).bind(summary.to_string()).execute(&pool).await.unwrap();
        let rows = remaining_suggestions(
            summary_suggestions("## Decisions\nProceed with the project.", Some(&summary)),
            &[],
            &[],
        );
        assert_eq!(rows[0].source_key.as_deref(), Some(key));
        assert_eq!(rows[0].owner.as_deref(), Some("Dana"));
        assert_eq!(rows[0].due_date.as_deref(), Some("2026-12-04"));
        assert!(load(&pool, meeting).await.unwrap().is_none());
        let id = rows[0].id.clone();
        save(&pool, meeting, rows, vec![]).await.unwrap();

        sqlx::query("UPDATE summary_processes SET result = ? WHERE meeting_id = ?")
            .bind(serde_json::json!({"markdown":"Replacement summary"}).to_string())
            .bind(meeting)
            .execute(&pool)
            .await
            .unwrap();
        let mut rows = load(&pool, meeting).await.unwrap().unwrap();
        rows[0].title = "Send the reviewed notes".into();
        save(&pool, meeting, rows, vec![]).await.unwrap();
        let passage = resolve_action_source(&pool, meeting, &id, key)
            .await
            .unwrap();
        let passage = serde_json::to_value(passage).unwrap();
        assert_eq!(passage["transcript_id"], "a");
        assert_eq!(passage["stale"], false);
        assert_eq!(passage["text"], "Äpfel project");
    }

    #[test]
    fn reordered_actions_keep_their_review_and_new_suggestions_can_be_added() {
        let mut reviewed = remaining_suggestions(
            summary_suggestions("## Action items\n- Send report\n- Review plan", None),
            &[],
            &[],
        );
        assert_eq!(reviewed.len(), 2);
        reviewed[0].title = "Send revised report".into();
        reviewed[1].done = true;
        let remaining = remaining_suggestions(
            summary_suggestions(
                "## Action items\n- Review plan\n- Confirm budget\n- Send report",
                None,
            ),
            &reviewed,
            &[],
        );
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].title, "Confirm budget");
    }

    #[test]
    fn edited_notes_do_not_offer_a_removed_rendered_suggestion_before_or_after_saving() {
        let mut original = serde_json::json!({
            "markdown":"## Action items\n- Remove this row\n- Keep this row",
            "structured_meeting_output":{"action_items":[
                {"task":"Remove this row"}, {"task":"Keep this row", "owner":"Dana"}, {"task":"Structured only"}
            ]}
        });
        let edit = "## Action items\n- Keep this row";
        for _ in 0..2 {
            let suggestions = summary_suggestions(edit, Some(&original));
            assert_eq!(
                suggestions
                    .iter()
                    .map(|row| row.action.task.as_str())
                    .collect::<Vec<_>>(),
                ["Keep this row", "Structured only"]
            );
            assert_eq!(suggestions[0].action.owner.as_deref(), Some("Dana"));
            let mut saved = original.clone();
            saved["markdown"] = edit.into();
            prune_removed_summary_actions(&original, &mut saved);
            original = saved;
        }
        assert_eq!(
            original["structured_meeting_output"]["action_items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
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
        sqlx::query("CREATE TABLE summary_processes (meeting_id TEXT PRIMARY KEY, result TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        let mut actions = remaining_suggestions(
            summary_suggestions(
                "## Aufgaben\n- [x] Bericht senden [00:01:18](#clawscribe-source-abc123)",
                None,
            ),
            &[],
            &[],
        );
        assert!(actions[0].done);
        assert_eq!(actions[0].source_key.as_deref(), Some("abc123"));
        save(&pool, "existing", actions.clone(), vec![])
            .await
            .unwrap();
        actions[0].title = "Confirm the revised report".into();
        actions[0].owner = Some("Dana".into());
        actions[0].due_date = Some("2026-11-30".into());
        save(&pool, "existing", actions.clone(), vec![])
            .await
            .unwrap();
        assert_eq!(load(&pool, "existing").await.unwrap(), Some(actions));
        save(&pool, "existing", vec![], vec![]).await.unwrap();
        assert_eq!(load(&pool, "existing").await.unwrap(), Some(vec![]));
        let saved = load_saved(&pool, "existing").await.unwrap().unwrap();
        let remaining = remaining_suggestions(
            summary_suggestions("## Aufgaben\n- New action\n- Bericht senden", None),
            &saved.actions,
            &saved.dismissed,
        );
        assert_eq!(
            remaining
                .iter()
                .map(|row| row.title.as_str())
                .collect::<Vec<_>>(),
            ["New action"]
        );
    }
}
