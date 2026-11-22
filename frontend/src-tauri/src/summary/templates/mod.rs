//! Meeting summary template management
//!
//! This module provides a flexible template system for generating meeting summaries.
//! It supports both built-in templates (embedded in the binary) and custom user templates
//! (loaded from the application data directory).
//!
//! # Architecture
//!
//! - **Built-in templates**: JSON files in `frontend/src-tauri/templates/` embedded at compile time
//! - **Custom templates**: JSON files in platform-specific app data directory
//! - **Fallback strategy**: Custom templates override built-in templates with the same ID
//!
//! # Usage
//!
//! ```rust
//! use app_lib::summary::templates;
//!
//! // Load a specific template
//! let template = templates::get_template("daily_standup").expect("daily standup template exists");
//!
//! // Generate markdown structure
//! let markdown = template.to_markdown_structure();
//!
//! // Generate LLM instructions
//! let instructions = template.to_section_instructions();
//!
//! // List available templates
//! let available = templates::list_templates();
//! ```
//!
//! # Custom Templates
//!
//! Users can add custom templates to:
//! - macOS: `~/Library/Application Support/ClawScribe/templates/`
//! - Windows: `%APPDATA%\ClawScribe\templates\`
//! - Linux: `~/.config/ClawScribe/templates/`
//!
//! Existing `Meetily/templates` directories are still read as a legacy fallback.
//!
//! Custom templates must follow the JSON schema defined in `types::Template`.

mod defaults;
mod loader;
mod storage;
mod types;

// Re-export public API
pub(crate) use loader::custom_template_dirs;
pub use loader::{
    get_template, list_templates, set_bundled_templates_dir, validate_and_parse_template,
    TemplateInfo,
};
pub use storage::{remove_template, resolve_default, save_template};
pub(crate) use storage::{validate_id, REMOVED_TEMPLATE};
pub use types::{Template, TemplateSection};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_integration() {
        // Test that we can load all built-in templates
        let templates = list_templates();
        assert!(!templates.is_empty());

        for template in templates {
            let result = get_template(&template.id);
            assert!(
                result.is_ok(),
                "Failed to load template '{}': {:?}",
                template.id,
                result.err()
            );
        }
    }

    #[test]
    fn test_template_metadata() {
        let templates = list_templates();
        assert!(!templates.is_empty());

        for template in templates {
            assert!(!template.id.is_empty());
            assert!(!template.name.is_empty());
            assert!(!template.description.is_empty());
        }
    }
}
