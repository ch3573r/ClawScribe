//! Startup recovery selects a new profile only after its database opens.
//! The original database, WAL and recording directories are never moved.
use std::{
    borrow::Cow,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};
use uuid::Uuid;

const MARKER: &str = "active-recovery-library";
const PREVIOUS_MARKER: &str = "previous-recovery-library";
const UNAVAILABLE: &str = "The recovery library could not be opened. Check available disk space and access to the app data folder.";
const ORIGINAL_UNAVAILABLE: &str = "The original library could not be selected. Check that its database and app data folder are available.";

struct Profile {
    base: PathBuf,
    identifier: String,
    active: Result<Option<Uuid>, String>,
}

static PROFILE: OnceLock<Profile> = OnceLock::new();

fn recovery_identifier(identifier: &str, id: Uuid) -> String {
    format!("{identifier}.recovery-{id}")
}

fn marker(base: &Path, identifier: &str) -> PathBuf {
    base.join(format!("{identifier}.{MARKER}"))
}

fn read_marker(base: &Path, identifier: &str) -> Result<Option<Uuid>, String> {
    read_marker_at(base, identifier, &marker(base, identifier))
}

fn previous_marker(base: &Path, identifier: &str) -> PathBuf {
    base.join(format!("{identifier}.{PREVIOUS_MARKER}"))
}

fn read_marker_at(base: &Path, identifier: &str, path: &Path) -> Result<Option<Uuid>, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata) if metadata.is_file() && !crate::local_path::is_link(&metadata) => {}
        _ => return Err(UNAVAILABLE.into()),
    }
    crate::local_path::require_unlinked_directory(base)?;
    let mut text = String::new();
    File::open(path)
        .and_then(|file| file.take(40).read_to_string(&mut text))
        .map_err(|_| UNAVAILABLE)?;
    let id = Uuid::parse_str(text.trim()).map_err(|_| UNAVAILABLE)?;
    if text != id.to_string() {
        return Err(UNAVAILABLE.into());
    }
    let root = base.join(recovery_identifier(identifier, id));
    crate::local_path::require_unlinked_directory(&root)?;
    if !super::manager::database_exists(&root).map_err(|_| UNAVAILABLE)? {
        return Err(UNAVAILABLE.into());
    }
    Ok(Some(id))
}

pub(crate) fn configure(context: &mut tauri::Context<tauri::Wry>) {
    let Some(base) = dirs::data_dir() else { return };
    let identifier = context.config().identifier.clone();
    let active = read_marker(&base, &identifier);
    if let Ok(Some(id)) = &active {
        context.config_mut().identifier = recovery_identifier(&identifier, *id);
        // Keep the main window's local storage, IndexedDB and legacy preference
        // handovers inside this profile too.
        for window in &mut context.config_mut().app.windows {
            window.data_directory = Some(PathBuf::from(recovery_identifier(&identifier, *id)));
            window.data_store_identifier = Some(*id.as_bytes());
        }
    }
    let _ = PROFILE.set(Profile {
        base,
        identifier,
        active,
    });
}

pub(crate) fn startup_status() -> Result<(), String> {
    match PROFILE.get().map(|profile| &profile.active) {
        Some(Err(error)) => Err(error.clone()),
        _ => Ok(()),
    }
}

pub(crate) fn selected_data_dir(base: &Path, identifier: &str) -> Result<PathBuf, String> {
    let selected = read_marker(base, identifier)?
        .map(|id| recovery_identifier(identifier, id))
        .unwrap_or_else(|| identifier.to_owned());
    Ok(base.join(selected))
}

pub(crate) fn data_dir() -> Option<PathBuf> {
    let profile = PROFILE.get()?;
    let id = profile.active.as_ref().ok()?.as_ref()?;
    Some(
        profile
            .base
            .join(recovery_identifier(&profile.identifier, *id)),
    )
}

