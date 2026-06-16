//! Secure token persistence using the OS keychain.
//!
//! Stores the Microsoft access + refresh tokens in the platform credential
//! store (Windows Credential Manager, macOS Keychain, Linux Secret Service).
//! The stored value is a JSON-serialized [`StoredToken`] — never written to
//! disk in plaintext.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::exports::auth::{
    refresh_access_token, MicrosoftAuthConfig, MsAuthError, TokenResponse,
};

const SERVICE_NAME: &str = "net.rismondo.openclaw.clawscribe.microsoft";
const ACCOUNT_NAME: &str = "default";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub user_id: String,
    pub user_display_name: String,
    pub user_email: Option<String>,
    pub tenant_id: String,
}

impl StoredToken {
    pub fn from_token_response(
        resp: &TokenResponse,
        user_id: String,
        user_display_name: String,
        user_email: Option<String>,
        tenant_id: String,
    ) -> Self {
        StoredToken {
            access_token: resp.access_token.clone(),
            refresh_token: resp.refresh_token.clone(),
            expires_at: Utc::now() + Duration::seconds(resp.expires_in as i64),
            user_id,
            user_display_name,
            user_email,
            tenant_id,
        }
    }

    pub fn is_access_token_valid(&self) -> bool {
        Utc::now() + Duration::seconds(60) < self.expires_at
    }
}

#[derive(Debug)]
pub enum TokenStoreError {
    KeyringError(String),
    SerializationError(String),
}

impl std::fmt::Display for TokenStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenStoreError::KeyringError(e) => write!(f, "Keyring error: {e}"),
            TokenStoreError::SerializationError(e) => write!(f, "Serialization error: {e}"),
        }
    }
}

pub fn save_token(token: &StoredToken) -> Result<(), TokenStoreError> {
    let json = serde_json::to_string(token)
        .map_err(|e| TokenStoreError::SerializationError(e.to_string()))?;
    let entry = keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME)
        .map_err(|e| TokenStoreError::KeyringError(e.to_string()))?;
    entry
        .set_password(&json)
        .map_err(|e| TokenStoreError::KeyringError(e.to_string()))
}

pub fn load_token() -> Result<Option<StoredToken>, TokenStoreError> {
    let entry = keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME)
        .map_err(|e| TokenStoreError::KeyringError(e.to_string()))?;
    match entry.get_password() {
        Ok(json) => {
            let token: StoredToken = serde_json::from_str(&json)
                .map_err(|e| TokenStoreError::SerializationError(e.to_string()))?;
            Ok(Some(token))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(TokenStoreError::KeyringError(e.to_string())),
    }
}

pub fn delete_token() -> Result<(), TokenStoreError> {
    let entry = keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME)
        .map_err(|e| TokenStoreError::KeyringError(e.to_string()))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(TokenStoreError::KeyringError(e.to_string())),
    }
}

/// Get a valid access token, refreshing if expired. Reads from the keychain.
/// Prefer [`ensure_valid_token`] when a session token is held in memory.
pub async fn get_valid_access_token(
    http: &reqwest::Client,
    config: &MicrosoftAuthConfig,
) -> Result<StoredToken, MsAuthError> {
    ensure_valid_token(http, config, None).await
}

/// Get a valid access token, refreshing if expired.
///
/// `current` is the session's in-memory token (the source of truth). When it is
/// `None` we fall back to the keychain. A refreshed token is written back to the
/// keychain on a best-effort basis: persistence failure does not fail the call,
/// because the caller keeps the returned token in memory for the session.
pub async fn ensure_valid_token(
    http: &reqwest::Client,
    config: &MicrosoftAuthConfig,
    current: Option<StoredToken>,
) -> Result<StoredToken, MsAuthError> {
    let stored = match current {
        Some(t) => t,
        None => load_token()
            .map_err(|e| MsAuthError::TokenRefreshFailed(e.to_string()))?
            .ok_or_else(|| MsAuthError::TokenRefreshFailed("No stored token".to_string()))?,
    };

    if stored.is_access_token_valid() {
        return Ok(stored);
    }

    let refresh = stored
        .refresh_token
        .as_deref()
        .ok_or_else(|| MsAuthError::TokenRefreshFailed("No refresh token".to_string()))?;

    let new_tokens = refresh_access_token(http, config, refresh).await?;

    let updated = StoredToken {
        access_token: new_tokens.access_token.clone(),
        refresh_token: new_tokens.refresh_token.or(stored.refresh_token),
        expires_at: Utc::now() + Duration::seconds(new_tokens.expires_in as i64),
        ..stored
    };

    // Best-effort: a keychain write failure must not invalidate a token we
    // already hold and can use for this session.
    if let Err(e) = save_token(&updated) {
        log::warn!("Failed to persist refreshed Microsoft token: {e}");
    }
    Ok(updated)
}
