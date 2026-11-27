use super::file_cleanup::{self, CleanupLocations};
use crate::{
    database::repositories::meeting::{MeetingsRepository, TrashedMeeting},
    state::AppState,
};
use sqlx::SqlitePool;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Runtime, State};

#[tauri::command]
pub(crate) async fn list_trashed_meetings(
    state: State<'_, AppState>,
) -> Result<Vec<TrashedMeeting>, String> {
    MeetingsRepository::trash(state.db_manager.pool())
        .await
        .map_err(|_| "Could not load Trash.".into())
}

#[tauri::command]
pub(crate) async fn restore_trashed_meeting<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<(), String> {
    let _work = crate::updates::admit_work()?;
    if !MeetingsRepository::restore_from_trash(state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|_| "Could not restore the meeting.")?
    {
        return Err("The meeting is no longer in Trash.".into());
    }
    let _ = app.emit("library-changed", ());
    Ok(())
}

#[tauri::command]
pub(crate) async fn purge_trashed_meeting<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    meeting_id: String,
) -> Result<bool, String> {
    let _work = crate::updates::admit_work()?;
    let pool = state.db_manager.pool();
    let ids = purge(pool, &meeting_id, false)
        .await?
        .ok_or("The meeting is no longer in Trash.")?;
    let _ = app.emit("library-meeting-deleted", &meeting_id);
    Ok(cleanup(pool, &CleanupLocations::for_app(&app).await, ids).await)
}

async fn purge(
    pool: &SqlitePool,
    meeting_id: &str,
    expired_only: bool,
) -> Result<Option<Vec<i64>>, String> {
    // Archives hold the same audio permit while capturing a filesystem snapshot.
    let _audio = crate::audio::inference::claim_job().map_err(|_| {
        "Finish the recording, transcription, or backup before deleting permanently."
    })?;
    let _export = crate::exports::commands::EXPORT_LOCK
        .try_lock()
        .map_err(|_| "Finish the active export before deleting permanently.")?;
    MeetingsRepository::purge_from_trash(pool, meeting_id, expired_only)
        .await
        .map(|deleted| deleted.map(|deleted| deleted.cleanup_ids))
        .map_err(|_| "Could not delete the meeting permanently.".into())
}

async fn cleanup(pool: &SqlitePool, locations: &CleanupLocations, ids: Vec<i64>) -> bool {
    let mut pending = false;
    for id in ids {
        // Recheck ownership and require an exact recording marker. The
        // original folder may have been moved or reused during retention.
        pending |= !matches!(
            file_cleanup::attempt(pool, locations, id, None).await,
            Ok(None)
        );
    }
    pending
}

pub(crate) async fn purge_expired<R: Runtime>(app: AppHandle<R>, pool: SqlitePool) {
    let mut interval = tokio::time::interval(Duration::from_secs(10 * 60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let Ok(_work) = crate::updates::admit_work() else {
            continue;
        };
        let ids = match MeetingsRepository::expired_trash_ids(&pool).await {
            Ok(ids) => ids,
            Err(_) => {
                log::warn!("Could not read expired meetings in Trash");
                continue;
            }
        };
        if ids.is_empty() {
            continue;
        }
        let locations = CleanupLocations::for_app(&app).await;
        for id in ids {
            match purge(&pool, &id, true).await {
                Ok(Some(cleanup_ids)) => {
                    let _ = app.emit("library-meeting-deleted", &id);
                    if cleanup(&pool, &locations, cleanup_ids).await {
                        log::warn!("Some expired meeting files are pending cleanup");
                    }
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    }
}
