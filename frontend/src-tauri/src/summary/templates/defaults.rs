/// Embedded default templates using compile-time inclusion
///
/// These templates are bundled into the binary and serve as fallbacks
/// when custom templates are not available.

/// Daily standup template for engineering/product teams
pub const DAILY_STANDUP: &str = include_str!("../../../templates/daily_standup.json");

/// Standard meeting notes template
pub const STANDARD_MEETING: &str = include_str!("../../../templates/standard_meeting.json");

const BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    ("daily_standup", DAILY_STANDUP),
    ("standard_meeting", STANDARD_MEETING),
    (
        "one_on_one",
        include_str!("../../../templates/one_on_one.json"),
    ),
    (
        "customer_discovery",
        include_str!("../../../templates/customer_discovery.json"),
    ),
    (
        "technical_design_review",
        include_str!("../../../templates/technical_design_review.json"),
    ),
    (
        "interview",
        include_str!("../../../templates/interview.json"),
    ),
    (
        "executive_brief",
        include_str!("../../../templates/executive_brief.json"),
    ),
];

/// Get a built-in template by identifier
///
/// # Arguments
/// * `id` - Template identifier (e.g., "daily_standup", "standard_meeting")
///
/// # Returns
/// The template JSON content if found, None otherwise
pub fn get_builtin_template(id: &str) -> Option<&'static str> {
    BUILTIN_TEMPLATES
        .iter()
        .find_map(|&(key, content)| (key == id).then_some(content))
}

/// List all built-in template identifiers
pub fn list_builtin_template_ids() -> Vec<&'static str> {
    BUILTIN_TEMPLATES.iter().map(|&(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_templates_are_valid() {
        for id in list_builtin_template_ids() {
            let result = super::super::loader::validate_and_parse_template(
                get_builtin_template(id).unwrap(),
            );
            assert!(
                result.is_ok(),
                "Built-in template '{}' contains invalid JSON: {:?}",
                id,
                result.err()
            );
        }
    }

    #[test]
    fn meeting_specific_templates_keep_evidence_in_their_instructions() {
        for (id, name) in [
            ("one_on_one", "1:1 check-in"),
            ("customer_discovery", "Customer / discovery call"),
            ("technical_design_review", "Technical / design review"),
            ("interview", "Interview"),
            ("executive_brief", "Executive brief"),
        ] {
            assert!(list_builtin_template_ids().contains(&id));
            let template = super::super::loader::validate_and_parse_template(
                get_builtin_template(id).unwrap(),
            )
            .unwrap();
            assert_eq!(template.name, name);
            assert!(template.to_section_instructions().contains("source tag"));
        }
    }

    #[test]
    fn test_get_builtin_template() {
        assert!(get_builtin_template("daily_standup").is_some());
        assert!(get_builtin_template("standard_meeting").is_some());
        assert!(get_builtin_template("nonexistent").is_none());
    }
}
