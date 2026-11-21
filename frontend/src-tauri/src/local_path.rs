//! One rule for paths that arrive from the webview.
//!
//! Asking the filesystem about a network path (`exists`, `canonicalize`,
//! `symlink_metadata`) already reaches the remote machine, and running a program
//! from one runs remote code. Rust treats `\/host/share`, `/\host\share`,
//! `\??\UNC\host\share` and `\\?\UNC\host\share` as network or device paths as
//! well, so refusing text that starts with two backslashes is not enough. The
//! rule is positive instead: only a local, absolute path in its plain form is
//! accepted, and it is checked before any filesystem call.
//!
//! * Windows: `X:` (a drive letter) followed by a separator, e.g. `C:\x` or
//!   `C:/x`. Nothing else: not UNC in any mix of slashes, not a device or
//!   verbatim path (`\\?\`, `\\.\`, `\??\`), not drive-relative (`C:x`), not
//!   root-relative (`\x`), not relative.
//! * Elsewhere: an absolute path that does not begin with two separators.
//!
//! A drive letter that Windows reports as a network drive (`GetDriveTypeW`) is
//! refused before any filesystem call; after `canonicalize` the path must still
//! be on a drive (`\\?\C:\...`), so a share that resolves to `\\?\UNC\...` is
//! refused there too. A `subst` of a share that Windows does not report as
//! remote cannot be told apart.
use std::path::{Component, Path, Prefix};

/// Whether a drive letter is a network drive (a mapped share). Asking the
/// mount table does not contact the server, so it is safe before any file call.
/// An unknown or unmapped letter is not reported as remote.
#[cfg(windows)]
fn drive_is_remote(letter: char) -> bool {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDriveTypeW(root_path_name: *const u16) -> u32;
    }
    const DRIVE_REMOTE: u32 = 4;
    let root: Vec<u16> = format!("{}:\\", letter.to_ascii_uppercase())
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: `root` is a NUL-terminated UTF-16 string that outlives the call.
    unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
}

fn separator(c: char) -> bool {
    c == '\\' || c == '/'
}

/// A path as comparable text: both slash directions are one, a verbatim `\\?\`
/// prefix is dropped, letter case does not matter on Windows, and a trailing
/// separator is ignored (a bare root keeps its own).
pub(crate) fn comparable_text(path: &str) -> String {
    let mut text = path.replace('\\', "/");
    if let Some(rest) = text.strip_prefix("//?/") {
        text = match rest.strip_prefix("UNC/") {
            Some(unc) => format!("//{unc}"),
            None => rest.to_string(),
        };
    }
    #[cfg(windows)]
    let text = text.to_ascii_lowercase();
    let trimmed = text.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.ends_with(':') {
        text
    } else {
        trimmed.to_string()
    }
}

/// Whether the text names `root` or something below it, judged by the text
/// alone (no filesystem call): a path that spells a trusted root is the only
/// kind of non-local text that may be looked at, and only without `..`
/// components, so it cannot leave the root by its own spelling.
pub(crate) fn lexically_inside(text: &str, root: &Path) -> bool {
    if text.is_empty() || text.contains('\0') {
        return false;
    }
    let comparable = comparable_text(text);
    if comparable.split('/').any(|part| part == "..") {
        return false;
    }
    let root = comparable_text(&root.to_string_lossy());
    if root.is_empty() {
        return false;
    }
    comparable == root || comparable.starts_with(&format!("{}/", root.trim_end_matches('/')))
}

/// Folders the tests declare to be network locations, so no real network is
/// needed: such a path reads as not local, exactly like a share would.
#[cfg(test)]
static TEST_REMOTE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub(crate) fn mark_remote_for_tests(folder: &Path) {
    TEST_REMOTE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push(comparable_text(&folder.to_string_lossy()));
}

fn marked_remote(text: &str) -> bool {
    #[cfg(test)]
    {
        let comparable = comparable_text(text);
        TEST_REMOTE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .any(|remote| comparable == *remote || comparable.starts_with(&format!("{remote}/")))
    }
    #[cfg(not(test))]
    {
        let _ = text;
        false
    }
}

/// Whether `path` is a plain local absolute path. Pure text; no filesystem call.
pub(crate) fn is_local_path_text(path: &str) -> bool {
    if path.is_empty()
        || path.contains('\0')
        || path.starts_with(char::is_whitespace)
        || marked_remote(path)
    {
        return false;
    }
    let mut chars = path.chars();
    let (first, second) = (chars.next(), chars.next());
    // Two separators at the start, in any mix: UNC or a device path.
    if first.is_some_and(separator) && second.is_some_and(separator) {
        return false;
    }
    #[cfg(windows)]
    {
        let third = chars.next();
        let drive_form = first.is_some_and(|c| c.is_ascii_alphabetic())
            && second == Some(':')
            && third.is_some_and(separator);
        if !drive_form || first.is_some_and(drive_is_remote) {
            return false;
        }
        let mut components = Path::new(path).components();
        matches!(
            (components.next(), components.next()),
            (Some(Component::Prefix(prefix)), Some(Component::RootDir))
                if matches!(prefix.kind(), Prefix::Disk(_))
        )
    }
    #[cfg(not(windows))]
    {
        let _ = (second, Component::RootDir, Prefix::Disk);
        first == Some('/')
    }
}

