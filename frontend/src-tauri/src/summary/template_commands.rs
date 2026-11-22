use crate::summary::templates;
use tauri::{Emitter, Runtime};
use tauri_plugin_store::StoreExt;

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

#[tauri::command]
pub async fn api_remove_template<R: Runtime>(
    app: tauri::AppHandle<R>,
    template_id: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || templates::remove_template(&template_id))
        .await
        .map_err(|_| "Could not remove the personal template")??;
    let _ = app.emit("summary-templates-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn api_list_templates() -> Result<Vec<templates::TemplateInfo>, String> {
    tauri::async_runtime::spawn_blocking(templates::list_templates)
        .await
        .map_err(|_| "Could not load templates".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
