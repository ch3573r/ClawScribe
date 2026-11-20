use crate::state::AppState;
use sqlx::SqlitePool;
use tauri::{AppHandle, Manager};

async fn enabled(pool: &SqlitePool, meeting_id: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM meeting_summary_bookmarks WHERE meeting_id = ?)",
    )
    .bind(meeting_id)
    .fetch_one(pool)
    .await
}

async fn set_enabled(
    pool: &SqlitePool,
    meeting_id: &str,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    if enabled {
        sqlx::query("INSERT INTO meeting_summary_bookmarks (meeting_id) VALUES (?) ON CONFLICT (meeting_id) DO NOTHING")
            .bind(meeting_id)
            .execute(pool)
            .await?;
    } else {
        sqlx::query("DELETE FROM meeting_summary_bookmarks WHERE meeting_id = ?")
            .bind(meeting_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn get_meeting_summary_bookmarks(
    app: AppHandle,
    meeting_id: String,
) -> Result<bool, String> {
    enabled(app.state::<AppState>().db_manager.pool(), &meeting_id)
        .await
        .map_err(|_| "Could not load the summary preference.".into())
}

#[tauri::command]
pub async fn set_meeting_summary_bookmarks(
    app: AppHandle,
    meeting_id: String,
    enabled: bool,
) -> Result<(), String> {
    set_enabled(
        app.state::<AppState>().db_manager.pool(),
        &meeting_id,
        enabled,
    )
    .await
    .map_err(|_| "Could not save the summary preference.".into())
}

pub(crate) async fn append_context(
    pool: &SqlitePool,
    meeting_id: &str,
    mut context: String,
) -> Result<String, String> {
    // Read consent and notes in one snapshot, scoped to the meeting. A missing
    // consent row (including after a restore from an older archive) means off.
    let notes: Vec<(f64, String)> = sqlx::query_as(
        "SELECT b.seconds, b.label FROM meeting_summary_bookmarks p
         JOIN meeting_bookmarks b ON b.meeting_id = p.meeting_id
         WHERE p.meeting_id = ?
         ORDER BY b.seconds, b.id LIMIT 40",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .map_err(|_| "Could not read the meeting's summary priorities.")?;
    let mut lines = Vec::new();
    for (seconds, label) in notes {
        if !seconds.is_finite() || !(0.0..=604800.0).contains(&seconds) {
            continue;
        }
        let cleaned: String = label
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let label: String = cleaned
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect();
        if label.is_empty() {
            continue;
        }
        let seconds = seconds as u64;
        let hours = seconds / 3600;
        let minutes = seconds / 60 % 60;
        let seconds = seconds % 60;
        lines.push(format!("- [{hours:02}:{minutes:02}:{seconds:02}] {label}"));
    }
    if !lines.is_empty() {
        if !context.is_empty() {
            context.push_str("\n\n");
        }
        context.push_str("User-marked moments\nUse these notes as context for finding related transcript passages. Base decisions and action items on what the transcript supports.\n\n");
        context.push_str(&lines.join("\n"));
    }
    Ok(context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn summary_priorities_are_private_until_enabled_for_that_meeting() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO meetings (id, title, created_at, updated_at) VALUES
             ('a', 'One', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
             ('b', 'Two', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
             INSERT INTO meeting_bookmarks (id, meeting_id, seconds, label) VALUES
             ('later', 'a', 65.9, 'Later note'), ('earlier', 'a', 1.0, 'Earlier note'),
             ('other', 'b', 2.0, 'Other note');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let context = || append_context(&pool, "a", "Meeting context".into());
        assert!(!enabled(&pool, "a").await.unwrap());
        assert_eq!(context().await.unwrap(), "Meeting context");
        set_enabled(&pool, "b", true).await.unwrap();
        assert_eq!(context().await.unwrap(), "Meeting context");
        set_enabled(&pool, "a", true).await.unwrap();
        let prompt = context().await.unwrap();
        assert!(prompt.starts_with("Meeting context\n\nUser-marked moments"));
        assert!(prompt.ends_with("- [00:00:01] Earlier note\n- [00:01:05] Later note"));
        assert!(!prompt.contains("Other note"));
        set_enabled(&pool, "a", false).await.unwrap();
        assert_eq!(context().await.unwrap(), "Meeting context");
        sqlx::query("DELETE FROM meetings WHERE id = 'b'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!enabled(&pool, "b").await.unwrap());
    }
}
