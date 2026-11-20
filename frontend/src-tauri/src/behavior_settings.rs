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
        self.save().map_err(|_| "Could not save the setting".to_string())
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

/// A language the transcription accepts: `auto` or a short code such as `en`
/// or `zh-CN`. Anything else is not stored.
pub(crate) fn valid_language(language: &str) -> bool {
    let mut parts = language.split(['-', '_']);
    let Some(first) = parts.next() else { return false };
    (2..=8).contains(&first.len())
        && first.chars().all(|c| c.is_ascii_alphabetic())
        && parts.clone().count() <= 2
        && parts.all(|part| (1..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric()))
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
            legacy_migrated: backing.read(LEGACY_MIGRATED).and_then(|value| value.as_bool()) == Some(true),
        }
    }

    /// Defaults everywhere: GPU and process isolation off, source attribution
    /// off, cloud transcription off, language `auto`.
    pub(crate) fn effective(&self) -> BehaviorSettings {
        BehaviorSettings {
            parakeet_directml: self.parakeet_directml.unwrap_or(false),
            parakeet_worker: self.parakeet_worker.unwrap_or(false),
            cloud_transcription: self.cloud_transcription == Some(true),
            source_attribution: self.source_attribution.unwrap_or(false),
            language: self.language.clone().unwrap_or_else(|| DEFAULT_LANGUAGE.to_string()),
            legacy_migrated: self.legacy_migrated,
        }
    }
}

/// What a store read gives when the store itself cannot be opened: the defaults.
pub(crate) fn defaults() -> BehaviorSettings {
    Stored::default().effective()
}

/// Writes one value and saves; on a failed save the previous value is put back.
fn persist(backing: &dyn Backing, key: &str, value: Value) -> Result<(), String> {
    let previous = backing.read(key);
    backing.write(key, value);
    if let Err(error) = backing.commit() {
        match previous {
            Some(previous) => backing.write(key, previous),
            None => backing.remove(key),
        }
        return Err(error);
    }
    Ok(())
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

/// Takes the webview's old values where the core has none, once. Returns the
/// settings after the hand-over. A repeated call changes nothing.
pub(crate) fn migrate_legacy(
    backing: &dyn Backing,
    legacy: &LegacyBehaviorSettings,
) -> Result<BehaviorSettings, String> {
    let stored = Stored::read(backing);
    if stored.legacy_migrated {
        return Ok(stored.effective());
    }
    let mut written: Vec<(&str, Option<Value>)> = Vec::new();
    let mut take = |key: &'static str, current: bool, value: Option<Value>| {
        if !current {
            if let Some(value) = value {
                written.push((key, backing.read(key)));
                backing.write(key, value);
            }
        }
    };
    take(PARAKEET_DIRECTML, stored.parakeet_directml.is_some(), legacy.parakeet_directml.map(|v| json!(v)));
    take(PARAKEET_WORKER, stored.parakeet_worker.is_some(), legacy.parakeet_worker.map(|v| json!(v)));
    take(CLOUD_TRANSCRIPTION, stored.cloud_transcription.is_some(), legacy.cloud_transcription.map(|v| json!(v)));
    take(SOURCE_ATTRIBUTION, stored.source_attribution.is_some(), legacy.source_attribution.map(|v| json!(v)));
    take(
        LANGUAGE,
        stored.language.is_some(),
        legacy.language.as_deref().filter(|language| valid_language(language)).map(|language| json!(language)),
    );
    written.push((LEGACY_MIGRATED, backing.read(LEGACY_MIGRATED)));
    backing.write(LEGACY_MIGRATED, json!(true));
    if let Err(error) = backing.commit() {
        for (key, previous) in written.into_iter().rev() {
            match previous {
                Some(previous) => backing.write(key, previous),
                None => backing.remove(key),
            }
        }
        return Err(error);
    }
    Ok(Stored::read(backing).effective())
}

/// Puts the stored values into the running core. Called during start-up,
/// before a recording can start.
pub(crate) fn apply(settings: &BehaviorSettings) {
    crate::parakeet_engine::parakeet_engine::USE_PARAKEET_DIRECTML
        .store(settings.parakeet_directml, Ordering::Relaxed);
    crate::parakeet_engine::parakeet_engine::USE_PARAKEET_WORKER
        .store(settings.parakeet_worker, Ordering::Relaxed);
    crate::audio::recording_commands::set_cloud_transcription_runtime(settings.cloud_transcription);
    crate::audio::transcription::worker::SOURCE_ATTRIBUTION_ENABLED
        .store(settings.source_attribution, Ordering::Relaxed);
    crate::set_language_runtime(&settings.language);
}

