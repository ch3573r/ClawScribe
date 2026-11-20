//! Calendar edits belong to the recorder's durable ID until library registration.
use super::calendar::{CalendarLink, CalendarSource};
use crate::exports::ms_auth_state::MicrosoftAuthState;
use sqlx::SqlitePool;
use tauri::{AppHandle, Manager, Runtime};

// Stop closes admission before stopping the streams. Before registering the
// meeting it waits for any edit already admitted to finish its database write.
pub(crate) static EDIT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(serde::Serialize)]
pub(crate) struct Draft {
    link: Option<CalendarLink>,
    revision: i64,
}

async fn read(pool: &SqlitePool, id: &str) -> Result<Draft, String> {
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT calendar_link, revision FROM recording_calendar_drafts WHERE recording_id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|_| "Could not read this recording's calendar.".to_string())?;
    match row {
        Some((stored, revision)) => Ok(Draft {
            link: super::calendar::decode(&stored)?,
            revision,
        }),
        None => Ok(Draft {
            link: None,
            revision: 0,
        }),
    }
}

async fn write<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    revision: i64,
    link: Option<CalendarLink>,
) -> Result<i64, String> {
    let auth = app.state::<MicrosoftAuthState>();
    // A sign-out either wins before the write or clears the newly staged link.
    let _auth = if link
        .as_ref()
        .is_some_and(|link| link.source() == CalendarSource::Microsoft && !link.is_reviewed())
    {
        let auth = auth.inner.read().await;
        if auth.current_token.is_none() {
            return Err("Microsoft calendar is disconnected.".into());
        }
        Some(auth)
    } else {
        None
    };
    let encoded = match link {
        Some(link) => link.normalized()?.encoded()?,
        None => "null".into(),
    };
    let state = app.state::<crate::state::AppState>();
    let pool = state.db_manager.pool();
    let result = if revision == 0 {
        sqlx::query("INSERT INTO recording_calendar_drafts (recording_id, calendar_link, revision) VALUES (?, ?, 1) ON CONFLICT(recording_id) DO NOTHING")
            .bind(id).bind(encoded).execute(pool).await
    } else {
        sqlx::query("UPDATE recording_calendar_drafts SET calendar_link = ?, revision = revision + 1 WHERE recording_id = ? AND revision = ?")
            .bind(encoded).bind(id).bind(revision).execute(pool).await
    }
    .map_err(|_| "Could not save this recording's calendar.".to_string())?;
    if result.rows_affected() == 0 {
        return Err("This recording's calendar changed. Reopen the attendance review.".into());
    }
    Ok(revision + 1)
}

pub(crate) async fn stage_initial<R: Runtime>(
    app: &AppHandle<R>,
    recording_id: Option<&str>,
    link: Option<CalendarLink>,
) -> Result<Option<String>, String> {
    if let Some(link) = link {
        let id = recording_id.ok_or("The recording folder is unavailable.")?;
        write(app, id, 0, Some(link)).await?;
        return Ok(Some(id.to_owned()));
    }
    Ok(None)
}

pub(crate) async fn discard_failed_start_draft(
    pool: &SqlitePool,
    staged_id: Option<&str>,
    empty_folder_removed: bool,
) -> Result<(), sqlx::Error> {
    if let (Some(id), true) = (staged_id, empty_folder_removed) {
        sqlx::query("DELETE FROM recording_calendar_drafts WHERE recording_id = ?")
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn get_active_recording_calendar<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<Draft, String> {
    let id = crate::audio::recording_commands::calendar_recording_id(&session_id)?;
    let state = app.state::<crate::state::AppState>();
    read(state.db_manager.pool(), &id).await
}

#[tauri::command]
pub(crate) async fn set_active_recording_calendar<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    revision: i64,
    link: Option<CalendarLink>,
) -> Result<i64, String> {
    let _edit = EDIT.lock().await;
    let id = crate::audio::recording_commands::calendar_recording_id(&session_id)?;
    write(&app, &id, revision, link).await
}
