//! Durable source generations and canonical transcript snapshots.
use super::types::*;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct SourceJob {
    pub source_id: String,
    pub meeting_id: String,
    pub revision: i64,
    pub generation: i64,
}
#[derive(Debug, Clone, FromRow)]
pub struct CanonicalRow {
    pub id: String,
    pub meeting_id: String,
    pub transcript: String,
    pub speaker: Option<String>,
    pub timestamp: String,
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
    pub duration: Option<f64>,
    pub word_timestamps_json: Option<String>,
}
pub fn fingerprint(row: &CanonicalRow, span: &TextSpan) -> Result<String, KnowledgeError> {
    let text = row
        .transcript
        .get(span.start_byte..span.end_byte)
        .ok_or(KnowledgeError::InvalidInput)?;
    if span.transcript_id != row.id {
        return Err(KnowledgeError::InvalidInput);
    }
    let bytes = serde_json::to_vec(&(
        "canonical-transcript-v1",
        &row.id,
        &row.meeting_id,
        span,
        text,
        &row.speaker,
        &row.timestamp,
        row.audio_start_time,
        row.audio_end_time,
        row.duration,
        &row.word_timestamps_json,
    ))
    .map_err(|_| KnowledgeError::InvalidInput)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
pub fn evidence(
    job: &SourceJob,
    row: &CanonicalRow,
    span: &TextSpan,
) -> Result<EvidenceRef, KnowledgeError> {
    let fingerprint = fingerprint(row, span)?;
    let identity = serde_json::to_vec(&(
        "evidence-v1",
        &job.source_id,
        job.revision,
        span,
        &fingerprint,
    ))
    .map_err(|_| KnowledgeError::InvalidInput)?;
    Ok(EvidenceRef {
        source_id: job.source_id.clone(),
        source_revision: job.revision,
        chunk_id: format!("{:x}", Sha256::digest(identity)),
        fingerprint,
        locator: EvidenceLocator::Transcript {
            meeting_id: row.meeting_id.clone(),
            transcript_ids: vec![row.id.clone()],
            spans: vec![span.clone()],
            start_seconds: row.audio_start_time,
        },
    })
}
pub async fn next_job(pool: &SqlitePool) -> Result<Option<SourceJob>, KnowledgeError> {
    Ok(sqlx::query_as("SELECT j.source_id,s.meeting_id,j.revision,j.generation FROM knowledge_index_jobs j JOIN knowledge_sources s ON s.id=j.source_id AND s.revision=j.revision AND s.generation=j.generation WHERE j.attempts<3 AND j.paused=0 ORDER BY j.attempts,j.source_id LIMIT 1").fetch_optional(pool).await?)
}
pub async fn current(pool: &SqlitePool, job: &SourceJob) -> Result<bool, KnowledgeError> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM knowledge_sources s JOIN knowledge_index_jobs j ON j.source_id=s.id WHERE s.id=? AND s.revision=? AND s.generation=? AND j.generation=s.generation AND j.paused=0").bind(&job.source_id).bind(job.revision).bind(job.generation).fetch_one(pool).await? == 1)
}
pub async fn rows_page(
    pool: &SqlitePool,
    job: &SourceJob,
    after: Option<(&str, &str)>,
) -> Result<Vec<CanonicalRow>, KnowledgeError> {
    if !current(pool, job).await? {
        return Err(KnowledgeError::Superseded);
    }
    let (timestamp, id) = after.unwrap_or(("", ""));
    Ok(sqlx::query_as("SELECT id,meeting_id,transcript,speaker,timestamp,audio_start_time,audio_end_time,duration,word_timestamps_json FROM transcripts WHERE meeting_id=? AND (timestamp>? OR (timestamp=? AND id>?)) ORDER BY timestamp,id LIMIT 32").bind(&job.meeting_id).bind(timestamp).bind(timestamp).bind(id).fetch_all(pool).await?)
}
pub async fn stage(
    pool: &SqlitePool,
    job: &SourceJob,
    row: &CanonicalRow,
    span: &TextSpan,
    ordinal: i64,
    vector: &[f32],
    space: &str,
) -> Result<(), KnowledgeError> {
    if vector.len() != 384 || vector.iter().any(|v| !v.is_finite()) {
        return Err(KnowledgeError::InvalidInput);
    }
    let reference = evidence(job, row, span)?;
    let mut tx = pool.begin().await?;
    // This conditional write obtains SQLite's writer lock before checking the
    // generation; later commands cannot dirty or replace it during publication.
    let guarded=sqlx::query("UPDATE knowledge_index_jobs SET failure=failure WHERE source_id=? AND revision=? AND generation=? AND paused=0 AND EXISTS(SELECT 1 FROM knowledge_sources s WHERE s.id=source_id AND s.revision=? AND s.generation=?)").bind(&job.source_id).bind(job.revision).bind(job.generation).bind(job.revision).bind(job.generation).execute(&mut *tx).await?;
    if guarded.rows_affected() != 1 {
        return Err(KnowledgeError::Superseded);
    }
    sqlx::query("INSERT INTO knowledge_chunks(id,source_id,revision,generation,ordinal,transcript_id,start_byte,end_byte,fingerprint,text) VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET generation=excluded.generation,ordinal=excluded.ordinal")
        .bind(&reference.chunk_id).bind(&job.source_id).bind(job.revision).bind(job.generation).bind(ordinal).bind(&row.id).bind(span.start_byte as i64).bind(span.end_byte as i64).bind(&reference.fingerprint).bind(&row.transcript[span.start_byte..span.end_byte]).execute(&mut *tx).await?;
    let bytes: Vec<u8> = vector.iter().flat_map(|v| v.to_le_bytes()).collect();
    sqlx::query("INSERT INTO knowledge_vectors(chunk_id,space,dimensions,vector) VALUES(?,?,384,?) ON CONFLICT(chunk_id) DO UPDATE SET space=excluded.space,vector=excluded.vector")
        .bind(&reference.chunk_id).bind(space).bind(bytes).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn publish(
    pool: &SqlitePool,
    job: &SourceJob,
    space: &str,
) -> Result<(), KnowledgeError> {
    let mut tx = pool.begin().await?;
    let changed=sqlx::query("UPDATE knowledge_sources SET semantic_revision=revision,semantic_space=? WHERE id=? AND revision=? AND generation=? AND EXISTS(SELECT 1 FROM knowledge_index_jobs j WHERE j.source_id=knowledge_sources.id AND j.generation=knowledge_sources.generation AND j.paused=0)")
        .bind(space).bind(&job.source_id).bind(job.revision).bind(job.generation).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(KnowledgeError::Superseded);
    }
    sqlx::query("DELETE FROM knowledge_chunks WHERE source_id=? AND generation!=?")
        .bind(&job.source_id)
        .bind(job.generation)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "DELETE FROM knowledge_index_jobs WHERE source_id=? AND revision=? AND generation=?",
    )
    .bind(&job.source_id)
    .bind(job.revision)
    .bind(job.generation)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