fn open<R: Runtime>(app: &AppHandle<R>) -> Result<std::sync::Arc<tauri_plugin_store::Store<R>>, String> {
    app.store(STORE_FILE).map_err(|_| "Could not open the settings".to_string())
}

/// Load and apply during start-up. A store that cannot be read gives the
/// defaults (the cloud opt-in stays off).
pub fn load_and_apply<R: Runtime>(app: &AppHandle<R>) {
    let settings = match open(app) {
        Ok(store) => Stored::read(store.as_ref()).effective(),
        Err(_) => {
            log::warn!("Behaviour settings could not be read; using the defaults");
            defaults()
        }
    };
    apply(&settings);
}

pub(crate) fn save_bool<R: Runtime>(app: &AppHandle<R>, key: &'static str, enabled: bool) -> Result<(), String> {
    persist_bool(open(app)?.as_ref(), key, enabled)
}

pub(crate) fn save_parakeet_directml<R: Runtime>(app: &AppHandle<R>, enabled: bool) -> Result<(), String> {
    save_bool(app, PARAKEET_DIRECTML, enabled)
}
pub(crate) fn save_parakeet_worker<R: Runtime>(app: &AppHandle<R>, enabled: bool) -> Result<(), String> {
    save_bool(app, PARAKEET_WORKER, enabled)
}
pub(crate) fn save_cloud_transcription<R: Runtime>(app: &AppHandle<R>, enabled: bool) -> Result<(), String> {
    save_bool(app, CLOUD_TRANSCRIPTION, enabled)
}
pub(crate) fn save_source_attribution<R: Runtime>(app: &AppHandle<R>, enabled: bool) -> Result<(), String> {
    save_bool(app, SOURCE_ATTRIBUTION, enabled)
}
pub(crate) fn save_language<R: Runtime>(app: &AppHandle<R>, language: &str) -> Result<(), String> {
    persist_language(open(app)?.as_ref(), language)
}

#[tauri::command]
pub async fn get_behavior_settings<R: Runtime>(app: AppHandle<R>) -> Result<BehaviorSettings, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(Stored::read(open(&app)?.as_ref()).effective()))
        .await
        .map_err(|_| "Could not read the settings".to_string())?
}

