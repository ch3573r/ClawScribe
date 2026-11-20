//! Provider-isolated credential references; legacy settings are migrated before use.
use base64::{engine::general_purpose::STANDARD, Engine};

const PREFIX: &str = "clawscribe:secret:v1:";
const SERVICE: &str = "net.rismondo.openclaw.clawscribe.providers";
const UNAVAILABLE: &str = "Protected credential storage is unavailable. Reconnect this provider after restoring access to your OS credential store.";

pub(crate) fn is_protected(value: &str) -> bool {
    value.starts_with("clawscribe:secret:")
}

/// Implements `Debug` for a struct that holds secrets: the fields listed after
/// `secret` print as `[redacted]`, the others normally. A secret therefore
/// cannot reach a log through `{:?}`.
macro_rules! debug_without_secrets {
    ($ty:ident { $($shown:ident),* $(,)? ; secret $($hidden:ident),+ $(,)? }) => {
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($ty))
                    $(.field(stringify!($shown), &self.$shown))*
                    $(.field(stringify!($hidden), &"[redacted]"))+
                    .finish()
            }
        }
    };
}
pub(crate) use debug_without_secrets;

/// What the webview may learn about a stored secret: whether there is one and,
/// for a long enough secret, its last four characters so the user can tell
/// which key it is. It is not reversible and it is the only form in which any
/// command returns a stored secret.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatus {
    pub stored: bool,
    pub hint: Option<String>,
}

/// Secrets shorter than this give no hint: four characters of a short secret
/// would be too much of it.
const MIN_HINT_SECRET_CHARS: usize = 20;

/// A new secret shorter than this is refused. Typing the four-character hint
/// back, or a stray character, must never replace a real key.
pub(crate) const MIN_NEW_SECRET_CHARS: usize = 8;

/// Why a typed secret is refused, or None. Whitespace around it is ignored.
pub(crate) fn new_secret_problem(secret: &str) -> Option<&'static str> {
    (secret.trim().chars().count() < MIN_NEW_SECRET_CHARS)
        .then_some("That is too short to be a key. Enter the full key.")
}

/// Why a save with an unchanged secret and a changed address is refused.
pub(crate) const KEY_REQUIRED_FOR_NEW_ADDRESS: &str =
    "The address changed. Enter the key again so it is only sent to the new address.";
const HINT_CHARS: usize = 4;

/// The last four characters of a secret of at least twenty characters.
pub(crate) fn secret_hint(secret: &str) -> Option<String> {
    let secret = secret.trim();
    let count = secret.chars().count();
    (count >= MIN_HINT_SECRET_CHARS).then(|| secret.chars().skip(count - HINT_CHARS).collect())
}

impl SecretStatus {
    pub(crate) fn absent() -> Self {
        Self {
            stored: false,
            hint: None,
        }
    }

    /// The status of a secret the core has just read (and does not return).
    pub(crate) fn of(secret: Option<&str>) -> Self {
        match secret.map(str::trim).filter(|secret| !secret.is_empty()) {
            Some(secret) => Self {
                stored: true,
                hint: secret_hint(secret),
            },
            None => Self::absent(),
        }
    }
}

