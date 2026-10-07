use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{migrate::MigrateDatabase, Result, Sqlite, SqlitePool, Transaction};
use std::fs;
use std::path::Path;
use std::time::Duration;
use tauri::Manager;

#[derive(Clone)]
pub struct DatabaseManager {
    pool: SqlitePool,
}

impl DatabaseManager {
    pub async fn new(tauri_db_path: &str, backend_db_path: &str) -> Result<Self> {
        if let Some(parent_dir) = Path::new(tauri_db_path).parent() {
            if !parent_dir.exists() {
                fs::create_dir_all(parent_dir).map_err(|e| sqlx::Error::Io(e))?;
            }
        }

        if !Path::new(tauri_db_path).exists() {
            if Path::new(backend_db_path).exists() {
                log::info!("Migrating the legacy local database");
                snapshot_database(Path::new(backend_db_path), Path::new(tauri_db_path)).await?;
            } else {
                log::info!("Creating the local database");
                Sqlite::create_database(tauri_db_path).await?;
            }
        }

        // Scrub replaced legacy credentials from database pages on every connection.
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("PRAGMA secure_delete = ON")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(connection_options(Path::new(tauri_db_path)))
            .await?;

        let migrator = sqlx::migrate!("./migrations");
        if backup_before_migrations(&pool, Path::new(tauri_db_path), &migrator)
            .await
            .is_err()
        {
            log::warn!("Could not create the pre-migration database backup; continuing startup");
        }
        Self::reconcile_line_ending_checksums(&pool, &migrator).await?;
        migrator.run(&pool).await?;
        sqlx::query("UPDATE knowledge_requests SET status='interrupted',failure='application_interrupted' WHERE status IN ('preparing','running')")
            .execute(&pool).await?;
        if prune_expired_snapshots(Path::new(tauri_db_path)).is_err() {
            log::warn!("Could not remove expired pre-migration database backups");
        }
        crate::database::repositories::summary::SummaryProcessesRepository::fail_interrupted_processes(&pool).await?;
        crate::database::repositories::setting::migrate_provider_credentials(&pool).await;
        // SQLite owns WAL recovery/checkpointing. Never delete WAL/SHM ourselves.
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&pool)
            .await?;

