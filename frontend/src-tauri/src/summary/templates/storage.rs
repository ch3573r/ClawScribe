use super::{
    get_template,
    loader::{get_custom_templates_dir, get_legacy_custom_templates_dir},
    Template,
};
use std::{
    io::{ErrorKind, Write},
    path::Path,
};

pub(crate) const REMOVED_TEMPLATE: &str = "{\"clawscribe_template_removed\":true}";

pub(crate) fn validate_id(id: &str) -> Result<(), String> {
    let stem = id
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    let numbered_device = ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(*prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "0" | "1"
                    | "2"
                    | "3"
                    | "4"
                    | "5"
                    | "6"
                    | "7"
                    | "8"
                    | "9"
                    | "\u{00b9}"
                    | "\u{00b2}"
                    | "\u{00b3}"
            )
        })
    });
    if id.is_empty()
        || id.len() > 120
        || id.trim() != id
        || id.ends_with('.')
        || id.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
        || matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        )
        || numbered_device
    {
        return Err("Invalid template identifier".into());
    }
    Ok(())
}

pub fn save_template(id: &str, template: &Template) -> Result<(), String> {
    let _guard = crate::library::backup::PORTABLE_SETTINGS_LOCK
        .try_lock()
        .map_err(|_| "Library settings are in use. Try again.")?;
    let directory = get_custom_templates_dir().ok_or("Template storage is unavailable")?;
    save_in(&directory, id, template)
}

pub fn remove_template(id: &str) -> Result<(), String> {
    let _guard = crate::library::backup::PORTABLE_SETTINGS_LOCK
        .try_lock()
        .map_err(|_| "Library settings are in use. Try again.")?;
    let directory = get_custom_templates_dir().ok_or("Template storage is unavailable")?;
    remove_in(&directory, get_legacy_custom_templates_dir().as_deref(), id)
}

fn check_directory(directory: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err("Template storage is a redirected directory".into());
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        _ => Err("Template storage is not an accessible directory".into()),
    }
}

fn remove_in(directory: &Path, legacy: Option<&Path>, id: &str) -> Result<(), String> {
    validate_id(id)?;
    check_directory(directory)?;
    let filename = format!("{id}.json");
    if let Some(legacy) = legacy {
        match std::fs::metadata(legacy.join(&filename)) {
            // A marker atomically replaces any personal override while leaving
            // the other application's original file intact.
            Ok(metadata) if metadata.is_file() => {
                return write_in(directory, &filename, REMOVED_TEMPLATE.as_bytes())
            }
            Err(error) if error.kind() != ErrorKind::NotFound => {
                return Err("Could not check legacy template storage".into())
            }
            _ => {}
        }
    }
    match std::fs::remove_file(directory.join(filename)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Could not remove the personal template".into()),
    }
}

fn save_in(directory: &Path, id: &str, template: &Template) -> Result<(), String> {
    validate_id(id)?;
    template.validate()?;
    let bytes = serde_json::to_vec_pretty(template).map_err(|_| "Could not serialize template")?;
    if bytes.len() > 128 * 1024 {
        return Err("Template must be smaller than 128 KiB".into());
    }
    write_in(directory, &format!("{id}.json"), &bytes)
}

fn write_in(directory: &Path, filename: &str, bytes: &[u8]) -> Result<(), String> {
    check_directory(directory)?;
    std::fs::create_dir_all(directory).map_err(|_| "Could not create template storage")?;
    let mut file =
        tempfile::NamedTempFile::new_in(directory).map_err(|_| "Could not stage template")?;
    file.write_all(bytes)
        .map_err(|_| "Could not write template")?;
    file.as_file()
        .sync_all()
        .map_err(|_| "Could not flush template")?;
    file.persist(directory.join(filename))
        .map_err(|_| "Could not save template")?;
    Ok(())
}

pub fn resolve_default(saved: Option<&str>) -> String {
    saved
        .filter(|id| get_template(id).is_ok())
        .unwrap_or("standard_meeting")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_replaces_and_rejects_invalid_edits_without_losing_template() {
        let directory = tempfile::tempdir().unwrap();
        let mut template = get_template("standard_meeting").unwrap();
        save_in(directory.path(), "custom-review", &template).unwrap();
        template.name = "Project review".into();
        save_in(directory.path(), "custom-review", &template).unwrap();
        let path = directory.path().join("custom-review.json");
        let saved = std::fs::read(&path).unwrap();
        let decoded: Template = serde_json::from_slice(&saved).unwrap();
        assert_eq!(decoded.name, "Project review");
        template.sections.clear();
        assert!(save_in(directory.path(), "custom-review", &template).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved);
    }

    #[test]
    fn rejects_windows_superscript_device_names() {
        for id in ["COM\u{00b9}", "LPT\u{00b2}.notes", "com\u{00b3}"] {
            assert!(validate_id(id).is_err(), "{id}");
        }
    }

    #[test]
    fn rejects_paths_and_recovers_missing_default() {
        for id in ["", "..", "../secret", "a/b", "a\\b", "C:secret", "a\n"] {
            assert!(validate_id(id).is_err());
        }
        assert_eq!(
            resolve_default(Some("missing-template")),
            "standard_meeting"
        );
        assert_eq!(resolve_default(Some("daily_standup")), "daily_standup");
    }

    #[test]
    fn removes_personal_templates_without_touching_bundled_or_legacy_files() {
        let personal = tempfile::tempdir().unwrap();
        let legacy = tempfile::tempdir().unwrap();
        let template = get_template("standard_meeting").unwrap();
        save_in(personal.path(), "custom-review", &template).unwrap();
        save_in(legacy.path(), "standard_meeting", &template).unwrap();
        remove_in(personal.path(), Some(legacy.path()), "custom-review").unwrap();
        assert!(!personal.path().join("custom-review.json").exists());
        assert!(remove_in(personal.path(), Some(legacy.path()), "../standard_meeting").is_err());

        remove_in(personal.path(), Some(legacy.path()), "standard_meeting").unwrap();
        assert!(legacy.path().join("standard_meeting.json").is_file());
        assert_eq!(
            std::fs::read_to_string(personal.path().join("standard_meeting.json")).unwrap(),
            REMOVED_TEMPLATE
        );
        save_in(personal.path(), "standard_meeting", &template).unwrap();
        assert_ne!(
            std::fs::read_to_string(personal.path().join("standard_meeting.json")).unwrap(),
            REMOVED_TEMPLATE
        );
    }
}
