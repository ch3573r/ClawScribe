use sqlx::{migrate::MigrateDatabase, Result, Sqlite, SqlitePool, Transaction};
use std::fs;
use std::path::Path;
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
                fs::copy(backend_db_path, tauri_db_path).map_err(|e| sqlx::Error::Io(e))?;
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
            .connect(tauri_db_path)
            .await?;

        let migrator = sqlx::migrate!("./migrations");
        Self::reconcile_line_ending_checksums(&pool, &migrator).await?;
        migrator.run(&pool).await?;
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

        fs::copy(legacy_db_path, &target_legacy_path).map_err(|e| sqlx::Error::Io(e))?;

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