        Ok(DatabaseManager { pool })
    }

    /// Repair `_sqlx_migrations` checksums that differ from this build's
    /// embedded migrations only by line endings.
    ///
    /// SQLx hashes migration files at compile time, so a build compiled from
    /// a CRLF-converted checkout embeds different checksums than one compiled
    /// from an LF checkout — and refuses to open databases stamped by the
    /// other variant ("previously applied but has been modified"), locking
    /// installed clients out of their data. When the stored checksum matches
    /// the embedded SQL under either line-ending convention, the migration is
    /// logically identical, so re-stamp it with this build's checksum.
    async fn reconcile_line_ending_checksums(
        pool: &SqlitePool,
        migrator: &sqlx::migrate::Migrator,
    ) -> Result<()> {
        // The ledger only exists once migrations have run at least once.
        let ledger_exists: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
        )
        .fetch_optional(pool)
        .await?;
        if ledger_exists.is_none() {
            return Ok(());
        }

        for migration in migrator.iter() {
            let stored: Option<Vec<u8>> =
                sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
                    .bind(migration.version)
                    .fetch_optional(pool)
                    .await?;
            let Some(stored) = stored else {
                continue;
            };
            if stored.as_slice() == migration.checksum.as_ref() {
                continue;
            }

            if line_ending_variant_checksums(&migration.sql)
                .iter()
                .any(|variant| variant.as_slice() == stored.as_slice())
            {
                log::warn!("Repairing known migration line-ending checksum drift");
                sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
                    .bind(migration.checksum.as_ref())
                    .bind(migration.version)
                    .execute(pool)
                    .await?;
            }
        }

        Ok(())
    }

    // NOTE: So for the first time users they needs to start the application
    // after they can just delete the existing .sqlite file and then copy the existing .db file to
    // the current app dir, So the system detects legacy db and copy it and starts with that data
    // (Newly created .sqlite with the copied content from .db)
    pub async fn new_from_app_handle(app_handle: &tauri::AppHandle) -> Result<Self> {
        // Resolve the app's data directory
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");
        if !app_data_dir.exists() {
            fs::create_dir_all(&app_data_dir).map_err(|e| sqlx::Error::Io(e))?;
        }

        // Define database paths
        let tauri_db_path = app_data_dir
            .join("meeting_minutes.sqlite")
            .to_string_lossy()
            .to_string();
        // Legacy backend DB path (for auto-migration if exists)
        let backend_db_path = app_data_dir
            .join("meeting_minutes.db")
            .to_string_lossy()
            .to_string();

        // WAL files can contain the only copy of committed transactions. Never
        // remove them in response to an opening error; preserve the complete
        // database for SQLite recovery or repair on a separate copy.
        match Self::new(&tauri_db_path, &backend_db_path).await {
            Ok(db_manager) => {
                log::info!("Database opened successfully");
                Ok(db_manager)
            }
            Err(e) => {
                log::error!(
                    "Database could not be opened. Database and recovery files were preserved."
                );
                Err(e)
            }
        }
    }

    /// Check if this is the first launch (sqlite database doesn't exist yet)
    pub async fn is_first_launch(app_handle: &tauri::AppHandle) -> Result<bool> {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");

        let tauri_db_path = app_data_dir.join("meeting_minutes.sqlite");

        Ok(!tauri_db_path.exists())
    }

    /// Import a legacy database from the specified path and initialize
    pub async fn import_legacy_database(
        app_handle: &tauri::AppHandle,
        legacy_db_path: &str,
    ) -> Result<Self> {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");

        if !app_data_dir.exists() {
            fs::create_dir_all(&app_data_dir).map_err(|e| sqlx::Error::Io(e))?;
        }

        // Copy legacy database to app data directory as meeting_minutes.db
        let target_legacy_path = app_data_dir.join("meeting_minutes.db");
        log::info!("Copying legacy database to current storage");

        snapshot_database(Path::new(legacy_db_path), &target_legacy_path).await?;

        // Now use the standard initialization which will detect and migrate the legacy db
        Self::new_from_app_handle(app_handle).await
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn with_transaction<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Transaction<'_, Sqlite>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut tx = self.pool.begin().await?;
        let result = f(&mut tx).await;

        match result {
            Ok(val) => {
                tx.commit().await?;
                Ok(val)
            }
            Err(err) => {
                tx.rollback().await?;
                Err(err)
            }
        }
    }

    /// Cleanup database connection and checkpoint WAL
    /// This should be called on application shutdown to ensure:
    /// - All WAL changes are written to the main database file
    /// - The .wal and .shm files are deleted
    /// - Connection pool is gracefully closed
    pub async fn cleanup(&self) -> Result<()> {
        log::info!("Starting database cleanup...");

        // Force checkpoint of WAL to main database file and remove WAL file
        // TRUNCATE mode: checkpoints all pages AND deletes the WAL file
        match sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await
        {
            Ok(_) => log::info!("WAL checkpoint completed successfully"),
            Err(_e) => log::warn!("WAL checkpoint failed (non-fatal)"),
        }

        // Close the connection pool gracefully
        self.pool.close().await;
        log::info!("Database connection pool closed");

        Ok(())
    }
}

fn connection_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true)
}

async fn backup_before_migrations(
    pool: &SqlitePool,
    database: &Path,
    migrator: &sqlx::migrate::Migrator,
) -> Result<()> {
    let ledger: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(pool)
    .await?;
    if ledger == 0 {
        let user_tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT GLOB 'sqlite_*'",
        )
        .fetch_one(pool)
        .await?;
        if user_tables == 0 {
            return Ok(());
        }
    }
    let applied: Vec<i64> = if ledger > 0 {
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success = 1")
            .fetch_all(pool)
            .await?
    } else {
        Vec::new()
    };
    if migrator
        .iter()
        .all(|migration| applied.contains(&migration.version))
    {
        return Ok(());
    }
    let root = database.parent().unwrap_or(Path::new(".")).join("backups");
    fs::create_dir_all(&root).map_err(sqlx::Error::Io)?;
    let old_version = applied.iter().max().copied().unwrap_or(0);
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%9f");
    let target = root.join(format!("meeting_minutes-{old_version}-{stamp}.sqlite"));
    if let Err(error) = sqlx::query("VACUUM INTO ?")
        .bind(target.to_string_lossy().as_ref())
        .execute(pool)
        .await
    {
        let _ = fs::remove_file(&target);
        return Err(error);
    }
    // Only prune our snapshots, after publishing a successful replacement.
    let pattern = regex::Regex::new(r"^meeting_minutes-\d+-\d{8}T\d{15}\.sqlite$")
        .expect("snapshot filename pattern");
    let mut backups = fs::read_dir(&root)
        .map_err(sqlx::Error::Io)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| pattern.is_match(&entry.file_name().to_string_lossy()))
        .filter_map(|entry| {
            let metadata = fs::symlink_metadata(entry.path()).ok()?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return None;
            }
            Some((metadata.modified().ok()?, entry.path()))
        })
        .collect::<Vec<_>>();
    backups.sort_by(|a, b| b.cmp(a));
    for (_, path) in backups.into_iter().skip(2) {
        fs::remove_file(path).map_err(sqlx::Error::Io)?;
    }
    Ok(())
}