trait CredentialBackend {
    fn save(&self, account: &str, value: &str) -> Result<(), String>;
    fn load(&self, account: &str) -> Result<String, String>;
    fn delete(&self, account: &str) -> Result<(), String>;
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
    fn delete(&self, account: &str) -> Result<(), String> {
        match keyring::Entry::new(SERVICE, account).and_then(|entry| entry.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(UNAVAILABLE.into()),
        }
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

/// Remove the external keyring entry. An inline DPAPI value is removed with its record.
pub(crate) fn forget(scope: &str, stored: &str) -> Result<(), String> {
    forget_with(scope, stored, &PlatformCredentials)
}

fn forget_with(scope: &str, stored: &str, backend: &dyn CredentialBackend) -> Result<(), String> {
    if let Some(id) = stored.strip_prefix(&format!("{PREFIX}keyring:")) {
        let id = uuid::Uuid::parse_str(id).map_err(|_| UNAVAILABLE.to_string())?;
        return backend.delete(&format!("{scope}/{id}"));
    }
    if stored.starts_with(&format!("{PREFIX}dpapi:")) {
        return Ok(());
    }
    Err(UNAVAILABLE.into())
}

/// The credential store as the repositories use it. The platform store is the
/// real one; tests use an in-memory one.
pub(crate) trait SecretStore: Send + Sync {
    fn seal(&self, scope: &str, value: &str) -> Result<String, String>;
    fn open(&self, scope: &str, stored: &str) -> Result<String, String>;
    fn forget(&self, scope: &str, stored: &str) -> Result<(), String>;
}

pub(crate) type Store = std::sync::Arc<dyn SecretStore>;

struct PlatformStore;
impl SecretStore for PlatformStore {
    fn seal(&self, scope: &str, value: &str) -> Result<String, String> {
        seal(scope, value)
    }
    fn open(&self, scope: &str, stored: &str) -> Result<String, String> {
        open(scope, stored)
    }
    fn forget(&self, scope: &str, stored: &str) -> Result<(), String> {
        forget(scope, stored)
    }
}

pub(crate) fn platform_store() -> Store {
    std::sync::Arc::new(PlatformStore)
}

const CHANGED: &str = "Provider settings changed; retry the operation";

/// Replaces a stored secret in the order that cannot lose it:
///
/// 1. write the new credential-store entry;
/// 2. `apply` the new reference to the database with a compare-and-set on the
///    old reference (`Ok(true)` when it took effect, `Ok(false)` when another
///    save got there first);
/// 3. only then remove the old entry.
///
/// If the update fails or loses the race, the new entry is removed and the old
/// one stays, so the row never points at a deleted entry and two concurrent
/// saves never orphan one. An entry that cannot be removed is logged without
/// detail. Returns the stored reference.
pub(crate) async fn commit_replacement<A, Fut>(
    store: &Store,
    scope: &str,
    value: &str,
    old_reference: Option<String>,
    apply: A,
) -> Result<String, String>
where
    A: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<bool, String>>,
{
    let (seal_store, seal_scope, seal_value) = (store.clone(), scope.to_string(), value.to_string());
    let new_reference = tokio::task::spawn_blocking(move || seal_store.seal(&seal_scope, &seal_value))
        .await
        .map_err(|_| UNAVAILABLE.to_string())??;
    match apply(new_reference.clone()).await {
        Ok(true) => {
            if let Some(old) = old_reference.filter(|old| old != &new_reference) {
                forget_in(store, scope, old).await;
            }
            Ok(new_reference)
        }
        outcome => {
            forget_in(store, scope, new_reference).await;
            match outcome {
                Ok(_) => Err(CHANGED.to_string()),
                Err(error) => Err(error),
            }
        }
    }
}

/// Removes the entry behind `reference` from `store`. A plaintext value from an
/// old version has no entry. A failure is logged without any detail.
pub(crate) async fn forget_in(store: &Store, scope: &str, reference: String) {
    if reference.is_empty() || !is_protected(&reference) {
        return;
    }
    let (store, scope) = (store.clone(), scope.to_string());
    let removed = tokio::task::spawn_blocking(move || store.forget(&scope, &reference)).await;
    if !matches!(removed, Ok(Ok(()))) {
        log::warn!("A credential could not be removed from the credential store");
    }
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
        fn delete(&self, account: &str) -> Result<(), String> {
            self.0.borrow_mut().remove(account);
            Ok(())
        }
    }
    impl MemoryCredentials {
        fn entries(&self) -> usize {
            self.0.borrow().len()
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

    #[test]
    fn forgetting_removes_the_entry_and_ignores_plaintext() {
        let backend = MemoryCredentials::default();
        let stored = seal_with("provider", "fake-key-eight-8888", &backend).unwrap();
        forget_with("provider", &stored, &backend).unwrap();
        assert_eq!(backend.entries(), 0);
        // A missing entry is not an error; a broken reference still fails closed.
        forget_with("provider", &stored, &backend).unwrap();
        assert!(forget_with("provider", "legacy plaintext", &backend).is_err());
    }

    #[test]
    fn the_hint_is_the_last_four_characters_of_a_long_enough_secret_only() {
        assert_eq!(secret_hint("fake-key-1234-fake-key"), Some("-key".into()));
        assert_eq!(secret_hint("12345678901234567890"), Some("7890".into()), "exactly twenty");
        assert_eq!(secret_hint("1234567890123456789"), None, "nineteen is too short");
        assert_eq!(secret_hint("123456789012"), None, "twelve no longer gives a hint");
        assert_eq!(secret_hint(""), None);
        assert_eq!(secret_hint("  fake-key-1234-fake-key  "), Some("-key".into()), "trimmed");
        // Multi-byte characters are counted as characters, never split.
        assert_eq!(secret_hint("ключ-ключ-ключ-ключ-ключ"), Some("ключ".into()));
    }

    #[test]
    fn a_status_serialises_without_the_secret_and_reflects_presence() {
        let secret = "fake-secret-value-9999-long";
        let status = SecretStatus::of(Some(secret));
        assert_eq!(
            status,
            SecretStatus {
                stored: true,
                hint: Some("long".into())
            }
        );
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains(secret));
        assert_eq!(json, r#"{"stored":true,"hint":"long"}"#);
        // Short secrets are stored but give no hint; nothing stored gives neither.
        assert_eq!(
            serde_json::to_string(&SecretStatus::of(Some("short"))).unwrap(),
            r#"{"stored":true,"hint":null}"#
        );
        for none in [None, Some(""), Some("   ")] {
            assert_eq!(SecretStatus::of(none), SecretStatus::absent());
        }
        assert!(!format!("{status:?}").contains(secret));
    }

    #[derive(Clone)]
    struct Holder {
        name: String,
        api_key: Option<String>,
    }
    debug_without_secrets!(Holder { name; secret api_key });

    #[test]
    fn debug_output_of_a_struct_with_secrets_never_contains_them() {
        let holder = Holder {
            name: "provider".into(),
            api_key: Some("fake-secret-value-4242".into()),
        };
        assert!(holder.api_key.is_some());
        let text = format!("{holder:?} {:#?}", holder.clone());
        assert!(!text.contains("fake-secret-value-4242"));
        assert!(text.contains("provider") && text.contains("[redacted]"));
    }

    #[test]
    fn a_new_key_must_be_at_least_eight_characters() {
        for refused in ["", "  ", "abcd", "1234567", "  1234567  "] {
            assert!(new_secret_problem(refused).is_some(), "{refused:?}");
        }
        for accepted in ["12345678", "  12345678  ", "fake-key-1234-fake-key"] {
            assert!(new_secret_problem(accepted).is_none(), "{accepted:?}");
        }
    }
}

/// An in-memory credential store for tests that must not touch the real one.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    #[derive(Default)]
    struct SharedBackend(Mutex<HashMap<String, String>>);
    impl CredentialBackend for SharedBackend {
        fn save(&self, account: &str, value: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(account.into(), value.into());
            Ok(())
        }
        fn load(&self, account: &str) -> Result<String, String> {
            self.0.lock().unwrap().get(account).cloned().ok_or_else(|| UNAVAILABLE.into())
        }
        fn delete(&self, account: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(account);
            Ok(())
        }
    }

    #[derive(Default)]
    pub(crate) struct MemoryStore(SharedBackend);
    impl MemoryStore {
        pub(crate) fn new() -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self::default())
        }
        pub(crate) fn entries(&self) -> usize {
            self.0 .0.lock().unwrap().len()
        }
        pub(crate) fn contains_value(&self, value: &str) -> bool {
            self.0 .0.lock().unwrap().values().any(|stored| stored == value)
        }
    }
    impl SecretStore for MemoryStore {
        fn seal(&self, scope: &str, value: &str) -> Result<String, String> {
            seal_with(scope, value, &self.0)
        }
        fn open(&self, scope: &str, stored: &str) -> Result<String, String> {
            open_with(scope, stored, &self.0)
        }
        fn forget(&self, scope: &str, stored: &str) -> Result<(), String> {
            forget_with(scope, stored, &self.0)
        }
    }
}