pub async fn record_failure(
    pool: &SqlitePool,
    job: &SourceJob,
    error: &KnowledgeError,
) -> Result<(), KnowledgeError> {
    let category = match error {
        KnowledgeError::Busy
        | KnowledgeError::Disabled
        | KnowledgeError::Cancelled
        | KnowledgeError::Superseded => return Ok(()),
        KnowledgeError::ModelUnavailable => "model_unavailable",
        KnowledgeError::Storage => "storage",
        _ => "indexing",
    };
    sqlx::query("UPDATE knowledge_index_jobs SET attempts=MIN(attempts+1,3),failure=? WHERE source_id=? AND revision=? AND generation=?").bind(category).bind(&job.source_id).bind(job.revision).bind(job.generation).execute(pool).await?;
    Ok(())
}

pub async fn requeue(pool: &SqlitePool, ids: &[String]) -> Result<(), KnowledgeError> {
    let ids = serde_json::to_string(ids).map_err(|_| KnowledgeError::InvalidInput)?;
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE knowledge_sources SET generation=generation+1,semantic_revision=NULL,semantic_space=NULL WHERE meeting_id IN(SELECT value FROM json_each(?))").bind(&ids).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO knowledge_index_jobs(source_id,revision,generation) SELECT id,revision,generation FROM knowledge_sources WHERE meeting_id IN(SELECT value FROM json_each(?)) ON CONFLICT(source_id) DO UPDATE SET revision=excluded.revision,generation=excluded.generation,attempts=0,failure=NULL,paused=0").bind(ids).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn invalidate_other_spaces(pool: &SqlitePool, space: &str) -> Result<(), KnowledgeError> {
    let ids:Vec<String>=sqlx::query_scalar("SELECT meeting_id FROM knowledge_sources WHERE semantic_space IS NOT NULL AND semantic_space!=?").bind(space).fetch_all(pool).await?;
    if !ids.is_empty() {
        requeue(pool, &ids).await?;
    }
    Ok(())
}
pub async fn status(
    pool: &SqlitePool,
    enabled: bool,
    space: &str,
) -> Result<IndexStatus, KnowledgeError> {
    let ready:i64=sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_sources WHERE semantic_revision=revision AND semantic_space=?").bind(space).fetch_one(pool).await?;
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_index_jobs WHERE attempts<3 AND paused=0",
    )
    .fetch_one(pool)
    .await?;
    let failed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_index_jobs WHERE attempts>=3")
            .fetch_one(pool)
            .await?;
    let paused: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_index_jobs WHERE paused=1")
            .fetch_one(pool)
            .await?;
    Ok(IndexStatus {
        keyword_ready: true,
        semantic_enabled: enabled,
        semantic_ready: ready as usize,
        pending: pending as usize,
        failed: failed as usize,
        reason: if !enabled {
            Some("disabled".into())
        } else if failed > 0 {
            Some("indexing_failed".into())
        } else if paused > 0 {
            Some("paused".into())
        } else if pending > 0 {
            Some("indexing".into())
        } else {
            None
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn database() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }
    async fn meeting(pool: &SqlitePool, id: &str) {
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES (?, 'Public fixture', '2026-10-07', '2026-10-07')")
            .bind(id).execute(pool).await.unwrap();
    }
    async fn transcript(pool: &SqlitePool) {
        sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES ('row','one','Release ATLAS-42 on Friday','00:01')")
            .execute(pool).await.unwrap();
    }
    async fn count(pool: &SqlitePool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
    }
    #[tokio::test]
    async fn clean_database_seeds_empty_meetings() {
        let pool = database().await;
        meeting(&pool, "one").await;
        let available = count(
            &pool,
            "SELECT COUNT(*) FROM sqlite_master WHERE name='knowledge_sources'",
        )
        .await;
        assert_eq!(available, 1, "canonical source generations must exist");
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM knowledge_sources").await,
            1
        );
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM knowledge_index_jobs").await,
            1
        );
    }
    #[tokio::test]
    async fn edited_source_cannot_publish_old_generation() {
        let pool = database().await;
        meeting(&pool, "one").await;
        transcript(&pool).await;
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='knowledge_sources'"
            )
            .await,
            1,
            "mutation generations are absent"
        );
        let old = count(
            &pool,
            "SELECT revision FROM knowledge_sources WHERE meeting_id='one'",
        )
        .await;
        sqlx::query("UPDATE knowledge_sources SET semantic_revision=revision, semantic_space='fixture' WHERE meeting_id='one'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE knowledge_index_jobs SET attempts=3")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE transcripts SET speaker='Speaker B', audio_start_time=3 WHERE id='row'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            count(
                &pool,
                "SELECT revision FROM knowledge_sources WHERE meeting_id='one'"
            )
            .await,
            old + 1
        );
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_sources WHERE semantic_revision IS NOT NULL"
            )
            .await,
            0
        );
        assert_eq!(
            count(&pool, "SELECT attempts FROM knowledge_index_jobs").await,
            0
        );
        let stale = sqlx::query("UPDATE knowledge_sources SET semantic_revision=? WHERE meeting_id='one' AND revision=?").bind(old).bind(old).execute(&pool).await.unwrap();
        assert_eq!(stale.rows_affected(), 0);
    }
    #[tokio::test]
    async fn transcript_move_invalidates_both_owners() {
        let pool = database().await;
        meeting(&pool, "one").await;
        meeting(&pool, "two").await;
        transcript(&pool).await;
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='knowledge_sources'"
            )
            .await,
            1,
            "move generations are absent"
        );
        let old = count(&pool, "SELECT SUM(revision) FROM knowledge_sources").await;
        sqlx::query("UPDATE transcripts SET meeting_id='two' WHERE id='row'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            count(&pool, "SELECT SUM(revision) FROM knowledge_sources").await,
            old + 2
        );
    }
    #[tokio::test]
    async fn delete_cascades_index() {
        let pool = database().await;
        meeting(&pool, "one").await;
        transcript(&pool).await;
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='knowledge_sources'"
            )
            .await,
            1,
            "index deletion is absent"
        );
        sqlx::query("DELETE FROM meetings WHERE id='one'")
            .execute(&pool)
            .await
            .unwrap();
        for table in [
            "knowledge_sources",
            "knowledge_chunks",
            "knowledge_vectors",
            "knowledge_index_jobs",
            "knowledge_fts",
        ] {
            assert_eq!(
                count(&pool, &format!("SELECT COUNT(*) FROM {table}")).await,
                0,
                "orphan in {table}"
            );
        }
    }
    #[tokio::test]
    async fn restore_requeues_index() {
        let pool = database().await;
        meeting(&pool, "one").await;
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='knowledge_sources'"
            )
            .await,
            1,
            "restore indexing is absent"
        );
        let id: String = sqlx::query_scalar("SELECT id FROM knowledge_sources")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        meeting(&pool, "one").await;
        transcript(&pool).await;
        let restored: String = sqlx::query_scalar("SELECT id FROM knowledge_sources")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(restored, id);
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_index_jobs WHERE attempts=0"
            )
            .await,
            1
        );
    }

    #[tokio::test]
    async fn old_worker_cannot_publish_or_delete_newer_job() {
        let pool = database().await;
        meeting(&pool, "one").await;
        transcript(&pool).await;
        let old = next_job(&pool).await.unwrap().unwrap();
        let row = rows_page(&pool, &old, None).await.unwrap().remove(0);
        let span = TextSpan {
            transcript_id: row.id.clone(),
            start_byte: 0,
            end_byte: row.transcript.len(),
        };
        let vector = super::super::embedding::normalize(vec![1.; 384]).unwrap();
        stage(&pool, &old, &row, &span, 0, &vector, "old-space")
            .await
            .unwrap();
        sqlx::query("UPDATE transcripts SET transcript='Changed public fixture' WHERE id='row'")
            .execute(&pool)
            .await
            .unwrap();
        let new = next_job(&pool).await.unwrap().unwrap();
        assert_eq!(
            publish(&pool, &old, "old-space").await,
            Err(KnowledgeError::Superseded)
        );
        assert_eq!(
            stage(&pool, &old, &row, &span, 0, &vector, "old-space").await,
            Err(KnowledgeError::Superseded)
        );
        record_failure(&pool, &old, &KnowledgeError::ProviderFailure)
            .await
            .unwrap();
        assert!(current(&pool, &new).await.unwrap());
        assert_eq!(
            count(&pool, "SELECT attempts FROM knowledge_index_jobs").await,
            0
        );
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_sources WHERE semantic_revision IS NOT NULL"
            )
            .await,
            0
        );
    }
    #[tokio::test]
    async fn pauses_do_not_consume_three_failure_attempts() {
        let pool = database().await;
        meeting(&pool, "one").await;
        let job = next_job(&pool).await.unwrap().unwrap();
        for error in [
            KnowledgeError::Busy,
            KnowledgeError::Disabled,
            KnowledgeError::Cancelled,
            KnowledgeError::Superseded,
        ] {
            record_failure(&pool, &job, &error).await.unwrap();
        }
        assert_eq!(
            count(&pool, "SELECT attempts FROM knowledge_index_jobs").await,
            0
        );
        for _ in 0..4 {
            record_failure(&pool, &job, &KnowledgeError::ProviderFailure)
                .await
                .unwrap();
        }
        assert_eq!(
            count(&pool, "SELECT attempts FROM knowledge_index_jobs").await,
            3
        );
        assert!(next_job(&pool).await.unwrap().is_none());
        transcript(&pool).await;
        assert!(next_job(&pool).await.unwrap().is_some());
    }
    #[tokio::test]
    async fn upgrade_seeds_existing_library_and_fts() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let migrations = sqlx::migrate!("./migrations");
        for migration in migrations.iter().filter(|m| m.version < 20261007000000) {
            sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
        }
        meeting(&pool, "one").await;
        meeting(&pool, "empty").await;
        transcript(&pool).await;
        let migration = migrations
            .iter()
            .find(|m| m.version == 20261007000000)
            .unwrap();
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM knowledge_sources").await,
            2
        );
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM knowledge_index_jobs").await,
            2
        );
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_fts WHERE knowledge_fts MATCH '\"ATLAS-42\"'"
            )
            .await,
            1
        );
        sqlx::query("UPDATE transcripts SET transcript='Corrected only',original_transcript='ATLAS-42' WHERE id='row'").execute(&pool).await.unwrap();
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_fts WHERE knowledge_fts MATCH '\"ATLAS-42\"'"
            )
            .await,
            0
        );
        sqlx::query("DELETE FROM transcripts")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM knowledge_index_jobs").await,
            0
        );
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM knowledge_fts").await, 0);
    }
}
