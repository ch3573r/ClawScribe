use crate::database::models::SummaryProcess;
use chrono::Utc;
use serde_json::Value;
use sqlx::SqlitePool;
use tracing::{error, info as log_info};

pub struct SummaryProcessesRepository;

#[cfg(test)]
mod tests {
    use super::*;

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

        // User edits invalidate the English generation cache but retain source identities.
        let previous: Option<String> =
            sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = ?")
                .bind(meeting_id)
                .fetch_optional(&mut *transaction)
                .await?
                .flatten();
        let mut saved = summary.clone();
        if let Some(sources) = previous
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .and_then(|value| value.get("summary_sources").cloned())
        {
            saved["summary_sources"] = sources;
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
        sqlx::query_as::<_, SummaryProcess>(
            "SELECT p.* FROM summary_processes p JOIN transcript_chunks t ON p.meeting_id = t.meeting_id WHERE p.meeting_id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(pool)
        .await
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
        mut result: Value, // Keep this as Value to handle both old and new formats if needed
        chunk_count: i64,
        processing_time: f64,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();
        if let Some(object) = result.as_object_mut() {
            object.remove("user_edited_at");
        }
        let result_str = serde_json::to_string(&result)
            .map_err(|e| sqlx::Error::Protocol(format!("Failed to serialize result: {}", e)))?;

        sqlx::query(
            r#"
            UPDATE summary_processes
            SET previous_result = CASE WHEN result IS NOT NULL AND TRIM(result) NOT IN ('', 'null', '{}') THEN result ELSE previous_result END,
                previous_result_at = CASE WHEN result IS NOT NULL AND TRIM(result) NOT IN ('', 'null', '{}') THEN CURRENT_TIMESTAMP ELSE previous_result_at END,
                status = 'completed', result = ?, updated_at = ?, end_time = ?, chunk_count = ?, processing_time = ?, error = NULL, result_backup = NULL, result_backup_timestamp = NULL
            WHERE meeting_id = ?
            "#
        )
        .bind(result_str)
        .bind(now)
        .bind(now)
        .bind(chunk_count)
        .bind(processing_time)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        log_info!(
            "Summary completed and backup cleared for meeting_id: {}",
            meeting_id
        );
        Ok(())
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
        sqlx::query("UPDATE summary_processes SET status = 'failed', error = 'Summary generation was interrupted when ClawScribe closed', updated_at = ?, end_time = ?, result = COALESCE(result_backup, result), result_backup = NULL, result_backup_timestamp = NULL WHERE LOWER(status) IN ('pending', 'processing', 'summarizing', 'regenerating')")
            .bind(now).bind(now).execute(pool).await?;
        Ok(())
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