fn prune_expired_snapshots(database: &Path) -> std::io::Result<()> {
    let root = database.parent().unwrap_or(Path::new(".")).join("backups");
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let pattern = regex::Regex::new(r"^meeting_minutes-\d+-\d{8}T\d{15}\.sqlite$")
        .expect("snapshot filename pattern");
    for entry in entries {
        let entry = entry?;
        if !pattern.is_match(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata
                .modified()?
                .elapsed()
                .is_ok_and(|age| age > Duration::from_secs(14 * 24 * 60 * 60))
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// A SQLite snapshot includes committed WAL pages even when another reader
/// prevents a complete checkpoint. Never copy only the main database file.
async fn snapshot_database(source: &Path, target: &Path) -> Result<()> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(source)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await?;
    let result = async {
        sqlx::query("PRAGMA wal_checkpoint(FULL)")
            .execute(&pool)
            .await?;
        let staged = tempfile::NamedTempFile::new_in(target.parent().unwrap_or(Path::new(".")))
            .map_err(sqlx::Error::Io)?
            .into_temp_path();
        sqlx::query("VACUUM INTO ?")
            .bind(staged.to_string_lossy().as_ref())
            .execute(&pool)
            .await?;
        std::fs::rename(&staged, target).map_err(sqlx::Error::Io)?;
        Ok(())
    }
    .await;
    pool.close().await;
    result
}

/// SHA-384 checksums of the migration SQL under both line-ending
/// conventions, matching how SQLx hashes migration files at compile time.
fn line_ending_variant_checksums(sql: &str) -> [Vec<u8>; 2] {
    use sha2::{Digest, Sha384};

    let lf = sql.replace("\r\n", "\n");
    let crlf = lf.replace('\n', "\r\n");
    [
        Sha384::digest(lf.as_bytes()).to_vec(),
        Sha384::digest(crlf.as_bytes()).to_vec(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha384};

    #[tokio::test]
    async fn startup_prunes_expired_snapshots_and_preserves_recent_and_unowned_files() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("meeting_minutes.sqlite");
        let legacy = dir.path().join("missing.db");
        let manager = DatabaseManager::new(database.to_str().unwrap(), legacy.to_str().unwrap())
            .await
            .unwrap();
        manager.pool.close().await;
        let backups = dir.path().join("backups");
        assert!(!backups.exists());
        fs::create_dir(&backups).unwrap();
        let old = backups.join("meeting_minutes-1-20260101T000000000000000.sqlite");
        let recent = backups.join("meeting_minutes-2-20260102T000000000000000.sqlite");
        let unowned = backups.join("manual.sqlite");
        for file in [&old, &recent, &unowned] {
            fs::write(file, b"snapshot").unwrap();
        }
        let expired = std::time::SystemTime::now() - Duration::from_secs(15 * 24 * 60 * 60);
        for file in [&old, &unowned] {
            fs::OpenOptions::new()
                .write(true)
                .open(file)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(expired))
                .unwrap();
        }
        let manager = DatabaseManager::new(database.to_str().unwrap(), legacy.to_str().unwrap())
            .await
            .unwrap();
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(unowned.exists());
        manager.pool.close().await;
    }

    #[tokio::test]
    async fn snapshots_only_for_pending_migrations_keep_the_newest_two() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meeting.sqlite");
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(connection_options(&path).create_if_missing(true))
            .await
            .unwrap();
        let migrator = sqlx::migrate!("./migrations");
        backup_before_migrations(&pool, &path, &migrator)
            .await
            .unwrap();
        assert!(
            !dir.path().join("backups").exists(),
            "fresh databases need no snapshot"
        );
        sqlx::query("CREATE TABLE legacy_data (value TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        backup_before_migrations(&pool, &path, &migrator)
            .await
            .unwrap();
        migrator.run(&pool).await.unwrap();
        let backup_dir = dir.path().join("backups");
        assert_eq!(fs::read_dir(&backup_dir).unwrap().count(), 1);
        backup_before_migrations(&pool, &path, &migrator)
            .await
            .unwrap();
        assert_eq!(fs::read_dir(&backup_dir).unwrap().count(), 1);
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)").execute(&pool).await.unwrap();
        for _ in 0..3 {
            backup_before_migrations(&pool, &path, &migrator)
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(fs::read_dir(&backup_dir).unwrap().count(), 2);
        for entry in fs::read_dir(&backup_dir).unwrap() {
            let snapshot = SqlitePool::connect(entry.unwrap().path().to_str().unwrap())
                .await
                .unwrap();
            let valid: String = sqlx::query_scalar("PRAGMA integrity_check")
                .fetch_one(&snapshot)
                .await
                .unwrap();
            assert_eq!(valid, "ok");
            snapshot.close().await;
        }
        pool.close().await;
    }

    #[tokio::test]
    async fn fresh_database_uses_wal_and_snapshot_includes_uncheckpointed_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meeting.sqlite");
        let db = DatabaseManager::new(
            path.to_str().unwrap(),
            dir.path().join("missing.db").to_str().unwrap(),
        )
        .await
        .unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(foreign_keys, 1);
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('wal-test', 'Synthetic meeting', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)").execute(db.pool()).await.unwrap();
        let target = dir.path().join("copy.sqlite");
        snapshot_database(&path, &target).await.unwrap();
        let copy = SqlitePool::connect(target.to_str().unwrap()).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings WHERE id = 'wal-test'")
            .fetch_one(&copy)
            .await
            .unwrap();
        assert_eq!(count, 1);
        copy.close().await;
        db.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn gapped_canonical_rows_keep_fts_alignment_through_snapshot_and_import() {
        use crate::knowledge::{retrieval, types::KnowledgeScope};
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.sqlite");
        let missing = directory.path().join("missing.sqlite");
        let database = DatabaseManager::new(source.to_str().unwrap(), missing.to_str().unwrap())
            .await
            .unwrap();
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('gap','Public gap fixture','2026-09-01','2026-09-01')").execute(database.pool()).await.unwrap();
        for (id, text) in [
            ("a", "First retained passage"),
            ("b", "Middle deleted passage"),
            ("c", "Last retained passage"),
        ] {
            sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES (?,'gap',?,'00:01')").bind(id).bind(text).execute(database.pool()).await.unwrap();
        }
        sqlx::query("DELETE FROM transcripts WHERE id='b'")
            .execute(database.pool())
            .await
            .unwrap();
        let rowids: Vec<i64> = sqlx::query_scalar("SELECT rowid FROM transcripts ORDER BY rowid")
            .fetch_all(database.pool())
            .await
            .unwrap();
        assert_eq!(
            rowids,
            vec![1, 3],
            "Fixture must contain a real physical rowid gap"
        );
        let snapshot = directory.path().join("snapshot.sqlite");
        snapshot_database(&source, &snapshot).await.unwrap();
        let imported = directory.path().join("imported.sqlite");
        // This uses DatabaseManager's actual VACUUM INTO import path a second time.
        let copy = DatabaseManager::new(imported.to_str().unwrap(), snapshot.to_str().unwrap())
            .await
            .unwrap();
        let misaligned:i64=sqlx::query_scalar("SELECT count(*) FROM transcripts t LEFT JOIN knowledge_fts f ON f.rowid=t.rowid WHERE f.transcript_id IS NOT t.id").fetch_one(copy.pool()).await.unwrap();
        assert_eq!(
            misaligned, 0,
            "Copied derived FTS rowids must still match canonical IDs"
        );
        sqlx::query("UPDATE transcripts SET transcript='Updated canonical signal' WHERE id='c'")
            .execute(copy.pool())
            .await
            .unwrap();
        let scope = retrieval::freeze_scope(
            copy.pool(),
            &KnowledgeScope::Meeting {
                meeting_id: "gap".into(),
            },
        )
        .await
        .unwrap();
        let result =
            retrieval::search_channels(copy.pool(), &scope, "Updated canonical signal", None)
                .await
                .unwrap();
        assert_eq!(result.len(), 1);
        assert!(
            matches!(&result[0].evidence.locator,crate::knowledge::types::EvidenceLocator::Transcript{transcript_ids,..} if transcript_ids==&["c"])
        );
        sqlx::query("DELETE FROM transcripts WHERE id='a'")
            .execute(copy.pool())
            .await
            .unwrap();
        assert!(
            retrieval::search_channels(copy.pool(), &scope, "First retained passage", None)
                .await
                .unwrap()
                .iter()
                .all(|passage| !passage.text.contains("First"))
        );
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM knowledge_fts")
            .fetch_one(copy.pool())
            .await
            .unwrap();
        assert_eq!(remaining, 1);
        copy.cleanup().await.unwrap();
        database.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn failed_open_preserves_committed_wal_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("recovery.sqlite");
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .pragma("wal_autocheckpoint", "0");
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE recovery_probe (value TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO recovery_probe VALUES ('synthetic committed data')")
            .execute(&pool)
            .await
            .unwrap();
        // Force a genuine migration collision. The failing opener must not erase WAL.
        sqlx::query("CREATE TABLE _sqlx_migrations (incompatible INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        let wal = path.with_file_name("recovery.sqlite-wal");
        assert!(std::fs::metadata(&wal).unwrap().len() > 0);
        assert!(DatabaseManager::new(
            path.to_str().unwrap(),
            directory.path().join("absent.db").to_str().unwrap()
        )
        .await
        .is_err());
        let value: String = sqlx::query_scalar("SELECT value FROM recovery_probe")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(value, "synthetic committed data");
        assert!(wal.exists());
    }

    async fn pool_with_migration_ledger() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE _sqlx_migrations (
                version BIGINT PRIMARY KEY,
                description TEXT NOT NULL,
                installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                success BOOLEAN NOT NULL,
                checksum BLOB NOT NULL,
                execution_time BIGINT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn repairs_checksum_stamped_by_opposite_line_ending_build() {
        let migrator = sqlx::migrate!("./migrations");
        let migration = migrator.iter().next().expect("at least one migration");

        // Simulate a database stamped by a build compiled from a checkout
        // with the opposite line endings.
        let lf = migration.sql.replace("\r\n", "\n");
        let opposite = if migration.sql.contains("\r\n") {
            lf
        } else {
            lf.replace('\n', "\r\n")
        };
        let opposite_checksum = Sha384::digest(opposite.as_bytes()).to_vec();
        assert_ne!(
            opposite_checksum.as_slice(),
            migration.checksum.as_ref(),
            "test requires the opposite-line-ending variant to differ"
        );

        let pool = pool_with_migration_ledger().await;
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
             VALUES (?, ?, 1, ?, 0)",
        )
        .bind(migration.version)
        .bind(migration.description.as_ref())
        .bind(&opposite_checksum)
        .execute(&pool)
        .await
        .unwrap();

        DatabaseManager::reconcile_line_ending_checksums(&pool, &migrator)
            .await
            .unwrap();

        let repaired: Vec<u8> =
            sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
                .bind(migration.version)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(repaired.as_slice(), migration.checksum.as_ref());
    }

    #[tokio::test]
    async fn leaves_matching_and_genuinely_modified_checksums_alone() {
        let migrator = sqlx::migrate!("./migrations");
        let mut migrations = migrator.iter();
        let matching = migrations.next().expect("at least one migration");
        let modified = migrations.next().expect("at least two migrations");

        let pool = pool_with_migration_ledger().await;
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
             VALUES (?, ?, 1, ?, 0)",
        )
        .bind(matching.version)
        .bind(matching.description.as_ref())
        .bind(matching.checksum.as_ref())
        .execute(&pool)
        .await
        .unwrap();

        // A checksum that matches neither line-ending variant must stay
        // untouched so real migration tampering still fails loudly.
        let bogus_checksum = vec![0xAB_u8; 48];
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
             VALUES (?, ?, 1, ?, 0)",
        )
        .bind(modified.version)
        .bind(modified.description.as_ref())
        .bind(&bogus_checksum)
        .execute(&pool)
        .await
        .unwrap();

        DatabaseManager::reconcile_line_ending_checksums(&pool, &migrator)
            .await
            .unwrap();

        let kept: Vec<u8> =
            sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
                .bind(matching.version)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(kept.as_slice(), matching.checksum.as_ref());

        let untouched: Vec<u8> =
            sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
                .bind(modified.version)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(untouched, bogus_checksum);
    }

    #[tokio::test]
    async fn reconcile_is_a_no_op_before_first_migration_run() {
        let migrator = sqlx::migrate!("./migrations");
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        DatabaseManager::reconcile_line_ending_checksums(&pool, &migrator)
            .await
            .unwrap();
    }
}
