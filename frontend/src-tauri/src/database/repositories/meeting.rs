use crate::api::{MeetingDetails, MeetingTranscript, TranscriptWord};
use crate::database::models::{MeetingModel, Transcript, TranscriptTimelineRow};
use chrono::Utc;
use sqlx::{Connection, Error as SqlxError, SqliteConnection, SqlitePool};
use tracing::error;

pub struct MeetingsRepository;

pub(crate) struct DeletedMeetingFiles {
    #[cfg(test)]
    pub folder: Option<String>,
    pub cleanup_ids: Vec<i64>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub(crate) struct TrashedMeeting {
    id: String,
    title: String,
    deleted_at: String,
    expires_at: String,
    delete_recording_on_purge: bool,
}

const TRASH_RETENTION: &str = "+30 days";
const EXPIRED: &str = "datetime(deleted_at, ?1) <= CURRENT_TIMESTAMP";
pub(crate) const NOT_FOUND: &str = "Meeting not found.";
// Serialize Trash with publication of transcript files outside SQLite.
pub(crate) static FILE_PUBLICATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl MeetingsRepository {
    pub(crate) async fn is_active<'e>(
        executor: impl sqlx::SqliteExecutor<'e>,
        meeting_id: &str,
    ) -> Result<bool, SqlxError> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM active_meetings WHERE id = ?)")
            .bind(meeting_id)
            .fetch_one(executor)
            .await
    }

    pub async fn get_meetings(pool: &SqlitePool) -> Result<Vec<MeetingModel>, sqlx::Error> {
        sqlx::query_as("SELECT id,title,created_at,updated_at,folder_path FROM active_meetings ORDER BY created_at DESC")
            .fetch_all(pool)
            .await
    }

    #[cfg(test)]
    pub async fn delete_meeting(pool: &SqlitePool, meeting_id: &str) -> Result<bool, SqlxError> {
        Self::delete_meeting_with_documents(pool, meeting_id)
            .await
            .map(|(deleted, _)| deleted)
    }

    pub(crate) async fn move_to_trash(
        pool: &SqlitePool,
        meeting_id: &str,
        delete_files: bool,
    ) -> Result<bool, SqlxError> {
        let _files = FILE_PUBLICATION_LOCK.lock().await;
        let result = sqlx::query(
            "UPDATE meetings SET deleted_at=CURRENT_TIMESTAMP, delete_recording_on_purge=?
             WHERE id=? AND deleted_at IS NULL",
        )
        .bind(delete_files)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn trash(pool: &SqlitePool) -> Result<Vec<TrashedMeeting>, SqlxError> {
        sqlx::query_as(
            "SELECT id,title,deleted_at,datetime(deleted_at,?) expires_at,
                    delete_recording_on_purge AND coalesce(trim(folder_path)<>'',0) delete_recording_on_purge
             FROM meetings WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC,id",
        )
        .bind(TRASH_RETENTION)
        .fetch_all(pool)
        .await
    }

    pub(crate) async fn restore_from_trash(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<bool, SqlxError> {
        let mut tx = pool.begin().await?;
        let result = sqlx::query(
            "UPDATE meetings SET deleted_at=NULL, delete_recording_on_purge=1
             WHERE id=? AND deleted_at IS NOT NULL",
        )
        .bind(meeting_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Ok(false);
        }
        // A discarded completion cannot leave a restored meeting permanently busy.
        super::summary::SummaryProcessesRepository::fail_interrupted_for(
            &mut *tx,
            Some(meeting_id),
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub(crate) async fn expired_trash_ids(pool: &SqlitePool) -> Result<Vec<String>, SqlxError> {
        sqlx::query_scalar(&format!(
            "SELECT id FROM meetings WHERE deleted_at IS NOT NULL AND {EXPIRED}
                                    ORDER BY deleted_at,id LIMIT 20"
        ))
        .bind(TRASH_RETENTION)
        .fetch_all(pool)
        .await
    }

    pub(crate) async fn purge_from_trash(
        pool: &SqlitePool,
        meeting_id: &str,
        expired_only: bool,
    ) -> Result<Option<DeletedMeetingFiles>, SqlxError> {
        Self::purge(pool, meeting_id, expired_only, None).await
    }

    #[cfg(test)]
    pub(crate) async fn delete_from_library(
        pool: &SqlitePool,
        meeting_id: &str,
        delete_files: bool,
    ) -> Result<Option<DeletedMeetingFiles>, SqlxError> {
        Self::purge(pool, meeting_id, false, Some(delete_files)).await
    }

    /// Verify eligibility inside the same write transaction that records file cleanup.
    async fn purge(
        pool: &SqlitePool,
        meeting_id: &str,
        expired_only: bool,
        immediate: Option<bool>,
    ) -> Result<Option<DeletedMeetingFiles>, SqlxError> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        let Some((title, folder, delete_recording)) =
            sqlx::query_as::<_, (String, Option<String>, bool)>(&format!(
                "SELECT title,folder_path,delete_recording_on_purge FROM meetings
                      WHERE id=?2 AND (?3 OR deleted_at IS NOT NULL) AND (NOT ?4 OR {EXPIRED})"
            ))
            .bind(TRASH_RETENTION)
            .bind(meeting_id)
            .bind(immediate.is_some())
            .bind(expired_only)
            .fetch_optional(&mut *tx)
            .await?
        else {
            return Ok(None);
        };
        let (deleted, originals) = delete_meeting_with_transaction(&mut tx, meeting_id).await?;
        if !deleted {
            return Ok(None);
        }
        let recording = folder
            .as_deref()
            .filter(|path| immediate.unwrap_or(delete_recording) && !path.trim().is_empty());
        let cleanup_ids = crate::library::file_cleanup::enqueue(
            &mut tx, meeting_id, &title, recording, &originals,
        )
        .await?;
        tx.commit().await?;
        Ok(Some(DeletedMeetingFiles {
            #[cfg(test)]
            folder,
            cleanup_ids,
        }))
    }

    /// Return original storage names only after the database deletion commits.
    #[cfg(test)]
    pub async fn delete_meeting_with_documents(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<(bool, Vec<String>), SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        match delete_meeting_with_transaction(&mut transaction, meeting_id).await {
            Ok((success, originals)) => {
                if success {
                    transaction.commit().await?;
                    tracing::info!(
                        "Successfully deleted meeting {} and all associated data",
                        meeting_id
                    );
                    Ok((true, originals))
                } else {
                    transaction.rollback().await?;
                    Ok((false, Vec::new()))
                }
            }
            Err(e) => {
                let _ = transaction.rollback().await;
                error!("Failed to delete meeting {}: {}", meeting_id, e);
                Err(e)
            }
        }
    }

    pub async fn get_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<MeetingDetails>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        // Get meeting details
        let meeting: Option<MeetingModel> = sqlx::query_as(
            "SELECT id, title, created_at, updated_at, folder_path FROM active_meetings WHERE id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(&mut *transaction)
        .await?;

        if meeting.is_none() {
            transaction.rollback().await?;
            return Err(SqlxError::RowNotFound);
        }

        if let Some(meeting) = meeting {
            // Get all transcripts for this meeting
            let transcripts =
                sqlx::query_as::<_, Transcript>("SELECT * FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time ASC, id ASC")
                    .bind(meeting_id)
                    .fetch_all(&mut *transaction)
                    .await?;

            transaction.commit().await?;

            // Convert Transcript to MeetingTranscript
            let meeting_transcripts = transcripts
                .into_iter()
                .map(|t| MeetingTranscript {
                    original_text: t.original_transcript,
                    id: t.id,
                    text: t.transcript,
                    timestamp: t.timestamp,
                    audio_start_time: t.audio_start_time,
                    audio_end_time: t.audio_end_time,
                    duration: t.duration,
                    speaker: t.speaker,
                    word_timestamps: parse_word_timestamps(t.word_timestamps_json.as_deref()),
                })
                .collect::<Vec<_>>();

            Ok(Some(MeetingDetails {
                id: meeting.id,
                title: meeting.title,
                created_at: meeting.created_at,
                updated_at: meeting.updated_at,
                transcripts: meeting_transcripts,
            }))
        } else {
            transaction.rollback().await?;
            Ok(None)
        }
    }

    /// Get meeting metadata without transcripts (for pagination)
    pub async fn get_meeting_metadata(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<MeetingModel>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let meeting: Option<MeetingModel> = sqlx::query_as(
            "SELECT id, title, created_at, updated_at, folder_path FROM active_meetings WHERE id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(pool)
        .await?;

        Ok(meeting)
    }

    /// Get meeting transcripts with pagination support
    pub async fn get_meeting_transcripts_paginated(
        pool: &SqlitePool,
        meeting_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<Transcript>, i64), SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        if !(1..=1000).contains(&limit) || offset < 0 {
            return Err(SqlxError::Protocol(
                "Transcript pages require a limit between 1 and 1000 and a non-negative offset"
                    .into(),
            ));
        }

        // Count and rows must refer to the same snapshot during retranscription.
        let mut transaction = pool.begin().await?;
        sqlx::query_scalar::<_, i64>("SELECT 1 FROM active_meetings WHERE id=?")
            .bind(meeting_id)
            .fetch_one(&mut *transaction)
            .await?;
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM transcripts WHERE meeting_id = ?")
            .bind(meeting_id)
            .fetch_one(&mut *transaction)
            .await?;

        // Get paginated transcripts ordered by audio_start_time
        let transcripts = sqlx::query_as::<_, Transcript>(
            "SELECT * FROM transcripts
             WHERE meeting_id = ?
             ORDER BY audio_start_time ASC, id ASC
             LIMIT ? OFFSET ?",
        )
        .bind(meeting_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok((transcripts, total.0))
    }

    pub async fn get_meeting_transcript_timeline(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Vec<TranscriptTimelineRow>, SqlxError> {
        // The review list uses this text for whole-meeting Find and follow-along.
        // Read word alignment and other large fields only in the paged API.
        sqlx::query_as(
            "SELECT id, transcript AS text, original_transcript AS original_text,
                    audio_start_time, audio_end_time, speaker
             FROM transcripts WHERE meeting_id = (SELECT id FROM active_meetings WHERE id=?)
             ORDER BY audio_start_time, id",
        )
        .bind(meeting_id)
        .fetch_all(pool)
        .await
    }

    pub async fn update_meeting_title(
        pool: &SqlitePool,
        meeting_id: &str,
        new_title: &str,
    ) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let now = Utc::now().naive_utc();

        let rows_affected = sqlx::query(
            "UPDATE meetings SET title = ?, updated_at = ? WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(new_title)
        .bind(now)
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;
        if rows_affected.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn update_meeting_name(
        pool: &SqlitePool,
        meeting_id: &str,
        new_title: &str,
    ) -> Result<bool, SqlxError> {
        let mut transaction = pool.begin().await?;
        let now = Utc::now();

        // Update meetings table
        let meeting_update = sqlx::query(
            "UPDATE meetings SET title = ?, updated_at = ? WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(new_title)
        .bind(now)
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

        if meeting_update.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false); // Meeting not found
        }

        // Update transcript_chunks table
        sqlx::query("UPDATE transcript_chunks SET meeting_name = ? WHERE meeting_id = ?")
            .bind(new_title)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;
        Ok(true)
    }

    pub async fn update_generated_meeting_name(
        pool: &SqlitePool,
        meeting_id: &str,
        current: &str,
        title: &str,
    ) -> Result<bool, SqlxError> {
        let mut transaction = pool.begin().await?;
        let updated = sqlx::query("UPDATE meetings SET title = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ? AND title = ? AND deleted_at IS NULL")
            .bind(title).bind(meeting_id).bind(current).execute(&mut *transaction).await?;
        if updated.rows_affected() == 0 {
            return Ok(false);
        }
        sqlx::query("UPDATE transcript_chunks SET meeting_name = ? WHERE meeting_id = ?")
            .bind(title)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(true)
    }
}

fn parse_word_timestamps(value: Option<&str>) -> Option<Vec<TranscriptWord>> {
    value.and_then(|json| serde_json::from_str::<Vec<TranscriptWord>>(json).ok())
}

async fn delete_meeting_with_transaction(
    transaction: &mut SqliteConnection,
    meeting_id: &str,
) -> Result<(bool, Vec<String>), SqlxError> {
    // Check if meeting exists
    let meeting_exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_optional(&mut *transaction)
        .await?;

    if meeting_exists.is_none() {
        error!("Meeting {} not found for deletion", meeting_id);
        return Ok((false, Vec::new()));
    }

    // Delete through sources before attachments cascade, preserving the same
    // invalidation/redaction behavior as explicit reference deletion.
    let documents: Vec<(String, String)> = sqlx::query_as(
        "SELECT d.id,d.storage_name FROM knowledge_documents d
         WHERE EXISTS (SELECT 1 FROM knowledge_document_attachments a
                       WHERE a.document_id=d.id AND a.meeting_id=?)
           AND NOT EXISTS (SELECT 1 FROM knowledge_document_attachments a
                           WHERE a.document_id=d.id AND a.meeting_id<>?)",
    )
    .bind(meeting_id)
    .bind(meeting_id)
    .fetch_all(&mut *transaction)
    .await?;
    for (id, _) in &documents {
        sqlx::query("DELETE FROM knowledge_sources WHERE id=? AND kind='document'")
            .bind(format!("document:{id}"))
            .execute(&mut *transaction)
            .await?;
    }

    // Live bookmarks may not have been associated by the library view yet.
    sqlx::query("DELETE FROM meeting_bookmarks WHERE meeting_id = ? OR (meeting_id IS NULL AND recording_id IS NULL AND folder_path IN (SELECT folder_path FROM meetings WHERE id = ?))")
        .bind(meeting_id).bind(meeting_id).execute(&mut *transaction).await?;

    // Delete from related tables in proper order
    // 1. Delete from transcript_chunks
    sqlx::query("DELETE FROM transcript_chunks WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 2. Delete from summary_processes
    sqlx::query("DELETE FROM summary_processes WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 3. Delete from transcripts
    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 4. Finally, delete the meeting
    let result = sqlx::query("DELETE FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    Ok((
        result.rows_affected() > 0,
        documents.into_iter().map(|(_, name)| name).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn trash_blocks_all_title_updates() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let id = "review-test";
        assert!(MeetingsRepository::move_to_trash(&pool, id, false)
            .await
            .unwrap());
        assert!(
            !MeetingsRepository::update_meeting_title(&pool, id, "Edited")
                .await
                .unwrap()
        );
        assert!(
            !MeetingsRepository::update_meeting_name(&pool, id, "Renamed")
                .await
                .unwrap()
        );
        assert!(!MeetingsRepository::update_generated_meeting_name(
            &pool,
            id,
            "Synthetic review",
            "Generated"
        )
        .await
        .unwrap());
        assert!(MeetingsRepository::restore_from_trash(&pool, id)
            .await
            .unwrap());
        let title: String = sqlx::query_scalar("SELECT title FROM active_meetings WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(title, "Synthetic review");
    }

    #[tokio::test]
    async fn moving_a_meeting_to_trash_hides_it_and_keeps_its_transcript() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        assert!(
            MeetingsRepository::move_to_trash(&pool, "review-test", true)
                .await
                .unwrap()
        );

        assert!(MeetingsRepository::get_meetings(&pool)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM transcripts WHERE meeting_id='review-test'"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            2
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pending_meeting_file_cleanup")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert!(
            MeetingsRepository::get_meeting_metadata(&pool, "review-test")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            crate::exports::metadata::Header::load(&pool, "review-test", false)
                .await
                .is_err()
        );
        let scope = crate::knowledge::types::KnowledgeScope::Meeting {
            meeting_id: "review-test".into(),
        };
        let frozen = crate::knowledge::retrieval::freeze_scope(&pool, &scope)
            .await
            .unwrap();
        assert!(frozen.meeting_ids.is_empty());
        assert!(MeetingsRepository::restore_from_trash(&pool, "review-test")
            .await
            .unwrap());
        assert_eq!(
            MeetingsRepository::get_meeting(&pool, "review-test")
                .await
                .unwrap()
                .unwrap()
                .transcripts
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn automatic_purge_rechecks_expiry_and_honors_recording_retention_choice() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let id = "review-test";
        sqlx::query("UPDATE meetings SET folder_path='synthetic recording' WHERE id=?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(MeetingsRepository::purge_from_trash(&pool, id, false)
            .await
            .unwrap()
            .is_none());
        assert!(MeetingsRepository::move_to_trash(&pool, id, false)
            .await
            .unwrap());
        assert!(MeetingsRepository::purge_from_trash(&pool, id, true)
            .await
            .unwrap()
            .is_none());
        let age = || async {
            sqlx::query("UPDATE meetings SET deleted_at=datetime('now','-31 days') WHERE id=?")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        };
        age().await;
        assert_eq!(
            MeetingsRepository::expired_trash_ids(&pool).await.unwrap(),
            [id]
        );
        assert!(MeetingsRepository::restore_from_trash(&pool, id)
            .await
            .unwrap());
        assert!(MeetingsRepository::purge_from_trash(&pool, id, true)
            .await
            .unwrap()
            .is_none());
        assert!(MeetingsRepository::move_to_trash(&pool, id, false)
            .await
            .unwrap());
        age().await;
        assert!(MeetingsRepository::purge_from_trash(&pool, id, true)
            .await
            .unwrap()
            .is_some());
        assert!(!MeetingsRepository::restore_from_trash(&pool, id)
            .await
            .unwrap());
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT kind FROM pending_meeting_file_cleanup ORDER BY kind"
            )
            .fetch_all(&pool)
            .await
            .unwrap(),
            ["codex", "exports", "outputs"]
        );
    }

    #[tokio::test]
    async fn timeline_keeps_saved_text_and_timing_without_word_alignment() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let timeline = MeetingsRepository::get_meeting_transcript_timeline(&pool, "review-test")
            .await
            .unwrap();
        assert_eq!(timeline.len(), 2);
        assert_eq!(timeline[1].text, "Project confirmed");
        assert_eq!(timeline[1].audio_start_time, Some(900.));
        assert!(serde_json::to_value(&timeline[1])
            .unwrap()
            .get("word_timestamps")
            .is_none());
        let (page, _) =
            MeetingsRepository::get_meeting_transcripts_paginated(&pool, "review-test", 1, 1)
                .await
                .unwrap();
        assert_eq!(page[0].word_timestamps_json.as_deref(), Some("[]"));
    }

    #[tokio::test]
    async fn pagination_is_bounded_and_matches_full_transcript_order() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time) VALUES ('c', 'review-test', 'Earlier passage', '00:00', 0)")
            .execute(&pool).await.unwrap();
        let all = MeetingsRepository::get_meeting(&pool, "review-test")
            .await
            .unwrap()
            .unwrap();
        let mut paged = Vec::new();
        for offset in 0..3 {
            let (rows, total) = MeetingsRepository::get_meeting_transcripts_paginated(
                &pool,
                "review-test",
                1,
                offset,
            )
            .await
            .unwrap();
            assert_eq!(total, 3);
            paged.extend(rows.into_iter().map(|row| row.id));
        }
        assert_eq!(paged, vec!["a", "c", "b"]);
        assert_eq!(
            paged,
            all.transcripts
                .into_iter()
                .map(|row| row.id)
                .collect::<Vec<_>>()
        );
        for (limit, offset) in [(-1, 0), (0, 0), (1001, 0), (1, -1)] {
            assert!(MeetingsRepository::get_meeting_transcripts_paginated(
                &pool,
                "review-test",
                limit,
                offset
            )
            .await
            .is_err());
        }
    }

    #[tokio::test]
    async fn pagination_uses_meeting_index_without_a_temporary_sort() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as("EXPLAIN QUERY PLAN SELECT * FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time ASC, id ASC LIMIT ? OFFSET ?")
            .bind("review-test").bind(100).bind(0).fetch_all(&pool).await.unwrap();
        assert!(plan
            .iter()
            .any(|row| row.3.contains("idx_transcripts_meeting_audio_id")));
        assert!(!plan.iter().any(|row| row.3.contains("TEMP B-TREE")));
    }
}
