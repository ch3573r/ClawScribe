//! User-confirmed follow-ups stored with the saved meeting.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::Emitter;

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
    pub details: Option<String>,
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
        sqlx::query_scalar("SELECT reviewed_actions FROM active_meetings WHERE id = ?")
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
        if action
            .details
            .as_ref()
            .is_some_and(|details| details.chars().count() > 4000)
        {
            return Err("Action notes must fit within 4,000 characters.".into());
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

#[derive(Serialize)]
pub(crate) struct ActionReview {
    actions: Vec<ReviewedAction>,
    revision: String,
}

fn revision(raw: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    raw.map(|raw| format!("{:x}", Sha256::digest(raw)))
        .unwrap_or_default()
}

async fn editor(pool: &SqlitePool, meeting_id: &str) -> Result<ActionReview, String> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT reviewed_actions FROM active_meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| "Could not load meeting actions.")?
            .ok_or("Meeting no longer exists.")?;
    Ok(ActionReview {
        revision: revision(raw.as_deref()),
        actions: raw.as_deref().map(decode).transpose()?.unwrap_or_default(),
    })
}

#[tauri::command]
pub(crate) async fn get_reviewed_action_editor(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<ActionReview, String> {
    editor(state.db_manager.pool(), &meeting_id).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MeetingActionsOverview {
    open_count: usize,
    has_suggestions: bool,
}

#[tauri::command]
pub(crate) async fn get_meeting_actions_overview(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    markdown: String,
) -> Result<MeetingActionsOverview, String> {
    meeting_actions_overview(state.db_manager.pool(), &meeting_id, &markdown).await
}

async fn meeting_actions_overview(
    pool: &SqlitePool,
    meeting_id: &str,
    markdown: &str,
) -> Result<MeetingActionsOverview, String> {
    let saved = load_saved(pool, meeting_id).await?.unwrap_or_default();
    let summary = sources::saved_result(pool, meeting_id).await?;
    let markdown = summary
        .as_ref()
        .and_then(|value| value.get("markdown"))
        .and_then(|value| value.as_str())
        .unwrap_or(markdown);
    Ok(MeetingActionsOverview {
        open_count: saved.actions.iter().filter(|action| !action.done).count(),
        has_suggestions: !remaining_suggestions(
            summary_suggestions(markdown, summary.as_ref()),
            &saved.actions,
            &saved.dismissed,
        )
        .is_empty(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldEdit<T> {
    from: T,
    to: T,
}

impl<T: PartialEq> FieldEdit<T> {
    fn apply(self, field: &mut T) -> Result<(), String> {
        if *field != self.from && *field != self.to {
            return Err("This action changed. Reload its latest values before saving.".into());
        }
        *field = self.to;
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReviewedActionEdit {
    id: String,
    title: Option<FieldEdit<String>>,
    details: Option<FieldEdit<Option<String>>>,
    done: Option<FieldEdit<bool>>,
    owner: Option<FieldEdit<Option<String>>>,
    due_date: Option<FieldEdit<Option<String>>>,
}

async fn edit_actions(
    pool: &SqlitePool,
    meeting_id: &str,
    edits: Vec<ReviewedActionEdit>,
) -> Result<Vec<ReviewedAction>, String> {
    let review = editor(pool, meeting_id).await?;
    let mut actions = review.actions;
    let mut seen = HashSet::new();
    if edits.is_empty()
        || edits.len() > 250
        || edits.iter().any(|edit| !seen.insert(edit.id.clone()))
    {
        return Err("Invalid meeting action edits.".into());
    }
    for edit in edits {
        let action = actions
            .iter_mut()
            .find(|action| action.id == edit.id)
            .ok_or("This action is no longer saved. Reload the meeting actions.")?;
        if let Some(change) = edit.title {
            change.apply(&mut action.title)?;
        }
        if let Some(change) = edit.details {
            change.apply(&mut action.details)?;
        }
        if let Some(change) = edit.done {
            change.apply(&mut action.done)?;
        }
        if let Some(change) = edit.owner {
            change.apply(&mut action.owner)?;
        }
        if let Some(change) = edit.due_date {
            change.apply(&mut action.due_date)?;
        }
    }
    validate(&mut actions)?;
    let edited: Vec<_> = actions
        .iter()
        .filter(|action| seen.contains(&action.id))
        .cloned()
        .collect();
    save_checked(
        pool,
        meeting_id,
        actions,
        Vec::new(),
        Some(&review.revision),
    )
    .await?;
    Ok(edited)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SummaryActionEdit {
    local_id: String,
    original_title_hash: String,
    title: Option<String>,
    details: Option<String>,
}

async fn save_summary_action_review(
    pool: &SqlitePool,
    meeting_id: &str,
    markdown: &str,
    edits: Vec<SummaryActionEdit>,
) -> Result<Vec<ReviewedAction>, String> {
    let fallback = parse_action_suggestions(markdown);
    if fallback.is_empty() || fallback.len() > 250 || edits.len() > fallback.len() {
        return Err("Reopen the export preview before saving these actions.".into());
    }
    // Keep all visible positional siblings and completed checklist entries.
    // Structured-only suggestions still go through the meeting's action editor.
    let summary = sources::saved_result(pool, meeting_id).await?;
    let suggestions = summary_suggestions(markdown, summary.as_ref());
    let mut actions = remaining_suggestions(
        suggestions.into_iter().take(fallback.len()).collect(),
        &[],
        &[],
    );
    let mut ids = HashSet::new();
    for edit in edits {
        if !ids.insert(edit.local_id.clone()) {
            return Err("Duplicate action edits.".into());
        }
        let action = actions
            .iter_mut()
            .find(|action| {
                !action.done
                    && action.legacy_action.as_ref().is_some_and(|legacy| {
                        legacy.id == edit.local_id && legacy.title_hash == edit.original_title_hash
                    })
            })
            .ok_or("Reopen the export preview before saving these actions.")?;
        if let Some(title) = edit.title {
            action.title = title;
        }
        action.details = edit.details;
    }
    validate(&mut actions)?;
    save_checked(pool, meeting_id, actions.clone(), Vec::new(), Some("")).await?;
    Ok(actions)
}

#[tauri::command]
pub(crate) async fn save_previewed_summary_actions(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    markdown: String,
    edits: Vec<SummaryActionEdit>,
) -> Result<Vec<ReviewedAction>, String> {
    let actions =
        save_summary_action_review(state.db_manager.pool(), &meeting_id, &markdown, edits).await?;
    let _ = app.emit("reviewed-actions-changed", &meeting_id);
    Ok(actions)
}

#[tauri::command]
pub(crate) async fn edit_reviewed_meeting_actions(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    edits: Vec<ReviewedActionEdit>,
) -> Result<Vec<ReviewedAction>, String> {
    let actions = edit_actions(state.db_manager.pool(), &meeting_id, edits).await?;
    let _ = app.emit("reviewed-actions-changed", &meeting_id);
    Ok(actions)
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
                details: None,
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
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    actions: Vec<ReviewedAction>,
    dismissed: Option<Vec<DismissedSuggestion>>,
    expected_revision: Option<String>,
) -> Result<(), String> {
    save_checked(
        state.db_manager.pool(),
        &meeting_id,
        actions,
        dismissed.unwrap_or_default(),
        expected_revision.as_deref(),
    )
    .await?;
    let _ = app.emit("reviewed-actions-changed", &meeting_id);
    Ok(())
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

#[cfg(test)]
async fn save(
    pool: &SqlitePool,
    meeting_id: &str,
    actions: Vec<ReviewedAction>,
    dismissed: Vec<DismissedSuggestion>,
) -> Result<(), String> {
    save_checked(pool, meeting_id, actions, dismissed, None).await
}

async fn save_checked(
    pool: &SqlitePool,
    meeting_id: &str,
    mut actions: Vec<ReviewedAction>,
    dismissed: Vec<DismissedSuggestion>,
    expected_revision: Option<&str>,
) -> Result<(), String> {
    validate_dismissals(&dismissed)?;
    // Snapshot the existing evidence and the current generation in the same
    // transaction. Fields supplied by the editor never replace saved evidence.
    let mut transaction = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| "Could not save meeting actions.")?;
    let previous: Option<String> =
        sqlx::query_scalar("SELECT reviewed_actions FROM active_meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| "Could not load meeting actions.")?
            .ok_or("Meeting no longer exists.")?;
    if expected_revision.is_some_and(|expected| revision(previous.as_deref()) != expected) {
        return Err("Meeting actions changed. Reload the latest actions before saving.".into());
    }
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
    use serde_json::json;

    #[tokio::test]
    async fn old_saved_actions_load_and_accept_local_edits_without_losing_identity() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let id = uuid::Uuid::from_u128(1).to_string();
        let raw = json!([
            {"id": id, "title": "Original title", "done": false, "owner": "Dana",
                "dueDate": "2026-12-01", "sourceKey": "1234",
                "legacyAction": {"id": "action-1", "titleHash": "1234567890abcdef"}},
            {"id": uuid::Uuid::from_u128(2).to_string(),
                "legacyAction": {"id": "action-2", "titleHash": "1234567890abcdea"}}
        ]);
        sqlx::query("UPDATE meetings SET reviewed_actions = ? WHERE id = 'review-test'")
            .bind(raw.to_string())
            .execute(&pool)
            .await
            .unwrap();
        let original = load_saved(&pool, "review-test").await.unwrap().unwrap();
        assert_eq!(original.actions[0].details, None);
        let edit = |value| serde_json::from_value(value).unwrap();
        let edited = edit_actions(
            &pool,
            "review-test",
            vec![edit(json!({
                "id": id, "title": {"from": "Original title", "to": "Edited title"},
                "details": {"from": null, "to": "Reviewed notes"},
                "done": {"from": false, "to": true}
            }))],
        )
        .await
        .unwrap();
        assert!(edited[0].done);
        assert_eq!(edited[0].details.as_deref(), Some("Reviewed notes"));
        assert!(edit_actions(
            &pool,
            "review-test",
            vec![edit(json!({
                "id": id, "title": {"from": "Original title", "to": "Stale title"}
            }))]
        )
        .await
        .is_err());
        edit_actions(
            &pool,
            "review-test",
            vec![edit(json!({
                "id": id, "done": {"from": true, "to": false},
                "owner": {"from": "Dana", "to": null},
                "dueDate": {"from": "2026-12-01", "to": "2026-12-02"}
            }))],
        )
        .await
        .unwrap();
        let saved = load_saved(&pool, "review-test").await.unwrap().unwrap();
        let result = &saved.actions[0];
        assert!(!result.done);
        assert_eq!(result.title, "Edited title");
        assert_eq!(result.details.as_deref(), Some("Reviewed notes"));
        assert_eq!(result.id, id);
        assert_eq!(result.owner, None);
        assert_eq!(result.due_date.as_deref(), Some("2026-12-02"));
        assert_eq!(result.source_key, original.actions[0].source_key);
        assert_eq!(result.legacy_action, original.actions[0].legacy_action);
        assert_eq!(saved.dismissed.len(), 1);
    }

    #[tokio::test]
    async fn action_overview_flags_only_unreviewed_summary_items() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let markdown = "## Action items\n- Send revised quote";
        let overview = meeting_actions_overview(&pool, "review-test", markdown)
            .await
            .unwrap();
        assert!(overview.has_suggestions);
        assert_eq!(overview.open_count, 0);
        let mut actions = remaining_suggestions(summary_suggestions(markdown, None), &[], &[]);
        save(&pool, "review-test", actions.clone(), vec![])
            .await
            .unwrap();
        let overview = meeting_actions_overview(&pool, "review-test", markdown)
            .await
            .unwrap();
        assert!(!overview.has_suggestions);
        assert_eq!(overview.open_count, 1);
        actions[0].done = true;
        save(&pool, "review-test", actions, vec![]).await.unwrap();
        let overview = meeting_actions_overview(&pool, "review-test", markdown)
            .await
            .unwrap();
        assert!(!overview.has_suggestions);
        assert_eq!(overview.open_count, 0);
    }

    #[tokio::test]
    async fn saving_a_preview_keeps_positional_siblings_and_completed_rows() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let markdown = "## Action items\n- [x] Send a quote\n- [ ] Send a quote";
        let fallback = parse_action_suggestions(markdown);
        assert_eq!(fallback.len(), 2);
        let edits = || {
            vec![SummaryActionEdit {
                local_id: fallback[1].action.local_action_id.clone(),
                original_title_hash: super::super::planner::action_hash(&fallback[1].action),
                title: Some("Send a revised quote".into()),
                details: Some("Confirm the new dates.".into()),
            }]
        };
        let saved = save_summary_action_review(&pool, "review-test", markdown, edits())
            .await
            .unwrap();
        assert_eq!(saved.len(), 2);
        assert!(saved[0].done);
        assert!(!saved[1].done);
        assert_eq!(saved[1].title, "Send a revised quote");
        assert_eq!(saved[1].details.as_deref(), Some("Confirm the new dates."));
        for (action, original) in saved.iter().zip(&fallback) {
            let legacy = action.legacy_action.as_ref().unwrap();
            assert_eq!(legacy.id, original.action.local_action_id);
            assert_eq!(
                legacy.title_hash,
                super::super::planner::action_hash(&original.action)
            );
        }
        assert!(
            save_summary_action_review(&pool, "review-test", markdown, edits())
                .await
                .is_err()
        );
        assert_eq!(load(&pool, "review-test").await.unwrap().unwrap(), saved);
    }

    #[tokio::test]
    async fn stale_action_editor_cannot_reopen_a_completed_action() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let mut action = ReviewedAction {
            id: uuid::Uuid::new_v4().to_string(),
            title: "Send review".into(),
            owner: None,
            due_date: None,
            done: false,
            source_key: None,
            legacy_action: None,
            details: None,
            source: None,
        };
        save(&pool, "review-test", vec![action.clone()], vec![])
            .await
            .unwrap();
        let raw: String =
            sqlx::query_scalar("SELECT reviewed_actions FROM meetings WHERE id = 'review-test'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let old_revision = revision(Some(&raw));
        action.done = true;
        save_checked(
            &pool,
            "review-test",
            vec![action.clone()],
            vec![],
            Some(&old_revision),
        )
        .await
        .unwrap();
        action.done = false;
        assert!(save_checked(
            &pool,
            "review-test",
            vec![action],
            vec![],
            Some(&old_revision)
        )
        .await
        .is_err());
        assert!(load(&pool, "review-test").await.unwrap().unwrap()[0].done);
    }

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
