//! Startup-only recovery of abandoned reference originals.
use super::{original_path, remove_original, store, Imports};
use sqlx::{Connection, SqlitePool};
use std::{
    path::Path,
    time::{Duration, SystemTime},
};

const UNKNOWN_FILE_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

async fn idle(imports: &Imports) -> bool {
    if imports.is_active()
        || crate::knowledge::scheduler::foreground_waiting()
        || crate::audio::recording_commands::is_recording().await
    {
        return false;
    }
    // Probe admission, then immediately release it. Background file/DB work
    // must not retain a permit that could reject or delay recording startup.
    crate::audio::inference::claim_job().is_ok()
}

pub(crate) async fn sweep(
    pool: &SqlitePool,
    root: &Path,
    imports: &Imports,
) -> Result<Vec<String>, String> {
    let mut warnings = Vec::new();
    let mut after = String::new();
    loop {
        tokio::task::yield_now().await;
        if !idle(imports).await {
            return Ok(warnings);
        }
        // Never queue this maintenance behind a foreground database borrower.
        let Some(mut connection) = pool.try_acquire() else {
            return Ok(warnings);
        };
        let mut tx = connection
            .begin()
            .await
            .map_err(|_| "Reference cleanup could not start")?;
        let id = sqlx::query_scalar::<_, String>(
            "SELECT id FROM knowledge_documents d WHERE id>?
             AND NOT EXISTS (SELECT 1 FROM knowledge_document_attachments a WHERE a.document_id=d.id)
             ORDER BY id LIMIT 1",
        ).bind(&after).fetch_optional(&mut *tx).await.map_err(|_| "Reference cleanup could not read documents")?;
        let Some(id) = id else {
            tx.rollback()
                .await
                .map_err(|_| "Reference cleanup could not finish reading")?;
            connection.return_to_pool().await;
            break;
        };
        if !idle(imports).await {
            return Ok(warnings);
        }
        let storage = store::delete_unattached(&mut tx, &id)
            .await
            .map_err(|_| "Reference cleanup could not delete an orphan")?;
        tx.commit()
            .await
            .map_err(|_| "Reference cleanup could not commit")?;
        connection.return_to_pool().await;
        after = id;
        if !idle(imports).await {
            return Ok(warnings);
        }
        if let Some(name) = storage {
            if let Err(warning) = remove_original(root, &name).await {
                warnings.push(warning);
            }
        }
    }

    if !idle(imports).await {
        return Ok(warnings);
    }
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(warnings),
        Err(_) => return Err("Reference cleanup could not read original storage".into()),
    };
    loop {
        tokio::task::yield_now().await;
        if !idle(imports).await {
            break;
        }
        let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| "Reference cleanup could not read a file")?
        else {
            break;
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // Ignore links, directories, staging files and all noncanonical names.
        if original_path(root, name).is_err()
            || !entry
                .file_type()
                .await
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        {
            continue;
        }
        let old = entry
            .metadata()
            .await
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age >= UNKNOWN_FILE_GRACE);
        if !old || !idle(imports).await {
            continue;
        }
        let Some(mut connection) = pool.try_acquire() else {
            break;
        };
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM knowledge_documents WHERE storage_name=?)",
        )
        .bind(name)
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| "Reference cleanup could not check original ownership")?;
        connection.return_to_pool().await;
        if !exists && idle(imports).await {
            if let Err(warning) = remove_original(root, name).await {
                warnings.push(warning);
            }
        }
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::documents::{store, DocumentBlock, DocumentFormat, ExtractedDocument};
    use tokio_util::sync::CancellationToken;

    async fn fixture() -> (sqlx::SqlitePool, tempfile::TempDir, String, String) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('public','Public meeting','2026-09-01','2026-09-01')")
            .execute(&pool).await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let orphan = "00000000-0000-4000-8000-000000000071";
        let attached = "00000000-0000-4000-8000-000000000072";
        for (id, hash) in [(orphan, 'a'), (attached, 'b')] {
            store::publish(
                &pool,
                "public",
                id,
                "Public reference.txt",
                20,
                &hash.to_string().repeat(64),
                &ExtractedDocument {
                    format: DocumentFormat::Text,
                    blocks: vec![DocumentBlock {
                        page: None,
                        paragraph: 1,
                        text: "Public reference text".into(),
                    }],
                },
                &CancellationToken::new(),
            )
            .await
            .unwrap();
            std::fs::write(
                root.path().join(format!("{id}.txt")),
                b"Public reference text",
            )
            .unwrap();
        }
        sqlx::query("DELETE FROM knowledge_document_attachments WHERE document_id=?")
            .bind(orphan)
            .execute(&pool)
            .await
            .unwrap();
        // Pool releases ping asynchronously; make the idle fixture deterministic
        // before exercising the sweep's nonwaiting connection admission.
        pool.acquire().await.unwrap().return_to_pool().await;
        (
            pool,
            root,
            format!("{orphan}.txt"),
            format!("{attached}.txt"),
        )
    }

    #[tokio::test]
    async fn sweep_removes_legacy_orphans_preserves_attached_and_fresh_files() {
        let _lock = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let (pool, root, orphan, attached) = fixture().await;
        let unknown = "00000000-0000-4000-8000-000000000073.md";
        let old = "00000000-0000-4000-8000-000000000074.pdf";
        for name in [unknown, old, "keep.txt"] {
            std::fs::write(root.path().join(name), b"Synthetic unknown file").unwrap();
        }
        let old_time =
            std::time::SystemTime::now() - UNKNOWN_FILE_GRACE - std::time::Duration::from_secs(60);
        for name in [old, "keep.txt"] {
            std::fs::File::options()
                .write(true)
                .open(root.path().join(name))
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(old_time))
                .unwrap();
        }
        let imports = super::super::Imports::default();
        assert!(sweep(&pool, root.path(), &imports)
            .await
            .unwrap()
            .is_empty());
        assert!(!root.path().join(&orphan).exists());
        assert!(!root.path().join(old).exists());
        for name in [attached.as_str(), unknown, "keep.txt"] {
            assert!(root.path().join(name).exists());
        }
        for table in [
            "knowledge_documents",
            "knowledge_document_blocks",
            "knowledge_document_fts",
        ] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                1
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM knowledge_sources WHERE id=?")
                .bind(format!("document:{}", orphan.trim_end_matches(".txt")))
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert!(sweep(&pool, root.path(), &imports)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn sweep_skips_recording_admission_and_active_import() {
        let _lock = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let (pool, root, orphan, _) = fixture().await;
        let imports = super::super::Imports::default();
        let recording = crate::audio::inference::claim_job().unwrap();
        assert!(sweep(&pool, root.path(), &imports)
            .await
            .unwrap()
            .is_empty());
        assert!(root.path().join(&orphan).exists());
        drop(recording);
        let lease = imports
            .begin("00000000-0000-4000-8000-000000000075")
            .unwrap();
        assert!(sweep(&pool, root.path(), &imports)
            .await
            .unwrap()
            .is_empty());
        assert!(root.path().join(&orphan).exists());
        drop(lease);
        sweep(&pool, root.path(), &imports).await.unwrap();
        assert!(!root.path().join(&orphan).exists());
        assert!(crate::audio::inference::claim_job().is_ok());
    }

    #[tokio::test]
    async fn original_removal_reports_failures_and_accepts_missing_files() {
        let root = tempfile::tempdir().unwrap();
        let name = "00000000-0000-4000-8000-000000000071.txt";
        assert!(super::super::remove_original(root.path(), name)
            .await
            .is_ok());
        std::fs::create_dir(root.path().join(name)).unwrap();
        assert!(super::super::remove_original(root.path(), name)
            .await
            .is_err());
        assert!(super::super::remove_original(root.path(), "../outside.txt")
            .await
            .is_err());
    }
}
