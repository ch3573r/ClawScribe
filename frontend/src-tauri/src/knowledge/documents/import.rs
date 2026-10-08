//! Cancellable staging and atomic publication; originals never use selected paths as identities.
use super::{DocumentAttachment, DocumentError, DocumentFormat, ExtractedDocument};
use sqlx::SqlitePool;
use std::{
    future::Future,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
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
    pool: SqlitePool,
    root: PathBuf,
    meeting: String,
    selected: PathBuf,
    cancel: CancellationToken,
    preempt: Arc<AtomicBool>,
    guard: G,
    extract: F,
) -> Result<DocumentAttachment, DocumentError>
where
    F: FnOnce(PathBuf, DocumentFormat, CancellationToken, Arc<AtomicBool>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<ExtractedDocument, DocumentError>> + Send,
{
    let cancel = cancel.child_token();
    let on_drop = CancelOnDrop(cancel.clone());
    let supervisor = tokio::spawn(async move {
        let _guard = guard;
        let watcher_cancel = cancel.clone();
        let watcher_preempt = preempt.clone();
        let _watcher = tokio::spawn(async move {
            loop {
                if watcher_preempt.load(Ordering::Acquire) {
                    watcher_cancel.cancel();
                    break;
                }
                tokio::select! {
                    _ = watcher_cancel.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
                }
            }
        });
        let completed = CancelOnDrop(cancel.clone());
        let result = import_inner(pool, root, meeting, selected, cancel, preempt, extract).await;
        drop(completed);
        result
    });
    let result = supervisor.await.map_err(|_| DocumentError::Storage)?;
    drop(on_drop);
    result
}

struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
struct Staged {
    file: tempfile::NamedTempFile,
    id: String,
    name: String,
    format: DocumentFormat,
    size: u64,
    hash: String,
}
fn check(cancel: &CancellationToken, preempt: &AtomicBool) -> Result<(), DocumentError> {
    if cancel.is_cancelled() || preempt.load(Ordering::Acquire) {
        Err(DocumentError::Cancelled)
    } else {
        Ok(())
    }
}
fn copy(
    root: PathBuf,
    selected: PathBuf,
    cancel: CancellationToken,
    preempt: Arc<AtomicBool>,
) -> Result<Staged, DocumentError> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    check(&cancel, &preempt)?;
    let format = DocumentFormat::from_extension(
        selected
            .extension()
            .and_then(|s| s.to_str())
            .ok_or(DocumentError::UnsupportedFormat)?,
    )?;
    let mut name = selected
        .file_name()
        .ok_or(DocumentError::FileUnavailable)?
        .to_string_lossy()
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    while name.len() > 1024 {
        name.pop();
    }
    if name.is_empty() {
        return Err(DocumentError::FileUnavailable);
    }
    let mut input = std::fs::File::open(&selected).map_err(|_| DocumentError::FileUnavailable)?;
    let metadata = input
        .metadata()
        .map_err(|_| DocumentError::FileUnavailable)?;
    if !metadata.is_file() {
        return Err(DocumentError::FileUnavailable);
    }
    if metadata.len() > super::INPUT_BYTES as u64 {
        return Err(DocumentError::InputLimit);
    }
    std::fs::create_dir_all(&root).map_err(|_| DocumentError::Storage)?;
    let mut file = tempfile::NamedTempFile::new_in(root).map_err(|_| DocumentError::Storage)?;
    let mut buffer = [0u8; 8192];
    let mut size = 0u64;
    let mut digest = Sha256::new();
    loop {
        check(&cancel, &preempt)?;
        let read = input
            .read(&mut buffer)
            .map_err(|_| DocumentError::FileUnavailable)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > super::INPUT_BYTES as u64 {
            return Err(DocumentError::InputLimit);
        }
        digest.update(&buffer[..read]);
        file.write_all(&buffer[..read])
            .map_err(|_| DocumentError::Storage)?;
    }
    file.as_file()
        .sync_all()
        .map_err(|_| DocumentError::Storage)?;
    check(&cancel, &preempt)?;
    Ok(Staged {
        file,
        id: uuid::Uuid::new_v4().to_string(),
        name,
        format,
        size,
        hash: format!("{:x}", digest.finalize()),
    })
}

async fn import_inner<F, Fut>(
    pool: SqlitePool,
    root: PathBuf,
    meeting: String,
    selected: PathBuf,
    cancel: CancellationToken,
    preempt: Arc<AtomicBool>,
    extract: F,
) -> Result<DocumentAttachment, DocumentError>
where
    F: FnOnce(PathBuf, DocumentFormat, CancellationToken, Arc<AtomicBool>) -> Fut,
    Fut: Future<Output = Result<ExtractedDocument, DocumentError>>,
{
    check(&cancel, &preempt)?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM meetings WHERE id=?)")
        .bind(&meeting)
        .fetch_one(&pool)
        .await?;
    if !exists {
        return Err(DocumentError::NotFound);
    }
    let staged = tokio::task::spawn_blocking({
        let root = root.clone();
        let cancel = cancel.clone();
        let preempt = preempt.clone();
        move || copy(root, selected, cancel, preempt)
    })
    .await
    .map_err(|_| DocumentError::Storage)??;
    let extracted = extract(
        staged.file.path().to_owned(),
        staged.format,
        cancel.clone(),
        preempt.clone(),
    )
    .await;
    let extracted = match extracted {
        Ok(value) => value,
        Err(error) => {
            tokio::task::spawn_blocking(move || drop(staged))
                .await
                .map_err(|_| DocumentError::Storage)?;
            return Err(error);
        }
    };
    if let Err(error) = check(&cancel, &preempt) {
        tokio::task::spawn_blocking(move || drop(staged))
            .await
            .map_err(|_| DocumentError::Storage)?;
        return Err(error);
    }
    let Staged {
        file,
        id,
        name,
        format,
        size,
        hash,
    } = staged;
    let destination = root.join(format!("{id}.{}", format.extension()));
    let rename = destination.clone();
    tokio::task::spawn_blocking(move || {
        file.persist_noclobber(rename)
            .map(|file| drop(file))
            .map_err(|_| DocumentError::Storage)
    })
    .await
    .map_err(|_| DocumentError::Storage)??;
    let result = super::store::publish(
        &pool, &meeting, &id, &name, size, &hash, &extracted, &cancel,
    )
    .await;
    match result {
        Ok(publication) => {
            if !publication.original_kept {
                tokio::fs::remove_file(destination)
                    .await
                    .map_err(|_| DocumentError::Storage)?;
            }
            Ok(publication.attachment)
        }
        Err(error) => {
            tokio::fs::remove_file(destination)
                .await
                .map_err(|_| DocumentError::Storage)?;
            Err(error)
        }
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
