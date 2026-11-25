//! Settings that change how the core behaves, owned and persisted by the core.
//!
//! They used to live in the webview's `localStorage` and were pushed to the
//! core when the window loaded, so the core ran with defaults until then (a
//! recording started from the tray, a shortcut or call detection right after
//! launch), a cleared webview profile reset them silently, and backups missed
//! them. They are stored in the app's store plugin (like the recording mode and
//! the recording preferences), loaded during start-up before anything can start
//! a recording, and read and written through commands.
//!
//! Storage rules:
//! * a value that is missing, unreadable or of the wrong type is "no value",
//!   and the default applies;
//! * the cloud transcription opt-in defaults to off and only a stored boolean
//!   `true` switches it on;
//! * the webview's old `localStorage` values are handed over once, only where
//!   the core has no value yet (`migrate_legacy_behavior_settings`).
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

pub const STORE_FILE: &str = "behavior_settings.json";

const PARAKEET_DIRECTML: &str = "parakeet_directml";
const PARAKEET_WORKER: &str = "parakeet_worker";
const CLOUD_TRANSCRIPTION: &str = "cloud_transcription";
const SOURCE_ATTRIBUTION: &str = "source_attribution";
const LANGUAGE: &str = "language";
const LEGACY_MIGRATED: &str = "legacy_migrated";
const SHOW_CONVERSATION_METRICS: &str = "show_conversation_metrics";
const SHOW_RECORDING_NOTIFICATION: &str = "show_recording_notification";

/// One change at a time, whether it comes from the interface or from the
/// hand-over of the webview's old values.
pub(crate) static CHANGE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub const DEFAULT_LANGUAGE: &str = "auto";

/// Where the values are kept. The store plugin implements it; tests use memory.
pub(crate) trait Backing {
    fn read(&self, key: &str) -> Option<Value>;
    fn write(&self, key: &str, value: Value);
    fn remove(&self, key: &str);
    fn commit(&self) -> Result<(), String>;
}

impl<R: Runtime> Backing for tauri_plugin_store::Store<R> {
    fn read(&self, key: &str) -> Option<Value> {
        self.get(key)
    }
    fn write(&self, key: &str, value: Value) {
        self.set(key, value);
    }
    fn remove(&self, key: &str) {
        self.delete(key);
    }
    fn commit(&self) -> Result<(), String> {
        self.save()
            .map_err(|_| "Could not save the setting".to_string())
    }
}

/// What the core has stored: `None` means "no value yet".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Stored {
    pub parakeet_directml: Option<bool>,
    pub parakeet_worker: Option<bool>,
    pub cloud_transcription: Option<bool>,
    pub source_attribution: Option<bool>,
    pub language: Option<String>,
    pub show_recording_notification: Option<bool>,
    pub show_conversation_metrics: Option<bool>,
    pub legacy_migrated: bool,
}

/// The settings as the webview reads them: every value is concrete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorSettings {
    pub parakeet_directml: bool,
    pub parakeet_worker: bool,
    pub cloud_transcription: bool,
    pub source_attribution: bool,
    pub language: String,
    /// Whether the "recording started" notice is shown (default yes).
    pub show_recording_notification: bool,
    pub show_conversation_metrics: bool,
    /// Whether the webview's old values were already handed over.
    pub legacy_migrated: bool,
}

/// The webview's old `localStorage` values; a value it never had is `None`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LegacyBehaviorSettings {
    pub parakeet_directml: Option<bool>,
    pub parakeet_worker: Option<bool>,
    pub cloud_transcription: Option<bool>,
    pub source_attribution: Option<bool>,
    pub language: Option<String>,
}

