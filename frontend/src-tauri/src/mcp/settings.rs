use serde::Serialize;
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager};

// The stdio process has no Tauri runtime. Read the same affirmative grant
// from disk for each tool call so an already-connected client can be revoked.
const GRANT_FILE: &str = "meeting-library-mcp.enabled";
const GRANT: &[u8] = b"read-meeting-library-v1\n";

pub(super) fn enabled(app_data: &Path) -> bool {
    read_grant(app_data).unwrap_or(false)
}

fn read_grant(app_data: &Path) -> io::Result<bool> {
    let path = app_data.join(GRANT_FILE);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
        Ok(meta) if !meta.is_file() || meta.len() != GRANT.len() as u64 => return Ok(false),
        Ok(_) => {}
    }
    let mut bytes = Vec::with_capacity(GRANT.len() + 1);
    fs::File::open(path)?
        .take(GRANT.len() as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes == GRANT)
}

pub(super) fn set_enabled(app_data: &Path, allow: bool) -> io::Result<()> {
    let path = app_data.join(GRANT_FILE);
    if !allow {
        return match fs::remove_file(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        };
    }
    fs::create_dir_all(app_data)?;
    let mut file = tempfile::NamedTempFile::new_in(app_data)?;
    file.write_all(GRANT)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

#[derive(Serialize)]
pub(crate) struct McpAccess {
    enabled: bool,
    command: PathBuf,
}

fn access(app: AppHandle, change: Option<bool>) -> Result<McpAccess, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| "Could not locate app data.")?;
    let command = std::env::current_exe().map_err(|_| "Could not locate ClawScribe.")?;
    if let Some(allow) = change {
        set_enabled(&directory, allow).map_err(|_| "Could not save local assistant access.")?;
    }
    let enabled = read_grant(&directory).map_err(|_| "Could not read local assistant access.")?;
    Ok(McpAccess { enabled, command })
}

#[tauri::command]
pub(crate) async fn get_mcp_access(app: AppHandle) -> Result<McpAccess, String> {
    tauri::async_runtime::spawn_blocking(move || access(app, None))
        .await
        .map_err(|_| "Could not read local assistant access.")?
}

#[tauri::command]
pub(crate) async fn set_mcp_access(app: AppHandle, enabled: bool) -> Result<McpAccess, String> {
    tauri::async_runtime::spawn_blocking(move || access(app, Some(enabled)))
        .await
        .map_err(|_| "Could not save local assistant access.")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_exact_grant_enables_access_and_removal_revokes_it() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert!(!enabled(root));
        set_enabled(root, true).unwrap();
        assert!(enabled(root));
        fs::write(root.join(GRANT_FILE), [GRANT, b"x"].concat()).unwrap();
        assert!(!enabled(root));
        set_enabled(root, true).unwrap();
        set_enabled(root, false).unwrap();
        set_enabled(root, false).unwrap();
        assert!(!enabled(root));
    }
}
