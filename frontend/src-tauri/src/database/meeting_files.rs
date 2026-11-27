//! Recording deletion is restricted to unshared, app-owned directories.
use std::path::{Path, PathBuf};

/// Reject redirection and unsupported reparse points. Rust's Windows
/// `is_symlink` recognizes name surrogates, not every reparse point.
pub(crate) fn is_link(_path: &Path, metadata: &std::fs::Metadata) -> bool {
    if metadata.is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
            && !reparse_tag(_path).is_ok_and(is_cloud_tag)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn is_cloud_tag(tag: u32) -> bool {
    // Windows SDK IO_REPARSE_TAG_CLOUD through IO_REPARSE_TAG_CLOUD_F.
    const CLOUD: u32 = 0x9000_001a;
    const CLOUD_MASK: u32 = 0x0000_f000;
    tag & !CLOUD_MASK == CLOUD
}

#[cfg(windows)]
fn reparse_tag(path: &Path) -> std::io::Result<u32> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0 | FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)?;
    reparse_tag_of(&file)
}

#[cfg(windows)]
fn reparse_tag_of(file: &std::fs::File) -> std::io::Result<u32> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            FileAttributeTagInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_TAG_INFO,
        },
    };
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: The handle and correctly sized output buffer live through the call.
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle() as isize),
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    }
    .map_err(std::io::Error::other)?;
    Ok(info.ReparseTag)
}

/// Hold directory handles without delete sharing while moving or purging a
/// cleanup stage, so its ancestors cannot be replaced underneath the operation.
#[cfg(windows)]
pub(crate) fn pin_directory_chain(path: &Path) -> std::io::Result<Vec<std::fs::File>> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let ancestors: Vec<_> = path
        .ancestors()
        .filter(|part| part.parent().is_some())
        .collect();
    let mut pinned = Vec::with_capacity(ancestors.len());
    for ancestor in ancestors.into_iter().rev() {
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES.0)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0 | FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(ancestor)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir()
            || (metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
                && !reparse_tag_of(&file).is_ok_and(is_cloud_tag))
        {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        pinned.push(file);
    }
    Ok(pinned)
}

/// Inspect parents before the leaf so metadata lookup does not traverse links.
pub(crate) fn directory_chain(path: &Path) -> std::io::Result<()> {
    if !path.is_absolute() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let parents: Vec<_> = path.ancestors().filter(|part| part.has_root()).collect();
    for parent in parents.into_iter().rev() {
        let metadata = std::fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || is_link(parent, &metadata) {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
    }
    Ok(())
}

pub(crate) const CLEANUP_PREFIX: &str = ".clawscribe-cleanup~";

pub(crate) fn is_cleanup_path(path: &Path) -> bool {
    path.components().any(|part| {
        part.as_os_str()
            .to_string_lossy()
            .to_ascii_lowercase()
            .starts_with(CLEANUP_PREFIX)
    })
}

/// Resolves a folder a caller named (for example a webview) to its canonical
/// path, and only if it is an existing directory strictly inside one of the
/// recording `roots`. Inspect the directory chain before canonicalizing to
/// reject links and junctions on either path. The error omits the path.
pub(crate) fn resolve_recording_folder(
    folder: &Path,
    roots: &[PathBuf],
) -> Result<PathBuf, String> {
    resolve_recording_folder_if_present(folder, roots)?.ok_or_else(|| REFUSED.into())
}

pub(crate) fn resolve_recording_folder_if_present(
    folder: &Path,
    roots: &[PathBuf],
) -> Result<Option<PathBuf>, String> {
    if folder.as_os_str().is_empty() {
        return Err(REFUSED.into());
    }
    // The text first: canonicalising a network path already contacts it, so a
    // path that is not on a local drive is only looked at when its own spelling
    // is inside a trusted root (a folder the user chose, network or not).
    if !crate::trusted_roots::text_allowed(&folder.to_string_lossy(), roots) {
        return Err(REFUSED.into());
    }
    if is_cleanup_path(folder) {
        return Err(REFUSED.into());
    }
    let canonical = match directory_chain(folder).and_then(|()| folder.canonicalize()) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(REFUSED.into()),
    };
    let inside = roots
        .iter()
        .filter(|root| directory_chain(root).is_ok())
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| canonical != root && canonical.starts_with(&root));
    if inside && !is_cleanup_path(&canonical) {
        Ok(Some(canonical))
    } else {
        Err(REFUSED.into())
    }
}

const REFUSED: &str = "This folder is not one of your recordings.";
const MAX_MARKER_BYTES: u64 = 1024 * 1024;
const MAX_CORE_FOLDERS: usize = 64;