/// A language the transcription accepts: `auto`, `auto-translate` (detect and
/// translate to English; named explicitly because it is longer than a code), or a
/// short code such as `en` or `zh-CN`. Anything else is not stored.
pub(crate) fn valid_language(language: &str) -> bool {
    if matches!(language, "auto" | "auto-translate") {
        return true;
    }
    let mut parts = language.split(['-', '_']);
    let Some(first) = parts.next() else {
        return false;
    };
    (2..=8).contains(&first.len())
        && first.chars().all(|c| c.is_ascii_alphabetic())
        && parts.clone().count() <= 2
        && parts.all(|part| {
            (1..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

impl Stored {
    /// Reads every value on its own: a wrong type or an invalid value is "no value".
    pub(crate) fn read(backing: &dyn Backing) -> Self {
        let boolean = |key: &str| backing.read(key).and_then(|value| value.as_bool());
        Self {
            parakeet_directml: boolean(PARAKEET_DIRECTML),
            parakeet_worker: boolean(PARAKEET_WORKER),
            cloud_transcription: boolean(CLOUD_TRANSCRIPTION),
            source_attribution: boolean(SOURCE_ATTRIBUTION),
            language: backing
                .read(LANGUAGE)
                .and_then(|value| value.as_str().map(str::to_string))
                .filter(|language| valid_language(language)),
            show_recording_notification: boolean(SHOW_RECORDING_NOTIFICATION),
            show_conversation_metrics: boolean(SHOW_CONVERSATION_METRICS),
            legacy_migrated: backing
                .read(LEGACY_MIGRATED)
                .and_then(|value| value.as_bool())
                == Some(true),
        }
    }

    /// Hardware-independent defaults. The effective DirectML setting is resolved
    /// at the command boundary, away from startup and the capture thread.
    pub(crate) fn effective(&self) -> BehaviorSettings {
        BehaviorSettings {
            parakeet_directml: self.parakeet_directml.unwrap_or(false),
            parakeet_worker: self.parakeet_worker.unwrap_or(false),
            cloud_transcription: self.cloud_transcription == Some(true),
            source_attribution: self.source_attribution.unwrap_or(false),
            language: self
                .language
                .clone()
                .unwrap_or_else(|| DEFAULT_LANGUAGE.to_string()),
            show_recording_notification: self.show_recording_notification.unwrap_or(true),
            show_conversation_metrics: self.show_conversation_metrics.unwrap_or(true),
            legacy_migrated: self.legacy_migrated,
        }
    }

    fn directml_at_startup(&self) -> Option<bool> {
        // An upgraded webview may still hold an explicit CPU choice. Keep the
        // old CPU default until that hand-over has had a chance to read it.
        self.parakeet_directml
            .or((!self.legacy_migrated).then_some(false))
    }
}

/// What a store read gives when the store itself cannot be opened: the defaults.
pub(crate) fn defaults() -> BehaviorSettings {
    Stored::default().effective()
}

/// Writes one value and saves; on a failed save the previous value is put back.
pub(crate) fn persist(backing: &dyn Backing, key: &str, value: Value) -> Result<(), String> {
    replace(backing, key, Some(value)).map(drop)
}

pub(crate) fn replace(
    backing: &dyn Backing,
    key: &str,
    value: Option<Value>,
) -> Result<Option<Value>, String> {
    let previous = backing.read(key);
    match value {
        Some(value) => backing.write(key, value),
        None => backing.remove(key),
    }
    if let Err(error) = backing.commit() {
        match previous {
            Some(previous) => backing.write(key, previous),
            None => backing.remove(key),
        }
        return Err(error);
    }
    Ok(previous)
}

pub(crate) fn persist_bool(backing: &dyn Backing, key: &str, enabled: bool) -> Result<(), String> {
    persist(backing, key, json!(enabled))
}

pub(crate) fn persist_language(backing: &dyn Backing, language: &str) -> Result<(), String> {
    if !valid_language(language) {
        return Err("That language is not supported".into());
    }
    persist(backing, LANGUAGE, json!(language))
}

/// One value to hand over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    ParakeetDirectml(bool),
    ParakeetWorker(bool),
    CloudTranscription(bool),
    SourceAttribution(bool),
    Language(String),
}

/// What the hand-over would change: the webview's old value for each setting
/// the core has no value for yet, nothing once the hand-over is done, and
/// nothing for a value that is not usable.
pub(crate) fn plan_migration(stored: &Stored, legacy: &LegacyBehaviorSettings) -> Vec<Change> {
    if stored.legacy_migrated {
        return Vec::new();
    }
    let mut plan = Vec::new();
    if let (None, Some(value)) = (stored.parakeet_directml, legacy.parakeet_directml) {
        plan.push(Change::ParakeetDirectml(value));
    }
    if let (None, Some(value)) = (stored.parakeet_worker, legacy.parakeet_worker) {
        plan.push(Change::ParakeetWorker(value));
    }
    if let (None, Some(value)) = (stored.cloud_transcription, legacy.cloud_transcription) {
        plan.push(Change::CloudTranscription(value));
    }
    if let (None, Some(value)) = (stored.source_attribution, legacy.source_attribution) {
        plan.push(Change::SourceAttribution(value));
    }
    if stored.language.is_none() {
        if let Some(language) = legacy
            .language
            .as_deref()
            .filter(|language| valid_language(language))
        {
            plan.push(Change::Language(language.to_string()));
        }
    }
    plan
}

/// Records that the hand-over is done.
pub(crate) fn finish_migration(backing: &dyn Backing) -> Result<(), String> {
    persist(backing, LEGACY_MIGRATED, json!(true))
}

/// Puts the stored values into the running core. Called during start-up,
/// before a recording can start.
pub(crate) fn apply(settings: &BehaviorSettings) {
    crate::parakeet_engine::parakeet_engine::USE_PARAKEET_WORKER
        .store(settings.parakeet_worker, Ordering::Relaxed);
    crate::audio::recording_commands::set_cloud_transcription_runtime(settings.cloud_transcription);
    crate::audio::transcription::worker::SOURCE_ATTRIBUTION_ENABLED
        .store(settings.source_attribution, Ordering::Relaxed);
    crate::set_language_runtime(&settings.language);
}

fn open<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<std::sync::Arc<tauri_plugin_store::Store<R>>, String> {
    app.store(STORE_FILE)
        .map_err(|_| "Could not open the settings".to_string())
}

/// Load and apply during start-up. A store that cannot be read gives the
/// defaults (the cloud opt-in stays off).
pub fn load_and_apply<R: Runtime>(app: &AppHandle<R>) {
    use crate::parakeet_engine::parakeet_engine::set_directml_preference;
    let settings = match open(app) {
        Ok(store) => {
            let stored = Stored::read(store.as_ref());
            set_directml_preference(stored.directml_at_startup());
            stored.effective()
        }
        Err(_) => {
            log::warn!("Behaviour settings could not be read; using the defaults");
            set_directml_preference(Some(false));
            defaults()
        }
    };
    apply(&settings);
    import_old_notification_preference(app);
}

/// The "recording started" notice used to be kept by the webview in the plugin
/// store file `preferences.json`. The core owns it now; the old value is read
/// here once, when the core has none.
fn import_old_notification_preference<R: Runtime>(app: &AppHandle<R>) {
    let Ok(store) = open(app) else { return };
    if Stored::read(store.as_ref())
        .show_recording_notification
        .is_some()
    {
        return;
    }
    let old = app
        .store("preferences.json")
        .ok()
        .and_then(|old| old.get("show_recording_notification"))
        .and_then(|value| value.as_bool());
    if let Some(value) = old {
        let _ = persist_bool(store.as_ref(), SHOW_RECORDING_NOTIFICATION, value);
    }
}

pub(crate) fn save_bool<R: Runtime>(
    app: &AppHandle<R>,
    key: &'static str,
    enabled: bool,
) -> Result<(), String> {
    persist_bool(open(app)?.as_ref(), key, enabled)
}

pub(crate) fn save_parakeet_directml<R: Runtime>(
    app: &AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    save_bool(app, PARAKEET_DIRECTML, enabled)
}
pub(crate) fn save_parakeet_worker<R: Runtime>(
    app: &AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    save_bool(app, PARAKEET_WORKER, enabled)
}
pub(crate) fn save_cloud_transcription<R: Runtime>(
    app: &AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    save_bool(app, CLOUD_TRANSCRIPTION, enabled)
}
pub(crate) fn save_source_attribution<R: Runtime>(
    app: &AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    save_bool(app, SOURCE_ATTRIBUTION, enabled)
}
pub(crate) fn save_language<R: Runtime>(app: &AppHandle<R>, language: &str) -> Result<(), String> {
    persist_language(open(app)?.as_ref(), language)
}

#[tauri::command]
pub async fn get_behavior_settings<R: Runtime>(
    app: AppHandle<R>,
) -> Result<BehaviorSettings, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut settings = Stored::read(open(&app)?.as_ref()).effective();
        settings.parakeet_directml = crate::parakeet_engine::parakeet_engine::directml_requested();
        Ok(settings)
    })
    .await
    .map_err(|_| "Could not read the settings".to_string())?
}

