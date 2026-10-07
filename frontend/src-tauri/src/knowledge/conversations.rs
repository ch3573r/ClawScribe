//! Durable conversation ownership, request identity and source dependencies.

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
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('fixture','Public fixture','2026-09-01','2026-09-01')")
            .execute(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn title_and_date_edits_invalidate_prompt_metadata_snapshot() {
        let pool = database().await;
        let before: i64 =
            sqlx::query_scalar("SELECT revision FROM knowledge_sources WHERE meeting_id='fixture'")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("UPDATE meetings SET title='Revised public title' WHERE id='fixture'")
            .execute(&pool)
            .await
            .unwrap();
        let renamed: i64 =
            sqlx::query_scalar("SELECT revision FROM knowledge_sources WHERE meeting_id='fixture'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            renamed,
            before + 1,
            "A queued prompt must detect changed title metadata"
        );
        sqlx::query("UPDATE meetings SET created_at='2026-09-03' WHERE id='fixture'")
            .execute(&pool)
            .await
            .unwrap();
        let redated: i64 =
            sqlx::query_scalar("SELECT revision FROM knowledge_sources WHERE meeting_id='fixture'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            redated,
            renamed + 1,
            "A changed meeting date cannot preserve a dated prompt snapshot"
        );
    }

    #[tokio::test]
    async fn durable_library_history_has_authoritative_tables_without_cache_foreign_keys() {
        let pool = database().await;
        for table in [
            "knowledge_owners",
            "knowledge_requests",
            "knowledge_messages",
            "knowledge_request_evidence",
            "knowledge_request_sources",
        ] {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(exists, "Missing authoritative conversation table: {table}");
            use sqlx::Row;
            let references = sqlx::query(&format!("PRAGMA foreign_key_list({table})"))
                .fetch_all(&pool)
                .await
                .unwrap();
            assert!(
                references.iter().all(|row| !matches!(
                    row.get::<String, _>("table").as_str(),
                    "knowledge_chunks" | "knowledge_vectors" | "knowledge_sources"
                )),
                "History must survive cache/source association rebuilds"
            );
        }
    }

    async fn pending_request(pool: &SqlitePool) {
        sqlx::query("INSERT INTO knowledge_owners(id,kind) VALUES ('library:fixture','library')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO knowledge_requests(id,owner_id,input_fingerprint,question,scope_json,frozen_ids_json,status,provider,model) VALUES ('00000000-0000-4000-8000-000000000001','library:fixture','input','Private dependent question','{}','[\"fixture\"]','running','builtin-ai','qwen3.5:4b')").execute(pool).await.unwrap();
        sqlx::query("INSERT INTO knowledge_messages(id,request_id,role,content) VALUES ('user','00000000-0000-4000-8000-000000000001','user','Private dependent question')").execute(pool).await.unwrap();
        sqlx::query("INSERT INTO knowledge_request_sources(request_id,source_id,revision) VALUES ('00000000-0000-4000-8000-000000000001','meeting:fixture',1)").execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn deleted_source_redacts_dependent_request_before_associations_disappear() {
        let pool = database().await;
        pending_request(&pool).await;
        sqlx::query("DELETE FROM meetings WHERE id='fixture'")
            .execute(&pool)
            .await
            .unwrap();
        let (status, question): (String, String) =
            sqlx::query_as("SELECT status,question FROM knowledge_requests")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "invalidated");
        assert!(question.is_empty());
        let content: String = sqlx::query_scalar("SELECT content FROM knowledge_messages")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(content.is_empty());
        let owners: i64 = sqlx::query_scalar("SELECT count(*) FROM knowledge_owners")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            owners, 1,
            "Independent durable library owner survives deletion"
        );
    }

    #[tokio::test]
    async fn cancelled_request_cannot_commit() {
        let pool = database().await;
        pending_request(&pool).await;
        sqlx::query("UPDATE knowledge_requests SET status='cancelled' WHERE id='00000000-0000-4000-8000-000000000001'").execute(&pool).await.unwrap();
        let insert=sqlx::query("INSERT INTO knowledge_messages(id,request_id,role,content) VALUES ('late','00000000-0000-4000-8000-000000000001','assistant','Late result')").execute(&pool).await;
        assert!(
            insert.is_err(),
            "Cancelled requests must reject late assistant inserts"
        );
    }
}