fn folder_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let text = text.to_ascii_lowercase();
    let trimmed = text.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.ends_with(':') {
        text
    } else {
        trimmed.to_string()
    }
}

/// Folders this process itself created for a recording or an import, as it
/// wrote them (key, original text). The webview can only name one of these, or
/// a folder in a recording root that carries the app's own marker.
static CORE_FOLDERS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn register_core_folder(folder: &Path) {
    let mut folders = CORE_FOLDERS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let key = folder_key(folder);
    folders.retain(|(known, _)| *known != key);
    folders.push((key, folder.to_string_lossy().into_owned()));
    if folders.len() > MAX_CORE_FOLDERS {
        folders.remove(0);
    }
}

/// The text the core itself used for this folder, when the core created it.
pub(crate) fn core_folder_text(folder: &Path) -> Option<String> {
    let key = folder_key(folder);
    CORE_FOLDERS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .find(|(known, _)| *known == key)
        .map(|(_, text)| text.clone())
}

/// A canonical path as plain text for the shell: without the `\\?\` prefix
/// Windows adds (a UNC path keeps its own).
pub(crate) fn plain_path(canonical: &Path) -> PathBuf {
    let text = canonical.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => canonical.to_path_buf(),
    }
}

/// The app's own marker in a recording folder: a regular `metadata.json` in the
/// format ClawScribe writes. Its `meeting_id`, when it has one, must equal
/// `meeting_id`; a recording that was saved before ids were written has none.
/// A file with a generic name alone never qualifies.
pub(crate) fn has_app_marker(folder: &Path, meeting_id: Option<&str>) -> bool {
    let Some(marker) = read_marker(folder) else {
        return false;
    };
    meeting_id.is_none_or(|expected| {
        if let Some(written) = marker.get("meeting_id").filter(|value| !value.is_null()) {
            return written.as_str() == Some(expected);
        }
        marker
            .get("recording_id")
            .filter(|value| !value.is_null())
            .is_none()
            || recording_id(&marker) == Some(expected)
    })
}

pub(crate) fn read_marker(folder: &Path) -> Option<serde_json::Value> {
    let path = folder.join("metadata.json");
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || is_link(&path, &metadata) || metadata.len() > MAX_MARKER_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let text = |name: &str| value.get(name).is_some_and(serde_json::Value::is_string);
    (text("version") && text("transcript_file")).then_some(value)
}

pub(crate) fn has_unpublished_import_owner(folder: &Path, meeting_id: &str) -> bool {
    matches!(
        std::fs::symlink_metadata(folder.join("metadata.json")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    ) && crate::audio::import::owns_import_folder(folder, meeting_id)
}

/// The stable meeting ID assigned before audio capture starts.
pub(crate) fn recording_id(marker: &serde_json::Value) -> Option<&str> {
    marker_id(marker, "recording_id")
}

pub(crate) fn stamped_meeting_id(marker: &serde_json::Value) -> Option<&str> {
    marker_id(marker, "meeting_id")
}

fn marker_id<'a>(marker: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    let id = marker.get(field)?.as_str()?;
    is_stable_recording_id(id).then_some(id)
}

pub(crate) fn is_stable_recording_id(id: &str) -> bool {
    id.strip_prefix("meeting-").is_some_and(|raw| {
        uuid::Uuid::parse_str(raw).is_ok_and(|uuid| uuid.hyphenated().to_string() == raw)
    })
}

pub(crate) fn has_invalid_marker_id(marker: &serde_json::Value) -> bool {
    ["meeting_id", "recording_id"].into_iter().any(|field| {
        marker.get(field).is_some_and(|value| !value.is_null())
            && marker_id(marker, field).is_none()
    }) || matches!(
        (recording_id(marker), stamped_meeting_id(marker)),
        (Some(recording), Some(meeting)) if recording != meeting
    )
}

/// Writes the meeting id into the folder's marker so the folder can later be
/// tied to exactly this meeting. A marker that already names a meeting is left.
pub(crate) fn stamp_meeting_id(folder: &Path, meeting_id: &str) -> Result<(), String> {
    let Some(mut marker) = read_marker(folder) else {
        return Err("The recording marker is missing".into());
    };
    if let Some(existing) = marker.get("meeting_id").and_then(|value| value.as_str()) {
        return if existing == meeting_id {
            Ok(())
        } else {
            Err("The recording marker already belongs to a meeting".into())
        };
    }
    marker["meeting_id"] = serde_json::json!(meeting_id);
    let json = serde_json::to_vec_pretty(&marker).map_err(|_| "Could not write the marker")?;
    crate::library::atomic_write(&folder.join("metadata.json"), &json)
}

