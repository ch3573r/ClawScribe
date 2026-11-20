//! Files the user chose in a native dialog the core opened itself.
//!
//! A command that reads or writes a file the webview names would let a
//! compromised webview write a zip or `.docx` anywhere, read any archive, or
//! upload any audio file. Instead the core opens the dialog, and remembers the
//! chosen path as a one-time grant for one purpose. The command that does the
//! work accepts only a path it holds a grant for in this session. A grant is
//! used up when taken (a command that fails gives it back so the user can retry
//! without choosing again), expires after a quarter of an hour, and the table
//! is small.
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;

const GRANT_LIFETIME: Duration = Duration::from_secs(15 * 60);
const MAX_GRANTS: usize = 16;
const AUDIO_EXTENSIONS: &[&str] = &["aac", "flac", "m4a", "mp3", "mp4", "ogg", "opus", "wav", "webm", "wma"];
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
}

struct Grant {
    purpose: Purpose,
    key: String,
    path: PathBuf,
    granted: Instant,
}

static GRANTS: Mutex<Vec<Grant>> = Mutex::new(Vec::new());

fn key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let text = text.to_ascii_lowercase();
    text
}

/// Remembers a chosen path for one purpose. Only a local path is remembered.
pub(crate) fn grant(purpose: Purpose, path: &Path) -> Result<(), String> {
    crate::local_path::check_local(&path.to_string_lossy())?;
    let mut grants = GRANTS.lock().unwrap_or_else(|error| error.into_inner());
    grants.retain(|grant| grant.granted.elapsed() < GRANT_LIFETIME);
    let wanted = key(path);
    grants.retain(|grant| !(grant.purpose == purpose && grant.key == wanted));
    grants.push(Grant { purpose, key: wanted, path: path.to_path_buf(), granted: Instant::now() });
    while grants.len() > MAX_GRANTS {
        grants.remove(0);
    }
    Ok(())
}

/// Uses up the grant for exactly this path and purpose. A path that was not
/// chosen in a dialog the core opened, a path chosen for another purpose, or an
/// expired one is refused. The text is checked as a local path first.
pub(crate) fn take(purpose: Purpose, text: &str) -> Result<PathBuf, String> {
    crate::local_path::check_local(text)?;
    let wanted = key(Path::new(text));
    let mut grants = GRANTS.lock().unwrap_or_else(|error| error.into_inner());
    grants.retain(|grant| grant.granted.elapsed() < GRANT_LIFETIME);
    let index = grants
        .iter()
        .position(|grant| grant.purpose == purpose && grant.key == wanted)
        .ok_or_else(|| CHOOSE_AGAIN.to_string())?;
    Ok(grants.remove(index).path)
}

/// Gives a grant back after the command it was taken for failed.
pub(crate) fn give_back(purpose: Purpose, path: &Path) {
    let _ = grant(purpose, path);
}

/// Whether the path (a file that exists) lies strictly inside one of `roots`
/// after resolving links, on a local disk. The other way a webview path is
/// accepted: the user's own recordings.
pub(crate) fn inside_roots(path: &Path, roots: &[PathBuf]) -> bool {
    let Ok(resolved) = crate::local_path::resolve_local(&path.to_string_lossy()) else {
        return false;
    };
    roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| resolved != root && resolved.starts_with(&root))
}

fn with_extension(path: PathBuf, extension: &str) -> PathBuf {
    if path.extension().is_some_and(|current| current.eq_ignore_ascii_case(extension)) {
        path
    } else {
        let mut name = path.into_os_string();
        name.push(".");
        name.push(extension);
        PathBuf::from(name)
    }
}