#[cfg(test)]
mod replacement_tests {
    use super::test_support::MemoryStore;
    use super::*;

    fn as_store(store: &std::sync::Arc<MemoryStore>) -> Store {
        store.clone()
    }

    #[tokio::test]
    async fn the_old_entry_is_deleted_only_after_the_update_took_effect() {
        let memory = MemoryStore::new();
        let store = as_store(&memory);
        let first = memory.seal("summary/openai", "fake-key-one-00000000").unwrap();
        assert_eq!(memory.entries(), 1);
        let observed = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let seen = observed.clone();
        let probe = memory.clone();
        let second = commit_replacement(&store, "summary/openai", "fake-key-two-11111111", Some(first.clone()), |_new| async move {
            // While the database update runs, both entries exist.
            *seen.lock().unwrap() = probe.entries();
            Ok(true)
        })
        .await
        .unwrap();
        assert_eq!(*observed.lock().unwrap(), 2, "the new entry is written before the update");
        assert_eq!(memory.entries(), 1, "the old entry is gone afterwards");
        assert!(memory.open("summary/openai", &first).is_err());
        assert_eq!(memory.open("summary/openai", &second).unwrap(), "fake-key-two-11111111");
    }

    #[tokio::test]
    async fn a_failing_update_removes_the_new_entry_and_keeps_the_old_one() {
        let memory = MemoryStore::new();
        let store = as_store(&memory);
        let first = memory.seal("summary/openai", "fake-key-one-00000000").unwrap();
        let error = commit_replacement(&store, "summary/openai", "fake-key-two-11111111", Some(first.clone()), |_new| async move {
            Err("database unavailable".to_string())
        })
        .await
        .unwrap_err();
        assert_eq!(error, "database unavailable");
        assert_eq!(memory.entries(), 1, "only the old entry remains");
        assert_eq!(memory.open("summary/openai", &first).unwrap(), "fake-key-one-00000000");
        assert!(!memory.contains_value("fake-key-two-11111111"));
    }

