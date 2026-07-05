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
                log::info!(
                    "Copying database from {} to {}",
                    backend_db_path,
                    tauri_db_path
                );
                fs::copy(backend_db_path, tauri_db_path).map_err(|e| sqlx::Error::Io(e))?;
            } else {
                log::info!("Creating database at {}", tauri_db_path);
                Sqlite::create_database(tauri_db_path).await?;
            }
        }

        let pool = SqlitePool::connect(tauri_db_path).await?;

        let migrator = sqlx::migrate!("./migrations");
        Self::reconcile_line_ending_checksums(&pool, &migrator).await?;
        migrator.run(&pool).await?;

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
                log::warn!(
                    "Repairing line-ending checksum drift for migration {} ({})",
                    migration.version,
                    migration.description
                );
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

        // WAL file paths for defensive cleanup
        let wal_path = app_data_dir.join("meeting_minutes.sqlite-wal");
        let shm_path = app_data_dir.join("meeting_minutes.sqlite-shm");

        log::info!("Tauri DB path: {}", tauri_db_path);
        log::info!("Legacy backend DB path: {}", backend_db_path);

        // Try to open database with defensive WAL handling
        match Self::new(&tauri_db_path, &backend_db_path).await {
            Ok(db_manager) => {
                log::info!("Database opened successfully");
                Ok(db_manager)
            }
            Err(e) => {
                // Check if error is due to corrupted WAL file
                let error_msg = e.to_string();
                if error_msg.contains("malformed") || error_msg.contains("corrupt") {
                    log::warn!("Database appears corrupted, likely due to orphaned WAL file. Attempting recovery...");
                    log::warn!("Error details: {}", error_msg);

                    // Delete potentially corrupted WAL/SHM files
                    if wal_path.exists() {
                        match fs::remove_file(&wal_path) {
                            Ok(_) => log::info!("Removed orphaned WAL file: {:?}", wal_path),
                            Err(e) => log::warn!("Failed to remove WAL file: {}", e),
                        }
                    }
                    if shm_path.exists() {
                        match fs::remove_file(&shm_path) {
                            Ok(_) => log::info!("Removed orphaned SHM file: {:?}", shm_path),
                            Err(e) => log::warn!("Failed to remove SHM file: {}", e),
                        }
                    }

                    // Retry connection without WAL files
                    log::info!("Retrying database connection after WAL cleanup...");
                    match Self::new(&tauri_db_path, &backend_db_path).await {
                        Ok(db_manager) => {
                            log::info!("Database opened successfully after WAL recovery");
                            Ok(db_manager)
                        }
                        Err(retry_err) => {
                            log::error!(
                                "Database connection failed even after WAL cleanup: {}",
                                retry_err
                            );
                            Err(retry_err)
                        }
                    }
                } else {
                    // Not a WAL-related error, propagate original error
                    log::error!("Database connection failed: {}", error_msg);
                    Err(e)
                }
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
        log::info!(
            "Copying legacy database from {} to {}",
            legacy_db_path,
            target_legacy_path.display()
        );

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
            Err(e) => log::warn!("WAL checkpoint failed (non-fatal): {}", e),
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
