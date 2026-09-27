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
    pub generation: u64,
    pub http: reqwest::Client,
    pub connection_state: MicrosoftConnectionState,
    pub pending_device_code: Option<String>,
    pub sign_in_cancel: Option<tokio_util::sync::CancellationToken>,
    pub user_display_name: Option<String>,
    pub user_email: Option<String>,
    pub user_id: Option<String>,
    /// In-memory copy of the active token. This — not the keychain — is the
    /// source of truth for the current session, so exports still work when the
    /// platform credential store is unavailable or a save fails.
    pub current_token: Option<token_store::StoredToken>,
}

fn auth_http_client(timeout: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .expect("Microsoft HTTP client configuration")
}

impl MicrosoftAuthState {
    pub fn new() -> Self {
        let config = MicrosoftAuthConfig::default();
        let http = auth_http_client(std::time::Duration::from_secs(30));

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
                None => (
                    MicrosoftConnectionState::NotConnected,
                    None,
                    None,
                    None,
                    None,
                ),
            };

        MicrosoftAuthState {
            inner: RwLock::new(MicrosoftAuthInner {
                config,
                generation: 0,
                http,
                connection_state,
                pending_device_code: None,
                sign_in_cancel: None,
                user_display_name,
                user_email,
                user_id,
                current_token,
            }),
        }
    }
}

impl MicrosoftAuthInner {
    // Call under the write lock: sign-out cannot interleave persistence and memory.
    pub fn accept_refreshed_token(
        &mut self,
        generation: u64,
        token: token_store::StoredToken,
        persist: impl FnOnce(&token_store::StoredToken) -> Result<(), token_store::TokenStoreError>,
    ) -> Result<(), String> {
        if self.generation != generation
            || self.connection_state != MicrosoftConnectionState::Connected
        {
            return Err("Microsoft session changed; sign in before exporting".into());
        }
        let changed = self
            .current_token
            .as_ref()
            .is_none_or(|current| !current.same_persisted_fields(&token));
        if changed && persist(&token).is_err() {
            log::warn!("Could not persist refreshed Microsoft session");
        }
        self.current_token = Some(token);
        Ok(())
    }

    /// Completing or cancelling consent must preserve an already connected session.
    pub fn finish_sign_in_failure(&mut self, generation: u64) -> bool {
        if self.generation != generation {
            return false;
        }
        self.sign_in_cancel = None;
        self.pending_device_code = None;
        self.connection_state = if self.current_token.is_some() {
            MicrosoftConnectionState::Connected
        } else {
            MicrosoftConnectionState::NotConnected
        };
        true
    }

    pub fn end_session(
        &mut self,
        delete: impl FnOnce() -> Result<(), token_store::TokenStoreError>,
    ) -> Result<(), String> {
        self.generation = self.generation.wrapping_add(1);
        if let Some(cancel) = self.sign_in_cancel.take() {
            cancel.cancel();
        }
        self.connection_state = MicrosoftConnectionState::NotConnected;
        self.pending_device_code = None;
        self.user_display_name = None;
        self.user_email = None;
        self.user_id = None;
        self.current_token = None;
        delete().map_err(|_| {
            "Signed out, but stored Microsoft credentials could not be removed. Retry sign-out."
                .into()
        })
    }
}

pub(crate) fn begin_sign_in(
    connection: &mut MicrosoftConnectionState,
) -> Result<tokio_util::sync::CancellationToken, String> {
    if *connection == MicrosoftConnectionState::Connecting {
        return Err("Microsoft sign-in is already in progress".into());
    }
    *connection = MicrosoftConnectionState::Connecting;
    Ok(tokio_util::sync::CancellationToken::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn auth_http_client_times_out_stalled_responses() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let error = auth_http_client(std::time::Duration::from_millis(50))
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap_err();
        assert!(error.is_timeout());
        server.abort();
    }

    #[tokio::test]
    async fn sign_out_during_refresh_cannot_restore_memory_or_credentials() {
        use std::sync::{Arc, Mutex};
        let token = token_store::StoredToken::from_token_response(
            &crate::exports::auth::TokenResponse {
                access_token: "test-access".into(),
                refresh_token: Some("test-refresh".into()),
                expires_in: 3600,
                token_type: "Bearer".into(),
                scope: "User.Read".into(),
            },
            "test-user".into(),
            "Test User".into(),
            None,
            "organizations".into(),
        );
        let credentials = Arc::new(Mutex::new(Some(token.clone())));
        let state = Arc::new(RwLock::new(MicrosoftAuthInner {
            config: MicrosoftAuthConfig::default(),
            generation: 0,
            http: reqwest::Client::new(),
            connection_state: MicrosoftConnectionState::Connected,
            pending_device_code: None,
            sign_in_cancel: None,
            user_display_name: None,
            user_email: None,
            user_id: None,
            current_token: Some(token.clone()),
        }));
        let generation = state.read().await.generation;
        let (complete, response) = tokio::sync::oneshot::channel();
        let refresh_state = state.clone();
        let refresh_credentials = credentials.clone();
        let refresh = tokio::spawn(async move {
            let updated = response.await.unwrap();
            refresh_state
                .write()
                .await
                .accept_refreshed_token(generation, updated, |token| {
                    *refresh_credentials.lock().unwrap() = Some(token.clone());
                    Ok(())
                })
        });
        state
            .write()
            .await
            .end_session(|| {
                *credentials.lock().unwrap() = None;
                Ok(())
            })
            .unwrap();
        complete.send(token).unwrap();
        assert!(refresh.await.unwrap().is_err());
        assert!(state.read().await.current_token.is_none());
        assert!(credentials.lock().unwrap().is_none());
    }

    #[test]
    fn unchanged_exports_do_not_persist_but_rotated_refresh_token_does() {
        let token = token_store::StoredToken::from_token_response(
            &crate::exports::auth::TokenResponse {
                access_token: "test-access".into(),
                refresh_token: Some("test-refresh".into()),
                expires_in: 3600,
                token_type: "Bearer".into(),
                scope: "User.Read Notes.Create".into(),
            },
            "test-user".into(),
            "Test User".into(),
            None,
            "organizations".into(),
        );
        let mut state = MicrosoftAuthInner {
            config: MicrosoftAuthConfig::default(),
            generation: 0,
            http: reqwest::Client::new(),
            connection_state: MicrosoftConnectionState::Connected,
            pending_device_code: None,
            sign_in_cancel: None,
            user_display_name: None,
            user_email: None,
            user_id: None,
            current_token: Some(token.clone()),
        };
        let mut writes = 0;
        let mut refreshed = token;
        refreshed.access_token = "test-new-access".into();
        refreshed.expires_at += chrono::Duration::hours(1);
        for _ in 0..3 {
            state
                .accept_refreshed_token(0, refreshed.clone(), |_| {
                    writes += 1;
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(writes, 0);
        refreshed.refresh_token = Some("test-new-refresh".into());
        state
            .accept_refreshed_token(0, refreshed.clone(), |_| {
                writes += 1;
                Ok(())
            })
            .unwrap();
        state
            .accept_refreshed_token(0, refreshed, |_| {
                writes += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(writes, 1);
        let mut changed_scope = state.current_token.clone().unwrap();
        changed_scope.granted_scopes.push_str(" Tasks.ReadWrite");
        state
            .accept_refreshed_token(0, changed_scope, |_| {
                writes += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(writes, 2);
    }

    #[test]
    fn concurrent_sign_in_is_rejected() {
        let mut state = MicrosoftConnectionState::NotConnected;
        assert!(begin_sign_in(&mut state).is_ok());
        assert!(begin_sign_in(&mut state).is_err());
    }
}