pub(crate) fn credential_service(service: &str) -> Cow<'_, str> {
    match PROFILE
        .get()
        .and_then(|profile| profile.active.as_ref().ok()?.as_ref())
    {
        Some(id) => Cow::Owned(format!("{service}.recovery-{id}")),
        None => Cow::Borrowed(service),
    }
}

#[tauri::command]
pub(crate) fn startup_recovery_active() -> bool {
    data_dir().is_some()
}

#[tauri::command]
pub(crate) fn return_to_original_library() -> Result<(), String> {
    let profile = PROFILE.get().ok_or(ORIGINAL_UNAVAILABLE)?;
    let id = profile
        .active
        .as_ref()
        .ok()
        .and_then(|id| *id)
        .ok_or(ORIGINAL_UNAVAILABLE)?;
    return_to_original_at(&profile.base, &profile.identifier, id)
}

fn return_to_original_at(base: &Path, identifier: &str, id: Uuid) -> Result<(), String> {
    let original = base.join(identifier);
    crate::local_path::require_unlinked_directory(&original).map_err(|_| ORIGINAL_UNAVAILABLE)?;
    if !super::manager::database_exists(&original).map_err(|_| ORIGINAL_UNAVAILABLE)? {
        return Err(ORIGINAL_UNAVAILABLE.into());
    }
    let previous = previous_marker(base, identifier);
    match read_marker(base, identifier).map_err(|_| ORIGINAL_UNAVAILABLE)? {
        Some(active) if active == id => {
            fs::rename(marker(base, identifier), previous).map_err(|_| ORIGINAL_UNAVAILABLE.into())
        }
        None if read_marker_at(base, identifier, &previous).ok().flatten() == Some(id) => Ok(()),
        _ => Err(ORIGINAL_UNAVAILABLE.into()),
    }
}

pub(super) async fn prepare() -> Result<(), String> {
    let profile = PROFILE.get().ok_or(UNAVAILABLE)?;
    if matches!(profile.active, Ok(None)) && resume_previous_at(&profile.base, &profile.identifier)?
    {
        return Ok(());
    }
    prepare_at(&profile.base, &profile.identifier).await
}

fn resume_previous_at(base: &Path, identifier: &str) -> Result<bool, String> {
    let Ok(Some(id)) = read_marker_at(base, identifier, &previous_marker(base, identifier)) else {
        return Ok(false);
    };
    stage_marker(base, id)?
        .persist_noclobber(marker(base, identifier))
        .map_err(|_| UNAVAILABLE)?;
    Ok(true)
}

async fn prepare_at(base: &Path, identifier: &str) -> Result<(), String> {
    crate::local_path::require_unlinked_directory(base).map_err(|_| UNAVAILABLE)?;
    let id = Uuid::new_v4();
    let root = base.join(recovery_identifier(identifier, id));
    fs::create_dir(&root).map_err(|_| UNAVAILABLE)?;
    let database = root.join(super::manager::DATABASE_FILENAME);
    let legacy = root.join("meeting_minutes.db");
    let manager = super::manager::DatabaseManager::new_isolated(
        database.to_str().ok_or(UNAVAILABLE)?,
        legacy.to_str().ok_or(UNAVAILABLE)?,
    )
    .await
    .map_err(|_| UNAVAILABLE)?;
    manager.cleanup().await.map_err(|_| UNAVAILABLE)?;

    stage_marker(base, id)?
        .persist(marker(base, identifier))
        .map_err(|_| UNAVAILABLE)?;
    Ok(())
}

