use crate::state::AppState;
use sqlx::SqlitePool;

pub(crate) async fn read(pool: &SqlitePool, meeting: &str) -> Result<String, String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT summary_context FROM meetings WHERE id = ?")
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
        "UPDATE meetings SET summary_context = COALESCE(summary_context, ?) WHERE id = ?"
    } else {
        "UPDATE meetings SET summary_context = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?"
    };
    let result = sqlx::query(query)
        .bind(context)
        .bind(meeting)
        .execute(pool)
        .await
        .map_err(|_| "Could not save meeting context".to_string())?;
    if result.rows_affected() == 0 && !legacy {
        return Err("Meeting not found".into());
    }
    Ok(())
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
