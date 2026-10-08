//! Canonical reference documents; meeting attachments are separate relations.
use super::{DocumentAttachment, DocumentBlock, DocumentError, DocumentFormat, ExtractedDocument};
use sqlx::{Row, SqlitePool};
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
    if cancel.is_cancelled() {
        return Err(DocumentError::Cancelled);
    }
    if uuid::Uuid::parse_str(id).is_err()
        || name.is_empty()
        || name.len() > 1024
        || name.chars().any(char::is_control)
        || size > super::INPUT_BYTES as u64
        || hash.len() != 64
        || !hash.bytes().all(|c| c.is_ascii_hexdigit())
        || extracted.blocks.is_empty()
        || extracted.blocks.iter().map(|b| b.text.len()).sum::<usize>() > super::TEXT_BYTES
    {
        return Err(DocumentError::Malformed);
    }
    for block in &extracted.blocks {
        if block.paragraph == 0
            || block.text.trim().is_empty()
            || matches!(block.page, Some(0))
            || block.page.is_some_and(|p| p as usize > super::PAGE_LIMIT)
            || (extracted.format == DocumentFormat::Pdf) != block.page.is_some()
        {
            return Err(DocumentError::Malformed);
        }
    }
    let mut tx = pool.begin().await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM meetings WHERE id=?)")
        .bind(meeting)
        .fetch_one(&mut *tx)
        .await?;
    if !exists {
        return Err(DocumentError::NotFound);
    }
    let duplicate: Option<String> =
        sqlx::query_scalar("SELECT id FROM knowledge_documents WHERE sha256=? AND format=?")
            .bind(hash)
            .bind(extracted.format.extension())
            .fetch_optional(&mut *tx)
            .await?;
    let original_kept = duplicate.is_none();
    let document_id = duplicate.as_deref().unwrap_or(id);
    if original_kept {
        let source = format!("document:{id}");
        sqlx::query("INSERT INTO knowledge_sources(id,kind) VALUES (?,'document')")
            .bind(&source)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO knowledge_documents(id,source_id,display_name,format,file_size,sha256,storage_name) VALUES (?,?,?,?,?,?,?)")
            .bind(id).bind(&source).bind(name).bind(extracted.format.extension()).bind(size as i64)
            .bind(hash).bind(format!("{id}.{}",extracted.format.extension())).execute(&mut *tx).await?;
        for (index, block) in extracted.blocks.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(DocumentError::Cancelled);
            }
            sqlx::query("INSERT INTO knowledge_document_blocks(id,document_id,ordinal,page,paragraph,text) VALUES (?,?,?,?,?,?)")
                .bind(format!("{id}:{}",index+1)).bind(id).bind(index as i64+1).bind(block.page)
                .bind(block.paragraph).bind(&block.text).execute(&mut *tx).await?;
        }
    }
    sqlx::query("INSERT INTO knowledge_document_attachments(meeting_id,document_id) VALUES (?,?) ON CONFLICT DO NOTHING")
        .bind(meeting).bind(document_id).execute(&mut *tx).await?;
    let attachment = read_attachment(&mut *tx, meeting, document_id).await?;
    if cancel.is_cancelled() {
        return Err(DocumentError::Cancelled);
    }
    tx.commit().await?;
    Ok(Publication {
        attachment,
        original_kept,
    })
}
pub async fn list(
    pool: &SqlitePool,
    meeting: &str,
) -> Result<Vec<DocumentAttachment>, DocumentError> {
    let rows = sqlx::query(&format!(
        "{ATTACHMENT_SELECT} WHERE a.meeting_id=? ORDER BY a.created_at,d.id"
    ))
    .bind(meeting)
    .fetch_all(pool)
    .await?;
    rows.iter().map(attachment).collect()
}
pub async fn blocks(
    pool: &SqlitePool,
    meeting: &str,
    id: &str,
) -> Result<Vec<DocumentBlock>, DocumentError> {
    let mut tx = pool.begin().await?;
    read_attachment(&mut *tx, meeting, id).await?;
    let rows = sqlx::query("SELECT page,paragraph,text FROM knowledge_document_blocks WHERE document_id=? ORDER BY ordinal")
        .bind(id).fetch_all(&mut *tx).await?;
    Ok(rows
        .iter()
        .map(|row| DocumentBlock {
            page: row.get("page"),
            paragraph: row.get("paragraph"),
            text: row.get("text"),
        })
        .collect())
}
pub async fn detach(pool: &SqlitePool, meeting: &str, id: &str) -> Result<(), DocumentError> {
    sqlx::query("DELETE FROM knowledge_document_attachments WHERE meeting_id=? AND document_id=?")
        .bind(meeting)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
pub async fn delete(pool: &SqlitePool, meeting: &str, id: &str) -> Result<String, DocumentError> {
    let mut tx = pool.begin().await?;
    read_attachment(&mut *tx, meeting, id).await?;
    let storage: String =
        sqlx::query_scalar("SELECT storage_name FROM knowledge_documents WHERE id=?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    // Existing source-deletion triggers invalidate/redact every dependent turn.
    sqlx::query("DELETE FROM knowledge_sources WHERE id=? AND kind='document'")
        .bind(format!("document:{id}"))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(storage)
}

const ATTACHMENT_SELECT: &str = "SELECT d.id,d.display_name,d.format,d.file_size,d.sha256,
    CASE WHEN s.semantic_revision=s.revision AND s.semantic_space IS NOT NULL THEN 'ready'
         WHEN j.failure IS NOT NULL THEN 'failed' WHEN j.paused=1 THEN 'paused'
         WHEN (SELECT enabled FROM knowledge_settings WHERE singleton=1)=0 THEN 'disabled'
         ELSE 'pending' END AS indexing_status
    FROM knowledge_document_attachments a JOIN knowledge_documents d ON d.id=a.document_id
    JOIN knowledge_sources s ON s.id=d.source_id LEFT JOIN knowledge_index_jobs j ON j.source_id=s.id";
fn attachment(row: &sqlx::sqlite::SqliteRow) -> Result<DocumentAttachment, DocumentError> {
    Ok(DocumentAttachment {
        id: row.get("id"),
        display_name: row.get("display_name"),
        format: DocumentFormat::from_extension(row.get::<&str, _>("format"))?,
        file_size: row.get::<i64, _>("file_size") as u64,
        sha256: row.get("sha256"),
        extraction_status: "ready".into(),
        indexing_status: row.get("indexing_status"),
    })
}
async fn read_attachment(
    connection: &mut sqlx::SqliteConnection,
    meeting: &str,
    id: &str,
) -> Result<DocumentAttachment, DocumentError> {
    let row = sqlx::query(&format!(
        "{ATTACHMENT_SELECT} WHERE a.meeting_id=? AND d.id=?"
    ))
    .bind(meeting)
    .bind(id)
    .fetch_optional(connection)
    .await?
    .ok_or(DocumentError::NotFound)?;
    attachment(&row)
}
impl From<sqlx::Error> for DocumentError {
    fn from(_: sqlx::Error) -> Self {
        Self::Storage
    }
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
        sqlx::query("INSERT INTO knowledge_owners(id,kind,meeting_id) VALUES ('meeting:aster','meeting','aster')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO knowledge_requests(id,owner_id,input_fingerprint,question,scope_json,frozen_ids_json,status,provider,model) VALUES ('reference-turn','meeting:aster','synthetic','What is the reference?','{}','[]','completed','synthetic','synthetic')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO knowledge_messages(id,request_id,role,content) VALUES ('reference-answer','reference-turn','assistant','Public reference content [K700]')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO knowledge_request_evidence(request_id,ordinal,reference_json,display_json) VALUES ('reference-turn',700,'{}','{}')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO knowledge_request_sources(request_id,source_id,revision) VALUES ('reference-turn',?,1)")
            .bind(format!("document:{}",attached.attachment.id)).execute(&pool).await.unwrap();
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
        let status: String =
            sqlx::query_scalar("SELECT status FROM knowledge_requests WHERE id='reference-turn'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "invalidated");
        let content: String = sqlx::query_scalar(
            "SELECT content FROM knowledge_messages WHERE id='reference-answer'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(content.is_empty());
        let maps: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM knowledge_request_evidence WHERE request_id='reference-turn'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(maps, 0);
    }
}