fn stage_marker(base: &Path, id: Uuid) -> Result<tempfile::NamedTempFile, String> {
    let mut file = tempfile::NamedTempFile::new_in(base).map_err(|_| UNAVAILABLE)?;
    file.write_all(id.to_string().as_bytes())
        .map_err(|_| UNAVAILABLE)?;
    file.as_file().sync_all().map_err(|_| UNAVAILABLE)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::manager::DATABASE_FILENAME;

    #[tokio::test]
    async fn selecting_recovery_opened_a_fresh_database_and_preserves_original_bytes() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().canonicalize().unwrap();
        let identifier = "test.clawscribe";
        let original = root.join(identifier);
        fs::create_dir(&original).unwrap();
        let database = original.join(DATABASE_FILENAME);
        let wal = original.join(format!("{DATABASE_FILENAME}-wal"));
        fs::write(&database, b"invalid database").unwrap();
        fs::write(&wal, b"original WAL").unwrap();

        prepare_at(&root, identifier).await.unwrap();
        let selected = selected_data_dir(&root, identifier).unwrap();
        assert_ne!(selected, original);
        assert_eq!(fs::read(&database).unwrap(), b"invalid database");
        assert_eq!(fs::read(&wal).unwrap(), b"original WAL");

        let manager = super::super::manager::DatabaseManager::new_isolated(
            selected.join(DATABASE_FILENAME).to_str().unwrap(),
            selected.join("not-a-legacy-database").to_str().unwrap(),
        )
        .await
        .unwrap();
        let meetings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings")
            .fetch_one(manager.pool())
            .await
            .unwrap();
        assert_eq!(meetings, 0);
        manager.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn returning_to_original_and_retrying_reuses_the_previous_recovery_workspace() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().canonicalize().unwrap();
        let identifier = "test.clawscribe";
        let original = root.join(identifier);
        fs::create_dir(&original).unwrap();
        let database = original.join(DATABASE_FILENAME);
        let wal = original.join(format!("{DATABASE_FILENAME}-wal"));
        fs::write(&database, b"original database").unwrap();
        fs::write(&wal, b"original WAL").unwrap();
        prepare_at(&root, identifier).await.unwrap();
        let id = read_marker(&root, identifier).unwrap().unwrap();
        let recovery = selected_data_dir(&root, identifier).unwrap();
        fs::write(recovery.join("saved-preference"), b"recovery setting").unwrap();

        for _ in 0..2 {
            return_to_original_at(&root, identifier, id).unwrap();
            assert_eq!(selected_data_dir(&root, identifier).unwrap(), original);
            assert!(resume_previous_at(&root, identifier).unwrap());
            assert_eq!(selected_data_dir(&root, identifier).unwrap(), recovery);
            assert_eq!(
                fs::read(recovery.join("saved-preference")).unwrap(),
                b"recovery setting"
            );
            assert_eq!(fs::read(&database).unwrap(), b"original database");
            assert_eq!(fs::read(&wal).unwrap(), b"original WAL");
        }
    }

    #[tokio::test]
    async fn returning_cannot_activate_a_missing_original_database() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().canonicalize().unwrap();
        let identifier = "test.clawscribe";
        prepare_at(&root, identifier).await.unwrap();
        let active = read_marker(&root, identifier).unwrap().unwrap();
        assert!(return_to_original_at(&root, identifier, active).is_err());
        assert_eq!(read_marker(&root, identifier).unwrap(), Some(active));
        assert!(!root.join(identifier).exists());
    }

    #[test]
    fn a_wal_without_the_database_is_not_a_first_launch() {
        let base = tempfile::tempdir().unwrap();
        let wal = base.path().join(format!("{DATABASE_FILENAME}-wal"));
        fs::write(&wal, b"original WAL").unwrap();
        assert!(super::super::manager::database_exists(base.path()).is_err());
        assert_eq!(fs::read(wal).unwrap(), b"original WAL");
    }

    #[test]
    fn recovery_marker_cannot_name_a_path_or_activate_a_missing_database() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().canonicalize().unwrap();
        let identifier = "test.clawscribe";
        fs::create_dir(root.join(identifier)).unwrap();
        for value in ["../unrelated".to_owned(), Uuid::new_v4().to_string()] {
            fs::write(marker(&root, identifier), value).unwrap();
            assert!(selected_data_dir(&root, identifier).is_err());
        }
    }
}
