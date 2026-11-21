use super::{defaults, storage, types::Template};
use once_cell::sync::Lazy;
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use tracing::warn;

static BUNDLED_TEMPLATES_DIR: Lazy<RwLock<Option<PathBuf>>> = Lazy::new(|| RwLock::new(None));
// Keep existing defaults, overrides, series settings and summary provenance addressable.
const PSYCHIATRIC_ID: &str = "psychatric_session";
const PSYCHIATRIC_FILENAME: &str = "psychiatric_session.json";

#[derive(Serialize)]
pub struct TemplateInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub personal: bool,
    pub bundled: bool,
}

pub fn set_bundled_templates_dir(path: PathBuf) {
    if let Ok(mut dir) = BUNDLED_TEMPLATES_DIR.write() {
        *dir = Some(path);
    }
}

pub(super) fn get_custom_templates_dir() -> Option<PathBuf> {
    if let Some(root) = crate::database::recovery::data_dir() {
        return Some(root.join("templates"));
    }
    Some(dirs::data_dir()?.join("ClawScribe").join("templates"))
}

pub(super) fn get_legacy_custom_templates_dir() -> Option<PathBuf> {
    if crate::database::recovery::data_dir().is_some() {
        return None;
    }
    Some(dirs::data_dir()?.join("Meetily").join("templates"))
}

pub(crate) fn custom_template_dirs() -> Vec<PathBuf> {
    [
        get_custom_templates_dir(),
        get_legacy_custom_templates_dir(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn bundled_dir() -> Option<PathBuf> {
    BUNDLED_TEMPLATES_DIR.read().ok()?.clone()
}

fn read_template(directory: &Path, filename: &str, source: &str) -> Option<String> {
    match std::fs::read_to_string(directory.join(filename)) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            warn!(source, kind = ?error.kind(), "Could not read template file");
            None
        }
    }
}

fn read_bundled(directory: &Path, id: &str) -> Option<String> {
    if id == PSYCHIATRIC_ID {
        if let Some(template) = read_template(directory, PSYCHIATRIC_FILENAME, "bundled") {
            return Some(template);
        }
    } else if id == "psychiatric_session" {
        // The corrected filename is a resource only. A personal template
        // that already uses this ID still resolves independently.
        return None;
    }
    read_template(directory, &format!("{id}.json"), "bundled")
}

fn read_personal(id: &str, personal: Option<&Path>, legacy: Option<&Path>) -> Option<String> {
    let filename = format!("{id}.json");
    match personal.and_then(|path| read_template(path, &filename, "personal")) {
        Some(content) if content == storage::REMOVED_TEMPLATE => None,
        Some(content) => Some(content),
        None => legacy.and_then(|path| read_template(path, &filename, "legacy")),
    }
}

pub fn get_template(id: &str) -> Result<Template, String> {
    storage::validate_id(id)?;
    let personal = read_personal(
        id,
        get_custom_templates_dir().as_deref(),
        get_legacy_custom_templates_dir().as_deref(),
    );
    let bundled = || {
        bundled_dir()
            .as_deref()
            .and_then(|path| read_bundled(path, id))
    };
    let content = personal
        .or_else(bundled)
        .or_else(|| defaults::get_builtin_template(id).map(str::to_owned))
        .ok_or("Template is no longer available")?;
    validate_and_parse_template(&content)
}

pub fn validate_and_parse_template(content: &str) -> Result<Template, String> {
    let template: Template = serde_json::from_str(content).map_err(|error| {
        format!(
            "Could not parse template JSON at line {}, column {}",
            error.line(),
            error.column()
        )
    })?;
    template.validate()?;
    Ok(template)
}

fn ids_in(directory: Option<&Path>, bundled: bool) -> Vec<String> {
    let Some(directory) = directory else {
        return Vec::new();
    };
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            warn!(bundled, kind = ?error.kind(), "Could not list templates");
            return Vec::new();
        }
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let filename = entry.file_name();
            let filename = filename.to_str()?;
            let id = if bundled && filename == PSYCHIATRIC_FILENAME {
                PSYCHIATRIC_ID
            } else {
                filename.strip_suffix(".json")?
            };
            storage::validate_id(id).ok()?;
            Some(id.to_owned())
        })
        .collect()
}

pub fn list_templates() -> Vec<TemplateInfo> {
    let bundled = bundled_dir();
    let personal = get_custom_templates_dir();
    let legacy = get_legacy_custom_templates_dir();
    let mut bundled_ids: BTreeSet<_> = ids_in(bundled.as_deref(), true).into_iter().collect();
    bundled_ids.extend(
        defaults::list_builtin_template_ids()
            .into_iter()
            .map(str::to_owned),
    );
    let mut ids = bundled_ids.clone();
    ids.extend(ids_in(personal.as_deref(), false));
    ids.extend(ids_in(legacy.as_deref(), false));
    ids.into_iter()
        .filter_map(|id| {
            let personal_content = read_personal(&id, personal.as_deref(), legacy.as_deref());
            let is_personal = personal_content.is_some();
            let is_bundled = bundled_ids.contains(&id);
            let content = personal_content
                .or_else(|| bundled.as_deref().and_then(|path| read_bundled(path, &id)))
                .or_else(|| defaults::get_builtin_template(&id).map(str::to_owned))?;
            match validate_and_parse_template(&content) {
                Ok(template) => Some(TemplateInfo {
                    id,
                    name: template.name,
                    description: template.description,
                    personal: is_personal,
                    bundled: is_bundled,
                }),
                Err(_) => {
                    warn!(
                        personal = is_personal,
                        bundled = is_bundled,
                        "Ignored an invalid summary template"
                    );
                    None
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrected_bundled_filename_uses_the_shipped_id() {
        let dir = tempfile::tempdir().unwrap();
        let content = include_str!("../../../templates/psychiatric_session.json");
        std::fs::write(dir.path().join(PSYCHIATRIC_FILENAME), content).unwrap();
        assert_eq!(ids_in(Some(dir.path()), true), [PSYCHIATRIC_ID]);
        assert_eq!(
            read_bundled(dir.path(), PSYCHIATRIC_ID).as_deref(),
            Some(content)
        );
        assert!(read_bundled(dir.path(), "psychiatric_session").is_none());
        assert!(validate_and_parse_template(content).is_ok());
    }

    #[test]
    fn removing_a_legacy_override_restores_the_bundled_fallback() {
        let personal = tempfile::tempdir().unwrap();
        let legacy = tempfile::tempdir().unwrap();
        let filename = "standard_meeting.json";
        std::fs::write(legacy.path().join(filename), defaults::STANDARD_MEETING).unwrap();
        assert!(read_personal(
            "standard_meeting",
            Some(personal.path()),
            Some(legacy.path())
        )
        .is_some());
        std::fs::write(personal.path().join(filename), storage::REMOVED_TEMPLATE).unwrap();
        assert!(read_personal(
            "standard_meeting",
            Some(personal.path()),
            Some(legacy.path())
        )
        .is_none());
    }
}
