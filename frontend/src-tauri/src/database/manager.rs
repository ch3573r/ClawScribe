use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{ConnectOptions, Connection, Result, Sqlite, SqlitePool, Transaction};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::Manager;

pub(super) const DATABASE_FILENAME: &str = "meeting_minutes.sqlite";
const SNAPSHOT_NAME: &str = r"^meeting_minutes-\d+-\d{8}T\d{15}\.sqlite$";
const STAGED_SNAPSHOT_NAME: &str =
    r"^\.meeting_minutes-staged-[a-zA-Z0-9]+\.sqlite(?:-journal|-wal|-shm)?$";

pub(super) fn database_exists(root: &Path) -> Result<bool> {
    let inspect = |name: &str| -> std::io::Result<bool> {
        match fs::symlink_metadata(root.join(name)) {
            Ok(metadata) if metadata.is_file() && !crate::local_path::is_link(&metadata) => {
                Ok(true)
            }
            Ok(_) => Err(std::io::Error::other(
                "The database path is not a regular file",
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    };
    let database = inspect(DATABASE_FILENAME)?;
    for suffix in ["-wal", "-shm", "-journal"] {
        if inspect(&format!("{DATABASE_FILENAME}{suffix}"))? && !database {
            return Err(std::io::Error::other(
                "Database recovery files exist without the database",
            )
            .into());
        }
    }
    Ok(database)
}

#[derive(Clone)]
pub struct DatabaseManager {
    pool: SqlitePool,
    path: PathBuf,
}

impl DatabaseManager {
    pub async fn new(tauri_db_path: &str, backend_db_path: &str) -> Result<Self> {
        Self::open(
            tauri_db_path,
            backend_db_path,
            true,
            publish_migration_snapshot,
        )
        .await
    }

    /// The same backup, repair and migration path as [`Self::new`], without the
    /// step that reads or seals provider credentials through the platform
    /// credential store. The packaged self-test opens its disposable profile this
    /// way so it can never read or write the real credential store.
    pub(crate) async fn new_isolated(tauri_db_path: &str, backend_db_path: &str) -> Result<Self> {
        Self::open(
            tauri_db_path,
            backend_db_path,
            false,
            publish_migration_snapshot,
        )
        .await
    }

    async fn open(
        tauri_db_path: &str,
        backend_db_path: &str,
        migrate_credentials: bool,
        publish_snapshot: SnapshotPublisher,
    ) -> Result<Self> {
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
                // SQLx's URL parser treats the `?` in Windows verbatim paths
                // as a query delimiter and decodes percent escapes in filenames.
                connection_options(Path::new(tauri_db_path))
                    .create_if_missing(true)
                    .connect()
                    .await?
                    .close()
                    .await?;
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
        if remove_abandoned_snapshot_staging(Path::new(tauri_db_path)).is_err() {
            log::warn!("Could not remove abandoned pre-migration database staging files");
        }
        if backup_before_migrations(&pool, Path::new(tauri_db_path), &migrator, publish_snapshot)
            .await
            .is_err()
        {
            log::warn!("Could not create the pre-migration database backup; continuing startup");
        }
        Self::reconcile_line_ending_checksums(&pool, &migrator).await?;
        migrator.run(&pool).await?;
        // Earlier builds could leave enrolled voices behind after opt-out.
        sqlx::query(
            "DELETE FROM speaker_profiles WHERE (SELECT enabled FROM speaker_profile_preferences WHERE id = 1) = 0",
        )
        .execute(&pool)
        .await?;
        sqlx::query("UPDATE knowledge_requests SET status='interrupted',failure='application_interrupted' WHERE status IN ('preparing','running')")
            .execute(&pool).await?;
        if prune_expired_snapshots(Path::new(tauri_db_path)).is_err() {
            log::warn!("Could not remove expired pre-migration database backups");
        }
        if scrub_speaker_profile_snapshots(Path::new(tauri_db_path))
            .await
            .is_err()
        {
            log::warn!("Could not scrub saved voices from pre-migration database backups");
        }
        crate::database::repositories::summary::SummaryProcessesRepository::fail_interrupted_processes(&pool).await?;
        if migrate_credentials {
            crate::database::repositories::setting::migrate_provider_credentials(&pool).await;
        }
        // SQLite owns WAL recovery and checkpointing for the live database.
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&pool)
            .await?;

        Ok(DatabaseManager {
            pool,
            path: PathBuf::from(tauri_db_path),
        })
    }

    /// The live DB uses secure_delete on every connection. Checkpoint old WAL
    /// frames as well and scrub any snapshots left by an earlier build.
    pub(crate) async fn finish_speaker_profile_deletion(&self) -> bool {
        let snapshots_clean = scrub_speaker_profile_snapshots(&self.path).await.is_ok();
        let checkpoint = sqlx::query_as::<_, (i64, i64, i64)>("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&self.pool)
            .await;
        snapshots_clean && matches!(checkpoint, Ok((0, _, _)))
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
            .map_err(std::io::Error::other)?;
        database_exists(&app_data_dir)?;
        if !app_data_dir.exists() {
            fs::create_dir_all(&app_data_dir).map_err(|e| sqlx::Error::Io(e))?;
        }

        // Define database paths
        let tauri_db_path = app_data_dir
            .join(DATABASE_FILENAME)
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
            .map_err(std::io::Error::other)?;
        Ok(!database_exists(&app_data_dir)?)
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

type SnapshotPublisher = fn(tempfile::TempPath, &Path) -> std::io::Result<()>;

async fn backup_before_migrations(
    pool: &SqlitePool,
    database: &Path,
    migrator: &sqlx::migrate::Migrator,
    publish_snapshot: SnapshotPublisher,
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
    verify_snapshot_directory(&root)?;
    let old_version = applied.iter().max().copied().unwrap_or(0);
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%9f");
    let target = root.join(format!("meeting_minutes-{old_version}-{stamp}.sqlite"));
    let staged = tempfile::Builder::new()
        .prefix(".meeting_minutes-staged-")
        .suffix(".sqlite")
        .tempfile_in(&root)?
        .into_temp_path();
    sqlx::query("VACUUM INTO ?")
        .bind(staged.to_string_lossy().as_ref())
        .execute(pool)
        .await?;
    publish_snapshot(staged, &target)?;
    if scrub_snapshot_profiles(&target).await.is_err() {
        log::warn!("Could not scrub saved voices from the new pre-migration database backup; keeping the rollback copy");
    }
    // Only prune our snapshots, after publishing a successful replacement.
    let pattern = regex::Regex::new(SNAPSHOT_NAME).expect("snapshot filename pattern");
    let mut backups = fs::read_dir(&root)
        .map_err(sqlx::Error::Io)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| pattern.is_match(&entry.file_name().to_string_lossy()))
        .filter_map(|entry| {
            let metadata = fs::symlink_metadata(entry.path()).ok()?;
            if !metadata.is_file() || crate::local_path::is_link(&metadata) {
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

fn publish_migration_snapshot(staged: tempfile::TempPath, target: &Path) -> std::io::Result<()> {
    // SQLite has closed the destination. Flush it before the same-directory
    // rename makes it visible to recovery and retention.
    fs::OpenOptions::new()
        .write(true)
        .open(&staged)?
        .sync_all()?;
    staged
        .persist_noclobber(target)
        .map_err(|error| error.error)
}

fn verify_snapshot_directory(root: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.is_dir() || crate::local_path::is_link(&metadata) {
        return Err(std::io::Error::other(
            "The database backup directory is not a regular directory",
        ));
    }
    Ok(())
}

fn remove_abandoned_snapshot_staging(database: &Path) -> std::io::Result<()> {
    let root = database.parent().unwrap_or(Path::new(".")).join("backups");
    match verify_snapshot_directory(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    // Only exact staging names and their SQLite sidecars qualify.
    let pattern = regex::Regex::new(STAGED_SNAPSHOT_NAME).expect("staging filename pattern");
    let mut result = Ok(());
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if pattern.is_match(&entry.file_name().to_string_lossy()) {
            let path = entry.path();
            let outcome = fs::symlink_metadata(&path).and_then(|metadata| {
                if metadata.is_file() && !crate::local_path::is_link(&metadata) {
                    fs::remove_file(&path)
                } else {
                    Ok(())
                }
            });
            if let Err(error) = outcome {
                result = Err(error);
            }
        }
    }
    result
}

async fn scrub_speaker_profile_snapshots(database: &Path) -> Result<()> {
    static SCRUB: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = SCRUB.lock().await;
    let root = database.parent().unwrap_or(Path::new(".")).join("backups");
    match verify_snapshot_directory(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let published = regex::Regex::new(SNAPSHOT_NAME).expect("snapshot filename pattern");
    let mut result = Ok(());
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        if published.is_match(&entry.file_name().to_string_lossy()) {
            if let Err(error) = scrub_snapshot_profiles(&entry.path()).await {
                result = Err(error);
            }
        }
    }
    result
}

async fn scrub_snapshot_profiles(path: &Path) -> Result<()> {
    // Avoid following links through SQLite's auxiliary filenames as well.
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut file = path.as_os_str().to_os_string();
        file.push(suffix);
        match fs::symlink_metadata(&file) {
            Ok(metadata) if metadata.is_file() && !crate::local_path::is_link(&metadata) => {}
            Err(error) if !suffix.is_empty() && error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => {
                return Err(
                    std::io::Error::other("A database backup path is not a regular file").into(),
                )
            }
        }
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .busy_timeout(Duration::from_secs(5));
    let mut reader = options.clone().read_only(true).connect().await?;
    // Backups predating speaker profiles have neither table.
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('speaker_profile_suggestions', 'speaker_profiles')",
    ).fetch_all(&mut reader).await?;
    let mut deletes = Vec::new();
    for (table, has_rows, delete) in [
        (
            "speaker_profile_suggestions",
            "SELECT EXISTS(SELECT 1 FROM speaker_profile_suggestions)",
            "DELETE FROM speaker_profile_suggestions",
        ),
        (
            "speaker_profiles",
            "SELECT EXISTS(SELECT 1 FROM speaker_profiles)",
            "DELETE FROM speaker_profiles",
        ),
    ] {
        if tables.iter().any(|name| name == table)
            && sqlx::query_scalar::<_, bool>(has_rows)
                .fetch_one(&mut reader)
                .await?
        {
            deletes.push(delete);
        }
    }
    reader.close().await?;
    if deletes.is_empty() {
        return Ok(());
    }

    let file = fs::OpenOptions::new().write(true).open(path)?;
    let original_mtime = file.metadata()?.modified()?;
    let scrub = async {
        let mut connection = options
            .journal_mode(SqliteJournalMode::Delete)
            .pragma("secure_delete", "ON")
            .connect()
            .await?;
        let deletion = async {
            let mut tx = connection.begin().await?;
            for statement in deletes {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
            tx.commit().await
        }
        .await;
        deletion.and(connection.close().await)
    }
    .await;
    // Scrubbing must not extend the lifetime of a copy, including after a
    // rollback or a journal-mode change.
    let restored = file.set_times(fs::FileTimes::new().set_modified(original_mtime));
    scrub.and(restored.map_err(sqlx::Error::Io))
}

#[cfg(test)]
fn stamp_snapshot(path: &Path, modified: std::time::SystemTime) -> std::io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .set_times(fs::FileTimes::new().set_modified(modified))
}

fn prune_expired_snapshots(database: &Path) -> std::io::Result<()> {
    let root = database.parent().unwrap_or(Path::new(".")).join("backups");
    match verify_snapshot_directory(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    let pattern = regex::Regex::new(SNAPSHOT_NAME).expect("snapshot filename pattern");
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !pattern.is_match(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_file()
            && !crate::local_path::is_link(&metadata)
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
/// prevents a complete checkpoint. Never copy only the main database file. The
/// source is opened read-only and is never created, checkpointed or modified:
/// it may be a database the app does not own.
async fn snapshot_database(source: &Path, target: &Path) -> Result<()> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(source)
                .read_only(true)
                .create_if_missing(false)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await?;
    let result = async {
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
    async fn fresh_database_uses_the_literal_filename() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("meeting%20minutes.sqlite");
        let legacy = dir.path().join("missing.db");
        let manager =
            DatabaseManager::new_isolated(database.to_str().unwrap(), legacy.to_str().unwrap())
                .await
                .unwrap();

        manager.cleanup().await.unwrap();
        assert!(database.is_file());
        assert!(!dir.path().join("meeting minutes.sqlite").exists());
    }

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
    async fn startup_removes_voices_left_by_an_earlier_opt_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATABASE_FILENAME);
        let legacy = dir.path().join("missing.db");
        let open =
            || DatabaseManager::new_isolated(path.to_str().unwrap(), legacy.to_str().unwrap());
        let manager = open().await.unwrap();
        sqlx::query("INSERT INTO speaker_profiles VALUES ('alex', 'model', 'Alex', X'00')")
            .execute(&manager.pool)
            .await
            .unwrap();
        manager.pool.close().await;
        let manager = open().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM speaker_profiles")
                .fetch_one(&manager.pool)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn scrubbing_a_clean_snapshot_preserves_its_bytes_and_expiration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATABASE_FILENAME);
        let backups = dir.path().join("backups");
        fs::create_dir(&backups).unwrap();
        let old = backups.join("meeting_minutes-1-20261101T000000000000000.sqlite");
        let mut connection = SqliteConnectOptions::new()
            .filename(&old)
            .create_if_missing(true)
            .connect()
            .await
            .unwrap();
        sqlx::query("CREATE TABLE speaker_profiles (embedding BLOB)")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE speaker_profile_suggestions (speaker TEXT)")
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();
        let expired = std::time::SystemTime::now() - Duration::from_secs(20 * 24 * 60 * 60);
        stamp_snapshot(&old, expired).unwrap();
        let bytes = fs::read(&old).unwrap();
        let modified = fs::metadata(&old).unwrap().modified().unwrap();

        scrub_speaker_profile_snapshots(&path).await.unwrap();

        assert_eq!(fs::read(&old).unwrap(), bytes);
        assert_eq!(fs::metadata(&old).unwrap().modified().unwrap(), modified);
        prune_expired_snapshots(&path).unwrap();
        assert!(!old.exists());
    }

    #[tokio::test]
    async fn startup_migrates_after_snapshot_publish_fails_and_cleans_the_stage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATABASE_FILENAME);
        let legacy = dir.path().join("missing.db");
        let mut connection = connection_options(&path)
            .create_if_missing(true)
            .connect()
            .await
            .unwrap();
        sqlx::query("CREATE TABLE legacy_data (value TEXT)")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO legacy_data VALUES ('preserved')")
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();

        let manager = DatabaseManager::open(
            path.to_str().unwrap(),
            legacy.to_str().unwrap(),
            false,
            |staged, target| {
                // A destination collision makes the real no-clobber rename fail.
                fs::copy(&staged, target).unwrap();
                Err(publish_migration_snapshot(staged, target).unwrap_err())
            },
        )
        .await
        .unwrap();

        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM speaker_profile_preferences")
                .fetch_one(&manager.pool)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT value FROM legacy_data")
                .fetch_one(&manager.pool)
                .await
                .unwrap(),
            "preserved"
        );
        let files = fs::read_dir(dir.path().join("backups"))
            .unwrap()
            .collect::<std::io::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(files.len(), 1);
        assert!(regex::Regex::new(SNAPSHOT_NAME)
            .unwrap()
            .is_match(&files[0].file_name().to_string_lossy()));
        manager.pool.close().await;
    }

    #[tokio::test]
    async fn a_failed_voice_scrub_keeps_the_published_rollback_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATABASE_FILENAME);
        let pool = SqlitePool::connect_with(connection_options(&path).create_if_missing(true))
            .await
            .unwrap();
        let migrator = sqlx::migrate!("./migrations");
        migrator.run(&pool).await.unwrap();
        sqlx::query("INSERT INTO speaker_profiles VALUES ('alex', 'model', 'Alex', X'00')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER fail_scrub BEFORE DELETE ON speaker_profiles BEGIN SELECT RAISE(ABORT, 'synthetic scrub refusal'); END")
            .execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)")
            .execute(&pool).await.unwrap();

        backup_before_migrations(&pool, &path, &migrator, publish_migration_snapshot)
            .await
            .unwrap();

        let backups = dir.path().join("backups");
        let snapshot = fs::read_dir(backups).unwrap().next().unwrap().unwrap();
        assert!(regex::Regex::new(SNAPSHOT_NAME)
            .unwrap()
            .is_match(&snapshot.file_name().to_string_lossy()));
        let mut copy = SqliteConnectOptions::new()
            .filename(snapshot.path())
            .read_only(true)
            .connect()
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
                .fetch_one(&mut copy)
                .await
                .unwrap(),
            "ok"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM speaker_profiles")
                .fetch_one(&mut copy)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn startup_clears_abandoned_staging_without_blocking_profile_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATABASE_FILENAME);
        let legacy = dir.path().join("missing.db");
        let backups = dir.path().join("backups");
        fs::create_dir(&backups).unwrap();
        let staged_files = [
            backups.join(".meeting_minutes-staged-Ab12.sqlite"),
            backups.join(".meeting_minutes-staged-Ab12.sqlite-journal"),
            backups.join(".meeting_minutes-staged-orphan.sqlite-wal"),
            backups.join(".meeting_minutes-staged-orphan.sqlite-shm"),
        ];
        let unrelated = backups.join(".meeting_minutes-staged-user-created.sqlite");
        for path in staged_files.iter().chain(std::iter::once(&unrelated)) {
            fs::write(path, b"incomplete SQLite file").unwrap();
        }
        // Delete/disable only scrubs published snapshots, even before startup cleanup.
        scrub_speaker_profile_snapshots(&path).await.unwrap();

        let manager =
            DatabaseManager::new_isolated(path.to_str().unwrap(), legacy.to_str().unwrap())
                .await
                .unwrap();
        assert!(staged_files.iter().all(|path| !path.exists()));
        assert!(unrelated.is_file());
        assert!(manager.finish_speaker_profile_deletion().await);
        manager.pool.close().await;
    }

    #[tokio::test]
    async fn migration_snapshots_strip_voice_records_and_old_copies_can_be_scrubbed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DATABASE_FILENAME);
        let pool = SqlitePool::connect_with(connection_options(&path).create_if_missing(true))
            .await
            .unwrap();
        let migrator = sqlx::migrate!("./migrations");
        migrator.run(&pool).await.unwrap();
        let embedding = (0..1024)
            .map(|index| (index % 137 + 101) as u8)
            .collect::<Vec<_>>();
        sqlx::query("INSERT INTO speaker_profiles VALUES ('alex', 'model', 'Alex', ?)")
            .bind(&embedding)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('saved', 'Saved meeting', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO speaker_profile_suggestions VALUES ('saved', 'Speaker 1', X'00', 'alex', 'model')")
            .execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)")
            .execute(&pool).await.unwrap();
        backup_before_migrations(&pool, &path, &migrator, publish_migration_snapshot)
            .await
            .unwrap();

        let backups = dir.path().join("backups");
        let published = fs::read_dir(&backups)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(!fs::read(&published)
            .unwrap()
            .windows(embedding.len())
            .any(|bytes| bytes == embedding));
        let old = backups.join("meeting_minutes-1-20261101T000000000000000.sqlite");
        let unrelated = backups.join("manual.sqlite");
        for snapshot in [&old, &unrelated] {
            sqlx::query("VACUUM INTO ?")
                .bind(snapshot.to_str().unwrap())
                .execute(&pool)
                .await
                .unwrap();
            assert!(fs::read(snapshot)
                .unwrap()
                .windows(embedding.len())
                .any(|bytes| bytes == embedding));
        }
        stamp_snapshot(
            &old,
            std::time::SystemTime::now() - Duration::from_secs(24 * 60 * 60),
        )
        .unwrap();
        let original_mtime = fs::metadata(&old).unwrap().modified().unwrap();
        scrub_speaker_profile_snapshots(&path).await.unwrap();
        assert_eq!(
            fs::metadata(&old).unwrap().modified().unwrap(),
            original_mtime
        );

        for snapshot in [&published, &old] {
            assert!(!fs::read(snapshot)
                .unwrap()
                .windows(embedding.len())
                .any(|bytes| bytes == embedding));
            let mut connection = SqliteConnectOptions::new()
                .filename(snapshot)
                .read_only(true)
                .connect()
                .await
                .unwrap();
            for query in [
                "SELECT COUNT(*) FROM speaker_profiles",
                "SELECT COUNT(*) FROM speaker_profile_suggestions",
            ] {
                assert_eq!(
                    sqlx::query_scalar::<_, i64>(query)
                        .fetch_one(&mut connection)
                        .await
                        .unwrap(),
                    0
                );
            }
            assert_eq!(
                sqlx::query_scalar::<_, String>("SELECT title FROM meetings WHERE id = 'saved'")
                    .fetch_one(&mut connection)
                    .await
                    .unwrap(),
                "Saved meeting"
            );
        }
        assert!(fs::read(unrelated)
            .unwrap()
            .windows(embedding.len())
            .any(|bytes| bytes == embedding));
        assert_eq!(
            sqlx::query_scalar::<_, Vec<u8>>("SELECT embedding FROM speaker_profiles")
                .fetch_one(&pool)
                .await
                .unwrap(),
            embedding
        );
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
        backup_before_migrations(&pool, &path, &migrator, publish_migration_snapshot)
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
        backup_before_migrations(&pool, &path, &migrator, publish_migration_snapshot)
            .await
            .unwrap();
        migrator.run(&pool).await.unwrap();
        let backup_dir = dir.path().join("backups");
        assert_eq!(fs::read_dir(&backup_dir).unwrap().count(), 1);
        let leftover = backup_dir.join(".meeting_minutes-staged-Leftover123.sqlite");
        fs::write(&leftover, b"partial snapshot").unwrap();
        stamp_snapshot(
            &leftover,
            std::time::SystemTime::now() + Duration::from_secs(24 * 60 * 60),
        )
        .unwrap();
        backup_before_migrations(&pool, &path, &migrator, publish_migration_snapshot)
            .await
            .unwrap();
        assert_eq!(fs::read_dir(&backup_dir).unwrap().count(), 2);
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)").execute(&pool).await.unwrap();
        for _ in 0..3 {
            backup_before_migrations(&pool, &path, &migrator, publish_migration_snapshot)
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(fs::read(&leftover).unwrap(), b"partial snapshot");
        remove_abandoned_snapshot_staging(&path).unwrap();
        assert!(!leftover.exists());
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
