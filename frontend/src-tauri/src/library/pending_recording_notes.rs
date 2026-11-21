use crate::state::AppState;
use serde::Serialize;
use sqlx::SqlitePool;

const PAGE_SIZE: i64 = 20;
const STALE: &str = "The pending notes changed. Close and reopen the list to review them again.";
const STORAGE_ERROR: &str = "Could not update pending recording notes. Try again.";

#[derive(Serialize, sqlx::FromRow)]
struct PendingNotes {
    recording_id: String,
    folder_name: Option<String>,
    first_note: Option<String>,
    note_count: i64,
    live_answer_count: i64,
    include_in_summary: bool,
}

#[derive(Serialize)]
pub struct PendingNotesPage {
    entries: Vec<PendingNotes>,
    has_more: bool,
}

async fn list(pool: &SqlitePool, page: u32) -> Result<PendingNotesPage, sqlx::Error> {
    let mut entries: Vec<PendingNotes> = sqlx::query_as(
        "WITH pending AS (
            SELECT recording_id FROM meeting_bookmarks
            WHERE meeting_id IS NULL AND recording_id IS NOT NULL
            UNION SELECT recording_id FROM recording_summary_bookmarks
            UNION SELECT recording_id FROM saved_live_answers WHERE meeting_id IS NULL
         )
         SELECT p.recording_id,
            NULLIF(substr(b.folder_path, -1024), '') AS folder_name,
            substr(b.label, 1, 160) AS first_note,
            (SELECT COUNT(*) FROM meeting_bookmarks
             WHERE recording_id = p.recording_id AND meeting_id IS NULL) AS note_count,
            (SELECT COUNT(*) FROM saved_live_answers
             WHERE recording_id = p.recording_id AND meeting_id IS NULL) AS live_answer_count,
            EXISTS(SELECT 1 FROM recording_summary_bookmarks
                   WHERE recording_id = p.recording_id) AS include_in_summary
         FROM pending p
         LEFT JOIN meeting_bookmarks b ON b.id = (
             SELECT id FROM meeting_bookmarks WHERE recording_id = p.recording_id AND meeting_id IS NULL
             ORDER BY seconds, id LIMIT 1
         )
         WHERE NOT EXISTS(SELECT 1 FROM meetings WHERE id = p.recording_id)
         ORDER BY p.recording_id LIMIT ? OFFSET ?",
    )
    .bind(PAGE_SIZE + 1)
    .bind(i64::from(page) * PAGE_SIZE)
    .fetch_all(pool)
    .await?;

    let has_more = entries.len() > PAGE_SIZE as usize;
    entries.truncate(PAGE_SIZE as usize);
    for entry in &mut entries {
        // The label is just for review and never used to authorize removal.
        // Keep the original directory path out of the IPC response.
        entry.folder_name = entry.folder_name.take().and_then(|path| {
            path.trim_end_matches(['/', '\\'])
                .rsplit(['/', '\\'])
                .next()
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        });
    }
    Ok(PendingNotesPage { entries, has_more })
}

#[tauri::command]
pub async fn list_pending_recording_notes(
    state: tauri::State<'_, AppState>,
    page: u32,
) -> Result<PendingNotesPage, String> {
    list(state.db_manager.pool(), page)
        .await
        .map_err(|_| "Could not list pending recording notes. Try again.".into())
}

#[tauri::command]
pub async fn delete_pending_recording_notes(
    state: tauri::State<'_, AppState>,
    recording_id: String,
    note_count: i64,
    live_answer_count: Option<i64>,
    include_in_summary: bool,
) -> Result<(), String> {
    let _job = crate::audio::inference::claim_job()
        .map_err(|_| "Finish the current recording or library operation and try again.")?;
    if crate::audio::recording_commands::capture_active() {
        return Err("Stop the recording and try again.".into());
    }
    delete_reviewed(
        state.db_manager.pool(),
        &recording_id,
        note_count,
        live_answer_count.unwrap_or_default(),
        include_in_summary,
    )
    .await
}

