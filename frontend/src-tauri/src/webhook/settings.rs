use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;
use tokio::sync::watch;
use uuid::Uuid;

use super::{delivery::destination, payload::Event};
use crate::{
    behavior_settings::{self, Backing},
    credentials::{self, SecretStatus, SecretStore},
};

const KEY: &str = "meeting_webhook";
const SCOPE: &str = "meeting-webhook";
const STORAGE_ERROR: &str = "Could not open webhook settings.";
const CREDENTIAL_ERROR: &str =
    "Webhook credentials are unavailable. Enter the destination URL again.";
pub(super) const SETTINGS_CHANGED: &str =
    "Webhook settings changed. Open the latest settings and try again.";

static EDIT: Mutex<()> = Mutex::new(());
pub(super) static CHANGED: LazyLock<watch::Sender<()>> = LazyLock::new(|| watch::channel(()).0);

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Switches {
    pub enabled: bool,
    pub on_summary: bool,
    pub on_recording: bool,
    pub include_summary: bool,
    pub include_transcript: bool,
    pub allow_unencrypted: bool,
}

impl Default for Switches {
    fn default() -> Self {
        Self {
            enabled: false,
            on_summary: true,
            on_recording: false,
            include_summary: true,
            include_transcript: false,
            allow_unencrypted: false,
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
struct Stored {
    #[serde(flatten)]
    switches: Switches,
    reference: String,
    revision: Option<Uuid>,
}

impl Stored {
    fn read(store: &dyn Backing) -> Result<Self, String> {
        store
            .read(KEY)
            .map(serde_json::from_value)
            .transpose()
            .map(Option::unwrap_or_default)
            .map_err(|_| STORAGE_ERROR.into())
    }

    fn allows(&self, event: Event, automatic: bool) -> bool {
        self.switches.enabled
            && !self.reference.is_empty()
            && (!automatic
                || match event {
                    Event::SummaryReady => self.switches.on_summary,
                    Event::RecordingSaved => self.switches.on_recording,
                    Event::Test => false,
                })
    }

    fn credentials(&self, secrets: &dyn SecretStore) -> Result<Option<Credentials>, String> {
        if self.reference.is_empty() {
            return Ok(None);
        }
        if !credentials::is_protected(&self.reference) {
            return Err(CREDENTIAL_ERROR.into());
        }
        let stored = secrets
            .open(SCOPE, &self.reference)
            .map_err(|_| CREDENTIAL_ERROR)?;
        serde_json::from_str(&stored)
            .map(Some)
            .map_err(|_| CREDENTIAL_ERROR.into())
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct Credentials {
    pub url: String,
    pub signing_secret: Option<String>,
}

credentials::debug_without_secrets!(Credentials { ; secret url, signing_secret });

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Changes {
    #[serde(flatten)]
    switches: Switches,
    revision: Option<Uuid>,
    url: Option<String>,
    signing_secret: Option<String>,
    remove_signing_secret: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    #[serde(flatten)]
    switches: Switches,
    revision: Option<Uuid>,
    configured: bool,
    destination: Option<String>,
    signing_secret: SecretStatus,
    configuration_error: Option<String>,
}

fn status(settings: &Stored, secrets: &dyn SecretStore) -> Status {
    let mut status = Status {
        switches: settings.switches,
        revision: settings.revision,
        configured: !settings.reference.is_empty(),
        destination: None,
        signing_secret: SecretStatus::absent(),
        configuration_error: None,
    };
    match settings.credentials(secrets) {
        Ok(Some(credential)) => {
            status.signing_secret = SecretStatus::of(credential.signing_secret.as_deref());
            match destination(&credential.url, settings.switches.allow_unencrypted) {
                Ok(url) => status.destination = Some(url.origin().ascii_serialization()),
                Err(error) => status.configuration_error = Some(error),
            }
        }
        Ok(None) => {}
        Err(error) => status.configuration_error = Some(error),
    }
    status
}

fn with_store<R: Runtime, T>(
    app: &AppHandle<R>,
    operation: impl FnOnce(&dyn Backing, &dyn SecretStore) -> Result<T, String>,
) -> Result<T, String> {
    let _guard = EDIT.lock().map_err(|_| STORAGE_ERROR)?;
    let store = app
        .store(behavior_settings::STORE_FILE)
        .map_err(|_| STORAGE_ERROR)?;
    operation(store.as_ref(), credentials::platform_store().as_ref())
}

pub(super) fn read<R: Runtime>(app: &AppHandle<R>) -> Result<Status, String> {
    with_store(app, |store, secrets| {
        Ok(status(&Stored::read(store)?, secrets))
    })
}

pub(super) struct Route {
    pub switches: Switches,
    pub credentials: Result<Option<Credentials>, String>,
}

pub(super) fn route<R: Runtime>(
    app: &AppHandle<R>,
    event: Event,
    automatic: bool,
    revision: Option<Uuid>,
) -> Result<Option<Route>, String> {
    with_store(app, |store, secrets| {
        let settings = Stored::read(store)?;
        if !automatic && revision != settings.revision {
            return Err(SETTINGS_CHANGED.into());
        }
        if !settings.allows(event, automatic) {
            return Ok(None);
        }
        Ok(Some(Route {
            switches: settings.switches,
            credentials: settings.credentials(secrets),
        }))
    })
}

pub(super) fn save<R: Runtime>(app: &AppHandle<R>, changes: Changes) -> Result<Status, String> {
    with_store(app, |store, secrets| {
        let previous = Stored::read(store)?;
        if previous.revision != changes.revision {
            return Err(SETTINGS_CHANGED.into());
        }
        let next = update(&previous, changes, store, secrets)?;
        Ok(status(&next, secrets))
    })
}

pub(super) fn remove<R: Runtime>(
    app: &AppHandle<R>,
    revision: Option<Uuid>,
) -> Result<Status, String> {
    with_store(app, |store, secrets| {
        let previous = Stored::read(store)?;
        if previous.revision != revision {
            return Err(SETTINGS_CHANGED.into());
        }
        let next = Stored {
            revision: Some(Uuid::new_v4()),
            ..Stored::default()
        };
        persist(&previous, &next, store, secrets)?;
        Ok(status(&next, secrets))
    })
}

fn update(
    previous: &Stored,
    changes: Changes,
    store: &dyn Backing,
    secrets: &dyn SecretStore,
) -> Result<Stored, String> {
    let replacement = changes
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty());
    let new_secret = changes
        .signing_secret
        .as_deref()
        .filter(|secret| !secret.is_empty());
    if changes.remove_signing_secret && new_secret.is_some() {
        return Err("Choose whether to replace or remove the signing secret.".into());
    }
    if let Some(secret) = new_secret {
        if let Some(problem) = credentials::new_secret_problem(secret) {
            return Err(problem.into());
        }
    }
    let has_changes =
        replacement.is_some() || new_secret.is_some() || changes.remove_signing_secret;
    let credential = previous.credentials(secrets);
    let next_reference = match credential {
        Ok(previous_credential) => {
            let old_url = previous_credential.as_ref().map(|value| value.url.as_str());
            let url = replacement.or(old_url);
            match url {
                Some(url) => {
                    let url = destination(url, changes.switches.allow_unencrypted)?;
                    if has_changes {
                        let keep_secret = !changes.remove_signing_secret
                            && old_url.is_some_and(|previous| previous == url.as_str());
                        let signing_secret = new_secret.map(str::to_owned).or_else(|| {
                            keep_secret
                                .then(|| previous_credential.and_then(|value| value.signing_secret))
                                .flatten()
                        });
                        seal(secrets, url.into(), signing_secret)?
                    } else {
                        previous.reference.clone()
                    }
                }
                None if changes.switches.enabled || new_secret.is_some() => {
                    return Err("Enter a webhook destination URL.".into())
                }
                None => String::new(),
            }
        }
        Err(_) if replacement.is_some() => {
            let url = destination(
                replacement.unwrap_or_default(),
                changes.switches.allow_unencrypted,
            )?;
            seal(secrets, url.into(), new_secret.map(str::to_owned))?
        }
        Err(_) if !changes.switches.enabled && !has_changes => previous.reference.clone(),
        Err(error) => return Err(error),
    };
    let next = Stored {
        switches: changes.switches,
        reference: next_reference,
        revision: Some(Uuid::new_v4()),
    };
    persist(previous, &next, store, secrets)?;
    Ok(next)
}

fn seal(
    secrets: &dyn SecretStore,
    url: String,
    signing_secret: Option<String>,
) -> Result<String, String> {
    let data = serde_json::to_string(&Credentials {
        url,
        signing_secret,
    })
    .map_err(|_| CREDENTIAL_ERROR)?;
    secrets
        .seal(SCOPE, &data)
        .map_err(|_| CREDENTIAL_ERROR.into())
}

fn persist(
    previous: &Stored,
    next: &Stored,
    store: &dyn Backing,
    secrets: &dyn SecretStore,
) -> Result<(), String> {
    let result = serde_json::to_value(next)
        .map_err(|_| STORAGE_ERROR.to_string())
        .and_then(|value| behavior_settings::persist(store, KEY, value))
        .map_err(|_| "Could not save webhook settings.".to_string());
    if result.is_ok() {
        CHANGED.send_replace(());
    }
    if previous.reference != next.reference {
        let obsolete = if result.is_ok() {
            &previous.reference
        } else {
            &next.reference
        };
        if credentials::is_protected(obsolete) && secrets.forget(SCOPE, obsolete).is_err() {
            log::warn!("Could not remove an obsolete webhook credential entry");
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{behavior_settings::test_memory::Memory, credentials::test_support::MemoryStore};

    #[test]
    fn disabled_missing_destination_and_independent_triggers_are_gated() {
        let mut stored = Stored::default();
        assert!(!stored.allows(Event::SummaryReady, true));
        stored.switches.enabled = true;
        assert!(!stored.allows(Event::SummaryReady, true));
        let secrets = MemoryStore::new();
        stored.reference = seal(
            secrets.as_ref(),
            "https://hooks.example.com/sample".into(),
            None,
        )
        .unwrap();
        assert!(stored.allows(Event::SummaryReady, true));
        assert!(!stored.allows(Event::RecordingSaved, true));
        assert!(!stored.allows(Event::Test, true));
        assert!(stored.allows(Event::Test, false));
        stored.switches.on_summary = false;
        stored.switches.on_recording = true;
        assert!(!stored.allows(Event::SummaryReady, true));
        assert!(stored.allows(Event::SummaryReady, false));
        assert!(stored.allows(Event::RecordingSaved, true));
        stored.switches.enabled = false;
        assert!(!stored.allows(Event::RecordingSaved, true));
        assert!(!stored.allows(Event::Test, false));
    }

    #[test]
    fn settings_status_errors_and_debug_redact_url_and_secret() {
        let store = Memory::default();
        let secrets = MemoryStore::new();
        let url = "https://hooks.example.com/private-path?key=sample-query";
        let secret = "sample-signing-material";
        let changes = Changes {
            switches: Switches {
                enabled: true,
                ..Switches::default()
            },
            revision: None,
            url: Some(url.into()),
            signing_secret: Some(secret.into()),
            remove_signing_secret: false,
        };
        let saved = update(&Stored::default(), changes, &store, secrets.as_ref()).unwrap();
        let stored = store.persisted(KEY).unwrap().to_string();
        let visible = serde_json::to_value(status(&saved, secrets.as_ref())).unwrap();
        let configuration_error = destination(
            "http://hooks.example.com/private-path?key=sample-query",
            true,
        )
        .unwrap_err();
        let credentials = saved.credentials(secrets.as_ref()).unwrap().unwrap();
        for output in [
            stored,
            visible.to_string(),
            format!("{credentials:?}"),
            configuration_error,
        ] {
            assert!(!output.contains(url));
            assert!(!output.contains("private-path"));
            assert!(!output.contains("sample-query"));
            assert!(!output.contains(secret));
        }
        assert_eq!(visible["destination"], "https://hooks.example.com");
        assert_eq!(visible["signingSecret"]["stored"], true);
        assert_eq!(visible["signingSecret"]["hint"], "rial");

        let replacement = || Changes {
            switches: saved.switches,
            revision: saved.revision,
            url: Some("https://hooks.example.com/replacement".into()),
            signing_secret: None,
            remove_signing_secret: false,
        };
        *store.fail_saves.borrow_mut() = true;
        assert!(update(&saved, replacement(), &store, secrets.as_ref()).is_err());
        assert_eq!(secrets.entries(), 1);
        assert_eq!(
            saved.credentials(secrets.as_ref()).unwrap().unwrap().url,
            url
        );
        *store.fail_saves.borrow_mut() = false;
        let saved_replacement = update(&saved, replacement(), &store, secrets.as_ref()).unwrap();
        assert_eq!(secrets.entries(), 1);
        assert!(saved.credentials(secrets.as_ref()).is_err());
        let current = saved_replacement
            .credentials(secrets.as_ref())
            .unwrap()
            .unwrap();
        assert!(current.signing_secret.is_none());
        assert!(current.url.ends_with("/replacement"));
    }
}