/// The folder a webview may attach to a new meeting: one this process created,
/// or an existing directory strictly inside a recording root that carries the
/// app's marker (a recording from an earlier run being recovered). Returns the
/// text to store. Refused before any filesystem call for UNC, device and
/// extended-length input.
pub(crate) fn authorize_new_meeting_folder(
    requested: &str,
    roots: &[PathBuf],
) -> Result<String, String> {
    if requested.trim().is_empty() || is_cleanup_path(Path::new(requested)) {
        return Err(REFUSED.into());
    }
    let path = Path::new(requested);
    // A folder created by this process may still be in use after the user
    // changes roots. Check that the directory chain still belongs to it.
    if let Some(text) = core_folder_text(path).filter(|_| {
        directory_chain(path).is_ok()
            && path
                .canonicalize()
                .is_ok_and(|path| !is_cleanup_path(&path))
    }) {
        return Ok(text);
    }
    if !crate::trusted_roots::text_allowed(requested, roots) {
        return Err(REFUSED.into());
    }
    let canonical = resolve_recording_folder(path, roots)?;
    if !has_app_marker(&canonical, None) {
        return Err(REFUSED.into());
    }
    Ok(plain_path(&canonical).to_string_lossy().into_owned())
}

/// The folder to show in the file explorer: an existing directory, not a link,
/// strictly inside a recording root. Network, device and extended-length input
/// is refused before any filesystem call.
pub(crate) fn authorize_folder_to_open(
    requested: &str,
    roots: &[PathBuf],
) -> Result<PathBuf, String> {
    if requested.trim().is_empty() || !crate::trusted_roots::text_allowed(requested, roots) {
        return Err(REFUSED.into());
    }
    resolve_recording_folder(Path::new(requested), roots).map(|canonical| plain_path(&canonical))
}

/// The recording folder the core saved for a meeting. Commands that work on a
/// meeting's files use this instead of a folder the webview names.
pub(crate) async fn registered_meeting_folder(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
    roots: &[PathBuf],
) -> Result<PathBuf, String> {
    registered_meeting_folder_if_present(pool, meeting_id, roots)
        .await?
        .ok_or_else(|| "This meeting has no recording folder".into())
}

/// Preserves the distinction between a meeting with no folder and one with an
/// invalid or unavailable saved folder.
pub(crate) async fn registered_meeting_folder_if_present(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
    roots: &[PathBuf],
) -> Result<Option<PathBuf>, String> {
    let folder: Option<String> =
        sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| "Could not read the meeting")?
            .ok_or("Meeting not found")?;
    let Some(folder) = folder.filter(|path| !path.trim().is_empty()) else {
        return Ok(None);
    };
    let roots = roots.to_vec();
    tokio::task::spawn_blocking(move || {
        // On a local drive, or inside a root the user trusted (a network folder).
        let path = Path::new(&folder);
        if !crate::trusted_roots::text_allowed(&folder, &roots)
            || is_cleanup_path(path)
            || directory_chain(path).is_err()
        {
            return Err("The recording folder is not available".into());
        }
        let resolved = crate::trusted_roots::resolve_allowed(&folder, &roots)?;
        if is_cleanup_path(&resolved) {
            return Err("The recording folder is not available".into());
        }
        Ok(Some(plain_path(&resolved)))
    })
    .await
    .map_err(|_| "Recording folder resolution was interrupted")?
}

/// Removes the recording folder the core registered for `meeting_id`: an
/// existing directory, not a link, strictly inside a recording root, no other
/// meeting's, and carrying this app's own marker for this meeting.
pub(crate) fn remove_recording_folder(
    folder: &Path,
    meeting_id: &str,
    roots: &[PathBuf],
    other_folders: &[PathBuf],
) -> Result<(), String> {
    remove_library_recording_folder(folder, meeting_id, roots, other_folders, false)
        .map_err(|reason| format!("Recording folder kept: {} ({reason})", folder.display()))
}

pub(crate) fn remove_library_recording_folder(
    folder: &Path,
    meeting_id: &str,
    roots: &[PathBuf],
    other_folders: &[PathBuf],
    exact_marker: bool,
) -> Result<(), &'static str> {
    if let Some(folder) =
        verify_recording_deletion(folder, meeting_id, roots, other_folders, exact_marker)?
    {
        let owner = if has_unpublished_import_owner(&folder, meeting_id) {
            crate::audio::import::IMPORT_OWNER_FILE
        } else {
            "metadata.json"
        };
        remove_recording_folder_contents(&folder, owner).map_err(|_| "file deletion failed")?;
    }
    Ok(())
}