async fn delete_reviewed(
    pool: &SqlitePool,
    recording_id: &str,
    reviewed_count: i64,
    reviewed_answer_count: i64,
    reviewed_consent: bool,
) -> Result<(), String> {
    let valid = recording_id.strip_prefix("meeting-").is_some_and(|text| {
        uuid::Uuid::parse_str(text).is_ok_and(|id| id.hyphenated().to_string() == text)
    });
    if !valid {
        return Err("The recording identity could not be verified.".into());
    }

    // A missing or unreadable directory never authorizes deletion. This action
    // removes the metadata the user reviewed, independent of folder discovery.
    let mut tx = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| STORAGE_ERROR)?;
    let (saved, count, answer_count, consent): (bool, i64, i64, bool) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?),
                (SELECT COUNT(*) FROM meeting_bookmarks WHERE meeting_id IS NULL AND recording_id = ?),
                (SELECT COUNT(*) FROM saved_live_answers WHERE meeting_id IS NULL AND recording_id = ?),
                EXISTS(SELECT 1 FROM recording_summary_bookmarks WHERE recording_id = ?)",
    )
    .bind(recording_id)
    .bind(recording_id)
    .bind(recording_id)
    .bind(recording_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| STORAGE_ERROR)?;
    if saved {
        return Err("This recording has already been saved.".into());
    }
    if (count == 0 && answer_count == 0 && !consent)
        || count != reviewed_count
        || answer_count != reviewed_answer_count
        || consent != reviewed_consent
    {
        return Err(STALE.into());
    }
    super::remove_pending_bookmarks(&mut tx, recording_id)
        .await
        .map_err(|_| STORAGE_ERROR)?;
    sqlx::query("DELETE FROM saved_live_answers WHERE recording_id = ? AND meeting_id IS NULL")
        .bind(recording_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| STORAGE_ERROR)?;
    tx.commit().await.map_err(|_| STORAGE_ERROR.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REMOVE: &str = "meeting-1799fc19-f74b-45da-8a87-dc48704444be";
    const RECOVER: &str = "meeting-75d0a116-a841-47a1-846c-21ef77046114";
    const CONSENT_ONLY: &str = "meeting-439568a4-2125-42a7-9c8d-815bfa81142e";

    #[tokio::test]
    async fn only_the_reviewed_pending_session_is_removed_and_other_notes_follow_recovery() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        for (id, label) in [(REMOVE, "Remove me"), (RECOVER, "Recover me")] {
            sqlx::query(
                "INSERT INTO meeting_bookmarks (id, folder_path, seconds, label, recording_id)
                 VALUES (?, 'missing/Synthetic recording', 5.1, ?, ?)",
            )
            .bind(id)
            .bind(label)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        }
        for id in [REMOVE, RECOVER, CONSENT_ONLY] {
            sqlx::query("INSERT INTO recording_summary_bookmarks VALUES (?)")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }

        let page = list(&pool, 0).await.unwrap();
        let entry = page
            .entries
            .iter()
            .find(|row| row.recording_id == REMOVE)
            .unwrap();
        assert_eq!(entry.first_note.as_deref(), Some("Remove me"));
        assert_eq!(entry.folder_name.as_deref(), Some("Synthetic recording"));
        assert!(entry.include_in_summary);
        assert!(!page.has_more);
        assert!(page
            .entries
            .iter()
            .any(|row| row.recording_id == CONSENT_ONLY && row.note_count == 0));

        assert!(delete_reviewed(&pool, "invalid", 1, 0, true).await.is_err());
        assert_eq!(
            delete_reviewed(&pool, REMOVE, 2, 0, true)
                .await
                .unwrap_err(),
            STALE
        );
        sqlx::raw_sql(
            "CREATE TRIGGER forbid_preference_delete BEFORE DELETE ON recording_summary_bookmarks
             BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            delete_reviewed(&pool, REMOVE, 1, 0, true)
                .await
                .unwrap_err(),
            STORAGE_ERROR
        );
        assert_eq!(list(&pool, 0).await.unwrap().entries.len(), 3);
        sqlx::query("DROP TRIGGER forbid_preference_delete")
            .execute(&pool)
            .await
            .unwrap();

        delete_reviewed(&pool, REMOVE, 1, 0, true).await.unwrap();
        delete_reviewed(&pool, CONSENT_ONLY, 0, 0, true)
            .await
            .unwrap();
        let entries = list(&pool, 0).await.unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].recording_id, RECOVER);

        sqlx::query(
            "INSERT INTO meetings (id, title, folder_path, created_at, updated_at)
             VALUES (?, 'Recovered', 'moved/Synthetic recording', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        )
        .bind(RECOVER)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            delete_reviewed(&pool, RECOVER, 1, true).await.unwrap_err(),
            "This recording has already been saved."
        );
        assert!(list(&pool, 0).await.unwrap().entries.is_empty());
        let summary =
            crate::library::summary_bookmarks::append_context(&pool, RECOVER, String::new())
                .await
                .unwrap();
        assert!(summary.ends_with("- [00:00:05] Recover me"));
    }
}
