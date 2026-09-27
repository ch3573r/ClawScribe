use super::{auth::MicrosoftAuthConfig, token_store::StoredToken};

pub(super) fn has_scope(granted: &str, required: &str) -> bool {
    granted.split_whitespace().any(|scope| {
        let scope = scope.trim_start_matches("https://graph.microsoft.com/");
        scope.eq_ignore_ascii_case(required)
            || match required {
                "Notes.Create" => ["Notes.ReadWrite", "Notes.ReadWrite.All"]
                    .iter()
                    .any(|s| scope.eq_ignore_ascii_case(s)),
                "Notes.Read" => ["Notes.ReadWrite", "Notes.Read.All", "Notes.ReadWrite.All"]
                    .iter()
                    .any(|s| scope.eq_ignore_ascii_case(s)),
                "Notes.ReadWrite" => scope.eq_ignore_ascii_case("Notes.ReadWrite.All"),
                "Files.ReadWrite" => scope.eq_ignore_ascii_case("Files.ReadWrite.All"),
                "Calendars.Read" => scope.eq_ignore_ascii_case("Calendars.ReadWrite"),
                _ => false,
            }
    })
}

pub(super) fn require_scope(token: &StoredToken, required: &str) -> Result<(), String> {
    if has_scope(&token.granted_scopes, required) {
        Ok(())
    } else {
        Err(format!("Permission not granted: {required}"))
    }
}

pub(super) fn missing_scopes(config: &MicrosoftAuthConfig, token: &StoredToken) -> Vec<String> {
    config
        .scopes
        .iter()
        .filter(|scope| {
            if scope.as_str() == "offline_access" {
                token
                    .refresh_token
                    .as_ref()
                    .is_none_or(|value| value.is_empty())
            } else {
                !has_scope(&token.granted_scopes, scope)
            }
        })
        .cloned()
        .collect()
}

pub(super) fn unavailable_exports(token: &StoredToken) -> Vec<String> {
    [
        ("OneNote export", "Notes.Create"),
        ("OneNote notebook discovery", "Notes.Read"),
        ("Planner and Microsoft To Do export", "Tasks.ReadWrite"),
        ("OneDrive/SharePoint export", "Files.ReadWrite"),
        ("Calendar lookup", "Calendars.Read"),
    ]
    .iter()
    .filter(|(_, scope)| !has_scope(&token.granted_scopes, scope))
    .map(|(label, _)| (*label).into())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_permissions_keep_approved_exports_available() {
        let token = StoredToken::from_token_response(
            &super::super::auth::TokenResponse {
                access_token: "test-token".into(),
                refresh_token: None,
                expires_in: 3600,
                token_type: "Bearer".into(),
                scope: "User.Read Notes.ReadWrite".into(),
            },
            "test-user".into(),
            "Test User".into(),
            None,
            "organizations".into(),
        );
        assert!(require_scope(&token, "Notes.Create").is_ok());
        assert_eq!(
            require_scope(&token, "Tasks.ReadWrite").unwrap_err(),
            "Permission not granted: Tasks.ReadWrite"
        );
        let missing = missing_scopes(&MicrosoftAuthConfig::default(), &token);
        assert!(missing.contains(&"offline_access".into()));
        assert!(missing.contains(&"Tasks.ReadWrite".into()));
        assert!(!missing.contains(&"Notes.Create".into()));
    }
}
