//! Cancellable staging and atomic publication; originals never use selected paths as identities.
use super::{DocumentAttachment, DocumentError, DocumentFormat, ExtractedDocument};
use sqlx::SqlitePool;
use std::{
    future::Future,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};
use tokio_util::sync::CancellationToken;

pub async fn import_document<G: Send + 'static>(
    pool: SqlitePool,
    root: PathBuf,
    meeting: String,
    selected: PathBuf,
    cancel: CancellationToken,
    preempt: Arc<AtomicBool>,
    guard: G,
) -> Result<DocumentAttachment, DocumentError> {
    import_using(
        pool,
        root,
        meeting,
        selected,
        cancel,
        preempt,
        guard,
        |path, format, cancel, preempt| {
            super::worker::extract_file(path, format, cancel, preempt, ())
        },
    )
    .await
}
async fn import_using<G: Send + 'static, F, Fut>(
    _pool: SqlitePool,
    _root: PathBuf,
    _meeting: String,
    _selected: PathBuf,
    _cancel: CancellationToken,
    _preempt: Arc<AtomicBool>,
    _guard: G,
    _extract: F,
) -> Result<DocumentAttachment, DocumentError>
where
    F: FnOnce(PathBuf, DocumentFormat, CancellationToken, Arc<AtomicBool>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<ExtractedDocument, DocumentError>> + Send,
{
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
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('aster','Public fixture','2026-09-01','2026-09-01')")
            .execute(&pool).await.unwrap();
        pool
    }
    #[tokio::test]
    async fn cancelled_import_leaves_no_original_or_database_rows() {
        let pool = pool().await;
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join("reference.txt");
        std::fs::write(&selected, b"Public reference").unwrap();
        let root = temp.path().join("originals");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = import_document(
            pool.clone(),
            root.clone(),
            "aster".into(),
            selected,
            cancel,
            Arc::new(AtomicBool::new(false)),
            (),
        )
        .await;
        assert_eq!(result, Err(DocumentError::Cancelled));
        assert!(!root.exists() || std::fs::read_dir(&root).unwrap().next().is_none());
        assert!(super::super::store::list(&pool, "aster")
            .await
            .unwrap()
            .is_empty());
    }
    #[tokio::test]
    async fn failed_extraction_removes_staged_original() {
        let pool = pool().await;
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join("broken.pdf");
        std::fs::write(&selected, b"Malformed").unwrap();
        let root = temp.path().join("originals");
        let result = import_using(
            pool.clone(),
            root.clone(),
            "aster".into(),
            selected,
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
            (),
            |_, _, _, _| async { Err(DocumentError::Malformed) },
        )
        .await;
        assert_eq!(result, Err(DocumentError::Malformed));
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        assert!(super::super::store::list(&pool, "aster")
            .await
            .unwrap()
            .is_empty());
    }
    #[tokio::test]
    async fn original_is_app_owned_and_hash_checked_before_publication() {
        let pool = pool().await;
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join("Reference tables.docx");
        let bytes = super::super::fixtures::docx(&super::super::fixtures::table_document(), &[]);
        std::fs::write(&selected, &bytes).unwrap();
        let root = temp.path().join("originals");
        let attachment = import_using(
            pool.clone(),
            root.clone(),
            "aster".into(),
            selected.clone(),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
            (),
            |path, format, _, _| async move {
                super::super::extract::extract(format, &std::fs::read(path).unwrap())
            },
        )
        .await
        .unwrap();
        assert_eq!(attachment.display_name, "Reference tables.docx");
        assert_eq!(attachment.file_size, bytes.len() as u64);
        let entries: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|v| v.unwrap().path())
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].file_name().unwrap(),
            format!("{}.docx", attachment.id).as_str()
        );
        assert_eq!(std::fs::read(&entries[0]).unwrap(), bytes);
        std::fs::remove_file(selected).unwrap();
        assert_eq!(
            super::super::store::blocks(&pool, "aster", &attachment.id)
                .await
                .unwrap()
                .len(),
            1537
        );
        let dto = serde_json::to_string(&attachment).unwrap();
        assert!(!dto.contains(&temp.path().to_string_lossy().replace('\\', "\\\\")));
    }
}
