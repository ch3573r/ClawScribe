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
}

impl MicrosoftAuthState {
    pub fn new() -> Self {
        let config = MicrosoftAuthConfig::default();
        let http = reqwest::Client::new();

        let (connection_state, user_display_name, user_email, user_id) =
            match token_store::load_token() {
                Ok(Some(t)) if t.is_access_token_valid() => (
                    MicrosoftConnectionState::Connected,
                    Some(t.user_display_name),
                    t.user_email,
                    Some(t.user_id),
                ),
                Ok(Some(t)) if t.refresh_token.is_some() => (
                    MicrosoftConnectionState::Connected,
                    Some(t.user_display_name),
                    t.user_email,
                    Some(t.user_id),
                ),
                _ => (MicrosoftConnectionState::NotConnected, None, None, None),
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
            }),
        }
    }
}
