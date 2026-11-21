use crate::database::models::SummaryProcess;
use chrono::Utc;
use serde_json::Value;
use sqlx::{FromRow, Row, SqlitePool};
use tracing::{error, info as log_info};

pub struct SummaryProcessesRepository;
const INTERRUPTED: &str = "Summary generation was interrupted when ClawScribe closed";
pub(crate) const RECOVERY_NEEDED: &str =
    "Review or retranscribe this meeting before generating notes.";
pub(crate) const NO_TRANSCRIPT: &str = "There is no saved transcript to summarize.";

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn editing_notes_preserves_their_generation_revision_and_email_without_reusing_cached_text(
    ) {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let meeting = "review-test";
        let revision: i64 =
            sqlx::query_scalar("SELECT summary_content_revision FROM meetings WHERE id = ?")
                .bind(meeting)
                .fetch_one(&pool)
                .await
                .unwrap();
        SummaryProcessesRepository::create_or_reset_process(&pool, meeting)
            .await
            .unwrap();
        let result = serde_json::json!({
            "markdown":"Generated",
            "english_cache":{"markdown":"Previous English text"},
            "generation":{"model":"example-model","template":"Standard","transcript_fingerprint":"original"},
            "structured_meeting_output":{
                "action_items":[{"task":"Send draft", "owner":"Dana"}],
                "follow_up_email":{"subject":"Next steps","body_markdown":"Send draft"}
            }
        });
        assert!(SummaryProcessesRepository::complete_for_revision(
            &pool,
            meeting,
            result,
            1,
            0.0,
            Some(revision)
        )
        .await
        .unwrap());
        sqlx::query("UPDATE transcripts SET transcript = 'Revised' WHERE id = 'a'")
            .execute(&pool)
            .await
            .unwrap();
        SummaryProcessesRepository::update_meeting_summary(
            &pool,
            meeting,
            &serde_json::json!({"markdown":"Edited notes"}),
        )
        .await
        .unwrap();
        let row = SummaryProcessesRepository::get_summary_data_for_meeting(&pool, meeting)
            .await
            .unwrap()
            .unwrap();
        let saved: Value = serde_json::from_str(row.result.as_deref().unwrap()).unwrap();
        assert_eq!(saved["transcript_content_revision"], revision);
        assert_ne!(row.current_content_revision, Some(revision));
        assert_eq!(saved["generation"]["model"], "example-model");
        assert_eq!(saved["follow_up_email"]["subject"], "Next steps");
        assert_eq!(
            saved["structured_meeting_output"]["action_items"][0]["owner"],
            "Dana"
        );
        assert!(saved.get("english_cache").is_none());
    }

    #[tokio::test]
    async fn repaired_recording_can_claim_only_the_recoverable_skip_once() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let meeting = "review-test";
        SummaryProcessesRepository::record_automatic_outcome(
            &pool,
            meeting,
            "skipped",
            RECOVERY_NEEDED,
            "recording",
        )
        .await
        .unwrap();
        assert!(!SummaryProcessesRepository::create_automatic_process(
            &pool, meeting, "import", "approved"
        )
        .await
        .unwrap());
        assert!(SummaryProcessesRepository::create_automatic_process(
            &pool,
            meeting,
            "recording",
            "approved"
        )
        .await
        .unwrap());
        assert!(!SummaryProcessesRepository::create_automatic_process(
            &pool,
            meeting,
            "recording",
            "approved"
        )
        .await
        .unwrap());
        sqlx::query("UPDATE summary_processes SET status = 'skipped', error = 'Automatic summaries are off.' WHERE meeting_id = ?")
            .bind(meeting).execute(&pool).await.unwrap();
        assert!(!SummaryProcessesRepository::create_automatic_process(
            &pool,
            meeting,
            "recording",
            "approved"
        )
        .await
        .unwrap());
    }

    #[tokio::test]
    async fn record_automatic_outcome_replaces_only_the_recoverable_skip() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let meeting = "review-test";

        // A RECOVERY_NEEDED skip is the recoverable state; subsequent skip reasons
        // (disabled, review required) replace it so the UI shows the current reason.
        SummaryProcessesRepository::record_automatic_outcome(
            &pool,
            meeting,
            "skipped",
            RECOVERY_NEEDED,
            "recording",
        )
        .await
        .unwrap();
        const DISABLED: &str = "Automatic summaries are off. You can generate notes here.";
        const REVIEW_REQUIRED: &str =
            "Review the summary provider in Settings to allow automatic notes at this destination.";
        SummaryProcessesRepository::record_automatic_outcome(
            &pool,
            meeting,
            "skipped",
            DISABLED,
            "recording",
        )
        .await
        .unwrap();
        let row = SummaryProcessesRepository::get_summary_data_for_meeting(&pool, meeting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.error.as_deref(), Some(DISABLED));

        // A second new reason must NOT overwrite the already-updated non-recoverable skip.
        SummaryProcessesRepository::record_automatic_outcome(
            &pool,
            meeting,
            "skipped",
            REVIEW_REQUIRED,
            "recording",
        )
        .await
        .unwrap();
        let row2 = SummaryProcessesRepository::get_summary_data_for_meeting(&pool, meeting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row2.error.as_deref(), Some(DISABLED));

        // A completed summary must never be overwritten.
        SummaryProcessesRepository::create_or_reset_process(&pool, meeting)
            .await
            .unwrap();
        SummaryProcessesRepository::update_process_completed(
            &pool,
            meeting,
            serde_json::json!({"markdown": "Real notes"}),
            1,
            0.0,
        )
        .await
        .unwrap();
        SummaryProcessesRepository::record_automatic_outcome(
            &pool,
            meeting,
            "skipped",
            RECOVERY_NEEDED,
            "recording",
        )
        .await
        .unwrap();
        let completed = SummaryProcessesRepository::get_summary_data_for_meeting(&pool, meeting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.status, "completed");
    }

    #[tokio::test]
    async fn startup_retries_only_interrupted_background_jobs_once_without_resetting_results() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let meeting = "review-test";
        SummaryProcessesRepository::create_or_reset_process(&pool, meeting)
            .await
            .unwrap();
        SummaryProcessesRepository::fail_interrupted_processes(&pool)
            .await
            .unwrap();
        assert!(
            SummaryProcessesRepository::automatic_resume_candidates(&pool, "approved", None)
                .await
                .unwrap()
                .is_empty()
        );

        sqlx::query("DELETE FROM summary_processes WHERE meeting_id = ?")
            .bind(meeting)
            .execute(&pool)
            .await
            .unwrap();
        assert!(SummaryProcessesRepository::create_automatic_process(
            &pool,
            meeting,
            "recording",
            "approved"
        )
        .await
        .unwrap());
        sqlx::query("UPDATE summary_processes SET result = 'previous', metadata = 'partial' WHERE meeting_id = ?")
            .bind(meeting).execute(&pool).await.unwrap();
        SummaryProcessesRepository::fail_interrupted_processes(&pool)
            .await
            .unwrap();
        assert_eq!(
            SummaryProcessesRepository::automatic_resume_candidates(&pool, "approved", None)
                .await
                .unwrap(),
            vec![(meeting.to_owned(), "approved".to_owned())]
        );
        assert!(
            SummaryProcessesRepository::automatic_resume_candidates(&pool, "different", None)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !SummaryProcessesRepository::claim_automatic_resume(&pool, meeting, "different")
                .await
                .unwrap()
        );
        assert!(
            SummaryProcessesRepository::claim_automatic_resume(&pool, meeting, "approved")
                .await
                .unwrap()
        );
        SummaryProcessesRepository::fail_interrupted_processes(&pool)
            .await
            .unwrap();
        assert!(
            !SummaryProcessesRepository::claim_automatic_resume(&pool, meeting, "approved")
                .await
                .unwrap()
        );
        assert!(
            SummaryProcessesRepository::automatic_resume_candidates(&pool, "approved", None)
                .await
                .unwrap()
                .is_empty()
        );
        let row = SummaryProcessesRepository::get_summary_data(&pool, meeting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.result.as_deref(), Some("previous"));
        assert_eq!(row.metadata.as_deref(), Some("partial"));
    }

    #[tokio::test]
    async fn full_and_partial_notes_follow_transcript_content_across_metadata_edits() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let meeting = "review-test";
        let snapshot =
            crate::summary::sources::prepare_snapshot(&pool, meeting, String::new(), true)
                .await
                .unwrap();
        let revision = snapshot.revision.unwrap();
        SummaryProcessesRepository::create_or_reset_process(&pool, meeting)
            .await
            .unwrap();
        sqlx::query("UPDATE meetings SET title = 'Renamed', created_at = '2026-10-20T12:00:00Z' WHERE id = ?")
            .bind(meeting).execute(&pool).await.unwrap();
        let result = serde_json::json!({"markdown": "Current notes"});
        assert!(SummaryProcessesRepository::complete_for_revision(
            &pool,
            meeting,
            result.clone(),
            1,
            0.0,
            Some(revision)
        )
        .await
        .unwrap());
        SummaryProcessesRepository::fail_with_partial(&pool, meeting, "Failed", revision, "Äpfel")
            .await
            .unwrap();
        assert_eq!(
            SummaryProcessesRepository::get_summary_status_data(&pool, meeting)
                .await
                .unwrap()
                .unwrap()
                .1
                .as_deref(),
            Some("Äpfel")
        );

        sqlx::query("UPDATE transcripts SET transcript = 'Edited' WHERE id = 'a'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!SummaryProcessesRepository::complete_for_revision(
            &pool,
            meeting,
            result,
            1,
            0.0,
            Some(revision)
        )
        .await
        .unwrap());
        assert!(!SummaryProcessesRepository::fail_with_partial(
            &pool,
            meeting,
            "Failed",
            revision,
            "Stale partial"
        )
        .await
        .unwrap());
        assert!(
            SummaryProcessesRepository::get_summary_status_data(&pool, meeting)
                .await
                .unwrap()
                .unwrap()
                .1
                .is_none()
        );
        let row = SummaryProcessesRepository::get_summary_data(&pool, meeting)
            .await
            .unwrap()
            .unwrap();
        assert!(row.result.unwrap().contains("Current notes"));
        assert!(row.metadata.unwrap().contains("Äpfel"));
    }

    #[tokio::test]
    async fn completion_keeps_one_previous_version_and_restore_swaps_both_ways() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        let old =
            serde_json::json!({"markdown":"Edited notes", "user_edited_at":"2026-01-01T00:00:00Z"});
        SummaryProcessesRepository::create_or_reset_process(&pool, "review-test")
            .await
            .unwrap();
        SummaryProcessesRepository::update_process_completed(
            &pool,
            "review-test",
            old.clone(),
            1,
            0.0,
        )
        .await
        .unwrap();
        SummaryProcessesRepository::update_meeting_summary(&pool, "review-test", &old)
            .await
            .unwrap();
        SummaryProcessesRepository::create_or_reset_process(&pool, "review-test")
            .await
            .unwrap();
        let generated = serde_json::json!({"markdown":"Generated notes", "user_edited_at":"discard this marker"});
        SummaryProcessesRepository::update_process_completed(
            &pool,
            "review-test",
            generated,
            1,
            0.0,
        )
        .await
        .unwrap();
        let current = SummaryProcessesRepository::get_summary_data(&pool, "review-test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(current.previous_result.as_ref().unwrap()).unwrap(),
            old
        );
        assert!(current.previous_result_at.is_some());
        assert!(current.result_backup.is_none());
        assert!(
            serde_json::from_str::<Value>(current.result.as_ref().unwrap())
                .unwrap()
                .get("user_edited_at")
                .is_none()
        );
        for expected in ["Edited notes", "Generated notes"] {
            SummaryProcessesRepository::restore_previous_summary(&pool, "review-test")
                .await
                .unwrap();
            let restored = SummaryProcessesRepository::get_summary_data(&pool, "review-test")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(restored.result.as_ref().unwrap()).unwrap()
                    ["markdown"],
                expected
            );
        }
    }

    #[tokio::test]
    async fn generation_prevents_restore_and_edit_writes() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        SummaryProcessesRepository::create_or_reset_process(&pool, "review-test")
            .await
            .unwrap();
        for status in ["PENDING", "processing", "summarizing", "regenerating"] {
            sqlx::query("UPDATE summary_processes SET status = ?, result = '{\"markdown\":\"Current\"}', previous_result = '{\"markdown\":\"Previous\"}' WHERE meeting_id = 'review-test'")
                .bind(status).execute(&pool).await.unwrap();
            assert!(
                SummaryProcessesRepository::restore_previous_summary(&pool, "review-test")
                    .await
                    .is_err()
            );
            assert!(SummaryProcessesRepository::update_meeting_summary(
                &pool,
                "review-test",
                &serde_json::json!({"markdown":"Edit"})
            )
            .await
            .is_err());
            let result = SummaryProcessesRepository::get_summary_data(&pool, "review-test")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&result.result.unwrap()).unwrap()["markdown"],
                "Current"
            );
        }
    }

    #[tokio::test]
    async fn previous_summary_migration_upgrades_the_previous_schema() {
        let directory = tempfile::tempdir().unwrap();
        for file in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations")).unwrap() {
            let file = file.unwrap();
            if file.file_name() != "20260927000001_summary_previous_result.sql" {
                std::fs::copy(file.path(), directory.path().join(file.file_name())).unwrap();
            }
        }
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate::Migrator::new(directory.path())
            .await
            .unwrap()
            .run(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('upgrade', 'Synthetic meeting', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result) VALUES ('upgrade', 'completed', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{\"markdown\":\"Preserved\"}')").execute(&pool).await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let saved = SummaryProcessesRepository::get_summary_data(&pool, "upgrade")
            .await
            .unwrap()
            .unwrap();
        assert!(saved.previous_result.is_none());
        assert!(saved.result.unwrap().contains("Preserved"));
    }
}

