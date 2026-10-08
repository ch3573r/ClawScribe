//! Canonical reference documents; meeting attachments are separate relations.
use super::{DocumentAttachment, DocumentBlock, DocumentError, DocumentFormat, ExtractedDocument};
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

pub struct Publication {
    pub attachment: DocumentAttachment,
    pub original_kept: bool,
}
pub async fn publish(
    pool: &SqlitePool,
    meeting: &str,
    id: &str,
    name: &str,
    size: u64,
    hash: &str,
    extracted: &ExtractedDocument,
    cancel: &CancellationToken,
) -> Result<Publication, DocumentError> {
    let _ = (pool, meeting, id, name, size, hash, extracted, cancel);
    Err(DocumentError::Storage)
}
pub async fn list(
    pool: &SqlitePool,
    meeting: &str,
) -> Result<Vec<DocumentAttachment>, DocumentError> {
    let _ = (pool, meeting);
    Err(DocumentError::Storage)
}
pub async fn blocks(
    pool: &SqlitePool,
    meeting: &str,
    id: &str,
) -> Result<Vec<DocumentBlock>, DocumentError> {
    let _ = (pool, meeting, id);
    Err(DocumentError::Storage)
}
pub async fn detach(pool: &SqlitePool, meeting: &str, id: &str) -> Result<(), DocumentError> {
    let _ = (pool, meeting, id);
    Err(DocumentError::Storage)
}
pub async fn delete(pool: &SqlitePool, meeting: &str, id: &str) -> Result<String, DocumentError> {
    let _ = (pool, meeting, id);
    Err(DocumentError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        for id in ["aster", "birch"] {
            sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES (?,?,'2026-09-01','2026-09-01')")
                .bind(id).bind(id).execute(&pool).await.unwrap();
        }
        pool
    }
    fn extracted() -> ExtractedDocument {
        super::super::extract::extract(
            DocumentFormat::Docx,
            &super::super::fixtures::docx(&super::super::fixtures::table_document(), &[]),
        )
        .unwrap()
    }
    async fn attach(pool: &SqlitePool, meeting: &str, id: &str) -> Publication {
        publish(
            pool,
            meeting,
            id,
            "Public reference.docx",
            12345,
            &"a".repeat(64),
            &extracted(),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn shared_document_survives_detach() {
        let pool = pool().await;
        let first = attach(&pool, "aster", "00000000-0000-4000-8000-000000000001").await;
        let second = attach(&pool, "birch", "00000000-0000-4000-8000-000000000002").await;
        assert_eq!(first.attachment.id, second.attachment.id);
        assert!(first.original_kept);
        assert!(!second.original_kept);
        detach(&pool, "aster", &first.attachment.id).await.unwrap();
        assert!(list(&pool, "aster").await.unwrap().is_empty());
        assert_eq!(
            blocks(&pool, "birch", &first.attachment.id)
                .await
                .unwrap()
                .len(),
            1537
        );
        assert_eq!(
            blocks(&pool, "aster", &first.attachment.id).await,
            Err(DocumentError::NotFound)
        );
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty());
    }
    #[tokio::test]
    async fn cancelled_import_not_published() {
        let pool = pool().await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = publish(
            &pool,
            "aster",
            "00000000-0000-4000-8000-000000000001",
            "Public reference.docx",
            12345,
            &"a".repeat(64),
            &extracted(),
            &cancel,
        )
        .await;
        assert!(matches!(result, Err(DocumentError::Cancelled)));
        for table in [
            "knowledge_documents",
            "knowledge_document_blocks",
            "knowledge_document_attachments",
        ] {
            let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(rows, 0);
        }
    }
    #[tokio::test]
    async fn explicit_delete_removes_all_relations_and_fts() {
        let pool = pool().await;
        let attached = attach(&pool, "aster", "00000000-0000-4000-8000-000000000001").await;
        attach(&pool, "birch", "00000000-0000-4000-8000-000000000002").await;
        delete(&pool, "aster", &attached.attachment.id)
            .await
            .unwrap();
        for table in [
            "knowledge_documents",
            "knowledge_document_blocks",
            "knowledge_document_attachments",
            "knowledge_document_fts",
        ] {
            let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(rows, 0);
        }
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty());
    }
}
