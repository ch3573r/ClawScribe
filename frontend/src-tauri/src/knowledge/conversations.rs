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
}
