use crate::summary::templates;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime};
use tauri_plugin_store::StoreExt;
use tracing::{info, warn};

/// Template metadata for UI display
#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateInfo {
    /// Template identifier (e.g., "daily_standup", "standard_meeting")
    pub id: String,

    /// Display name for the template
    pub name: String,

    /// Brief description of the template's purpose
    pub description: String,
}

/// Detailed template structure for preview/debugging
#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateDetails {
    /// Template identifier
    pub id: String,

    /// Display name
    pub name: String,

    /// Description
    pub description: String,

    /// List of section titles in order
    pub sections: Vec<String>,
}

pub fn default_template<R: Runtime>(app: &tauri::AppHandle<R>) -> Result<String, String> {
    let store = app
        .store("summary_preferences.json")
        .map_err(|_| "Could not load summary preferences")?;
    let saved = store.get("default_template");
    Ok(templates::resolve_default(
        saved.as_ref().and_then(|v| v.as_str()),
    ))
}

#[tauri::command]
pub async fn api_get_default_template<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || default_template(&app))
        .await
        .map_err(|_| "Could not load summary preferences")?
}

#[tauri::command]
pub async fn api_save_default_template<R: Runtime>(
    app: tauri::AppHandle<R>,
    template_id: String,
) -> Result<(), String> {
    let worker_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        templates::get_template(&template_id)?;
        let store = worker_app
            .store("summary_preferences.json")
            .map_err(|_| "Could not load summary preferences")?;
        let previous = store.get("default_template");
        store.set("default_template", serde_json::json!(template_id));
        if store.save().is_err() {
            match previous {
                Some(value) => store.set("default_template", value),
                None => {
                    store.delete("default_template");
                }
            }
            return Err("Could not save default template".into());
        }
        Ok(())
    })
    .await
    .map_err(|_| "Could not save default template")??;
    let _ = app.emit("summary-templates-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn api_get_template_editor(template_id: String) -> Result<templates::Template, String> {
    tauri::async_runtime::spawn_blocking(move || templates::get_template(&template_id))
        .await
        .map_err(|_| "Could not load template")?
}

#[tauri::command]
pub async fn api_save_template<R: Runtime>(
    app: tauri::AppHandle<R>,
    template_id: String,
    template: templates::Template,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || templates::save_template(&template_id, &template))
        .await
        .map_err(|_| "Could not save template")??;
    let _ = app.emit("summary-templates-changed", ());
    Ok(())
}

/// Lists all available templates
///
/// Returns templates from both built-in (embedded) and custom (user data directory) sources.
/// Templates are automatically discovered - no code changes needed to add new templates.
///
/// # Returns
/// Vector of TemplateInfo with id, name, and description for each template
#[tauri::command]
pub async fn api_list_templates<R: Runtime>(
    _app: tauri::AppHandle<R>,
) -> Result<Vec<TemplateInfo>, String> {
    info!("api_list_templates called");

    let templates = templates::list_templates();

    let template_infos: Vec<TemplateInfo> = templates
        .into_iter()
        .map(|(id, name, description)| TemplateInfo {
            id,
            name,
            description,
        })
        .collect();

    info!("Found {} available templates", template_infos.len());

    Ok(template_infos)
}

/// Gets detailed information about a specific template
///
/// # Arguments
/// * `template_id` - Template identifier (e.g., "daily_standup")
///
/// # Returns
/// TemplateDetails with full template structure
#[tauri::command]
pub async fn api_get_template_details<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_id: String,
) -> Result<TemplateDetails, String> {
    info!(
        "api_get_template_details called for template_id: {}",
        template_id
    );

    let template = templates::get_template(&template_id)?;

    let section_titles: Vec<String> = template
        .sections
        .iter()
        .map(|section| section.title.clone())
        .collect();

    let details = TemplateDetails {
        id: template_id,
        name: template.name,
        description: template.description,
        sections: section_titles,
    };

    info!("Retrieved template details for '{}'", details.name);

    Ok(details)
}

/// Validates a custom template JSON string
///
/// Useful for template editor UI or validation before saving custom templates
///
/// # Arguments
/// * `template_json` - Raw JSON string of the template
///
/// # Returns
/// Ok(template_name) if valid, Err(error_message) if invalid
#[tauri::command]
pub async fn api_validate_template<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_json: String,
) -> Result<String, String> {
    info!("api_validate_template called");

    match templates::validate_and_parse_template(&template_json) {
        Ok(template) => {
            info!("Template '{}' validated successfully", template.name);
            Ok(template.name)
        }
        Err(e) => {
            warn!("Template validation failed: {}", e);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_list_templates() {
        // This test requires the templates to be embedded/available
        // In a real test environment, you might want to mock the templates module

        // For now, just verify the function compiles and runs
        // You can expand this with more specific assertions
    }

    #[tokio::test]
    async fn test_validate_template_valid() {
        let valid_json = r#"
        {
            "name": "Test Template",
            "description": "A test template",
            "sections": [
                {
                    "title": "Summary",
                    "instruction": "Provide a summary",
                    "format": "paragraph"
                }
            ]
        }"#;

        // Mock app handle would be needed for actual testing
        // For now, test the validation logic directly
        let result = templates::validate_and_parse_template(valid_json);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_template_invalid() {
        let invalid_json = "invalid json";

        let result = templates::validate_and_parse_template(invalid_json);
        assert!(result.is_err());
    }
}
