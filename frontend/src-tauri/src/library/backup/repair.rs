//! Restore audio for matching meetings while preserving their local data.
use super::{archive_audio_file, failure, Manifest};
use crate::database::meeting_files::{
    resolve_recording_folder_if_present, verify_recording_deletion,
};
use crate::library::archive::storage::{mark_backup_stage, pin_backup_folder, remove_backup_stage};
use serde_json::Value;
use sqlx::SqliteConnection;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

pub(super) type SavedFolders = HashMap<String, Option<String>>;

pub(super) async fn saved_folders(db: &mut SqliteConnection) -> Result<SavedFolders, String> {
    Ok(sqlx::query_as("SELECT id, folder_path FROM meetings")
        .fetch_all(db)
        .await
        .map_err(failure)?
        .into_iter()
        .collect())
}

#[derive(Debug, PartialEq, Eq)]
enum Destination {
    NewFolder,
    Existing(PathBuf),
}

fn destination(id: &str, folders: &SavedFolders, roots: &[PathBuf]) -> Option<Destination> {
    let saved = folders.get(id)?;
    let Some(path) = saved.as_deref().filter(|path| !path.trim().is_empty()) else {
        return Some(Destination::NewFolder);
    };
    let path = Path::new(path);
    let Some(folder) = resolve_recording_folder_if_present(path, roots).ok()? else {
        return Some(Destination::NewFolder);
    };
    if fs::read_dir(&folder).is_ok_and(|mut entries| entries.next().is_none()) {
        return Some(Destination::NewFolder);
    }
    if !is_missing_audio(&folder) {
        return None;
    }
    let other_folders = folders
        .iter()
        .filter(|(other, _)| other.as_str() != id)
        .filter_map(|(_, path)| path.as_ref().filter(|path| !path.trim().is_empty()))
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    // Reuse the checks for a trusted recording directory with an app marker
    // and no other meeting using that directory or an overlapping path.
    let verified = verify_recording_deletion(path, id, roots, &other_folders, false).ok()??;
    (verified == folder).then_some(Destination::Existing(folder))
}

fn is_missing_audio(folder: &Path) -> bool {
    fs::read_dir(folder).is_ok_and(|entries| {
        entries.into_iter().all(|entry| {
            entry.is_ok_and(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if archive_audio_file(&name) || name.eq_ignore_ascii_case(".audio-spool") {
                    return false;
                }
                if name.eq_ignore_ascii_case(".checkpoints") {
                    let path = entry.path();
                    return fs::symlink_metadata(&path).is_ok_and(|metadata| {
                        metadata.is_dir()
                            && !crate::local_path::is_link(&metadata)
                            && fs::read_dir(&path).is_ok_and(|mut entries| entries.next().is_none())
                    });
                }
                true
            })
        })
    })
}

pub(super) struct Plan {
    folders: SavedFolders,
    roots: Vec<PathBuf>,
    targets: HashMap<String, Destination>,
}

impl Plan {
    pub(super) fn new(folders: SavedFolders, roots: Vec<PathBuf>) -> Self {
        Self {
            folders,
            roots,
            targets: HashMap::new(),
        }
    }

    pub(super) fn has_meeting(&self, id: &str) -> bool {
        self.folders.contains_key(id)
    }

    pub(super) fn has_targets(&self) -> bool {
        !self.targets.is_empty()
    }

    pub(super) fn include_audio(&mut self, id: &str) -> bool {
        let Some(destination) = destination(id, &self.folders, &self.roots) else {
            return false;
        };
        self.targets.insert(id.to_string(), destination);
        true
    }

    pub(super) async fn apply(
        mut self,
        db: &mut SqliteConnection,
        stage: &Path,
        manifest: &Manifest,
    ) -> Result<RepairedAudio, String> {
        if self.targets.is_empty() {
            return Ok(RepairedAudio::default());
        }
        let current = saved_folders(db).await?;
        let mut targets = Vec::new();
        for (index, row) in manifest.tables["meetings"].iter().enumerate() {
            let id = super::text_field(row, "id")?;
            if let Some(destination) = self.targets.remove(id) {
                if self.folders.get(id) != current.get(id) {
                    return Err(CHANGED.into());
                }
                let files = manifest.files[&index]
                    .iter()
                    .filter(|name| archive_audio_file(name))
                    .cloned()
                    .collect::<Vec<_>>();
                targets.push((index, id.to_owned(), files, destination));
            }
        }
        let stage = stage.to_path_buf();
        let roots = self.roots;
        let (result, reattached) = tokio::task::spawn_blocking(move || {
            let mut result = RepairedAudio::default();
            let mut reattached = Vec::new();
            for (index, id, names, expected) in targets {
                let source = stage.join(index.to_string());
                if destination(&id, &current, &roots).as_ref() != Some(&expected) {
                    return Err(CHANGED.into());
                }
                let mut files = 0;
                match expected {
                    Destination::NewFolder => {
                        files = fs::read_dir(&source)
                            .map_err(failure)?
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(failure)?
                            .len();
                        result.staged.insert(index);
                        reattached.push((id, source));
                    }
                    Destination::Existing(folder) => {
                        let _pins = pin_backup_folder(&folder).map_err(failure)?;
                        if !is_missing_audio(&folder) {
                            return Err(CHANGED.into());
                        }
                        for name in names {
                            let mut input = match File::open(source.join(&name)) {
                                Ok(input) => input,
                                // Empty archive entries are intentionally not extracted.
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                    continue
                                }
                                Err(error) => return Err(failure(error)),
                            };
                            let mut temp =
                                tempfile::NamedTempFile::new_in(&folder).map_err(failure)?;
                            std::io::copy(&mut input, &mut temp).map_err(failure)?;
                            temp.as_file().sync_all().map_err(failure)?;
                            let identity = mark_backup_stage(temp.as_file()).map_err(failure)?;
                            let path = folder.join(name);
                            temp.persist_noclobber(&path).map_err(failure)?;
                            result.published.0.push((path, identity));
                            files += 1;
                        }
                    }
                }
                if files == 0 {
                    return Err(CHANGED.into());
                }
                result.meetings += 1;
                result.files += files;
            }
            Ok::<_, String>((result, reattached))
        })
        .await
        .map_err(failure)??;
        for (id, folder) in reattached {
            sqlx::query("UPDATE meetings SET folder_path = ? WHERE id = ?")
                .bind(folder.to_string_lossy().as_ref())
                .bind(id)
                .execute(&mut *db)
                .await
                .map_err(failure)?;
        }
        Ok(result)
    }
}

const CHANGED: &str = "A recording folder changed during restore. Retry the restore.";

#[derive(Default)]
pub(super) struct RepairedAudio {
    pub(super) staged: HashSet<usize>,
    pub(super) meetings: usize,
    pub(super) files: usize,
    published: PublishedAudio,
}

impl RepairedAudio {
    pub(super) fn keep(mut self) {
        self.published.0.clear();
    }
}

#[derive(Default)]
struct PublishedAudio(Vec<(PathBuf, Value)>);

impl Drop for PublishedAudio {
    fn drop(&mut self) {
        for (path, identity) in &self.0 {
            let _ = remove_backup_stage(path, identity);
        }
    }
}