/// Hands the webview's old values to the core, once and only where the core
/// has none. Each value goes through the same function as the interface's own
/// change (so live transcription starts over for a new language, a busy job
/// slot refuses an engine change, and nothing is applied twice), under the same
/// lock. A refusal stops the hand-over before it is recorded as done; what was
/// already handed over stays, and a later call continues with the rest.
#[tauri::command]
pub async fn migrate_legacy_behavior_settings(
    app: AppHandle,
    legacy: LegacyBehaviorSettings,
) -> Result<BehaviorSettings, String> {
    let _change = CHANGE_LOCK.lock().await;
    let reader = app.clone();
    let stored = tauri::async_runtime::spawn_blocking(move || {
        Ok::<_, String>(Stored::read(open(&reader)?.as_ref()))
    })
    .await
    .map_err(|_| "Could not migrate the settings".to_string())??;
    for change in plan_migration(&stored, &legacy) {
        match change {
            Change::ParakeetDirectml(value) => {
                crate::parakeet_engine::commands::apply_parakeet_directml(&app, value).await?
            }
            Change::ParakeetWorker(value) => {
                crate::parakeet_engine::commands::apply_parakeet_worker(&app, value).await?
            }
            Change::CloudTranscription(value) => {
                crate::audio::recording_commands::apply_cloud_transcription(&app, value).await?
            }
            Change::SourceAttribution(value) => {
                crate::audio::recording_commands::apply_source_attribution(&app, value).await?
            }
            Change::Language(language) => crate::apply_language(&app, &language).await?,
        }
    }
    let activate_default = !stored.legacy_migrated
        && stored.parakeet_directml.is_none()
        && legacy.parakeet_directml.is_none();
    let job = activate_default
        .then(crate::audio::inference::claim_job)
        .transpose()?;
    let finisher = app.clone();
    let mut settings = tauri::async_runtime::spawn_blocking(move || {
        let store = open(&finisher)?;
        finish_migration(store.as_ref())?;
        Ok::<_, String>(Stored::read(store.as_ref()).effective())
    })
    .await
    .map_err(|_| "Could not migrate the settings".to_string())??;
    if activate_default {
        crate::parakeet_engine::commands::apply_running_directml_preference(None).await;
    }
    drop(job);
    drop(_change);
    settings.parakeet_directml =
        crate::parakeet_engine::commands::get_parakeet_use_directml().await?;
    Ok(settings)
}

