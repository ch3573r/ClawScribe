//! Durable source generations and canonical transcript snapshots.

#[cfg(test)]
mod tests {
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
}