pub(crate) fn verify_recording_deletion(
    folder: &Path,
    meeting_id: &str,
    roots: &[PathBuf],
    other_folders: &[PathBuf],
    exact_marker: bool,
) -> Result<Option<PathBuf>, &'static str> {
    if is_cleanup_path(folder)
        || !crate::trusted_roots::text_allowed(&folder.to_string_lossy(), roots)
    {
        return Err("not a recording location");
    }
    match directory_chain(folder) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return if roots.iter().any(|root| {
                crate::local_path::lexically_inside(&folder.to_string_lossy(), root)
                    && directory_chain(root).is_ok()
            }) {
                Ok(None)
            } else {
                Err("the recording location is unavailable")
            };
        }
        Err(_) => return Err("the folder could not be inspected as an unlinked directory"),
    }
    let canonical = resolve_recording_folder(folder, roots)
        .map_err(|_| "outside the configured recording locations")?;
    if contains_recording_root(&canonical, roots) {
        return Err("the folder contains a configured recording location");
    }
    let marker_error = "no ClawScribe recording marker for this meeting";
    let owner_file = folder.join(crate::audio::import::IMPORT_OWNER_FILE);
    let has_import_owner = match std::fs::symlink_metadata(owner_file) {
        Ok(_) if crate::audio::import::owns_import_folder(folder, meeting_id) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        _ => return Err(marker_error),
    };
    // An import may commit before metadata is published. If metadata exists,
    // both ownership records must agree.
    let unpublished = has_import_owner && has_unpublished_import_owner(folder, meeting_id);
    if !unpublished && (exact_marker || has_import_owner) {
        let marker = read_marker(folder).ok_or(marker_error)?;
        let ids = [marker.get("meeting_id"), marker.get("recording_id")];
        let mut present = ids.into_iter().flatten().filter(|value| !value.is_null());
        if present.next().and_then(serde_json::Value::as_str) != Some(meeting_id)
            || present.any(|id| id.as_str() != Some(meeting_id))
        {
            return Err(marker_error);
        }
    } else if !unpublished && !has_app_marker(folder, Some(meeting_id)) {
        return Err(marker_error);
    }
    let compare = |a: &Path, b: &Path| {
        let a = folder_key(a);
        let b = folder_key(b);
        a == b
            || a.strip_prefix(&b).is_some_and(|rest| rest.starts_with('/'))
            || b.strip_prefix(&a).is_some_and(|rest| rest.starts_with('/'))
    };
    if other_folders.iter().any(|other| {
        !other.as_os_str().is_empty()
            && (compare(other, folder)
                || (crate::trusted_roots::text_allowed(&other.to_string_lossy(), roots)
                    && other
                        .canonicalize()
                        .is_ok_and(|other| compare(&other, &canonical))))
    }) {
        return Err("another meeting uses this folder");
    }
    Ok(Some(canonical))
}

pub(crate) fn contains_recording_root(canonical: &Path, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .filter(|root| directory_chain(root).is_ok())
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| root.starts_with(canonical))
}

