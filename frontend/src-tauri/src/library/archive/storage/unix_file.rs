use super::{fs, File, Path};
use std::{io, os::unix::fs::MetadataExt};

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
