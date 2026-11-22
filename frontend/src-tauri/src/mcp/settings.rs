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
const FOLLOW_THROUGH_FILE: &str = "meeting-follow-through-mcp.enabled";
const FOLLOW_THROUGH_GRANT: &[u8] = b"read-reviewed-meeting-follow-through-v1\n";

pub(super) fn enabled(app_data: &Path) -> bool {
    read_grant(app_data, GRANT_FILE, GRANT).unwrap_or(false)
}

pub(super) fn allowed(app_data: &Path, follow_through: bool) -> bool {
    if follow_through {
        read_grant(app_data, FOLLOW_THROUGH_FILE, FOLLOW_THROUGH_GRANT).unwrap_or(false)
    } else {
        enabled(app_data)
    }
}

fn read_grant(app_data: &Path, file: &str, grant: &[u8]) -> io::Result<bool> {
    let path = app_data.join(file);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
        Ok(meta) if !meta.is_file() || meta.len() != grant.len() as u64 => return Ok(false),
        Ok(_) => {}
    }
    let mut bytes = Vec::with_capacity(grant.len() + 1);
    fs::File::open(path)?
        .take(grant.len() as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes == grant)
}

pub(super) fn set_enabled(app_data: &Path, allow: bool) -> io::Result<()> {
    set_grant(app_data, GRANT_FILE, GRANT, allow)
}

pub(super) fn set_follow_through(app_data: &Path, allow: bool) -> io::Result<()> {
    set_grant(app_data, FOLLOW_THROUGH_FILE, FOLLOW_THROUGH_GRANT, allow)
}

fn set_grant(app_data: &Path, name: &str, grant: &[u8], allow: bool) -> io::Result<()> {
    let path = app_data.join(name);
    if !allow {
        return match fs::remove_file(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        };
    }
    fs::create_dir_all(app_data)?;
    let mut file = tempfile::NamedTempFile::new_in(app_data)?;
    file.write_all(grant)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

#[derive(Serialize)]
pub(crate) struct McpAccess {
    enabled: bool,
    follow_through: bool,
    command: PathBuf,
}

fn access(app: AppHandle, change: Option<(bool, bool)>) -> Result<McpAccess, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| "Could not locate app data.")?;
    let command = std::env::current_exe().map_err(|_| "Could not locate ClawScribe.")?;
    if let Some((allow, follow_through)) = change {
        let result = if follow_through {
            set_follow_through(&directory, allow)
        } else {
            set_enabled(&directory, allow)
        };
        result.map_err(|_| "Could not save local assistant access.")?;
    }
    let enabled = read_grant(&directory, GRANT_FILE, GRANT)
        .map_err(|_| "Could not read local assistant access.")?;
    let follow_through = read_grant(&directory, FOLLOW_THROUGH_FILE, FOLLOW_THROUGH_GRANT)
        .map_err(|_| "Could not read local assistant access.")?;
    Ok(McpAccess {
        enabled,
        follow_through,
        command,
    })
}

#[tauri::command]
pub(crate) async fn get_mcp_access(app: AppHandle) -> Result<McpAccess, String> {
    tauri::async_runtime::spawn_blocking(move || access(app, None))
        .await
        .map_err(|_| "Could not read local assistant access.")?
}

#[tauri::command]
pub(crate) async fn set_mcp_access(app: AppHandle, enabled: bool) -> Result<McpAccess, String> {
    tauri::async_runtime::spawn_blocking(move || access(app, Some((enabled, false))))
        .await
        .map_err(|_| "Could not save local assistant access.")?
}

#[tauri::command]
pub(crate) async fn set_mcp_follow_through_access(
    app: AppHandle,
    enabled: bool,
) -> Result<McpAccess, String> {
    tauri::async_runtime::spawn_blocking(move || access(app, Some((enabled, true))))
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
