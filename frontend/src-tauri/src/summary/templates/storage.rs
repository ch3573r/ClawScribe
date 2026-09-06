use super::{get_template, loader::get_custom_templates_dir, Template};
use std::{io::Write, path::Path};

pub(super) fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 120
        || id.trim() != id
        || id.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
        || matches!(id, "." | "..")
    {
        return Err("Invalid template identifier".into());
    }
    Ok(())
}

pub fn save_template(id: &str, template: &Template) -> Result<(), String> {
    let directory = get_custom_templates_dir().ok_or("Template storage is unavailable")?;
    save_in(&directory, id, template)
}

fn save_in(directory: &Path, id: &str, template: &Template) -> Result<(), String> {
    validate_id(id)?;
    template.validate()?;
    let bytes = serde_json::to_vec_pretty(template).map_err(|_| "Could not serialize template")?;
    if bytes.len() > 128 * 1024 {
        return Err("Template must be smaller than 128 KiB".into());
    }
    std::fs::create_dir_all(directory).map_err(|_| "Could not create template storage")?;
    let mut file =
        tempfile::NamedTempFile::new_in(directory).map_err(|_| "Could not stage template")?;
    file.write_all(&bytes)
        .map_err(|_| "Could not write template")?;
    file.as_file()
        .sync_all()
        .map_err(|_| "Could not flush template")?;
    file.persist(directory.join(format!("{id}.json")))
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
}
