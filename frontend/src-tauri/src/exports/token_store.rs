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
    /// Space-separated scopes actually granted by Entra (the token's `scope`
    /// response field). Used to diagnose insufficient-permission failures.
    #[serde(default)]
    pub granted_scopes: String,
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
            granted_scopes: resp.scope.clone(),
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

/// The subset of a token that is persisted between sessions.
///
/// The access token is deliberately NOT persisted: it is short-lived (~1h) and
/// large. The Windows Credential Manager blob is capped at 2560 bytes, and a
/// full Graph access-token JWT plus refresh token easily exceeds it — which
/// silently failed every keychain write and lost the sign-in on restart. Only
/// the refresh token (plus metadata) is needed to restore a session; it is
/// re-exchanged for an access token on first use.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedToken {
    refresh_token: Option<String>,
    user_id: String,
    user_display_name: String,
    user_email: Option<String>,
    tenant_id: String,
    #[serde(default)]
    granted_scopes: String,
}

impl PersistedToken {
    fn from_stored(t: &StoredToken) -> Self {
        PersistedToken {
            refresh_token: t.refresh_token.clone(),
            user_id: t.user_id.clone(),
            user_display_name: t.user_display_name.clone(),
            user_email: t.user_email.clone(),
            tenant_id: t.tenant_id.clone(),
            granted_scopes: t.granted_scopes.clone(),
        }
    }

    /// Reconstruct an in-memory token. The access token starts empty and
    /// expired so the first use refreshes it via the refresh token.
    fn into_stored(self) -> StoredToken {
        StoredToken {
            access_token: String::new(),
            refresh_token: self.refresh_token,
            // Already-expired, so the first use refreshes via the refresh token.
            expires_at: Utc::now() - Duration::hours(1),
            user_id: self.user_id,
            user_display_name: self.user_display_name,
            user_email: self.user_email,
            tenant_id: self.tenant_id,
            granted_scopes: self.granted_scopes,
        }
    }
}

/// Plaintext file fallback used when the OS keychain is unavailable or rejects
/// the write (e.g. the Windows Credential Manager size limit). Lives in the
/// per-user app config dir.
fn fallback_token_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("ClawScribe").join("ms-token.json"))
}

fn save_to_file(json: &str) -> Result<(), TokenStoreError> {
    let path = fallback_token_path()
        .ok_or_else(|| TokenStoreError::KeyringError("no config dir for token fallback".into()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TokenStoreError::SerializationError(e.to_string()))?;
    }
    std::fs::write(&path, json).map_err(|e| TokenStoreError::SerializationError(e.to_string()))
}

fn load_from_file() -> Option<PersistedToken> {
    let path = fallback_token_path()?;
    let json = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&json).ok()
}

fn delete_file() {
    if let Some(path) = fallback_token_path() {
        let _ = std::fs::remove_file(path);
    }
}

pub fn save_token(token: &StoredToken) -> Result<(), TokenStoreError> {
    let json = serde_json::to_string(&PersistedToken::from_stored(token))
        .map_err(|e| TokenStoreError::SerializationError(e.to_string()))?;

    // Prefer the OS keychain; fall back to a file if it is unavailable or
    // rejects the write. Keeping both in sync avoids a stale fallback shadowing
    // a fresh keychain entry, so on a successful keychain write we clear the file.
    let keychain = keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME)
        .map_err(|e| TokenStoreError::KeyringError(e.to_string()))
        .and_then(|entry| {
            entry
                .set_password(&json)
                .map_err(|e| TokenStoreError::KeyringError(e.to_string()))
        });

    match keychain {
        Ok(()) => {
            delete_file();
            Ok(())
        }
        Err(e) => {
            log::warn!("Keychain token write failed ({e}); using file fallback");
            save_to_file(&json)
        }
    }
}

pub fn load_token() -> Result<Option<StoredToken>, TokenStoreError> {
    let entry = keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME)
        .map_err(|e| TokenStoreError::KeyringError(e.to_string()))?;
    match entry.get_password() {
        Ok(json) => {
            let persisted: PersistedToken = serde_json::from_str(&json)
                .map_err(|e| TokenStoreError::SerializationError(e.to_string()))?;
            Ok(Some(persisted.into_stored()))
        }
        // Nothing in the keychain — check the file fallback before giving up.
        Err(keyring::Error::NoEntry) => Ok(load_from_file().map(PersistedToken::into_stored)),
        Err(_) => Ok(load_from_file().map(PersistedToken::into_stored)),
    }
}

pub fn delete_token() -> Result<(), TokenStoreError> {
    delete_file();
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

    let granted_scopes = if new_tokens.scope.is_empty() {
        stored.granted_scopes.clone()
    } else {
        new_tokens.scope.clone()
    };
    let updated = StoredToken {
        access_token: new_tokens.access_token.clone(),
        refresh_token: new_tokens.refresh_token.or(stored.refresh_token),
        expires_at: Utc::now() + Duration::seconds(new_tokens.expires_in as i64),
        granted_scopes,
        ..stored
    };

    // Best-effort: a keychain write failure must not invalidate a token we
    // already hold and can use for this session.
    if let Err(e) = save_token(&updated) {
        log::warn!("Failed to persist refreshed Microsoft token: {e}");
    }
    Ok(updated)
}
