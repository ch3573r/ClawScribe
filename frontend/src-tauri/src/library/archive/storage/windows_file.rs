use super::{linked, File, Path};
use std::{
    fs::OpenOptions,
    io,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
};
use windows::Win32::{
    Foundation::{BOOLEAN, HANDLE},
    Storage::FileSystem::{
        FileDispositionInfo, GetFileInformationByHandle, SetFileInformationByHandle,
        BY_HANDLE_FILE_INFORMATION, DELETE, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    },
};

pub(super) fn pin_directories(folder: &Path) -> io::Result<Vec<File>> {
    let ancestors = folder.ancestors().collect::<Vec<_>>();
    ancestors
        .into_iter()
        .rev()
        .filter(|path| path.parent().is_some())
        .map(|path| {
            let file = OpenOptions::new()
                .access_mode(FILE_READ_ATTRIBUTES.0)
                .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
                .open(path)?;
            let metadata = file.metadata()?;
            if !metadata.is_dir() || linked(&metadata) {
                return Err(io::Error::other("Expected an ordinary directory"));
            }
            Ok(file)
        })
        .collect()
}

pub(super) fn open_audio(path: &Path, removing: bool) -> io::Result<File> {
    let access = FILE_READ_ATTRIBUTES.0 | if removing { DELETE.0 } else { 0 };
    OpenOptions::new()
        .access_mode(access)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
}

pub(super) fn open_marker(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
}

pub(super) fn file_identity(file: &File) -> io::Result<(u32, u32, u32, u64)> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: The handle and the output buffer remain valid throughout the call.
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle() as isize), &mut info) }
        .map_err(io::Error::other)?;
    if info.nNumberOfLinks != 1 || (info.nFileIndexHigh == 0 && info.nFileIndexLow == 0) {
        return Err(io::Error::other("Audio has no unique file identity"));
    }
    let created = (u64::from(info.ftCreationTime.dwHighDateTime) << 32)
        | u64::from(info.ftCreationTime.dwLowDateTime);
    Ok((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
        created,
    ))
}

pub(super) use file_identity as staging_identity;

pub(super) fn remove_audio(_path: &Path, file: &File) -> io::Result<()> {
    let disposition = FILE_DISPOSITION_INFO {
        DeleteFile: BOOLEAN(1),
    };
    // SAFETY: The handle was opened with DELETE for the reviewed file. The
    // disposition buffer has the layout and size expected by FileDispositionInfo.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle() as isize),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    }
    .map_err(io::Error::other)
}