/// Whether the "recording started" notice is shown.
#[tauri::command]
pub async fn set_recording_notification_preference<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    let _change = CHANGE_LOCK.lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        persist_bool(open(&app)?.as_ref(), SHOW_RECORDING_NOTIFICATION, enabled)
    })
    .await
    .map_err(|_| "Could not save the setting".to_string())?
}

#[tauri::command]
pub async fn set_conversation_metrics_preference<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    let _change = CHANGE_LOCK.lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        persist_bool(open(&app)?.as_ref(), SHOW_CONVERSATION_METRICS, enabled)
    })
    .await
    .map_err(|_| "Could not save the setting".to_string())?
}

#[cfg(test)]
pub(crate) mod test_memory {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};

    #[derive(Default)]
    pub(crate) struct Memory {
        pub(crate) values: RefCell<HashMap<String, Value>>,
        pub(crate) saved: RefCell<HashMap<String, Value>>,
        pub(crate) fail_saves: RefCell<bool>,
    }
    impl Memory {
        pub(crate) fn with(entries: &[(&str, Value)]) -> Self {
            let memory = Self::default();
            for (key, value) in entries {
                memory
                    .values
                    .borrow_mut()
                    .insert(key.to_string(), value.clone());
                memory
                    .saved
                    .borrow_mut()
                    .insert(key.to_string(), value.clone());
            }
            memory
        }
        pub(crate) fn persisted(&self, key: &str) -> Option<Value> {
            self.saved.borrow().get(key).cloned()
        }
    }
    impl Backing for Memory {
        fn read(&self, key: &str) -> Option<Value> {
            self.values.borrow().get(key).cloned()
        }
        fn write(&self, key: &str, value: Value) {
            self.values.borrow_mut().insert(key.into(), value);
        }
        fn remove(&self, key: &str) {
            self.values.borrow_mut().remove(key);
        }
        fn commit(&self) -> Result<(), String> {
            if *self.fail_saves.borrow() {
                return Err("Could not save the setting".into());
            }
            *self.saved.borrow_mut() = self.values.borrow().clone();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_memory::Memory;
    use super::*;

    #[test]
    fn an_empty_store_gives_the_defaults_with_everything_off() {
        let settings = Stored::read(&Memory::default()).effective();
        assert_eq!(settings, defaults());
        assert!(!settings.parakeet_directml && !settings.parakeet_worker);
        assert!(!settings.cloud_transcription && !settings.source_attribution);
        assert_eq!(settings.language, "auto");
        assert!(!settings.legacy_migrated);
    }

    #[test]
    fn the_cloud_opt_in_is_on_only_for_a_stored_boolean_true() {
        for unreadable in [
            json!("true"),
            json!(1),
            json!("yes"),
            json!(null),
            json!({}),
            json!([true]),
            json!("on"),
        ] {
            let memory = Memory::with(&[(CLOUD_TRANSCRIPTION, unreadable.clone())]);
            assert!(
                !Stored::read(&memory).effective().cloud_transcription,
                "{unreadable}"
            );
        }
        assert!(
            !Stored::read(&Memory::with(&[(CLOUD_TRANSCRIPTION, json!(false))]))
                .effective()
                .cloud_transcription
        );
        assert!(
            Stored::read(&Memory::with(&[(CLOUD_TRANSCRIPTION, json!(true))]))
                .effective()
                .cloud_transcription
        );
    }

    #[test]
    fn a_wrong_typed_or_invalid_value_is_no_value_and_does_not_disturb_the_others() {
        let memory = Memory::with(&[
            (PARAKEET_DIRECTML, json!("true")),
            (PARAKEET_WORKER, json!(true)),
            (LANGUAGE, json!("not a language!")),
            (SOURCE_ATTRIBUTION, json!(true)),
        ]);
        let stored = Stored::read(&memory);
        assert_eq!(stored.parakeet_directml, None);
        assert_eq!(stored.language, None);
        let settings = stored.effective();
        assert!(!settings.parakeet_directml);
        assert!(settings.parakeet_worker && settings.source_attribution);
        assert_eq!(settings.language, "auto");
    }

    #[test]
    fn languages_are_auto_or_short_codes() {
        for ok in ["auto", "en", "de", "zh-CN", "pt_BR", "yue", "sr-Latn-RS"] {
            assert!(valid_language(ok), "{ok}");
        }
        for bad in [
            "",
            "e",
            "englishlanguage",
            "en-",
            "-en",
            "en us",
            "en;rm",
            "12",
            "../x",
            "a-b-c-d",
            "auto-translate-now",
        ] {
            assert!(!valid_language(bad), "{bad}");
        }
    }

    #[test]
    fn a_saved_value_persists_and_a_failed_save_restores_the_stored_value() {
        let memory = Memory::default();
        persist_bool(&memory, CLOUD_TRANSCRIPTION, true).unwrap();
        assert_eq!(memory.persisted(CLOUD_TRANSCRIPTION), Some(json!(true)));
        *memory.fail_saves.borrow_mut() = true;
        assert!(persist_bool(&memory, CLOUD_TRANSCRIPTION, false).is_err());
        assert_eq!(
            memory.read(CLOUD_TRANSCRIPTION),
            Some(json!(true)),
            "the stored value is back"
        );
        assert!(persist_bool(&memory, PARAKEET_WORKER, true).is_err());
        assert_eq!(
            memory.read(PARAKEET_WORKER),
            None,
            "a value that did not exist is removed again"
        );
        assert!(persist_language(&memory, "bad language").is_err());
        assert_eq!(memory.read(LANGUAGE), None);
    }

    #[test]
    fn migration_plans_the_webview_value_only_where_the_core_has_none() {
        let stored = Stored {
            parakeet_worker: Some(false),
            language: Some("de".into()),
            ..Stored::default()
        };
        let legacy = LegacyBehaviorSettings {
            parakeet_directml: Some(true),
            parakeet_worker: Some(true),
            cloud_transcription: Some(true),
            source_attribution: Some(true),
            language: Some("fr".into()),
        };
        assert_eq!(
            plan_migration(&stored, &legacy),
            vec![
                Change::ParakeetDirectml(true),
                Change::CloudTranscription(true),
                Change::SourceAttribution(true),
            ],
            "the core keeps its own worker value and language"
        );
    }

    #[test]
    fn migration_is_planned_once_and_unusable_values_are_ignored() {
        let migrated = Stored {
            legacy_migrated: true,
            ..Stored::default()
        };
        let legacy = LegacyBehaviorSettings {
            cloud_transcription: Some(true),
            ..Default::default()
        };
        assert!(
            plan_migration(&migrated, &legacy).is_empty(),
            "after the hand-over nothing is taken"
        );
        let odd = LegacyBehaviorSettings {
            language: Some("???".into()),
            ..Default::default()
        };
        assert!(plan_migration(&Stored::default(), &odd).is_empty());
        assert!(plan_migration(&Stored::default(), &LegacyBehaviorSettings::default()).is_empty());
    }

    #[test]
    fn finishing_the_migration_records_it_and_a_failed_save_can_be_retried() {
        let memory = Memory::default();
        *memory.fail_saves.borrow_mut() = true;
        assert!(finish_migration(&memory).is_err());
        assert!(!Stored::read(&memory).legacy_migrated);
        *memory.fail_saves.borrow_mut() = false;
        finish_migration(&memory).unwrap();
        assert!(Stored::read(&memory).legacy_migrated);
        assert_eq!(memory.persisted(LEGACY_MIGRATED), Some(json!(true)));
    }

    #[test]
    fn every_language_the_interface_can_send_is_accepted_and_nonsense_is_not() {
        // The selector's own list: the validator and the interface cannot drift apart.
        let source = include_str!("../../src/constants/languages.ts");
        let codes: Vec<&str> = source
            .lines()
            .filter_map(|line| line.split("code: '").nth(1))
            .filter_map(|rest| rest.split('\'').next())
            .collect();
        assert!(
            codes.len() >= 30,
            "the language list was read: {}",
            codes.len()
        );
        assert!(codes.contains(&"auto") && codes.contains(&"auto-translate"));
        for code in &codes {
            assert!(valid_language(code), "{code}");
            let memory = Memory::default();
            persist_language(&memory, code).unwrap();
            assert_eq!(
                Stored::read(&memory).language.as_deref(),
                Some(*code),
                "{code} survives a save and a read"
            );
        }
        for bad in [
            "auto-translate-now",
            "auto_translate",
            "translate",
            "auto-",
            "auto translate",
        ] {
            assert!(!valid_language(bad), "{bad}");
        }
    }

    #[test]
    fn the_notification_preference_defaults_to_shown_and_keeps_a_stored_no() {
        assert!(
            Stored::read(&Memory::default())
                .effective()
                .show_recording_notification
        );
        let memory = Memory::default();
        persist_bool(&memory, SHOW_RECORDING_NOTIFICATION, false).unwrap();
        assert!(
            !Stored::read(&memory)
                .effective()
                .show_recording_notification
        );
        let odd = Memory::with(&[(SHOW_RECORDING_NOTIFICATION, json!("no"))]);
        assert!(
            Stored::read(&odd).effective().show_recording_notification,
            "an unreadable value is the default"
        );
    }

    #[test]
    fn conversation_metrics_are_shown_until_explicitly_hidden() {
        let memory = Memory::default();
        assert!(Stored::read(&memory).effective().show_conversation_metrics);
        persist_bool(&memory, SHOW_CONVERSATION_METRICS, false).unwrap();
        assert!(!Stored::read(&memory).effective().show_conversation_metrics);
    }

    #[test]
    fn the_legacy_payload_reads_camel_case_and_tolerates_missing_fields() {
        let legacy: LegacyBehaviorSettings =
            serde_json::from_value(json!({"parakeetDirectml": true, "cloudTranscription": false}))
                .unwrap();
        assert_eq!(legacy.parakeet_directml, Some(true));
        assert_eq!(legacy.cloud_transcription, Some(false));
        assert_eq!(legacy.language, None);
        let settings = serde_json::to_value(defaults()).unwrap();
        for key in [
            "parakeetDirectml",
            "parakeetWorker",
            "cloudTranscription",
            "sourceAttribution",
            "language",
            "showRecordingNotification",
            "showConversationMetrics",
            "legacyMigrated",
        ] {
            assert!(settings.get(key).is_some(), "{key}");
        }
    }
}
#[test]
fn parakeet_defers_the_automatic_default_until_the_legacy_choice_is_read() {
    let mut stored = Stored::default();
    assert_eq!(stored.directml_at_startup(), Some(false));
    stored.legacy_migrated = true;
    assert_eq!(stored.directml_at_startup(), None);
    for explicit in [true, false] {
        stored.parakeet_directml = Some(explicit);
        assert_eq!(stored.directml_at_startup(), Some(explicit));
    }
}