fn remove_recording_folder_contents(folder: &Path, owner: &str) -> std::io::Result<()> {
    // Keep the ownership marker while removing children, so a file held open
    // during this attempt can be retried after its handle closes.
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == owner || (cfg!(windows) && name.eq_ignore_ascii_case(owner)) {
            continue;
        }
        let kind = entry.file_type()?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileTypeExt;
            if kind.is_symlink_dir() {
                std::fs::remove_dir(entry.path())?;
                continue;
            }
        }
        if kind.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    std::fs::remove_file(folder.join(owner))?;
    std::fs::remove_dir(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletion_accepts_an_unpublished_import_only_with_exact_unshared_ownership() {
        let base = tempfile::tempdir().unwrap();
        let roots = [base.path().to_path_buf()];
        let folder = base.path().join("imported");
        std::fs::create_dir(&folder).unwrap();
        let id = "meeting-4f7b980e-b123-4ca9-82e7-927204456483";
        let owner = folder.join(crate::audio::import::IMPORT_OWNER_FILE);
        std::fs::write(&owner, id).unwrap();
        for exact in [false, true] {
            assert!(verify_recording_deletion(&folder, id, &roots, &[], exact)
                .unwrap()
                .is_some());
            assert!(verify_recording_deletion(&folder, "foreign", &roots, &[], exact).is_err());
            assert!(verify_recording_deletion(
                &folder,
                id,
                &roots,
                &[folder.join("nested")],
                exact
            )
            .is_err());
            assert!(verify_recording_deletion(&folder, id, &[folder.clone()], &[], exact).is_err());
        }
        std::fs::write(folder.join("metadata.json"), b"foreign metadata").unwrap();
        assert!(verify_recording_deletion(&folder, id, &roots, &[], true).is_err());
        std::fs::remove_file(folder.join("metadata.json")).unwrap();
        remove_library_recording_folder(&folder, id, &roots, &[], true).unwrap();
        assert!(!folder.exists());
    }

    #[cfg(windows)]
    #[test]
    fn only_cloud_files_reparse_tags_are_allowed() {
        for variant in 0..=15 {
            assert!(is_cloud_tag(0x9000_001a | (variant << 12)));
        }
        for tag in [0, 0xa000_0003, 0xa000_000c, 0xb000_701a, 0x8000_0021] {
            assert!(!is_cloud_tag(tag));
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires a Cloud Files placeholder on the designated Windows runner"]
    fn windows_cloud_placeholder_metadata_uses_the_cloud_tag_without_being_a_symlink() {
        let path = PathBuf::from(
            std::env::var_os("CLAWSCRIBE_TEST_CLOUD_PLACEHOLDER")
                .expect("set the Cloud Files placeholder fixture path"),
        );
        let metadata = std::fs::symlink_metadata(&path).expect("inspect placeholder metadata");
        assert!(is_cloud_tag(
            reparse_tag(&path).expect("read the Cloud Files tag")
        ));
        assert!(!metadata.file_type().is_symlink());
        assert!(!is_link(&path, &metadata));
    }

    #[test]
    fn rejects_conflicting_capture_and_meeting_ids() {
        let id = "meeting-4f7b980e-b123-4ca9-82e7-927204456483";
        let mut marker = serde_json::json!({"recording_id": id});
        assert!(!has_invalid_marker_id(&marker));
        marker["meeting_id"] = serde_json::json!(id);
        assert!(!has_invalid_marker_id(&marker));
        marker["meeting_id"] = serde_json::json!("meeting-3a95bb87-432e-4ec8-8122-92ff46ed794d");
        assert!(has_invalid_marker_id(&marker));
    }

    fn write_marker(folder: &Path, meeting_id: Option<&str>) {
        let mut marker = serde_json::json!({
            "version": "1.0", "meeting_name": "Synthetic", "transcript_file": "transcripts.json",
            "status": "completed"
        });
        if let Some(id) = meeting_id {
            marker["meeting_id"] = serde_json::json!(id);
        }
        std::fs::write(folder.join("metadata.json"), marker.to_string()).unwrap();
    }

    #[test]
    fn previous_default_root_remains_allowed_after_save_folder_changes() {
        let base = tempfile::tempdir().unwrap();
        let default = base.path().join("default");
        let current = base.path().join("current");
        let folder = default.join("meeting");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir(&current).unwrap();
        write_marker(&folder, Some("meeting-1"));
        remove_recording_folder(&folder, "meeting-1", &[current, default], &[]).unwrap();
        assert!(!folder.exists());
    }

    #[test]
    fn a_folder_strictly_inside_a_root_resolves_to_its_canonical_path() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let folder = root.join("meeting");
        std::fs::create_dir_all(&folder).unwrap();
        let other_root = base.path().join("elsewhere");
        std::fs::create_dir(&other_root).unwrap();
        let resolved = resolve_recording_folder(&folder, &[other_root, root.clone()]).unwrap();
        assert_eq!(resolved, folder.canonicalize().unwrap());
    }

    #[test]
    fn traversal_outside_missing_and_root_itself_are_refused_without_naming_the_path() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let folder = root.join("meeting");
        let outside = base.path().join("outside");
        let sibling = base.path().join("recordings-evil");
        for dir in [&folder, &outside, &sibling] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(root.join("file.txt"), b"x").unwrap();
        let roots = [root.clone()];
        let traversal = folder.join("..").join("..").join("outside");
        for refused in [
            &traversal,
            &outside,
            &sibling,
            &root,
            &root.join("missing"),
            &root.join("file.txt"),
            &PathBuf::new(),
        ] {
            let error = resolve_recording_folder(refused, &roots).unwrap_err();
            assert_eq!(error, "This folder is not one of your recordings.");
        }
        // The same traversal that stays inside is fine.
        assert!(resolve_recording_folder(&folder.join("..").join("meeting"), &roots).is_ok());
        // No configured root means nothing is allowed.
        assert!(resolve_recording_folder(&folder, &[]).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_that_leaves_the_root_is_refused() {
        use std::os::windows::process::CommandExt;
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let outside = base.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let junction = root.join("looks-inside");
        let result = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(result.status.success());
        let metadata = std::fs::symlink_metadata(&junction).unwrap();
        assert!(metadata.file_type().is_symlink());
        assert_eq!(reparse_tag(&junction).unwrap(), 0xa000_0003);
        assert!(is_link(&junction, &metadata));
        assert!(resolve_recording_folder(&junction, &[root]).is_err());
        std::fs::remove_dir(&junction).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn junction_and_junction_ancestor_are_refused() {
        use std::os::windows::process::CommandExt;
        let base = tempfile::tempdir().unwrap();
        let target = base.path().join("original");
        let folder = target.join("meeting");
        std::fs::create_dir_all(&folder).unwrap();
        write_marker(&folder, Some("meeting-1"));
        write_marker(&target, Some("meeting-1"));
        let junction = base.path().join("junction");
        let result = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(result.status.success());
        let roots = [base.path().to_path_buf()];
        assert!(
            authorize_folder_to_open(&junction.join("meeting").to_string_lossy(), &roots).is_err()
        );
        assert!(
            authorize_new_meeting_folder(&junction.join("meeting").to_string_lossy(), &roots)
                .is_err()
        );
        assert!(resolve_recording_folder(&folder, &[junction.clone()]).is_err());
        assert!(remove_recording_folder(&junction, "meeting-1", &roots, &[]).is_err());
        assert!(
            remove_recording_folder(&junction.join("meeting"), "meeting-1", &roots, &[]).is_err()
        );
        assert!(folder.join("metadata.json").exists());
        std::fs::remove_dir(&junction).unwrap();
    }

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
                write_marker(&folder, Some("meeting-1"));
            }
            let others = if case == "shared" {
                vec![folder.clone()]
            } else {
                vec![]
            };
            let result = remove_recording_folder(&folder, "meeting-1", &[root.clone()], &others);
            if case == "normal" {
                assert!(result.is_ok());
                assert!(!folder.exists());
            } else {
                assert!(result.unwrap_err().contains(&folder.display().to_string()));
                assert!(folder.join("audio.wav").exists());
            }
        }
        assert!(remove_recording_folder(&root, "meeting-1", &[root.clone()], &[]).is_err());
    }

    #[test]
    fn a_generic_file_a_foreign_marker_or_an_outcome_file_alone_never_allow_deletion() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        std::fs::create_dir(&root).unwrap();
        let cases: [(&str, Option<&[u8]>, Option<&str>); 5] = [
            ("empty-object", Some(b"{}"), None),
            ("not-json", Some(b"plain text"), None),
            ("outcome-only", None, None),
            ("other-meeting", None, Some("meeting-2")),
            ("directory-named-like-marker", None, None),
        ];
        for (name, content, other_id) in cases {
            let folder = root.join(name);
            std::fs::create_dir(&folder).unwrap();
            std::fs::write(folder.join("audio.wav"), b"synthetic audio").unwrap();
            match (content, other_id, name) {
                (Some(bytes), _, _) => std::fs::write(folder.join("metadata.json"), bytes).unwrap(),
                (None, Some(id), _) => write_marker(&folder, Some(id)),
                (None, None, "outcome-only") => {
                    std::fs::write(folder.join("recording-outcome.json"), b"{}").unwrap()
                }
                _ => std::fs::create_dir(folder.join("metadata.json")).unwrap(),
            }
            let error =
                remove_recording_folder(&folder, "meeting-1", &[root.clone()], &[]).unwrap_err();
            assert!(error.contains("marker"), "{name}: {error}");
            assert!(folder.join("audio.wav").exists(), "{name} was kept");
        }
    }

    #[test]
    fn a_recording_saved_before_ids_were_written_is_still_removable() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let folder = root.join("older");
        std::fs::create_dir_all(&folder).unwrap();
        write_marker(&folder, None);
        remove_recording_folder(&folder, "meeting-1", &[root], &[]).unwrap();
        assert!(!folder.exists());
    }

    #[test]
    fn unc_device_and_extended_length_text_is_refused_before_any_filesystem_call() {
        for refused in [
            r"\\server\share\x",
            r"\\?\C:\x",
            r"\\.\pipe\x",
            "//host/share/x",
            r"  \\server\x",
        ] {
            assert!(!crate::local_path::is_local_path_text(refused), "{refused}");
            assert!(authorize_folder_to_open(refused, &[PathBuf::from("C:/")]).is_err());
            assert!(authorize_new_meeting_folder(refused, &[PathBuf::from("C:/")]).is_err());
            assert!(
                remove_recording_folder(Path::new(refused), "meeting-1", &[], &[])
                    .unwrap_err()
                    .contains("not a recording location")
            );
        }
        #[cfg(not(windows))]
        assert!(crate::local_path::is_local_path_text("/home/x"));
        #[cfg(windows)]
        {
            assert!(crate::local_path::is_local_path_text(r"C:\Users\x"));
            assert!(!crate::local_path::is_local_path_text("/home/x"));
        }
    }

    #[test]
    fn the_folder_to_open_must_be_an_existing_unlinked_directory_inside_a_root() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let folder = root.join("meeting");
        let outside = base.path().join("outside");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join("file.txt"), b"x").unwrap();
        let roots = [root.clone()];
        let opened = authorize_folder_to_open(&folder.to_string_lossy(), &roots).unwrap();
        assert_eq!(opened, plain_path(&folder.canonicalize().unwrap()));
        for refused in [
            outside.to_string_lossy().into_owned(),
            root.to_string_lossy().into_owned(),
            root.join("missing").to_string_lossy().into_owned(),
            root.join("file.txt").to_string_lossy().into_owned(),
            folder
                .join("..")
                .join("..")
                .join("outside")
                .to_string_lossy()
                .into_owned(),
            String::new(),
        ] {
            assert_eq!(
                authorize_folder_to_open(&refused, &roots).unwrap_err(),
                REFUSED
            );
        }
        assert!(authorize_folder_to_open(&folder.to_string_lossy(), &[]).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_is_never_opened_even_when_it_points_inside_the_root() {
        use std::os::windows::process::CommandExt;
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let junction = root.join("link");
        let result = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&real)
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(result.status.success());
        // A link is never opened, even when it points inside the root.
        assert_eq!(
            authorize_folder_to_open(&junction.to_string_lossy(), &[root]).unwrap_err(),
            REFUSED
        );
        std::fs::remove_dir(&junction).unwrap();
    }

    #[test]
    fn a_new_meeting_may_only_name_a_core_created_folder_or_a_marked_one_in_a_root() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("recordings");
        let created = root.join("created");
        let recovered = root.join("recovered");
        let unmarked = root.join("unmarked");
        let outside = base.path().join("outside");
        for dir in [&created, &recovered, &unmarked, &outside] {
            std::fs::create_dir_all(dir).unwrap();
        }
        write_marker(&recovered, None);
        write_marker(&outside, None);
        let roots = [root.clone()];

        // Not created by the core and unmarked: refused.
        assert!(authorize_new_meeting_folder(&created.to_string_lossy(), &roots).is_err());
        register_core_folder(&created);
        assert_eq!(
            authorize_new_meeting_folder(&created.to_string_lossy(), &roots).unwrap(),
            created.to_string_lossy()
        );
        // The same folder spelled differently is the core's folder, stored as the core wrote it.
        let spelled = format!("{}{}", created.to_string_lossy(), std::path::MAIN_SEPARATOR);
        assert_eq!(
            authorize_new_meeting_folder(&spelled, &roots).unwrap(),
            created.to_string_lossy()
        );
        // A marked folder in a root (recovery after a restart) is accepted.
        assert!(authorize_new_meeting_folder(&recovered.to_string_lossy(), &roots).is_ok());
        // Unmarked, outside every root, the root itself, missing, empty: refused.
        for refused in [
            unmarked.to_string_lossy().into_owned(),
            outside.to_string_lossy().into_owned(),
            root.to_string_lossy().into_owned(),
            root.join("missing").to_string_lossy().into_owned(),
            String::new(),
        ] {
            assert_eq!(
                authorize_new_meeting_folder(&refused, &roots).unwrap_err(),
                REFUSED
            );
        }
        assert!(authorize_new_meeting_folder(&recovered.to_string_lossy(), &[]).is_err());
    }

    #[tokio::test]
    async fn a_meetings_folder_is_the_one_the_core_saved_for_it() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let base = tempfile::tempdir().unwrap();
        let folder = base.path().join("meeting");
        std::fs::create_dir_all(&folder).unwrap();
        for (id, path) in [
            ("saved", folder.to_string_lossy().into_owned()),
            ("network", r"\\h\s\meeting".to_string()),
            ("relative", "meeting".to_string()),
            (
                "gone",
                base.path().join("gone").to_string_lossy().into_owned(),
            ),
        ] {
            sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES (?, 't', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?)")
                .bind(id)
                .bind(path)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('no-folder', 't', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            registered_meeting_folder(&pool, "saved", &[])
                .await
                .unwrap(),
            plain_path(&folder.canonicalize().unwrap())
        );
        for refused in [
            "network",
            "relative",
            "gone",
            "no-folder",
            "unknown-meeting",
        ] {
            assert!(
                registered_meeting_folder(&pool, refused, &[])
                    .await
                    .is_err(),
                "{refused}"
            );
        }
    }

    /// A recordings folder on a share: marked as network for the tests, so no
    /// real network is needed.
    fn pretend_share(base: &Path) -> (PathBuf, PathBuf) {
        let share = base.join("share");
        let root = share.join("recordings");
        std::fs::create_dir_all(root.join("meeting")).unwrap();
        std::fs::create_dir_all(share.join("other").join("meeting")).unwrap();
        crate::local_path::mark_remote_for_tests(&share);
        (share, root)
    }

    #[test]
    fn a_trusted_network_root_works_like_a_local_one_for_save_open_recover_and_delete() {
        let base = tempfile::tempdir().unwrap();
        let (share, root) = pretend_share(base.path());
        let meeting = root.join("meeting");
        write_marker(&meeting, None);
        let text = meeting.to_string_lossy().into_owned();
        let roots = [root.clone()];
        assert!(
            !crate::local_path::is_local_path_text(&text),
            "the test root reads as network"
        );

        // Save and recover: a marked folder in the root is accepted (the saved text is the folder's own).
        let saved = authorize_new_meeting_folder(&text, &roots).unwrap();
        assert!(Path::new(&saved).ends_with("meeting"));
        // Open.
        assert!(authorize_folder_to_open(&text, &roots).is_ok());
        assert!(resolve_recording_folder(&meeting, &roots).is_ok());
        // A folder the core created itself is accepted first, even with no root known.
        let created = root.join("created-by-the-core");
        std::fs::create_dir_all(&created).unwrap();
        register_core_folder(&created);
        assert!(authorize_new_meeting_folder(&created.to_string_lossy(), &[]).is_ok());
        // Delete.
        write_marker(&meeting, Some("meeting-1"));
        remove_recording_folder(&meeting, "meeting-1", &roots, &[]).unwrap();
        assert!(!meeting.exists());
        let _ = share;
    }

    #[test]
    fn network_text_outside_the_trusted_root_or_without_any_root_is_refused_before_the_disk_is_asked(
    ) {
        let base = tempfile::tempdir().unwrap();
        let (share, root) = pretend_share(base.path());
        let other = share.join("other").join("meeting");
        write_marker(&other, None);
        write_marker(&root.join("meeting"), None);
        let roots = [root.clone()];
        let outside = other.to_string_lossy().into_owned();
        let sneaky = format!("{}/../other/meeting", root.to_string_lossy());
        for text in [&outside, &sneaky] {
            assert!(authorize_new_meeting_folder(text, &roots).is_err());
            assert!(authorize_folder_to_open(text, &roots).is_err());
            assert!(resolve_recording_folder(Path::new(text), &roots).is_err());
            assert!(remove_recording_folder(Path::new(text), "meeting-1", &roots, &[]).is_err());
        }
        // With no trusted root the same folder is refused: the place is not trusted on its own.
        let inside = root.join("meeting").to_string_lossy().into_owned();
        assert!(authorize_new_meeting_folder(&inside, &[]).is_err());
        assert!(authorize_folder_to_open(&inside, &[]).is_err());
        assert!(other.exists(), "nothing was deleted");
    }

    #[tokio::test]
    async fn a_meeting_in_a_trusted_network_root_resolves_and_one_outside_it_does_not() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let base = tempfile::tempdir().unwrap();
        let (share, root) = pretend_share(base.path());
        for (id, path) in [
            (
                "inside",
                root.join("meeting").to_string_lossy().into_owned(),
            ),
            (
                "outside",
                share
                    .join("other")
                    .join("meeting")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ] {
            sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES (?, 't', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?)")
                .bind(id)
                .bind(path)
                .execute(&pool)
                .await
                .unwrap();
        }
        let roots = [root.clone()];
        assert!(registered_meeting_folder(&pool, "inside", &roots)
            .await
            .is_ok());
        assert!(registered_meeting_folder(&pool, "outside", &roots)
            .await
            .is_err());
        assert!(registered_meeting_folder(&pool, "inside", &[])
            .await
            .is_err());
    }

    #[test]
    fn playback_lookups_through_the_shared_folder_helper_follow_the_trusted_roots() {
        let base = tempfile::tempdir().unwrap();
        let (_share, root) = pretend_share(base.path());
        let meeting = root.join("meeting");
        crate::trusted_roots::set_snapshot(vec![root.clone()]);
        let found = crate::canonicalize_existing_dir(&meeting).unwrap();
        assert!(
            found.is_some(),
            "a folder in a trusted network root is found"
        );
        crate::trusted_roots::set_snapshot(vec![]);
        assert!(
            crate::canonicalize_existing_dir(&meeting)
                .unwrap()
                .is_none(),
            "without the root it is not looked at"
        );
    }

    #[test]
    fn stamping_writes_the_meeting_id_once() {
        let base = tempfile::tempdir().unwrap();
        write_marker(base.path(), None);
        stamp_meeting_id(base.path(), "meeting-1").unwrap();
        assert!(has_app_marker(base.path(), Some("meeting-1")));
        assert!(!has_app_marker(base.path(), Some("meeting-2")));
        assert!(stamp_meeting_id(base.path(), "meeting-2").is_err());
        assert!(
            has_app_marker(base.path(), Some("meeting-1")),
            "an existing id is kept"
        );
        assert!(stamp_meeting_id(&base.path().join("missing"), "meeting-1").is_err());
    }
}
