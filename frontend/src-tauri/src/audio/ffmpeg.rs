use ffmpeg_sidecar::paths::sidecar_dir;
use std::path::{Path, PathBuf};

#[cfg(not(windows))]
const EXECUTABLE_NAME: &str = "ffmpeg";
#[cfg(windows)]
const EXECUTABLE_NAME: &str = "ffmpeg.exe";

pub(super) const FFMPEG_UNAVAILABLE: &str =
    "Bundled FFmpeg is missing. Repair the ClawScribe installation.";

pub fn find_ffmpeg_path() -> Option<PathBuf> {
    find_installed_ffmpeg(&sidecar_dir().ok()?)
}

// The sidecar directory is the directory of the running app executable.
fn find_installed_ffmpeg(sidecar: &Path) -> Option<PathBuf> {
    [Some(sidecar.to_path_buf()), packaged_resource_dir(sidecar)]
        .into_iter()
        .flatten()
        .find_map(|directory| {
            let directory = directory.canonicalize().ok()?;
            let binary = directory.join(EXECUTABLE_NAME).canonicalize().ok()?;
            (binary.parent() == Some(directory.as_path()) && binary.is_file()).then_some(binary)
        })
}

fn packaged_resource_dir(sidecar: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return Some(sidecar.parent()?.join("Resources"));
    }
    if cfg!(target_os = "linux") {
        return Some(sidecar.join("lib"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_lookup_requires_a_file_in_an_installation_directory() {
        let temp = tempfile::tempdir().unwrap();
        let install = temp.path().join("app");
        std::fs::create_dir(&install).unwrap();
        std::fs::write(temp.path().join(EXECUTABLE_NAME), b"outside app").unwrap();
        std::fs::create_dir(install.join(EXECUTABLE_NAME)).unwrap();
        assert!(find_installed_ffmpeg(&install).is_none());

        let bundled = install.join(EXECUTABLE_NAME);
        std::fs::remove_dir(&bundled).unwrap();
        std::fs::write(&bundled, b"bundled").unwrap();
        assert_eq!(
            find_installed_ffmpeg(&install),
            Some(bundled.canonicalize().unwrap())
        );
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_lookup_rejects_links_outside_the_installation_directory() {
        let temp = tempfile::tempdir().unwrap();
        let install = temp.path().join("app");
        std::fs::create_dir(&install).unwrap();
        let external = temp.path().join(EXECUTABLE_NAME);
        std::fs::write(&external, b"outside app").unwrap();
        std::os::unix::fs::symlink(&external, install.join(EXECUTABLE_NAME)).unwrap();
        assert!(find_installed_ffmpeg(&install).is_none());
    }
}