/// A folder a save target is written into must itself be local once resolved.
pub(crate) fn save_target_is_local(path: &Path) -> Result<(), String> {
    crate::local_path::check_local(&path.to_string_lossy())?;
    let parent = path.parent().ok_or_else(|| crate::local_path::LOCAL_ONLY.to_string())?;
    crate::local_path::resolve_local(&parent.to_string_lossy()).map(|_| ())
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

async fn pick_save<R: Runtime>(
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

/// Opens the file dialog for a hosted-transcription test file. Returns the
/// chosen path, which the test command then accepts once.
#[tauri::command]
pub async fn pick_transcription_test_audio<R: Runtime>(app: AppHandle<R>) -> Result<Option<String>, String> {
    let Some(path) = pick_file(app, "Audio", AUDIO_EXTENSIONS).await? else { return Ok(None) };
    crate::local_path::check_local(&path.to_string_lossy())?;
    let extension_ok = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| AUDIO_EXTENSIONS.iter().any(|known| known.eq_ignore_ascii_case(extension)));
    let usable = std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file());
    if !extension_ok || !usable {
        return Err("Choose a supported audio file for the provider test".into());
    }
    grant(Purpose::TestAudio, &path)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Opens the save dialog for a library backup.
#[tauri::command]
pub async fn pick_backup_save_path<R: Runtime>(app: AppHandle<R>, suggested_name: String) -> Result<Option<String>, String> {
    let Some(path) = pick_save(app, "ClawScribe meeting archive", "zip", suggested_name).await? else { return Ok(None) };
    let path = with_extension(path, "zip");
    save_target_is_local(&path)?;
    grant(Purpose::BackupSave, &path)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Opens the file dialog for a library backup to restore.
#[tauri::command]
pub async fn pick_backup_restore_path<R: Runtime>(app: AppHandle<R>) -> Result<Option<String>, String> {
    let Some(path) = pick_file(app, "ClawScribe meeting archive", &["zip"]).await? else { return Ok(None) };
    crate::local_path::check_local(&path.to_string_lossy())?;
    grant(Purpose::BackupOpen, &path)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// Opens the save dialog for a Word export.
#[tauri::command]
pub async fn pick_word_save_path<R: Runtime>(app: AppHandle<R>, suggested_name: String) -> Result<Option<String>, String> {
    let Some(path) = pick_save(app, "Word document", "docx", suggested_name).await? else { return Ok(None) };
    let path = with_extension(path, "docx");
    save_target_is_local(&path)?;
    grant(Purpose::WordSave, &path)?;
    Ok(Some(path.to_string_lossy().into_owned()))
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
        // Another purpose cannot use it.
        assert!(take(Purpose::BackupOpen, &path.to_string_lossy()).is_err());
        assert!(take(Purpose::WordSave, &path.to_string_lossy()).is_err());
        let granted = take(Purpose::BackupSave, &path.to_string_lossy()).unwrap();
        assert_eq!(granted, path);
        assert_eq!(take(Purpose::BackupSave, &path.to_string_lossy()).unwrap_err(), CHOOSE_AGAIN, "used up");
    }

    #[test]
    fn a_path_nobody_chose_is_refused_and_a_failed_command_gets_its_grant_back() {
        let _serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        let chosen = local("chosen.docx");
        let other = local("other.docx");
        grant(Purpose::WordSave, &chosen).unwrap();
        assert!(take(Purpose::WordSave, &other.to_string_lossy()).is_err());
        let taken = take(Purpose::WordSave, &chosen.to_string_lossy()).unwrap();
        give_back(Purpose::WordSave, &taken);
        assert!(take(Purpose::WordSave, &chosen.to_string_lossy()).is_ok(), "retry without choosing again");
    }

    #[test]
    fn network_and_device_paths_are_never_granted_or_taken() {
        let _serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        for refused in [r"\\h\s\x.zip", r"\/h/s/x.zip", r"/\h\s\x.zip", r"\??\UNC\h\s\x.zip", r"\\?\C:\x.zip", "x.zip", "C:x.zip"] {
            assert!(grant(Purpose::BackupSave, Path::new(refused)).is_err(), "{refused}");
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
        for index in 0..40 {
            grant(Purpose::TestAudio, &local(&format!("many-{index}.wav"))).unwrap();
        }
        assert!(GRANTS.lock().unwrap().len() <= MAX_GRANTS);
        assert!(take(Purpose::TestAudio, &local("many-0.wav").to_string_lossy()).is_err(), "the oldest was dropped");
        assert!(take(Purpose::TestAudio, &local("many-39.wav").to_string_lossy()).is_ok());
    }

    #[test]
    fn the_extension_is_added_when_the_dialog_left_it_out() {
        let _serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(with_extension(PathBuf::from("a"), "zip"), PathBuf::from("a.zip"));
        assert_eq!(with_extension(PathBuf::from("a.ZIP"), "zip"), PathBuf::from("a.ZIP"));
        assert_eq!(with_extension(PathBuf::from("a.zip.txt"), "zip"), PathBuf::from("a.zip.txt.zip"));
    }

    #[test]
    fn files_inside_a_recording_root_are_recognised_and_others_are_not() {
        let _serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
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
    fn a_save_target_needs_a_local_existing_folder() {
        let _serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        let base = tempfile::tempdir().unwrap();
        assert!(save_target_is_local(&base.path().join("out.zip")).is_ok());
        assert!(save_target_is_local(&base.path().join("missing").join("out.zip")).is_err());
        assert!(save_target_is_local(Path::new(r"\\h\s\out.zip")).is_err());
    }
}
