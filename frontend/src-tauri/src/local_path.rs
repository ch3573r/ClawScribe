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
    let root: Vec<u16> = format!("{}:\\", letter.to_ascii_uppercase()).encode_utf16().chain(Some(0)).collect();
    // SAFETY: `root` is a NUL-terminated UTF-16 string that outlives the call.
    unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
}

fn separator(c: char) -> bool {
    c == '\\' || c == '/'
}

/// Whether `path` is a plain local absolute path. Pure text; no filesystem call.
pub(crate) fn is_local_path_text(path: &str) -> bool {
    if path.is_empty() || path.contains('\0') || path.starts_with(char::is_whitespace) {
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

/// For a command that takes a path from the webview: refuse anything that is
/// not a plain local absolute path, before the first filesystem call.
pub(crate) fn check_local(text: &str) -> Result<(), String> {
    if is_local_path_text(text) {
        Ok(())
    } else {
        Err(LOCAL_ONLY.into())
    }
}

/// `check_local`, then resolve the path and require the result to be on a local
/// disk too (a mapped network drive resolves to a UNC path and is refused).
pub(crate) fn resolve_local(text: &str) -> Result<std::path::PathBuf, String> {
    check_local(text)?;
    let resolved = Path::new(text).canonicalize().map_err(|_| LOCAL_ONLY.to_string())?;
    if is_local_canonical(&resolved) {
        Ok(resolved)
    } else {
        Err(LOCAL_ONLY.into())
    }
}

/// Whether a canonicalised path is on a local disk (`\\?\C:\...` on Windows).
pub(crate) fn is_local_canonical(path: &Path) -> bool {
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
