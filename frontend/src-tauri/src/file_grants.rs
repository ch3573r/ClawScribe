//! Files and folders the user chose in a native dialog the core opened itself.
//!
//! Trust comes from the dialog, not from where a path points: what the user
//! picked there is trusted for that one purpose, a network location included. A
//! command that reads or writes a file or folder the webview names would let a
//! compromised webview write a zip or `.docx` anywhere, read any archive,
//! upload any audio file or change where recordings go. Instead the core opens
//! the dialog and remembers the chosen path as a one-time grant for one
//! purpose; the command that does the work accepts only a path it holds a grant
//! for. A grant is used up when taken (a command that fails gives it back with
//! its original time, so a retry needs no new choice but the lifetime does not
//! restart), it expires after a quarter of an hour, and the table is small.
//! Programs the core runs are never granted this way: they stay local-only.
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;

const GRANT_LIFETIME: Duration = Duration::from_secs(15 * 60);
const MAX_GRANTS: usize = 32;
pub(crate) const AUDIO_EXTENSIONS: &[&str] = &["aac", "flac", "m4a", "mp3", "mp4", "ogg", "opus", "wav", "webm", "wma"];
pub(crate) const DOCUMENT_EXTENSIONS: &[&str] = &["pdf", "docx", "txt", "md"];
const CHOOSE_AGAIN: &str = "Choose the file again.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// An audio file to try a hosted transcription provider with.
    TestAudio,
    /// Where to write a library backup (`.zip`).
    BackupSave,
    /// A library backup to restore (`.zip`).
    BackupOpen,
    /// Where to write a Word export (`.docx`).
    WordSave,
    /// The folder recordings are saved in.
    RecordingsFolder,
    /// A reference document to attach to a meeting.
    ReferenceDocument,
    /// An audio file to import (chosen or dropped).
    AudioImport,
    /// The folder of ClawScribe's own Codex profile.
    CodexHome,
}

struct Grant {
    purpose: Purpose,
    key: String,
    path: PathBuf,
    granted: Instant,
}

/// A grant that was taken: the path and when it was originally granted.
#[derive(Debug, Clone)]
pub(crate) struct Taken {
    pub path: PathBuf,
    granted: Instant,
}

static GRANTS: Mutex<Vec<Grant>> = Mutex::new(Vec::new());

fn key(path: &Path) -> String {
    crate::local_path::comparable_text(&path.to_string_lossy())
}

fn sensible(path: &Path) -> Result<(), String> {
    let text = path.to_string_lossy();
    if text.is_empty() || text.contains('\0') || (!path.is_absolute() && !text.starts_with("\\\\")) {
        return Err(crate::local_path::LOCAL_ONLY.into());
    }
    Ok(())
}

fn insert(purpose: Purpose, path: &Path, granted: Instant) -> Result<(), String> {
    sensible(path)?;
    let mut grants = GRANTS.lock().unwrap_or_else(|error| error.into_inner());
    grants.retain(|grant| grant.granted.elapsed() < GRANT_LIFETIME);
    let wanted = key(path);
    grants.retain(|grant| !(grant.purpose == purpose && grant.key == wanted));
    grants.push(Grant { purpose, key: wanted, path: path.to_path_buf(), granted });
    grants.sort_by_key(|grant| grant.granted);
    while grants.len() > MAX_GRANTS {
        grants.remove(0);
    }
    Ok(())
}

/// Remembers a path the user chose in a dialog the core opened (or dropped on
/// the window, which the core saw), for one purpose.
pub(crate) fn grant(purpose: Purpose, path: &Path) -> Result<(), String> {
    insert(purpose, path, Instant::now())
}

fn find(grants: &mut Vec<Grant>, purpose: Purpose, text: &str) -> Option<usize> {
    grants.retain(|grant| grant.granted.elapsed() < GRANT_LIFETIME);
    let wanted = crate::local_path::comparable_text(text);
    grants.iter().position(|grant| grant.purpose == purpose && grant.key == wanted)
}

