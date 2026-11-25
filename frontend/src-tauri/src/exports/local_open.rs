use std::path::{Path, PathBuf};

use crate::{file_grants, local_path};

const UNAVAILABLE: &str = "The saved export is no longer available.";

pub(crate) fn allow_saved_file(path: &Path) -> bool {
    local_path::is_local_path_text(&path.to_string_lossy())
        && file_grants::grant(file_grants::Purpose::OpenExport, path).is_ok()
}

fn saved_file(text: &str) -> Result<PathBuf, String> {
    let path = file_grants::peek(file_grants::Purpose::OpenExport, text)?;
    if !local_path::is_local_path_text(&path.to_string_lossy())
        || !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("docx") || ext.eq_ignore_ascii_case("md"))
    {
        return Err(UNAVAILABLE.into());
    }
    let folder = path.parent().ok_or(UNAVAILABLE)?;
    local_path::require_unlinked_directory(folder).map_err(|_| UNAVAILABLE)?;
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| UNAVAILABLE)?;
    if !metadata.is_file() || local_path::is_link(&metadata) {
        return Err(UNAVAILABLE.into());
    }
    Ok(path)
}

#[tauri::command]
pub(crate) async fn open_saved_meeting_export(
    path: String,
    show_in_folder: bool,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let path = saved_file(&path)?;
        if show_in_folder {
            return show_file(&path);
        }
        open_document(&path)
    })
    .await
    .map_err(|_| "Opening the saved export was interrupted.")?
}

#[cfg(windows)]
fn show_file(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let quoted = local_path::explorer_argument(path)?;
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,{quoted}"))
        .spawn()
        .map(|_| ())
        .map_err(|_| "Could not show the saved export in File Explorer.".into())
}

#[cfg(target_os = "macos")]
fn show_file(path: &Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|_| "Could not show the saved export in Finder.".into())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn show_file(path: &Path) -> Result<(), String> {
    open_document(path.parent().ok_or(UNAVAILABLE)?)
}

fn open_document(path: &Path) -> Result<(), String> {
    let failed = || "Could not open the saved export. Check your default app.".to_string();
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            core::{w, PCWSTR},
            Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
        };
        let filename: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // The validated document path is passed directly to the OS association handler.
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(filename.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            return Err(failed());
        }
    }
    #[cfg(not(windows))]
    std::process::Command::new(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    })
    .arg(path)
    .spawn()
    .map_err(|_| failed())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_actions_require_a_grant_for_an_existing_export_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("notes & links.md");
        // The native picker's path is given in normal drive form on Windows.
        let text = path.to_string_lossy();
        let path = Path::new(text.strip_prefix(r"\\?\").unwrap_or(&text));
        let text = path.to_string_lossy();
        std::fs::write(path, "notes").unwrap();
        assert!(saved_file(&text).is_err());
        assert!(allow_saved_file(path));
        assert_eq!(saved_file(&text).unwrap(), path);
        std::fs::remove_file(path).unwrap();
        assert!(saved_file(&text).is_err());
    }
}