impl SummaryProcessesRepository {
    /// Retrieves the current summary process state for a given meeting ID.
    pub async fn get_summary_data(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<SummaryProcess>, sqlx::Error> {
        sqlx::query_as::<_, SummaryProcess>("SELECT * FROM summary_processes WHERE meeting_id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
    }

    pub async fn update_meeting_summary(
        pool: &SqlitePool,
        meeting_id: &str,
        summary: &Value,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = pool.begin().await?;

        let meeting_exists: bool = sqlx::query("SELECT 1 FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(&mut *transaction)
            .await?
            .is_some();

        if !meeting_exists {
            log_info!(
                "Attempted to save summary for a non-existent meeting_id: {}",
                meeting_id
            );
            transaction.rollback().await?;
            return Ok(false);
        }

        let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM summary_processes WHERE meeting_id = ? AND LOWER(status) IN ('pending', 'processing', 'summarizing', 'regenerating'))")
            .bind(meeting_id).fetch_one(&mut *transaction).await?;
        if busy {
            return Err(sqlx::Error::Protocol(
                "Wait for summary generation to finish before saving edits.".into(),
            ));
        }

        // Keep the generation provenance, structured suggestions, and email when editing the document.
        // Never reuse the English generation cache after an edit.
        let previous: Option<String> =
            sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = ?")
                .bind(meeting_id)
                .fetch_optional(&mut *transaction)
                .await?
                .flatten();
        let mut saved = summary.clone();
        if let Some(previous) = previous.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()) {
            for key in [
                "summary_sources",
                "generation",
                "transcript_content_revision",
                "follow_up_email",
                "structured_meeting_output",
            ] {
                if let Some(value) = previous.get(key) {
                    saved[key] = value.clone();
                }
            }
            if let Some(email) = previous.pointer("/structured_meeting_output/follow_up_email") {
                saved["follow_up_email"] = email.clone();
            }
            if saved.get("generation").is_none() {
                if let Some(source) = previous.pointer("/english_cache/source") {
                    saved["generation"] = serde_json::json!({
                        "model": source.get("model_name"),
                        "template": source.get("template_id"),
                        "transcript_fingerprint": source.get("transcript_fingerprint"),
                    });
                }
            }
        }
        let result_json = serde_json::to_string(&saved);
        if result_json.is_err() {
            error!("Can't convert the json to string for saving to Database");
            transaction.rollback().await?;
            return Ok(false);
        }
        let now = Utc::now();

        sqlx::query("UPDATE summary_processes SET result = ?, updated_at = ? WHERE meeting_id = ?")
            .bind(&result_json.unwrap())
            .bind(now)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;

        log_info!(
            "Successfully updated summary and timestamp for meeting_id: {}",
            meeting_id
        );
        Ok(true)
    }

    pub async fn get_summary_data_for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<SummaryProcess>, sqlx::Error> {
        Self::get_summary_status_data(pool, meeting_id)
            .await
            .map(|row| row.map(|(process, _)| process))
    }

    pub(crate) async fn get_summary_status_data(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<(SummaryProcess, Option<String>)>, sqlx::Error> {
        // Return notes, status, and revision validity from the same snapshot.
        // Legacy partials keep their original revision check until regenerated.
        let row = sqlx::query(r#"
            SELECT p.*, m.summary_content_revision AS current_content_revision, CASE WHEN LOWER(p.status) = 'failed' AND json_valid(p.metadata) THEN CASE
                WHEN json_type(p.metadata, '$.partial_notes') = 'text' AND (
                    json_extract(p.metadata, '$.transcript_content_revision') = m.summary_content_revision
                    OR (json_type(p.metadata, '$.transcript_content_revision') IS NULL AND EXISTS (
                        SELECT 1 FROM knowledge_sources s WHERE s.meeting_id = p.meeting_id AND s.kind = 'meeting'
                          AND s.revision = json_extract(p.metadata, '$.transcript_revision')
                    ))
                ) THEN json_extract(p.metadata, '$.partial_notes') END END AS current_partial_notes
            FROM summary_processes p LEFT JOIN meetings m ON m.id = p.meeting_id
            WHERE p.meeting_id = ? AND (m.id IS NOT NULL OR EXISTS (
                SELECT 1 FROM transcript_chunks t WHERE t.meeting_id = p.meeting_id
            ))
        "#).bind(meeting_id).fetch_optional(pool).await?;
        row.as_ref()
            .map(|row| {
                Ok((
                    SummaryProcess::from_row(row)?,
                    row.try_get("current_partial_notes")?,
                ))
            })
            .transpose()
    }

    pub(crate) async fn can_start_automatic_process(
        pool: &SqlitePool,
        meeting: &str,
        origin: &str,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            SELECT NOT EXISTS (
                SELECT 1 FROM summary_processes WHERE meeting_id = ?
                AND NOT (? = 'recording' AND status = 'skipped'
                    AND (error IS ? OR error IS ?) AND (automatic_origin = 'recording' OR automatic_origin IS NULL)
                    AND result IS NULL AND result_backup IS NULL AND previous_result IS NULL AND metadata IS NULL)
            )
        "#,
        )
        .bind(meeting)
        .bind(origin)
        .bind(RECOVERY_NEEDED)
        .bind(NO_TRANSCRIPT)
        .fetch_one(pool)
        .await
    }

    pub(crate) async fn create_automatic_process(
        pool: &SqlitePool,
        meeting: &str,
        origin: &str,
        fingerprint: &str,
    ) -> Result<bool, sqlx::Error> {
        Ok(sqlx::query(r#"
            INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, start_time, automatic_origin, automatic_request_fingerprint)
            SELECT id, 'pending', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?,
                COALESCE((SELECT profile || ':' FROM retranscription_notes_retry WHERE meeting_id = meetings.id), '') || ?
            FROM meetings WHERE id = ?
            ON CONFLICT(meeting_id) DO UPDATE SET status = excluded.status, error = NULL,
                start_time = excluded.start_time, updated_at = excluded.updated_at, end_time = NULL,
                automatic_origin = excluded.automatic_origin, automatic_request_fingerprint = excluded.automatic_request_fingerprint
            WHERE excluded.automatic_origin = 'recording' AND summary_processes.status = 'skipped'
              AND summary_processes.error IN (?, ?) AND (summary_processes.automatic_origin = 'recording' OR summary_processes.automatic_origin IS NULL)
              AND summary_processes.result IS NULL AND summary_processes.result_backup IS NULL
              AND summary_processes.previous_result IS NULL AND summary_processes.metadata IS NULL
        "#).bind(origin).bind(fingerprint).bind(meeting).bind(RECOVERY_NEEDED).bind(NO_TRANSCRIPT).execute(pool).await?.rows_affected() == 1)
    }

    pub(crate) async fn automatic_resume_candidates(
        pool: &SqlitePool,
        fingerprint: &str,
        profile: Option<&str>,
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        sqlx::query_as("SELECT meeting_id, automatic_request_fingerprint FROM summary_processes
            WHERE automatic_origin IN ('recording', 'import') AND automatic_request_fingerprint IN (?1, ?2 || ':' || ?1)
            AND automatic_resume_attempted = 0 AND status = 'failed' AND error = ?3 ORDER BY start_time DESC, meeting_id LIMIT 3")
            .bind(fingerprint).bind(profile).bind(INTERRUPTED).fetch_all(pool).await
    }

    pub(crate) async fn claim_automatic_resume(
        pool: &SqlitePool,
        meeting: &str,
        fingerprint: &str,
    ) -> Result<bool, sqlx::Error> {
        Ok(sqlx::query("UPDATE summary_processes SET status = 'pending', automatic_resume_attempted = 1, error = NULL, start_time = CURRENT_TIMESTAMP, end_time = NULL, updated_at = CURRENT_TIMESTAMP WHERE meeting_id = ? AND automatic_origin IN ('recording', 'import') AND automatic_request_fingerprint = ? AND automatic_resume_attempted = 0 AND status = 'failed' AND error = ?")
            .bind(meeting).bind(fingerprint).bind(INTERRUPTED).execute(pool).await?.rows_affected() == 1)
    }

    pub(crate) async fn record_automatic_outcome(
        pool: &SqlitePool,
        meeting: &str,
        status: &str,
        reason: &str,
        origin: &str,
    ) -> Result<(), sqlx::Error> {
        // Atomically replace a recoverable transcript skip with
        // the new reason. Any other existing state — manual, completed, another skip
        // reason — is preserved by the WHERE guard on the DO UPDATE.
        sqlx::query(r#"
            INSERT INTO summary_processes (meeting_id, status, error, created_at, updated_at, end_time, automatic_origin)
            SELECT id, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ? FROM meetings WHERE id = ?
            ON CONFLICT(meeting_id) DO UPDATE SET
                status = excluded.status, error = excluded.error,
                updated_at = excluded.updated_at, end_time = excluded.end_time,
                automatic_origin = excluded.automatic_origin
            WHERE excluded.automatic_origin = 'recording' AND summary_processes.status = 'skipped'
                AND summary_processes.error IN (?, ?)
                AND (summary_processes.automatic_origin = 'recording' OR summary_processes.automatic_origin IS NULL)
                AND summary_processes.result IS NULL
                AND summary_processes.result_backup IS NULL
                AND summary_processes.previous_result IS NULL
                AND summary_processes.metadata IS NULL
        "#)
            .bind(status).bind(reason).bind(origin).bind(meeting).bind(RECOVERY_NEEDED).bind(NO_TRANSCRIPT).execute(pool).await?;
        Ok(())
    }

    pub async fn create_or_reset_process(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<(), sqlx::Error> {
        log_info!(
            "Creating or resetting summary process for meeting_id: {}",
            meeting_id
        );
        let now = Utc::now();
        sqlx::query(
            r#"
            INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, start_time, result, error)
            VALUES (?, 'PENDING', ?, ?, ?, NULL, NULL)
            ON CONFLICT(meeting_id) DO UPDATE SET
                status = 'PENDING',
                updated_at = excluded.updated_at,
                start_time = excluded.start_time,
                end_time = NULL,
                chunk_count = 0,
                metadata = NULL,
                automatic_origin = NULL,
                automatic_request_fingerprint = NULL,
                automatic_resume_attempted = 0,
                result_backup = result,
                result_backup_timestamp = excluded.updated_at,
                result = result,
                error = NULL
            "#
        )
        .bind(meeting_id)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
        log_info!(
            "Backed up existing summary before regeneration for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    pub async fn update_process_completed(
        pool: &SqlitePool,
        meeting_id: &str,
        result: Value,
        chunk_count: i64,
        processing_time: f64,
    ) -> Result<(), sqlx::Error> {
        Self::complete_for_revision(pool, meeting_id, result, chunk_count, processing_time, None)
            .await
            .map(|_| ())
    }

    pub(crate) async fn complete_for_revision(
        pool: &SqlitePool,
        meeting_id: &str,
        mut result: Value,
        chunk_count: i64,
        processing_time: f64,
        revision: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        let now = Utc::now();
        if let Some(object) = result.as_object_mut() {
            object.remove("user_edited_at");
            if let Some(revision) = revision {
                object.insert("transcript_content_revision".into(), revision.into());
            }
        }
        let result_str = serde_json::to_string(&result)
            .map_err(|e| sqlx::Error::Protocol(format!("Failed to serialize result: {}", e)))?;

        let updated = sqlx::query(
            r#"
            UPDATE summary_processes
            SET previous_result = CASE WHEN result IS NOT NULL AND TRIM(result) NOT IN ('', 'null', '{}') THEN result ELSE previous_result END,
                previous_result_at = CASE WHEN result IS NOT NULL AND TRIM(result) NOT IN ('', 'null', '{}') THEN CURRENT_TIMESTAMP ELSE previous_result_at END,
                status = 'completed', result = ?, updated_at = ?, end_time = ?, chunk_count = ?, processing_time = ?, error = NULL, metadata = NULL, result_backup = NULL, result_backup_timestamp = NULL
            WHERE meeting_id = ? AND (? IS NULL OR ? = (SELECT summary_content_revision FROM meetings WHERE id = summary_processes.meeting_id))
            "#
        )
        .bind(result_str)
        .bind(now)
        .bind(now)
        .bind(chunk_count)
        .bind(processing_time)
        .bind(meeting_id)
        .bind(revision)
        .bind(revision)
        .execute(pool)
        .await?;
        log_info!(
            "Summary completed and backup cleared for meeting_id: {}",
            meeting_id
        );
        Ok(updated.rows_affected() == 1)
    }

    pub async fn restore_previous_summary(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<(), sqlx::Error> {
        let mut transaction = pool.begin().await?;
        let updated = sqlx::query("UPDATE summary_processes SET result = previous_result, previous_result = result, previous_result_at = updated_at, updated_at = ?, status = 'completed', error = NULL WHERE meeting_id = ? AND previous_result IS NOT NULL AND LOWER(status) NOT IN ('pending', 'processing', 'summarizing', 'regenerating')")
            .bind(Utc::now()).bind(meeting_id).execute(&mut *transaction).await?;
        if updated.rows_affected() == 0 {
            return Err(sqlx::Error::Protocol(
                "No previous summary is available, or summary generation is still running.".into(),
            ));
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn update_process_failed(
        pool: &SqlitePool,
        meeting_id: &str,
        error: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();

        // Restore from backup if it exists, otherwise keep current result
        sqlx::query(
            r#"
            UPDATE summary_processes
            SET
                status = 'failed',
                error = ?,
                updated_at = ?,
                end_time = ?,
                result = COALESCE(result_backup, result),
                result_backup = NULL,
                result_backup_timestamp = NULL
            WHERE meeting_id = ?
            "#,
        )
        .bind(error)
        .bind(now)
        .bind(now)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        log_info!(
            "Summary generation failed and backup restored for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    pub(crate) async fn fail_interrupted_processes(pool: &SqlitePool) -> Result<(), sqlx::Error> {
        let now = Utc::now();
        sqlx::query("UPDATE summary_processes SET status = 'failed', error = ?, updated_at = ?, end_time = ?, result = CASE WHEN automatic_origin IS NULL THEN COALESCE(result_backup, result) ELSE result END, result_backup = CASE WHEN automatic_origin IS NOT NULL THEN result_backup END, result_backup_timestamp = CASE WHEN automatic_origin IS NOT NULL THEN result_backup_timestamp END WHERE LOWER(status) IN ('pending', 'processing', 'summarizing', 'regenerating')")
            .bind(INTERRUPTED).bind(now).bind(now).execute(pool).await?;
        Ok(())
    }

    pub(crate) async fn fail_with_partial(
        pool: &SqlitePool,
        meeting: &str,
        reason: &str,
        revision: i64,
        notes: &str,
    ) -> Result<bool, sqlx::Error> {
        let metadata = serde_json::json!({
            "partial_notes": notes,
            "transcript_content_revision": revision,
        });
        Ok(sqlx::query(r#"
            UPDATE summary_processes SET status = 'failed', error = ?1,
                updated_at = CURRENT_TIMESTAMP, end_time = CURRENT_TIMESTAMP,
                metadata = CASE WHEN automatic_resume_attempted = 1 AND json_valid(metadata) THEN CASE
                    WHEN json_extract(metadata, '$.transcript_content_revision') = ?4
                      AND length(json_extract(metadata, '$.partial_notes')) >= length(json_extract(?2, '$.partial_notes'))
                    THEN metadata ELSE ?2 END ELSE ?2 END,
                result = COALESCE(result_backup, result), result_backup = NULL, result_backup_timestamp = NULL
            WHERE meeting_id = ?3 AND ?4 = (SELECT summary_content_revision FROM meetings WHERE id = summary_processes.meeting_id)
        "#).bind(reason).bind(metadata.to_string()).bind(meeting).bind(revision).execute(pool).await?.rows_affected() == 1)
    }

    pub async fn update_process_cancelled(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<(), sqlx::Error> {
        Self::update_process_cancelled_with_reason(
            pool,
            meeting_id,
            "Generation was cancelled by user",
        )
        .await
    }

    pub async fn update_process_cancelled_with_reason(
        pool: &SqlitePool,
        meeting_id: &str,
        reason: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();

        // Restore from backup if it exists, otherwise keep current result
        sqlx::query(
            r#"
            UPDATE summary_processes
            SET
                status = 'cancelled',
                updated_at = ?,
                end_time = ?,
                error = ?,
                result = COALESCE(result_backup, result),
                result_backup = NULL,
                result_backup_timestamp = NULL
            WHERE meeting_id = ?
            "#,
        )
        .bind(now)
        .bind(now)
        .bind(reason)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        log_info!(
            "Marked summary process as cancelled and restored backup for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }
}