/// Uses up the grant for exactly this path and purpose. A path that was not
/// chosen in a dialog the core opened, a path chosen for another purpose, or an
/// expired one is refused.
pub(crate) fn take(purpose: Purpose, text: &str) -> Result<Taken, String> {
    let mut grants = GRANTS.lock().unwrap_or_else(|error| error.into_inner());
    let index = find(&mut grants, purpose, text).ok_or_else(|| CHOOSE_AGAIN.to_string())?;
    let grant = grants.remove(index);
    Ok(Taken { path: grant.path, granted: grant.granted })
}

/// The granted path, without using the grant up (a check before the real use).
pub(crate) fn peek(purpose: Purpose, text: &str) -> Result<PathBuf, String> {
    let mut grants = GRANTS.lock().unwrap_or_else(|error| error.into_inner());
    let index = find(&mut grants, purpose, text).ok_or_else(|| CHOOSE_AGAIN.to_string())?;
    Ok(grants[index].path.clone())
}

/// Gives a grant back after the command it was taken for failed, with the time
/// it was originally granted: a failed command does not extend its lifetime.
pub(crate) fn give_back(purpose: Purpose, taken: &Taken) {
    let _ = insert(purpose, &taken.path, taken.granted);
}

/// Whether the path (a file that exists) lies strictly inside one of `roots`
/// after resolving links: the user's own recordings, local or on the network.
pub(crate) fn inside_roots(path: &Path, roots: &[PathBuf]) -> bool {
    let Ok(resolved) = crate::trusted_roots::resolve_allowed(&path.to_string_lossy(), roots) else {
        return false;
    };
    roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| resolved != root && resolved.starts_with(&root))
}

/// The path with the extension the dialog should have given it. When adding it
/// changes the path the user confirmed and a file by that name exists, the
/// choice is refused rather than overwriting it silently.
pub(crate) fn with_extension(path: PathBuf, extension: &str) -> Result<PathBuf, String> {
    if path.extension().is_some_and(|current| current.eq_ignore_ascii_case(extension)) {
        return Ok(path);
    }
    let mut name = path.into_os_string();
    name.push(".");
    name.push(extension);
    let changed = PathBuf::from(name);
    if changed.exists() {
        return Err(format!("A file named like that with the .{extension} ending already exists. Choose another name."));
    }
    Ok(changed)
}

/// A folder a save target is written into must exist.
pub(crate) fn save_target_parent_exists(path: &Path) -> Result<(), String> {
    sensible(path)?;
    let parent = path.parent().ok_or_else(|| crate::local_path::LOCAL_ONLY.to_string())?;
    if parent.is_dir() {
        Ok(())
    } else {
        Err("The folder for the file does not exist.".into())
    }
}

async fn pick_file<R: Runtime>(app: AppHandle<R>, name: &'static str, extensions: &'static [&'static str]) -> Result<Option<PathBuf>, String> {
    tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter(name, extensions)
            .blocking_pick_file()
            .and_then(|chosen| chosen.into_path().ok())
    })
    .await
    .map_err(|_| "The file dialog failed".to_string())
}

pub(crate) async fn pick_save<R: Runtime>(
    app: AppHandle<R>,
    name: &'static str,
    extension: &'static str,
    suggested: String,
) -> Result<Option<PathBuf>, String> {
    let suggested: String = suggested
        .chars()
        .filter(|c| !matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') && !c.is_control())
        .take(120)
        .collect();
    tokio::task::spawn_blocking(move || {
        let mut dialog = app.dialog().file().add_filter(name, std::slice::from_ref(&extension));
        if !suggested.trim().is_empty() {
            dialog = dialog.set_file_name(suggested);
        }
        dialog.blocking_save_file().and_then(|chosen| chosen.into_path().ok())
    })
    .await
    .map_err(|_| "The file dialog failed".to_string())
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.iter().any(|known| known.eq_ignore_ascii_case(extension)))
}

/// Opens the file dialog for a hosted-transcription test file. Returns the
/// chosen path, which the test command then accepts once.
#[tauri::command]
pub async fn pick_transcription_test_audio<R: Runtime>(app: AppHandle<R>) -> Result<Option<String>, String> {
    let Some(path) = pick_file(app, "Audio", AUDIO_EXTENSIONS).await? else { return Ok(None) };
    let usable = std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file());
    if !has_extension(&path, AUDIO_EXTENSIONS) || !usable {
        return Err("Choose a supported audio file for the provider test".into());
    }
    grant(Purpose::TestAudio, &path)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Opens the file dialog for a reference document to attach to a meeting.
