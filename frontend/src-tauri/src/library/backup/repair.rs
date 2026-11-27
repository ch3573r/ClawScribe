//! Restore audio for matching meetings while preserving their local data.
use super::{archive_audio_file, failure, Manifest};
use crate::database::meeting_files::{
    directory_chain, read_marker, resolve_recording_folder_if_present, verify_recording_deletion,
};
use crate::library::archive::storage::{mark_backup_stage, pin_backup_folder, remove_backup_stage};
use serde_json::Value;
use sqlx::SqliteConnection;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

pub(super) type SavedFolders = HashMap<String, (Option<String>, bool)>;

pub(super) async fn saved_folders(db: &mut SqliteConnection) -> Result<SavedFolders, String> {
    Ok(sqlx::query_as::<_, (String, Option<String>, bool)>(
        "SELECT id, folder_path, saved_audio_removed_by_user FROM meetings",
    )
    .fetch_all(db)
    .await
    .map_err(failure)?
    .into_iter()
    .map(|(id, folder, removed)| (id, (folder, removed)))
    .collect())
}

#[derive(Debug, PartialEq, Eq)]
enum Destination {
    NewFolder,
    Existing(PathBuf),
    Unavailable,
}

fn destination(id: &str, folders: &SavedFolders, roots: &[PathBuf]) -> Option<Destination> {
    let (saved, removed) = folders.get(id)?;
    if *removed {
        return None;
    }
    let Some(path) = saved.as_deref().filter(|path| !path.trim().is_empty()) else {
        return Some(Destination::NewFolder);
    };
    let path = Path::new(path);
    if !roots.iter().any(|root| {
        crate::local_path::lexically_inside(&path.to_string_lossy(), root)
            && directory_chain(root).is_ok()
    }) {
        return Some(Destination::Unavailable);
    }
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
        .filter_map(|(_, (path, _))| path.as_ref().filter(|path| !path.trim().is_empty()))
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
    roots: Option<Vec<PathBuf>>,
    targets: HashMap<String, Destination>,
    pub(super) unavailable: usize,
}

impl Plan {
    pub(super) fn new(folders: SavedFolders, roots: Option<Vec<PathBuf>>) -> Self {
        Self {
            folders,
            roots,
            targets: HashMap::new(),
            unavailable: 0,
        }
    }

    pub(super) fn has_meeting(&self, id: &str) -> bool {
        self.folders.contains_key(id)
    }

    pub(super) fn has_targets(&self) -> bool {
        !self.targets.is_empty()
    }

    pub(super) fn select_audio(&mut self, id: &str) -> super::Extract {
        let Some(roots) = &self.roots else {
            return super::Extract::Skip;
        };
        let Some(destination) = destination(id, &self.folders, roots) else {
            return super::Extract::Skip;
        };
        let extract = match destination {
            Destination::NewFolder => super::Extract::AudioWithMarkers,
            Destination::Existing(_) => super::Extract::Audio,
            Destination::Unavailable => {
                self.unavailable += 1;
                return super::Extract::Skip;
            }
        };
        self.targets.insert(id.to_string(), destination);
        extract
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
        let roots = self.roots.unwrap_or_default();
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
                        ensure_owner(&source, &id)?;
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
                    Destination::Unavailable => return Err(CHANGED.into()),
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

pub(super) fn marker_file(name: &str) -> bool {
    matches!(
        name,
        "metadata.json" | "recording-outcome.json" | crate::audio::import::IMPORT_OWNER_FILE
    )
}

fn ensure_owner(folder: &Path, id: &str) -> Result<(), String> {
    let import_owner = folder.join(crate::audio::import::IMPORT_OWNER_FILE);
    if import_owner.exists() && !crate::audio::import::owns_import_folder(folder, id) {
        fs::remove_file(import_owner).map_err(failure)?;
    }
    let mut marker = read_marker(folder)
        .filter(|marker| {
            ["meeting_id", "recording_id"].iter().all(|key| {
                marker
                    .get(*key)
                    .is_none_or(|value| value.is_null() || value.as_str() == Some(id))
            })
        })
        .unwrap_or_else(|| {
            serde_json::json!({
                "version": "1.0",
                "transcript_file": "transcripts.json",
                "status": "completed",
                "source": "backup_restore",
            })
        });
    marker["meeting_id"] = Value::String(id.to_owned());
    if marker["source"] == "backup_restore" && marker["audio_file"].is_null() {
        let paths = fs::read_dir(folder).map_err(failure)?;
        let mut audio = None;
        for path in paths {
            let entry = path.map_err(failure)?;
            let name = entry.file_name();
            if archive_audio_file(&name.to_string_lossy()) {
                if audio.is_some() {
                    audio = None;
                    break;
                }
                audio = name.into_string().ok();
            }
        }
        if let Some(name) = audio {
            marker["audio_file"] = Value::String(name);
        }
    }
    let mut file = File::create(folder.join("metadata.json")).map_err(failure)?;
    serde_json::to_writer(&mut file, &marker).map_err(failure)?;
    file.sync_all().map_err(failure)
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
