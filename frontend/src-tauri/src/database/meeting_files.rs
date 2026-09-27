//! Recording deletion is restricted to unshared, app-owned directories.
use std::path::{Path, PathBuf};

fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 // Includes junctions/reparse points.
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(crate) fn remove_recording_folder(
    folder: &Path,
    roots: &[PathBuf],
    other_folders: &[PathBuf],
) -> Result<(), String> {
    let retained = |reason: &str| format!("Recording folder kept: {} ({reason})", folder.display());
    let metadata = match std::fs::symlink_metadata(folder) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(retained("could not inspect the folder")),
    };
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(retained("not a regular recording directory"));
    }
    let canonical = folder
        .canonicalize()
        .map_err(|_| retained("could not resolve the folder"))?;
    if !roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| canonical != root && canonical.starts_with(root))
    {
        return Err(retained("outside the configured recording locations"));
    }
    // Reject linked ancestors as well as a junction at the folder itself.
    for ancestor in folder.ancestors() {
        if std::fs::symlink_metadata(ancestor).is_ok_and(|metadata| is_link(&metadata)) {
            return Err(retained("the path contains a link or junction"));
        }
    }
    if !["metadata.json", "recording-outcome.json"]
        .iter()
        .any(|name| {
            std::fs::symlink_metadata(folder.join(name))
                .is_ok_and(|metadata| metadata.is_file() && !is_link(&metadata))
        })
    {
        return Err(retained("no ClawScribe recording marker"));
    }
    if other_folders.iter().any(|other| {
        other == folder
            || other.canonicalize().is_ok_and(|other| {
                other == canonical || other.starts_with(&canonical) || canonical.starts_with(other)
            })
    }) {
        return Err(retained("another meeting uses this folder"));
    }
    std::fs::remove_dir_all(folder).map_err(|_| retained("file deletion failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unshared_marked_recordings_inside_an_allowed_root_are_removed() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        std::fs::create_dir(&root).unwrap();
        for case in ["normal", "outside", "shared", "unmarked"] {
            let folder = if case == "outside" {
                base.path().join(case)
            } else {
                root.join(case)
            };
            std::fs::create_dir(&folder).unwrap();
            std::fs::write(folder.join("audio.wav"), b"synthetic audio").unwrap();
            if case != "unmarked" {
                std::fs::write(folder.join("metadata.json"), b"{}").unwrap();
            }
            let others = if case == "shared" {
                vec![folder.clone()]
            } else {
                vec![]
            };
            let result = remove_recording_folder(&folder, &[root.clone()], &others);
            if case == "normal" {
                assert!(result.is_ok());
                assert!(!folder.exists());
            } else {
                assert!(result.unwrap_err().contains(&folder.display().to_string()));
                assert!(folder.join("audio.wav").exists());
            }
        }
        assert!(remove_recording_folder(&root, &[root.clone()], &[]).is_err());
    }
}
