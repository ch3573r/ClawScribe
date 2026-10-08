//! Public synthetic attachment-storage contracts, including upgraded databases.
use sqlx::sqlite::SqlitePoolOptions;

#[tokio::test]
async fn document_schema_has_canonical_blocks_and_separate_attachment_owners() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name IN \
         ('knowledge_documents','knowledge_document_blocks','knowledge_document_attachments') \
         ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        tables,
        vec![
            "knowledge_document_attachments",
            "knowledge_document_blocks",
            "knowledge_documents"
        ],
        "Reference documents require canonical blocks and independent meeting relations"
    );
    let foreign_keys: Vec<(String, String)> = sqlx::query_as(
        "SELECT \"table\",on_delete FROM pragma_foreign_key_list('knowledge_document_attachments') \
         ORDER BY \"table\"",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        foreign_keys,
        vec![
            ("knowledge_documents".into(), "CASCADE".into()),
            ("meetings".into(), "CASCADE".into())
        ]
    );
}

#[tokio::test]
async fn document_schema_upgrade_preserves_populated_meetings_conversations_and_citations() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let full = sqlx::migrate!("./migrations");
    let shipped = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            full.iter()
                .filter(|m| m.version < 20261008000001)
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    shipped.run(&pool).await.unwrap();
    for id in ["aster", "birch", "cedar"] {
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES (?,?,'2026-09-01','2026-09-01')")
            .bind(id).bind(id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES (?,?,?,'2026-09-01')")
            .bind(format!("row-{id}")).bind(id).bind("Public synthetic meeting decision.").execute(&pool).await.unwrap();
        let owner = format!("meeting:{id}");
        let request = format!("request:{id}");
        sqlx::query("INSERT INTO knowledge_owners(id,kind,meeting_id) VALUES (?,'meeting',?)")
            .bind(&owner)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO knowledge_requests(id,owner_id,input_fingerprint,question,scope_json,frozen_ids_json,status,provider,model) VALUES (?,?,'synthetic','What was decided?','{}','[]','completed','synthetic','synthetic')")
            .bind(&request).bind(&owner).execute(&pool).await.unwrap();
        for role in ["user", "assistant"] {
            sqlx::query("INSERT INTO knowledge_messages(id,request_id,role,content) VALUES (?,?,?,'Public synthetic history [K700].')")
                .bind(format!("{request}:{role}")).bind(&request).bind(role).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO knowledge_request_evidence(request_id,ordinal,reference_json,display_json) VALUES (?,700,'{}','{}')")
            .bind(&request).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO knowledge_request_sources(request_id,source_id,revision) VALUES (?,?,2)",
        )
        .bind(&request)
        .bind(&owner)
        .execute(&pool)
        .await
        .unwrap();
    }
    let tables:Vec<String>=sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND (name LIKE 'knowledge_%' OR name IN ('meetings','transcripts')) ORDER BY name")
        .fetch_all(&pool).await.unwrap();
    let mut counts = Vec::new();
    for table in tables {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        counts.push((table, count));
    }
    full.run(&pool).await.unwrap();
    for (table, before) in counts {
        let after: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(before, after, "Canonical rows changed in {table}");
    }
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .unwrap()
        .is_empty());
    let citations: Vec<i64> =
        sqlx::query_scalar("SELECT ordinal FROM knowledge_request_evidence ORDER BY request_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(citations, vec![700, 700, 700]);
}
