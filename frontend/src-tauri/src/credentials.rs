//! Provider-isolated credential references; legacy settings are migrated before use.
use base64::{engine::general_purpose::STANDARD, Engine};

const PREFIX: &str = "clawscribe:secret:v1:";
const SERVICE: &str = "net.rismondo.openclaw.clawscribe.providers";
const UNAVAILABLE: &str = "Protected credential storage is unavailable. Reconnect this provider after restoring access to your OS credential store.";

pub(crate) fn is_protected(value: &str) -> bool {
    value.starts_with("clawscribe:secret:")
}

trait CredentialBackend {
    fn save(&self, account: &str, value: &str) -> Result<(), String>;
    fn load(&self, account: &str) -> Result<String, String>;
}

struct PlatformCredentials;
impl CredentialBackend for PlatformCredentials {
    fn save(&self, account: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, account)
            .and_then(|entry| entry.set_password(value))
            .map_err(|_| UNAVAILABLE.into())
    }
    fn load(&self, account: &str) -> Result<String, String> {
        keyring::Entry::new(SERVICE, account)
            .and_then(|entry| entry.get_password())
            .map_err(|_| UNAVAILABLE.into())
    }
}

pub(crate) fn seal(scope: &str, value: &str) -> Result<String, String> {
    seal_with(scope, value, &PlatformCredentials)
}

fn seal_with(scope: &str, value: &str, backend: &dyn CredentialBackend) -> Result<String, String> {
    if value.is_empty() {
        return Ok(String::new());
    }
    let id = uuid::Uuid::new_v4();
    if backend.save(&format!("{scope}/{id}"), value).is_ok() {
        return Ok(format!("{PREFIX}keyring:{id}"));
    }
    // Only encrypted bytes may be stored in the settings file/SQLite fallback.
    #[cfg(target_os = "windows")]
    {
        let plaintext = format!("{scope}\0{value}");
        let encrypted = crate::exports::token_store::protect_bytes(plaintext.as_bytes())
            .map_err(|_| UNAVAILABLE.to_string())?;
        Ok(format!("{PREFIX}dpapi:{}", STANDARD.encode(encrypted)))
    }
    #[cfg(not(target_os = "windows"))]
    Err(UNAVAILABLE.into())
}

pub(crate) fn open(scope: &str, stored: &str) -> Result<String, String> {
    open_with(scope, stored, &PlatformCredentials)
}

fn open_with(scope: &str, stored: &str, backend: &dyn CredentialBackend) -> Result<String, String> {
    if stored.is_empty() {
        return Ok(String::new());
    }
    if let Some(id) = stored.strip_prefix(&format!("{PREFIX}keyring:")) {
        let id = uuid::Uuid::parse_str(id).map_err(|_| UNAVAILABLE.to_string())?;
        return backend.load(&format!("{scope}/{id}"));
    }
    #[cfg(target_os = "windows")]
    if let Some(encoded) = stored.strip_prefix(&format!("{PREFIX}dpapi:")) {
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| UNAVAILABLE.to_string())?;
        let plaintext = crate::exports::token_store::unprotect_bytes(&bytes)
            .map_err(|_| UNAVAILABLE.to_string())?;
        let plaintext = String::from_utf8(plaintext).map_err(|_| UNAVAILABLE.to_string())?;
        return plaintext
            .strip_prefix(&format!("{scope}\0"))
            .map(str::to_string)
            .ok_or_else(|| UNAVAILABLE.into());
    }
    // Never interpret a broken reference or encrypted blob as a plaintext key.
    Err(UNAVAILABLE.into())
}

pub(crate) async fn seal_async(scope: String, value: String) -> Result<String, sqlx::Error> {
    tokio::task::spawn_blocking(move || seal(&scope, &value))
        .await
        .map_err(|_| sqlx::Error::Protocol(UNAVAILABLE.into()))?
        .map_err(sqlx::Error::Protocol)
}

pub(crate) async fn open_async(scope: String, value: String) -> Result<String, sqlx::Error> {
    tokio::task::spawn_blocking(move || open(&scope, &value))
        .await
        .map_err(|_| sqlx::Error::Protocol(UNAVAILABLE.into()))?
        .map_err(sqlx::Error::Protocol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};
    #[derive(Default)]
    struct MemoryCredentials(RefCell<HashMap<String, String>>);
    impl CredentialBackend for MemoryCredentials {
        fn save(&self, account: &str, value: &str) -> Result<(), String> {
            self.0.borrow_mut().insert(account.into(), value.into());
            Ok(())
        }
        fn load(&self, account: &str) -> Result<String, String> {
            self.0
                .borrow()
                .get(account)
                .cloned()
                .ok_or_else(|| UNAVAILABLE.into())
        }
    }
    #[test]
    fn references_do_not_contain_secrets_and_are_provider_isolated() {
        let backend = MemoryCredentials::default();
        let stored = seal_with("summary/openai", "synthetic credential", &backend).unwrap();
        assert!(!stored.contains("synthetic credential"));
        assert_eq!(
            open_with("summary/openai", &stored, &backend).unwrap(),
            "synthetic credential"
        );
        assert!(open_with("transcription/openai", &stored, &backend).is_err());
    }
    #[test]
    fn plaintext_and_corrupt_references_fail_closed() {
        let backend = MemoryCredentials::default();
        for stored in [
            "legacy value",
            "clawscribe:secret:v1:keyring:broken",
            "clawscribe:secret:v1:dpapi:broken",
        ] {
            assert!(open_with("provider", stored, &backend).is_err());
        }
    }
}