#[tauri::command]
pub async fn pick_reference_document<R: Runtime>(app: AppHandle<R>) -> Result<Option<String>, String> {
    let Some(path) = pick_file(app, "Reference documents", DOCUMENT_EXTENSIONS).await? else { return Ok(None) };
    if !has_extension(&path, DOCUMENT_EXTENSIONS) {
        return Err("Choose a PDF, Word, text or Markdown document".into());
    }
    grant(Purpose::ReferenceDocument, &path)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Opens the save dialog for a library backup.
#[tauri::command]
pub async fn pick_backup_save_path<R: Runtime>(app: AppHandle<R>, suggested_name: String) -> Result<Option<String>, String> {
    let Some(chosen) = pick_save(app, "ClawScribe meeting archive", "zip", suggested_name).await? else { return Ok(None) };
    let path = with_extension(chosen, "zip")?;
    save_target_parent_exists(&path)?;
    grant(Purpose::BackupSave, &path)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Opens the file dialog for a library backup to restore.
#[tauri::command]
pub async fn pick_backup_restore_path<R: Runtime>(app: AppHandle<R>) -> Result<Option<String>, String> {
    let Some(path) = pick_file(app, "ClawScribe meeting archive", &["zip"]).await? else { return Ok(None) };
    grant(Purpose::BackupOpen, &path)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Opens the save dialog for a Word export.
#[tauri::command]
pub async fn pick_word_save_path<R: Runtime>(app: AppHandle<R>, suggested_name: String) -> Result<Option<String>, String> {
    let Some(chosen) = pick_save(app, "Word document", "docx", suggested_name).await? else { return Ok(None) };
    let path = with_extension(chosen, "docx")?;
    save_target_parent_exists(&path)?;
    grant(Purpose::WordSave, &path)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Grants the audio files of a drop on the window. The core saw the drop, so it
/// is a user choice; the paths are never taken from a webview argument.
pub(crate) fn grant_dropped_audio(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| has_extension(path, AUDIO_EXTENSIONS))
        .filter(|path| grant(Purpose::AudioImport, path).is_ok())
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    static SERIAL: Mutex<()> = Mutex::new(());

    fn local(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("clawscribe-grant-test-{name}"))
    }

    #[test]
    fn a_granted_path_is_accepted_once_and_for_its_purpose_only() {
        let _serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        let path = local("once.zip");
        grant(Purpose::BackupSave, &path).unwrap();
        assert!(take(Purpose::BackupOpen, &path.to_string_lossy()).is_err());
        assert!(take(Purpose::WordSave, &path.to_string_lossy()).is_err());
        assert!(peek(Purpose::BackupSave, &path.to_string_lossy()).is_ok(), "a peek does not use it up");
        let granted = take(Purpose::BackupSave, &path.to_string_lossy()).unwrap();
        assert_eq!(granted.path, path);
        assert_eq!(take(Purpose::BackupSave, &path.to_string_lossy()).unwrap_err(), CHOOSE_AGAIN, "used up");
    }

    #[test]
    fn a_path_nobody_chose_is_refused_and_a_failed_command_gets_its_grant_back_with_its_time() {
        let _serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        let chosen = local("chosen.docx");
        let other = local("other.docx");
        grant(Purpose::WordSave, &chosen).unwrap();
        assert!(take(Purpose::WordSave, &other.to_string_lossy()).is_err());
        let taken = take(Purpose::WordSave, &chosen.to_string_lossy()).unwrap();
        let original = taken.granted;
        give_back(Purpose::WordSave, &taken);
        let again = take(Purpose::WordSave, &chosen.to_string_lossy()).unwrap();
        assert_eq!(again.granted, original, "the lifetime does not restart");
    }

    #[test]
    fn a_given_back_grant_that_is_too_old_is_not_revived() {
        let _serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        let path = local("stale.zip");
        let stale = Instant::now().checked_sub(GRANT_LIFETIME + Duration::from_secs(1));
        if let Some(stale) = stale {
            give_back(Purpose::BackupOpen, &Taken { path: path.clone(), granted: stale });
            assert!(take(Purpose::BackupOpen, &path.to_string_lossy()).is_err());
        }
    }

    #[test]
    fn a_chosen_network_location_is_granted_but_relative_text_is_not() {
        let _serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        // The user picked it in a dialog the core opened: the place is trusted.
        #[cfg(windows)]
        {
            grant(Purpose::BackupSave, Path::new(r"\\server\share\out.zip")).unwrap();
            assert!(take(Purpose::BackupSave, r"\\SERVER\share\OUT.zip").is_ok());
        }
        for refused in ["x.zip", "relative/x.zip", ""] {
            assert!(grant(Purpose::BackupSave, Path::new(refused)).is_err(), "{refused}");
        }
        // Without a grant nothing is accepted, whatever it looks like.
        for refused in [r"\\h\s\x.zip", "x.zip"] {
            assert!(take(Purpose::BackupSave, refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn spelling_variants_of_the_granted_path_match_and_the_table_stays_small() {
        let _serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        let path = local("Spelling.zip");
        grant(Purpose::BackupOpen, &path).unwrap();
        #[cfg(windows)]
        assert!(take(Purpose::BackupOpen, &path.to_string_lossy().to_ascii_uppercase().replace('\\', "/")).is_ok());
        #[cfg(not(windows))]
        assert!(take(Purpose::BackupOpen, &path.to_string_lossy()).is_ok());
        for index in 0..60 {
            grant(Purpose::TestAudio, &local(&format!("many-{index}.wav"))).unwrap();
        }
        assert!(GRANTS.lock().unwrap().len() <= MAX_GRANTS);
        assert!(take(Purpose::TestAudio, &local("many-0.wav").to_string_lossy()).is_err(), "the oldest was dropped");
        assert!(take(Purpose::TestAudio, &local("many-59.wav").to_string_lossy()).is_ok());
    }

    #[test]
    fn adding_the_extension_never_silently_overwrites_another_file() {
        let base = tempfile::tempdir().unwrap();
        assert_eq!(with_extension(base.path().join("a"), "zip").unwrap(), base.path().join("a.zip"));
        assert_eq!(with_extension(base.path().join("a.ZIP"), "zip").unwrap(), base.path().join("a.ZIP"));
        assert_eq!(
            with_extension(base.path().join("a.zip.txt"), "zip").unwrap(),
            base.path().join("a.zip.txt.zip")
        );
        // The user confirmed `b`; `b.zip` is somebody else's file.
        std::fs::write(base.path().join("b.zip"), b"mine").unwrap();
        assert!(with_extension(base.path().join("b"), "zip").is_err());
        assert_eq!(std::fs::read(base.path().join("b.zip")).unwrap(), b"mine");
        // A path that already has the extension is the one the user confirmed.
        assert!(with_extension(base.path().join("b.zip"), "zip").is_ok());
    }

    #[test]
    fn dropped_audio_is_granted_and_other_files_are_not() {
        let _serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        let audio = local("drop.wav");
        let other = local("drop.exe");
        let granted = grant_dropped_audio(&[audio.clone(), other.clone()]);
        assert_eq!(granted, vec![audio.to_string_lossy().into_owned()]);
        assert!(take(Purpose::AudioImport, &audio.to_string_lossy()).is_ok());
        assert!(take(Purpose::AudioImport, &other.to_string_lossy()).is_err());
    }

    #[test]
    fn files_inside_a_recording_root_are_recognised_and_others_are_not() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let inside = root.join("meeting").join("audio.wav");
        std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
        std::fs::write(&inside, b"x").unwrap();
        let outside = base.path().join("other.wav");
        std::fs::write(&outside, b"x").unwrap();
        assert!(inside_roots(&inside, &[root.clone()]));
        assert!(!inside_roots(&outside, &[root.clone()]));
        assert!(!inside_roots(&root, &[root.clone()]));
        assert!(!inside_roots(Path::new(r"\\h\s\recordings\a.wav"), &[root]));
    }

    #[test]
    fn a_save_target_needs_an_existing_folder() {
        let base = tempfile::tempdir().unwrap();
        assert!(save_target_parent_exists(&base.path().join("out.zip")).is_ok());
        assert!(save_target_parent_exists(&base.path().join("missing").join("out.zip")).is_err());
        assert!(save_target_parent_exists(Path::new("out.zip")).is_err());
    }
}
