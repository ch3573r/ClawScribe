use crate::{database::repositories::meeting::NOT_FOUND, state::AppState};
use sqlx::SqlitePool;

pub(crate) async fn read(pool: &SqlitePool, meeting: &str) -> Result<String, String> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT summary_context FROM active_meetings WHERE id = ?",
    )
    .bind(meeting)
    .fetch_optional(pool)
    .await
    .map_err(|_| "Could not load meeting context".to_string())?
    .map(|context| context.unwrap_or_default())
    .ok_or_else(|| "Meeting not found".to_string())
}

pub(crate) async fn write(
    pool: &SqlitePool,
    meeting: &str,
    context: &str,
    legacy: bool,
) -> Result<(), String> {
    let context = (!context.trim().is_empty()).then_some(context);
    let query = if legacy {
        "UPDATE meetings SET summary_context = COALESCE(summary_context, ?) WHERE id = ? AND deleted_at IS NULL"
    } else {
        "UPDATE meetings SET summary_context = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ? AND deleted_at IS NULL"
    };
    let save_error = |_| "Could not save meeting context".to_string();
    let mut tx = pool.begin().await.map_err(save_error)?;
    let result = sqlx::query(query)
        .bind(context)
        .bind(meeting)
        .execute(&mut *tx)
        .await
        .map_err(save_error)?;
    if result.rows_affected() == 0 {
        // Legacy migration can discard a context whose meeting is gone.
        let unavailable = !legacy
            || sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ? AND deleted_at IS NOT NULL)",
            )
            .bind(meeting)
            .fetch_one(&mut *tx)
            .await
            .map_err(save_error)?;
        if unavailable {
            return Err(NOT_FOUND.into());
        }
    }
    tx.commit().await.map_err(save_error)
}

#[tauri::command]
pub async fn get_meeting_context(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<String, String> {
    read(state.db_manager.pool(), &meeting_id).await
}

#[tauri::command]
pub async fn set_meeting_context(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    context: String,
    only_if_missing: Option<bool>,
) -> Result<(), String> {
    write(
        state.db_manager.pool(),
        &meeting_id,
        &context,
        only_if_missing.unwrap_or(false),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn trash_rejects_context_writes_without_modifying_the_meeting() {
        use crate::database::repositories::meeting::MeetingsRepository;

        let pool = crate::database::transcript_edits::tests::fixture().await;
        let id = "review-test";
        assert!(MeetingsRepository::move_to_trash(&pool, id, false)
            .await
            .unwrap());
        for legacy in [true, false] {
            assert_eq!(
                write(&pool, id, "Draft context", legacy).await.unwrap_err(),
                NOT_FOUND
            );
        }
        assert!(MeetingsRepository::restore_from_trash(&pool, id)
            .await
            .unwrap());
        assert_eq!(read(&pool, id).await.unwrap(), "");
    }

    #[tokio::test]
    async fn legacy_context_migration_skips_missing_meetings() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        write(&pool, "missing", "Stale context", true)
            .await
            .unwrap();
        assert_eq!(
            write(&pool, "missing", "Draft context", false)
                .await
                .unwrap_err(),
            NOT_FOUND
        );
    }

    #[tokio::test]
    async fn legacy_context_never_replaces_saved_context_and_clear_is_persisted() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        write(&pool, "review-test", "Legacy context", true)
            .await
            .unwrap();
        assert_eq!(read(&pool, "review-test").await.unwrap(), "Legacy context");
        write(&pool, "review-test", "Reviewed context", false)
            .await
            .unwrap();
        write(&pool, "review-test", "Legacy context", true)
            .await
            .unwrap();
        assert_eq!(
            read(&pool, "review-test").await.unwrap(),
            "Reviewed context"
        );
        write(&pool, "review-test", "", false).await.unwrap();
        assert_eq!(read(&pool, "review-test").await.unwrap(), "");
    }
}