    #[tokio::test]
    async fn a_lost_race_removes_the_new_entry_and_keeps_the_old_one() {
        let memory = MemoryStore::new();
        let store = as_store(&memory);
        let first = memory.seal("summary/openai", "fake-key-one-00000000").unwrap();
        let error = commit_replacement(&store, "summary/openai", "fake-key-two-11111111", Some(first.clone()), |_new| async move {
            Ok(false)
        })
        .await
        .unwrap_err();
        assert!(error.contains("changed"));
        assert_eq!(memory.entries(), 1);
        assert_eq!(memory.open("summary/openai", &first).unwrap(), "fake-key-one-00000000");
    }

    #[tokio::test]
    async fn replacing_without_an_old_reference_or_with_plaintext_removes_nothing_else() {
        let memory = MemoryStore::new();
        let store = as_store(&memory);
        let other = memory.seal("scope/other", "fake-other-key-2222").unwrap();
        for old in [None, Some(String::new()), Some("legacy plaintext".to_string())] {
            commit_replacement(&store, "scope/new", "fake-key-three-3333", old, |_new| async move { Ok(true) })
                .await
                .unwrap();
        }
        assert!(memory.open("scope/other", &other).is_ok(), "another scope's entry is untouched");
        assert_eq!(memory.entries(), 4);
    }
}
