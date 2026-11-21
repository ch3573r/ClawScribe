use super::{fs, File, Path};
use std::{io, os::unix::fs::MetadataExt};
use xattr::FileExt;

const STAGE_MARKER: &str = "user.clawscribe.backup-stage";

pub(super) fn pin_directories(_folder: &Path) -> io::Result<Vec<File>> {
    Ok(Vec::new())
}

pub(super) fn open_audio(path: &Path, _removing: bool) -> io::Result<File> {
    let original = fs::symlink_metadata(path)?;
    let file = File::open(path)?;
    let opened = file.metadata()?;
    if !original.is_file() || (original.dev(), original.ino()) != (opened.dev(), opened.ino()) {
        return Err(io::Error::other("Audio changed while it was opened"));
    }
    Ok(file)
}

pub(super) fn open_marker(path: &Path) -> io::Result<File> {
    open_audio(path, false)
}

pub(super) fn file_identity(file: &File) -> io::Result<(u64, u64, i64, i64)> {
    let metadata = file.metadata()?;
    if metadata.nlink() != 1 {
        return Err(io::Error::other("Audio has no unique file identity"));
    }
    Ok((
        metadata.dev(),
        metadata.ino(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    ))
}

// An inode and birth timestamp can both be reused on rapid replacement.
// Record a per-file marker that survives writes and stays with the inode.
pub(super) fn mark_staging(file: &File) -> io::Result<()> {
    file.set_xattr(STAGE_MARKER, uuid::Uuid::new_v4().as_bytes())
}

pub(super) fn staging_identity(file: &File) -> io::Result<(u64, u64, Vec<u8>)> {
    let metadata = file.metadata()?;
    if metadata.nlink() != 1 {
        return Err(io::Error::other(
            "Archive stage has no unique file identity",
        ));
    }
    let marker = file
        .get_xattr(STAGE_MARKER)?
        .filter(|marker| marker.len() == 16)
        .ok_or_else(|| io::Error::other("Archive stage has no ownership marker"))?;
    Ok((metadata.dev(), metadata.ino(), marker))
}

pub(super) fn remove_audio(path: &Path, file: &File) -> io::Result<()> {
    let identity = file_identity(file)?;
    let current = fs::symlink_metadata(path)?;
    if !current.is_file()
        || current.nlink() != 1
        || identity
            != (
                current.dev(),
                current.ino(),
                current.ctime(),
                current.ctime_nsec(),
            )
    {
        return Err(io::Error::other("Audio changed before removal"));
    }
    fs::remove_file(path)
}