/// Hands the webview's old values to the core, once and only where the core
/// has none, applies the result, and returns the settings.
#[tauri::command]
pub async fn migrate_legacy_behavior_settings<R: Runtime>(
    app: AppHandle<R>,
    legacy: LegacyBehaviorSettings,
) -> Result<BehaviorSettings, String> {
    let worker_app = app.clone();
    let (before, after) = tauri::async_runtime::spawn_blocking(move || -> Result<_, String> {
        let store = open(&worker_app)?;
        let before = Stored::read(store.as_ref()).effective();
        let after = migrate_legacy(store.as_ref(), &legacy)?;
        Ok((before, after))
    })
    .await
    .map_err(|_| "Could not migrate the settings".to_string())??;
    if before != after {
        apply(&after);
        // A loaded model keeps its old settings until it is reloaded.
        if before.parakeet_directml != after.parakeet_directml || before.parakeet_worker != after.parakeet_worker {
            crate::parakeet_engine::commands::unload_for_new_settings().await;
        }
    }
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};

    #[derive(Default)]
    struct Memory {
        values: RefCell<HashMap<String, Value>>,
        saved: RefCell<HashMap<String, Value>>,
        fail_saves: RefCell<bool>,
    }
    impl Memory {
        fn with(entries: &[(&str, Value)]) -> Self {
            let memory = Self::default();
            for (key, value) in entries {
                memory.values.borrow_mut().insert(key.to_string(), value.clone());
                memory.saved.borrow_mut().insert(key.to_string(), value.clone());
            }
            memory
        }
        fn persisted(&self, key: &str) -> Option<Value> {
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
        for unreadable in [json!("true"), json!(1), json!("yes"), json!(null), json!({}), json!([true]), json!("on")] {
            let memory = Memory::with(&[(CLOUD_TRANSCRIPTION, unreadable.clone())]);
            assert!(!Stored::read(&memory).effective().cloud_transcription, "{unreadable}");
        }
        assert!(!Stored::read(&Memory::with(&[(CLOUD_TRANSCRIPTION, json!(false))])).effective().cloud_transcription);
        assert!(Stored::read(&Memory::with(&[(CLOUD_TRANSCRIPTION, json!(true))])).effective().cloud_transcription);
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
        for bad in ["", "e", "englishlanguage", "en-", "-en", "en us", "en;rm", "12", "../x", "a-b-c-d"] {
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
        assert_eq!(memory.read(CLOUD_TRANSCRIPTION), Some(json!(true)), "the stored value is back");
        assert!(persist_bool(&memory, PARAKEET_WORKER, true).is_err());
        assert_eq!(memory.read(PARAKEET_WORKER), None, "a value that did not exist is removed again");
        assert!(persist_language(&memory, "bad language").is_err());
        assert_eq!(memory.read(LANGUAGE), None);
    }

    #[test]
    fn migration_takes_the_webview_value_only_where_the_core_has_none() {
        let memory = Memory::with(&[(PARAKEET_WORKER, json!(false)), (LANGUAGE, json!("de"))]);
        let legacy = LegacyBehaviorSettings {
            parakeet_directml: Some(true),
            parakeet_worker: Some(true),
            cloud_transcription: Some(true),
            source_attribution: Some(true),
            language: Some("fr".into()),
        };
        let settings = migrate_legacy(&memory, &legacy).unwrap();
        assert!(settings.parakeet_directml, "the core had none: the webview value is taken");
        assert!(!settings.parakeet_worker, "the core had a value: it keeps its own");
        assert!(settings.cloud_transcription && settings.source_attribution);
        assert_eq!(settings.language, "de", "the core's own language stays");
        assert!(settings.legacy_migrated);
        assert_eq!(memory.persisted(LEGACY_MIGRATED), Some(json!(true)));
        assert_eq!(memory.persisted(PARAKEET_DIRECTML), Some(json!(true)));
    }

    #[test]
    fn migration_happens_once_and_a_later_call_changes_nothing() {
        let memory = Memory::default();
        let first = LegacyBehaviorSettings { cloud_transcription: Some(true), ..Default::default() };
        assert!(migrate_legacy(&memory, &first).unwrap().cloud_transcription);
        persist_bool(&memory, CLOUD_TRANSCRIPTION, false).unwrap();
        let later = LegacyBehaviorSettings { cloud_transcription: Some(true), source_attribution: Some(true), ..Default::default() };
        let settings = migrate_legacy(&memory, &later).unwrap();
        assert!(!settings.cloud_transcription, "a setting changed in the app is not overwritten by the old value");
        assert!(!settings.source_attribution);
    }

    #[test]
    fn a_webview_without_old_values_migrates_to_the_defaults_and_unusable_values_are_ignored() {
        let memory = Memory::default();
        let settings = migrate_legacy(&memory, &LegacyBehaviorSettings { language: Some("???".into()), ..Default::default() }).unwrap();
        assert_eq!(settings, BehaviorSettings { legacy_migrated: true, ..defaults() });
        assert_eq!(memory.read(LANGUAGE), None);
    }

    #[test]
    fn a_failed_migration_save_changes_nothing_and_can_be_retried() {
        let memory = Memory::default();
        *memory.fail_saves.borrow_mut() = true;
        let legacy = LegacyBehaviorSettings { cloud_transcription: Some(true), ..Default::default() };
        assert!(migrate_legacy(&memory, &legacy).is_err());
        assert_eq!(Stored::read(&memory), Stored::default());
        *memory.fail_saves.borrow_mut() = false;
        assert!(migrate_legacy(&memory, &legacy).unwrap().cloud_transcription);
    }

    #[test]
    fn the_legacy_payload_reads_camel_case_and_tolerates_missing_fields() {
        let legacy: LegacyBehaviorSettings =
            serde_json::from_value(json!({"parakeetDirectml": true, "cloudTranscription": false})).unwrap();
        assert_eq!(legacy.parakeet_directml, Some(true));
        assert_eq!(legacy.cloud_transcription, Some(false));
        assert_eq!(legacy.language, None);
        let settings = serde_json::to_value(defaults()).unwrap();
        for key in ["parakeetDirectml", "parakeetWorker", "cloudTranscription", "sourceAttribution", "language", "legacyMigrated"] {
            assert!(settings.get(key).is_some(), "{key}");
        }
    }
}
