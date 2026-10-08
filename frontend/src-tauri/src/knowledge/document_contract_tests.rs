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
