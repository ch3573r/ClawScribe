//! Tauri-managed state for the Microsoft Graph connection.

use tokio::sync::RwLock;

use crate::exports::auth::MicrosoftAuthConfig;
use crate::exports::model::MicrosoftConnectionState;
use crate::exports::token_store;

pub struct MicrosoftAuthState {
    pub(crate) inner: RwLock<MicrosoftAuthInner>,
}

pub(crate) struct MicrosoftAuthInner {
    pub config: MicrosoftAuthConfig,
    pub http: reqwest::Client,
    pub connection_state: MicrosoftConnectionState,
    pub pending_device_code: Option<String>,
    pub user_display_name: Option<String>,
    pub user_email: Option<String>,
    pub user_id: Option<String>,
    /// In-memory copy of the active token. This — not the keychain — is the
    /// source of truth for the current session, so exports still work when the
    /// platform credential store is unavailable or a save fails.
    pub current_token: Option<token_store::StoredToken>,
}

impl MicrosoftAuthState {
    pub fn new() -> Self {
        let config = MicrosoftAuthConfig::default();
        let http = reqwest::Client::new();

        let restored = match token_store::load_token() {
            Ok(Some(t)) if t.is_access_token_valid() || t.refresh_token.is_some() => Some(t),
            _ => None,
        };

        let (connection_state, user_display_name, user_email, user_id, current_token) =
            match restored {
                Some(t) => (
                    MicrosoftConnectionState::Connected,
                    Some(t.user_display_name.clone()),
                    t.user_email.clone(),
                    Some(t.user_id.clone()),
                    Some(t),
                ),
                None => (MicrosoftConnectionState::NotConnected, None, None, None, None),
            };

        MicrosoftAuthState {
            inner: RwLock::new(MicrosoftAuthInner {
                config,
                http,
                connection_state,
                pending_device_code: None,
                user_display_name,
                user_email,
                user_id,
                current_token,
            }),
        }
    }
}