/// The single argument handed to the Windows file explorer for a folder: the
/// path in double quotes, so a comma, a space or a semicolon (a meeting title
/// can contain any) cannot split it into options, as explorer reads `,` as a
/// separator. A path that contains a quote is refused.
pub(crate) fn explorer_argument(folder: &Path) -> Result<String, String> {
    let text = folder.to_string_lossy();
    if text.contains('"') || text.contains('\0') {
        return Err("The folder cannot be opened".into());
    }
    Ok(format!("\"{text}\""))
}

/// Opens a folder in the file explorer with its path passed as one quoted argument.
#[cfg(windows)]
pub(crate) fn open_in_explorer(folder: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("explorer")
        .raw_arg(explorer_argument(folder)?)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Failed to open folder: {error}"))
}

/// What a refused path says. It never contains the path.
pub(crate) const LOCAL_ONLY: &str = "Choose a file or folder on a local drive.";

/// Whether a canonicalised path is on a local disk (`\\?\C:\...` on Windows).
pub(crate) fn is_local_canonical(path: &Path) -> bool {
    if marked_remote(&path.to_string_lossy()) {
        return false;
    }
    #[cfg(windows)]
    {
        matches!(
            path.components().next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::VerbatimDisk(_) | Prefix::Disk(_))
        )
    }
    #[cfg(not(windows))]
    {
        let _ = (path, Component::RootDir, Prefix::Disk);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_device_and_relative_forms_are_refused_in_every_slash_mix() {
        for refused in [
            r"\\h\s\x",
            r"\/h/s",
            r"/\h\s\x",
            "//h/s/x",
            r"\??\UNC\h\s",
            r"\\?\C:\x",
            r"\\?\UNC\h\s\x",
            r"\\.\pipe\x",
            "C:x",
            r"C:",
            r"\x",
            "x",
            r"relative\path",
            "",
            " C:\\x",
            "C:\\x\0y",
        ] {
            assert!(!is_local_path_text(refused), "{refused:?}");
        }
    }

    #[test]
    fn a_path_is_lexically_inside_a_root_only_by_its_own_spelling() {
        let root = Path::new(r"\\server\share\Recordings");
        for inside in [
            r"\\server\share\Recordings\Weekly",
            r"//SERVER/Share/recordings/weekly/",
            r"\\?\UNC\server\share\Recordings\a\b",
            r"\\server\share\Recordings",
        ] {
            assert!(lexically_inside(inside, root), "{inside}");
        }
        for outside in [
            r"\\server\share\Recordings-evil",
            r"\\server\share\Other",
            r"\\other\share\Recordings\x",
            r"\\server\share\Recordings\..\Other",
            r"\\server\share\Recordings\a\..\..\x",
            r"\\server\share",
            "",
        ] {
            assert!(!lexically_inside(outside, root), "{outside}");
        }
    }

    #[test]
    fn a_folder_the_tests_mark_as_network_reads_as_not_local() {
        let base = tempfile::tempdir().unwrap();
        let share = base.path().join("pretend-share");
        std::fs::create_dir_all(&share).unwrap();
        assert!(is_local_path_text(&share.to_string_lossy()));
        mark_remote_for_tests(&share);
        assert!(!is_local_path_text(&share.to_string_lossy()));
        assert!(!is_local_path_text(&share.join("inner").to_string_lossy()));
        assert!(!is_local_canonical(&share.canonicalize().unwrap()));
        assert!(
            is_local_path_text(&base.path().join("pretend-share-not").to_string_lossy()),
            "only below the marked folder"
        );
    }

    #[test]
    fn the_explorer_argument_is_one_quoted_path_even_with_commas_and_spaces() {
        assert_eq!(
            explorer_argument(Path::new(r"C:\Rec\Weekly, sync; Q3 notes_2026-11-01")).unwrap(),
            r#""C:\Rec\Weekly, sync; Q3 notes_2026-11-01""#
        );
        assert!(explorer_argument(Path::new(r#"C:\Rec\a"b"#)).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_plain_drive_path_is_accepted() {
        for accepted in [r"C:\x", "C:/x", r"d:\Recordings\Weekly 1", r"C:\", "c:/"] {
            assert!(is_local_path_text(accepted), "{accepted:?}");
        }
        assert!(is_local_canonical(Path::new(r"\\?\C:\x")));
        // The system drive is a local one; an unmapped letter is not called remote.
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        assert!(!drive_is_remote(system.chars().next().unwrap()));
        assert!(!is_local_canonical(Path::new(r"\\?\UNC\h\s\x")));
        assert!(!is_local_canonical(Path::new(r"\\h\s\x")));
    }

    #[cfg(not(windows))]
    #[test]
    fn a_plain_absolute_path_is_accepted() {
        assert!(is_local_path_text("/home/x"));
        assert!(!is_local_path_text("//h/s"));
        assert!(is_local_canonical(Path::new("/home/x")));
    }
}
