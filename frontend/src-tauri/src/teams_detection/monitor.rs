//! The core's call monitor.
//!
//! It decides when a detected call starts a recording (or asks first) and when
//! the end of that call stops it. This used to run in the window, whose timers
//! are throttled while the window sits hidden in the tray, so a call could go
//! unnoticed or its end be seen late. Now the core scans on its own schedule.
//!
//! * The settings (mode, apps, automatic stop) are pushed here by the window and
//!   kept in the core's own settings folder, so the monitor works right after
//!   launch, before any window has loaded. Switching detection off is `Mode::Off`.
//! * The window delivers the chosen devices and calendar selection to the
//!   native recorder, which claims the request and reports the actual session.
//! * Stopping is done by the core with the session the recorder reported. The
//!   recorder checks that session under its own stop guard, so a manual
//!   recording is never stopped by a call ending.
//! * A prompt stays pending until the call is over or the user answers. It is
//!   shown by the window and by the tray menu, so it can be answered while the
//!   window is hidden.
pub mod join;
pub(crate) mod prompt_window;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};

use super::calls::{CallScan, CallWindow, Provider};
use crate::library::calendar::CalendarLink;
use crate::start_request::{self, StartOutcome, StartSource};

/// How often the core looks for calls while detection is on.
const POLL_INTERVAL: Duration = Duration::from_secs(12);
/// Consecutive completed scans that must show a call gone before it has ended.
const MISSES_TO_END: u8 = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Off,
    Prompt,
    Auto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSettings {
    pub mode: Mode,
    pub providers: Vec<Provider>,
    #[serde(default)]
    pub auto_stop: bool,
}

impl Default for CallSettings {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            providers: vec![Provider::Teams],
            auto_stop: false,
        }
    }
}

impl CallSettings {
    /// The chosen apps in the settings order, each once.
    pub fn normalized(mut self) -> Self {
        let chosen = std::mem::take(&mut self.providers);
        self.providers = Provider::ALL
            .iter()
            .copied()
            .filter(|provider| chosen.contains(provider))
            .collect();
        self
    }

    fn watching(&self) -> bool {
        self.mode != Mode::Off && !self.providers.is_empty()
    }
}

// ── Decisions ─────────────────────────────────────────────────────────────

/// Whether a scan shows this call over. A different browser tab, a hidden
/// toolbar, an inaccessible desktop or a failed scan cannot prove that a call
/// ended; only a positive closure observation counts.
fn ended_on_scan(call: &CallWindow, scan: &CallScan) -> bool {
    scan.supported
        && scan.complete
        && !scan.active.iter().any(|active| active.key == call.key)
        && (scan.ended.iter().any(|ended| ended.key == call.key)
            || !scan.open_windows.contains(&call.window))
}

struct Observed {
    call: CallWindow,
    misses: u8,
    /// Which observation this is. A call remembered again gets a new serial, so
    /// a start decided on the earlier observation can tell it is out of date.
    serial: u64,
}

#[derive(Default)]
struct ObservedCalls {
    calls: HashMap<String, Observed>,
    last_serial: u64,
}

impl ObservedCalls {
    fn serial(&self, key: &str) -> Option<u64> {
        self.calls.get(key).map(|observed| observed.serial)
    }

    fn windows(&self) -> Vec<String> {
        let mut windows: Vec<String> = self
            .calls
            .values()
            .map(|observed| observed.call.window.clone())
            .collect();
        windows.sort();
        windows.dedup();
        windows
    }

    fn remember(&mut self, call: &CallWindow) -> u64 {
        self.last_serial += 1;
        let serial = self.last_serial;
        self.calls.insert(
            call.key.clone(),
            Observed {
                call: call.clone(),
                misses: 0,
                serial,
            },
        );
        serial
    }

    fn forget(&mut self, key: &str) {
        self.calls.remove(key);
    }

    fn interrupted(&mut self) {
        for observed in self.calls.values_mut() {
            observed.misses = 0;
        }
    }

    /// Counts the scan against every remembered call and returns the keys of the
    /// calls that have now ended.
    fn update(&mut self, scan: &CallScan) -> Vec<String> {
        let mut ended = Vec::new();
        for (key, observed) in self.calls.iter_mut() {
            observed.misses = if ended_on_scan(&observed.call, scan) {
                observed.misses + 1
            } else {
                0
            };
            if observed.misses >= MISSES_TO_END {
                ended.push(key.clone());
            }
        }
        for key in &ended {
            self.calls.remove(key);
        }
        ended
    }
}

/// A recording that call detection started and may stop when the call ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub call: CallWindow,
    pub session: String,
}

/// A call waiting for the user's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub call: CallWindow,
    serial: u64,
    key: String,
    /// True when an automatic start failed and the user may try again.
    pub retry: bool,
}

/// What the rest of the app looks like to the monitor.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    /// Nothing is recording, stopping or starting.
    pub idle: bool,
    /// A recording is running (a manual one, from the monitor's point of view).
    pub recording: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Nothing,
    /// The call of a recording that detection started has ended.
    Stop(Owner),
    /// Start recording this call without asking.
    Start {
        call: CallWindow,
        serial: u64,
    },
    /// A prompt was set; read it with `Tracker::prompt`.
    Prompt,
}

pub struct Tracker {
    settings: CallSettings,
    join: Option<join::Invitation>,
    /// Counts settings changes, so work begun under old settings can tell.
    revision: u64,
    seen: ObservedCalls,
    owner: Option<Owner>,
    prompt: Option<Prompt>,
}

impl Tracker {
    pub fn new(settings: CallSettings) -> Self {
        Self {
            settings,
            join: None,
            revision: 0,
            seen: ObservedCalls::default(),
            owner: None,
            prompt: None,
        }
    }

    pub fn settings(&self) -> &CallSettings {
        &self.settings
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn watching(&self) -> bool {
        self.join.is_some() || self.settings.watching()
    }

    fn providers(&self) -> Vec<Provider> {
        if self.join.is_none() {
            return self.settings.providers.clone();
        }
        let mut providers = if self.settings.watching() {
            self.settings.providers.clone()
        } else {
            vec![]
        };
        if !providers.contains(&Provider::Teams) {
            providers.push(Provider::Teams);
        }
        providers
    }

    fn mode_for(&self, call: &CallWindow) -> Mode {
        if let Some(join) = &self.join {
            return join.mode(call);
        }
        if self.settings.providers.contains(&call.provider) {
            self.settings.mode
        } else {
            Mode::Off
        }
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
    }

    pub fn watched_windows(&self) -> Vec<String> {
        self.seen.windows()
    }

    /// New settings drop the pending prompt and any start or scan decided under
    /// the old ones, and relinquish automatic stopping that is no longer
    /// allowed for the running recording.
    pub fn apply_settings(&mut self, next: CallSettings) {
        if self.settings == next {
            return;
        }
        self.revision += 1;
        if self.join.is_none() {
            self.prompt = None;
        }
        self.seen.interrupted();
        self.settings = next;
        let keep = self.owner.as_ref().is_some_and(|owner| {
            self.settings.auto_stop
                && self.settings.mode != Mode::Off
                && self.settings.providers.contains(&owner.call.provider)
        });
        if !keep {
            self.owner = None;
        }
    }

    /// One finished scan. `scan` is `None` when the scan itself failed, and
    /// `revision` is the one the scan began under.
    pub fn observe(&mut self, revision: u64, scan: Option<&CallScan>, ctx: Context) -> Decision {
        if revision != self.revision || !self.watching() {
            return Decision::Nothing;
        }
        let Some(scan) = scan.filter(|scan| scan.supported && scan.complete) else {
            // Unknown observation: it neither ends a call nor starts one.
            self.seen.interrupted();
            self.drop_prompt();
            return Decision::Nothing;
        };
        let prompt_gone = self.prompt.as_ref().is_some_and(|prompt| {
            !scan.active.contains(&prompt.call) || (self.join.is_some() && scan.active.len() != 1)
        });
        if prompt_gone {
            self.drop_prompt();
        }
        let ended = self.seen.update(scan);
        let owner_ended = self
            .owner
            .as_ref()
            .is_some_and(|owner| ended.contains(&owner.call.key));
        if owner_ended {
            let owner = self.owner.take();
            if self.settings.auto_stop {
                if let Some(owner) = owner {
                    return Decision::Stop(owner);
                }
            }
            return Decision::Nothing;
        }
        if scan.active.len() != 1 {
            return Decision::Nothing;
        }
        let call = &scan.active[0];
        let mode = self.mode_for(call);
        let baseline = self.join.as_ref().map_or(0, |join| join.baseline_serial);
        if mode == Mode::Off
            || self
                .seen
                .serial(&call.key)
                .is_some_and(|serial| serial > baseline)
        {
            return Decision::Nothing;
        }
        if !ctx.idle {
            // A manual recording seen during this call stays manual, also after
            // the user stops it. Anything else busy (for example a save) is
            // looked at again on the next scan.
            if ctx.recording && self.owner.is_none() {
                self.seen.remember(call);
            }
            return Decision::Nothing;
        }
        let serial = self.seen.remember(call);
        match mode {
            Mode::Auto => Decision::Start {
                call: call.clone(),
                serial,
            },
            Mode::Prompt => {
                self.prompt = Some(Prompt {
                    call: call.clone(),
                    serial,
                    key: self.prompt_key(call, serial),
                    retry: false,
                });
                Decision::Prompt
            }
            Mode::Off => Decision::Nothing,
        }
    }

    fn drop_prompt(&mut self) {
        if let Some(prompt) = self.prompt.take() {
            if self.join.is_some() {
                self.seen.forget(&prompt.call.key);
            }
        }
    }

    fn prompt_key(&self, call: &CallWindow, serial: u64) -> String {
        self.join.as_ref().map_or_else(
            || format!("{}-{serial}", call.key),
            |join| format!("{}-{serial}", join.id),
        )
    }

    /// Whether a start decided earlier may still go ahead. An automatic start
    /// needs automatic mode; one the user confirmed needs only that detection is
    /// still on for the call's app.
    pub fn may_begin(
        &self,
        call: &CallWindow,
        serial: u64,
        revision: u64,
        idle: bool,
        confirmed: bool,
    ) -> bool {
        let mode = self.mode_for(call);
        let mode_allows = mode == Mode::Auto || (confirmed && mode == Mode::Prompt);
        revision == self.revision
            && idle
            && mode_allows
            && self.seen.serial(&call.key) == Some(serial)
    }

    /// The user accepted the prompt for `key`. Returns what a start needs.
    pub fn confirm(&mut self, key: &str) -> Option<(CallWindow, u64, u64)> {
        let prompt = self.prompt.take_if(|prompt| prompt.key == key)?;
        Some((prompt.call, prompt.serial, self.revision))
    }

    /// The recording this call started is running; remember it for stopping.
    pub fn started(&mut self, call: &CallWindow, session: Option<String>, revision: u64) {
        if let Some(session) = session {
            if revision == self.revision
                && self.settings.auto_stop
                && self.settings.watching()
                && self.settings.providers.contains(&call.provider)
            {
                self.owner = Some(Owner {
                    call: call.clone(),
                    session,
                });
            }
        }
    }

    /// The start did not happen because the app was busy: look again next scan.
    pub fn start_deferred(&mut self, call: &CallWindow, serial: u64) {
        if self.seen.serial(&call.key) == Some(serial) {
            self.seen.forget(&call.key);
        }
    }

    /// The start failed. When trying again can help, offer it.
    pub fn start_failed(&mut self, call: &CallWindow, serial: u64, revision: u64, retryable: bool) {
        if retryable && revision == self.revision && self.seen.serial(&call.key) == Some(serial) {
            self.prompt = Some(Prompt {
                call: call.clone(),
                serial,
                key: self.prompt_key(call, serial),
                retry: true,
            });
        }
    }

    /// Stopping the recording failed; try again after more evidence.
    pub fn stop_failed(&mut self, owner: Owner, revision: u64) {
        if revision == self.revision {
            self.seen.remember(&owner.call);
            self.owner = Some(owner);
        }
    }
}

// ── Settings storage ──────────────────────────────────────────────────────

fn settings_path() -> Option<PathBuf> {
    if let Some(root) = crate::database::recovery::data_dir() {
        return Some(root.join("call_detection.json"));
    }
    dirs::config_dir().map(|dir| dir.join("ClawScribe").join("call_detection.json"))
}

fn load_settings() -> Option<CallSettings> {
    let text = std::fs::read_to_string(settings_path()?).ok()?;
    serde_json::from_str::<CallSettings>(&text)
        .ok()
        .map(CallSettings::normalized)
}

fn save_settings(settings: &CallSettings) -> Result<(), String> {
    const FAILED: &str = "Could not save call detection settings.";
    let path = settings_path().ok_or(FAILED)?;
    let parent = path.parent().ok_or(FAILED)?;
    std::fs::create_dir_all(parent).map_err(|_| FAILED.to_string())?;
    let json = serde_json::to_string_pretty(settings).map_err(|_| FAILED.to_string())?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, json).map_err(|_| FAILED.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|_| FAILED.to_string())
}

// ── Running monitor ───────────────────────────────────────────────────────

static TRACKER: LazyLock<Mutex<Tracker>> =
    LazyLock::new(|| Mutex::new(Tracker::new(load_settings().unwrap_or_default())));

/// The lock is never held across an await.
fn tracker() -> MutexGuard<'static, Tracker> {
    TRACKER.lock().unwrap_or_else(|error| error.into_inner())
}

/// What the window and the tray show for a pending prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptView {
    pub key: String,
    pub provider: Provider,
    pub retry: bool,
    pub event_title: Option<String>,
}

impl Tracker {
    fn prompt_view(&self) -> Option<PromptView> {
        self.prompt().map(|prompt| PromptView {
            key: prompt.key.clone(),
            provider: prompt.call.provider,
            retry: prompt.retry,
            event_title: self
                .join
                .as_ref()
                .map(|join| join.link.subject().unwrap_or("Untitled event").into()),
        })
    }
}

/// The prompt waiting for an answer, if any.
pub fn pending_prompt() -> Option<PromptView> {
    tracker().prompt_view()
}

/// Any new capture invalidates prompts and ownership from older sessions.
pub(crate) fn capture_started<R: Runtime>(app: &AppHandle<R>) {
    let before = {
        let mut tracker = tracker();
        tracker.owner = None;
        let before = tracker.prompt_view();
        tracker.drop_prompt();
        before
    };
    sync_prompt(app, before, None);
}

/// Tells the window and the tray when the pending prompt changed.
fn sync_prompt<R: Runtime>(
    app: &AppHandle<R>,
    before: Option<PromptView>,
    after: Option<PromptView>,
) {
    if before == after {
        return;
    }
    if before.as_ref().is_some_and(|p| p.event_title.is_some())
        || after.as_ref().is_some_and(|p| p.event_title.is_some())
    {
        let _ = app.emit("join-recording-changed", ());
    }
    match &after {
        Some(view) => {
            let _ = app.emit("call-detection-prompt", view.clone());
        }
        None => {
            let _ = app.emit("call-detection-prompt-cleared", ());
        }
    }
    crate::tray::update_tray_menu(app);
    prompt_window::refresh(app);
}

fn notify<R: Runtime>(app: &AppHandle<R>, kind: &str, provider: Provider) {
    let _ = app.emit(
        "call-detection-notice",
        serde_json::json!({ "kind": kind, "provider": provider, "label": provider.label() }),
    );
}

async fn context() -> Context {
    let recording = crate::audio::recording_commands::is_recording().await;
    let busy = crate::audio::recording_commands::capture_active()
        || crate::audio::recording_commands::is_starting()
        || start_request::is_pending();
    Context {
        idle: !busy,
        recording: recording && !crate::audio::recording_commands::is_stopping(),
    }
}

/// Starts the recording for a call through the window, after checking again
/// that this is still the one call and nothing else is recording.
async fn begin<R: Runtime>(
    app: &AppHandle<R>,
    call: CallWindow,
    serial: u64,
    revision: u64,
    confirmed: bool,
) {
    let ctx = context().await;
    let allowed = {
        let tracker = tracker();
        tracker.may_begin(&call, serial, revision, ctx.idle, confirmed)
    };
    if !allowed {
        tracker().start_deferred(&call, serial);
        return;
    }
    // Even a click on a prompt that is still visible, or a retry, needs fresh
    // evidence that this call is the only active one.
    let providers = tracker().providers();
    let scan = super::calls::scan(providers, vec![call.window.clone()])
        .await
        .ok();
    let same_call = scan.as_ref().is_some_and(|scan| {
        scan.supported && scan.complete && scan.active.as_slice() == std::slice::from_ref(&call)
    });
    let ctx = context().await;
    let invitation = {
        let mut tracker = tracker();
        if !same_call
            || ctx.recording
            || !tracker.may_begin(&call, serial, revision, ctx.idle, confirmed)
        {
            tracker.start_deferred(&call, serial);
            return;
        }
        tracker
            .join
            .as_ref()
            .map(|j| (j.id.clone(), j.link.clone()))
    };
    let (join_id, calendar) = invitation.unzip();
    // Admitting a calendar start consumes its invitation and advances the revision once.
    let admitted_revision = if calendar.is_some() {
        revision + 1
    } else {
        revision
    };

    let evidence = CallStartEvidence {
        call: call.clone(),
        serial,
        revision,
        confirmed,
        observed: std::time::Instant::now(),
        calendar,
    };
    let Some(outcome) =
        start_request::request_and_wait(app, StartSource::Call(call.provider), Some(evidence))
            .await
    else {
        // Reconsider only on a later completed scan with fresh evidence.
        tracker().start_deferred(&call, serial);
        return;
    };
    match outcome {
        StartOutcome::Started { session_id } => {
            let mut monitor = tracker();
            let session_id = session_id
                .filter(|id| crate::audio::recording_commands::active_capture(id).is_some());
            monitor.started(&call, session_id, admitted_revision);
            drop(monitor);
            notify(app, "started", call.provider);
        }
        StartOutcome::Busy => {
            tracker().start_deferred(&call, serial);
        }
        StartOutcome::Failed { retryable } => {
            if !retryable {
                if let Some(id) = join_id {
                    join::cancel(app, &id);
                }
            } else {
                offer_retry(app, &call, serial, revision, true);
            }
        }
        StartOutcome::TimedOut => {
            log::warn!("The window did not answer the call recording request in time");
            offer_retry(app, &call, serial, revision, true);
        }
    }
}

pub(crate) struct CallStartEvidence {
    call: CallWindow,
    serial: u64,
    revision: u64,
    confirmed: bool,
    observed: std::time::Instant,
    pub(crate) calendar: Option<CalendarLink>,
}

impl CallStartEvidence {
    pub(crate) fn is_current(&self) -> bool {
        self.observed.elapsed() < Duration::from_secs(10)
            && tracker().may_begin(&self.call, self.serial, self.revision, true, self.confirmed)
    }

    pub(crate) async fn verify(&self) -> bool {
        if !self.is_current() {
            return false;
        }
        let providers = tracker().providers();
        let scan = super::calls::scan(providers, vec![self.call.window.clone()]).await;
        self.is_current()
            && scan.is_ok_and(|scan| {
                scan.supported
                    && scan.complete
                    && scan.active.as_slice() == std::slice::from_ref(&self.call)
            })
    }

    /// Linearization point for cancellation, expiry and recording admission.
    pub(crate) fn admit<R: Runtime>(&self, app: &AppHandle<R>) -> Result<(), String> {
        let mut tracker = tracker();
        if self.observed.elapsed() >= Duration::from_secs(10)
            || !tracker.may_begin(&self.call, self.serial, self.revision, true, self.confirmed)
        {
            return Err(start_request::EXPIRED.into());
        }
        if self.calendar.is_some() {
            let id = tracker
                .join
                .as_ref()
                .ok_or(start_request::EXPIRED)?
                .id
                .clone();
            tracker.clear_join(&id);
            drop(tracker);
            let _ = app.emit("join-recording-changed", ());
        }
        Ok(())
    }
}

fn offer_retry<R: Runtime>(
    app: &AppHandle<R>,
    call: &CallWindow,
    serial: u64,
    revision: u64,
    retryable: bool,
) {
    let (before, after) = {
        let mut tracker = tracker();
        let before = tracker.prompt_view();
        tracker.start_failed(call, serial, revision, retryable);
        (before, tracker.prompt_view())
    };
    sync_prompt(app, before, after);
}

/// Stops the recording a call started, once the call has ended.
async fn stop_owned<R: Runtime>(app: &AppHandle<R>, owner: Owner, revision: u64) -> bool {
    let provider = owner.call.provider;
    match crate::audio::recording_commands::stop_detected_call_recording(
        app.clone(),
        owner.session.clone(),
    )
    .await
    {
        Ok(true) => {
            notify(app, "stopped", provider);
            true
        }
        Ok(false) => false,
        Err(error) => {
            log::warn!("Could not stop the recording of an ended call: {error}");
            tracker().stop_failed(owner, revision);
            notify(app, "stop-failed", provider);
            false
        }
    }
}

async fn tick<R: Runtime>(app: AppHandle<R>) -> bool {
    let (providers, windows, revision) = {
        let tracker = tracker();
        if !tracker.watching() {
            return false;
        }
        (
            tracker.providers(),
            tracker.watched_windows(),
            tracker.revision(),
        )
    };
    if !crate::tray::check_can_record(&app).await {
        return false;
    }
    let scan = super::calls::scan(providers, windows).await.ok();
    let ctx = context().await;
    let (before, decision, after) = {
        let mut tracker = tracker();
        let before = tracker.prompt_view();
        let decision = tracker.observe(revision, scan.as_ref(), ctx);
        (before, decision, tracker.prompt_view())
    };
    sync_prompt(&app, before, after);
    match decision {
        Decision::Nothing | Decision::Prompt => false,
        Decision::Stop(owner) => stop_owned(&app, owner, revision).await,
        Decision::Start { call, serial } => {
            begin(&app, call, serial, revision, false).await;
            false
        }
    }
}

/// Starts the monitor. It idles while detection is off.
pub fn spawn<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        loop {
            // A tick that panics must not end the monitor.
            match tauri::async_runtime::spawn(tick(app.clone())).await {
                Ok(true) => continue, // A new call may have started while the old recording saved.
                Ok(false) => {}
                Err(error) => log::error!("Call monitor: a scan failed unexpectedly: {error}"),
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

/// The user answered the prompt for `key` (from the window or the tray).
pub(crate) async fn confirm_prompt<R: Runtime>(app: &AppHandle<R>, key: &str) {
    let (before, taken) = {
        let mut tracker = tracker();
        let before = tracker.prompt_view();
        let taken = tracker.confirm(key);
        (before, taken)
    };
    let Some((call, serial, revision)) = taken else {
        return;
    };
    sync_prompt(app, before, None);
    begin(app, call, serial, revision, true).await;
}

// ── Commands ──────────────────────────────────────────────────────────────

/// The settings the core holds, or `None` when it has none yet (the window then
/// hands over what it has, such as settings from an earlier version).
#[tauri::command]
pub fn get_call_detection_settings() -> Option<CallSettings> {
    load_settings()
}

#[tauri::command]
pub fn set_call_detection_settings<R: Runtime>(
    app: AppHandle<R>,
    settings: CallSettings,
) -> Result<(), String> {
    let settings = settings.normalized();
    save_settings(&settings)?;
    let (before, after) = {
        let mut tracker = tracker();
        let before = tracker.prompt_view();
        tracker.apply_settings(settings);
        (before, tracker.prompt_view())
    };
    sync_prompt(&app, before, after);
    Ok(())
}

#[tauri::command]
pub fn get_pending_call_prompt() -> Option<PromptView> {
    pending_prompt()
}

#[tauri::command]
pub async fn confirm_call_detection_prompt<R: Runtime>(
    app: AppHandle<R>,
    key: String,
) -> Result<(), String> {
    confirm_prompt(&app, &key).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(provider: Provider, window: &str, key: &str) -> CallWindow {
        CallWindow {
            provider,
            window: window.to_string(),
            key: key.to_string(),
            teams_room: None,
        }
    }

    fn meet() -> CallWindow {
        call(Provider::GoogleMeet, "9-a", "room-a")
    }

    fn snapshot(active: &[CallWindow], open: &[&str], ended: &[CallWindow]) -> CallScan {
        CallScan {
            supported: true,
            complete: true,
            active: active.to_vec(),
            ended: ended.to_vec(),
            open_windows: open.iter().map(|window| window.to_string()).collect(),
        }
    }

    fn settings(mode: Mode, auto_stop: bool) -> CallSettings {
        CallSettings {
            mode,
            providers: vec![Provider::GoogleMeet],
            auto_stop,
        }
    }

    const IDLE: Context = Context {
        idle: true,
        recording: false,
    };

    #[test]
    fn settings_keep_the_listed_order_and_each_app_once() {
        let normalized = CallSettings {
            mode: Mode::Prompt,
            providers: vec![Provider::GoogleMeet, Provider::Teams, Provider::GoogleMeet],
            auto_stop: false,
        }
        .normalized();
        assert_eq!(
            normalized.providers,
            vec![Provider::Teams, Provider::GoogleMeet]
        );
    }

    #[test]
    fn settings_from_the_window_parse_with_the_names_it_uses() {
        let parsed: CallSettings = serde_json::from_str(
            r#"{"mode":"auto","providers":["teams","googleMeet"],"autoStop":true}"#,
        )
        .unwrap();
        assert_eq!(parsed.mode, Mode::Auto);
        assert_eq!(
            parsed.providers,
            vec![Provider::Teams, Provider::GoogleMeet]
        );
        assert!(parsed.auto_stop);
        let without_stop: CallSettings =
            serde_json::from_str(r#"{"mode":"off","providers":[]}"#).unwrap();
        assert!(!without_stop.auto_stop);
    }

    #[test]
    fn a_hidden_tab_incomplete_scan_or_failed_scan_resets_closure_evidence() {
        let mut seen = ObservedCalls::default();
        seen.remember(&meet());
        let incomplete = CallScan {
            complete: false,
            ..snapshot(&[], &[], &[])
        };
        let still_open = snapshot(&[], &["9-a"], &[]);
        let call_active = snapshot(&[meet()], &["9-a"], &[]);
        for unknown in [&still_open, &incomplete, &call_active] {
            assert!(seen.update(&snapshot(&[], &[], &[])).is_empty());
            assert!(seen.update(&snapshot(&[], &[], &[])).is_empty());
            assert!(seen.update(unknown).is_empty());
        }
        seen.interrupted();
        assert!(seen.update(&snapshot(&[], &[], &[])).is_empty());
        assert!(seen.update(&snapshot(&[], &[], &[])).is_empty());
        let ended = seen.update(&snapshot(&[], &["9-a"], &[meet()]));
        assert_eq!(ended, vec!["room-a".to_string()]);
    }

    #[test]
    fn automatic_mode_starts_once_and_stops_only_after_sustained_absence() {
        let mut tracker = Tracker::new(settings(Mode::Auto, true));
        let revision = tracker.revision();
        let scan = snapshot(&[meet()], &["9-a"], &[]);
        let decision = tracker.observe(revision, Some(&scan), IDLE);
        let Decision::Start { call, serial } = decision else {
            panic!("expected a start, got {decision:?}");
        };
        assert!(tracker.may_begin(&call, serial, revision, true, false));
        tracker.started(&call, Some("session-1".to_string()), revision);

        // The same call is not started twice.
        let busy = Context {
            idle: false,
            recording: true,
        };
        assert_eq!(
            tracker.observe(revision, Some(&scan), busy),
            Decision::Nothing
        );

        let gone = snapshot(&[], &[], &[]);
        assert_eq!(
            tracker.observe(revision, Some(&snapshot(&[], &["9-a"], &[])), busy),
            Decision::Nothing
        );
        assert_eq!(
            tracker.observe(revision, Some(&gone), busy),
            Decision::Nothing
        );
        assert_eq!(tracker.observe(revision, None, busy), Decision::Nothing);
        assert_eq!(
            tracker.observe(revision, Some(&gone), busy),
            Decision::Nothing
        );
        assert_eq!(
            tracker.observe(revision, Some(&gone), busy),
            Decision::Nothing
        );
        let stop = tracker.observe(revision, Some(&gone), busy);
        assert!(matches!(&stop, Decision::Stop(owner) if owner.session == "session-1"));
        // Once only.
        assert_eq!(
            tracker.observe(revision, Some(&gone), busy),
            Decision::Nothing
        );
    }

    #[test]
    fn a_manual_recording_is_never_stopped_and_a_revoked_grant_is_honoured() {
        // A recording that was already running stays manual.
        let mut manual = Tracker::new(settings(Mode::Auto, true));
        let revision = manual.revision();
        let running = Context {
            idle: false,
            recording: true,
        };
        let scan = snapshot(&[meet()], &["9-a"], &[]);
        assert_eq!(
            manual.observe(revision, Some(&scan), running),
            Decision::Nothing
        );
        for _ in 0..4 {
            assert_eq!(
                manual.observe(revision, Some(&snapshot(&[], &[], &[])), running),
                Decision::Nothing
            );
        }

        // Switching automatic stopping off relinquishes the recording.
        let mut revoked = Tracker::new(settings(Mode::Auto, true));
        let revision = revoked.revision();
        let Decision::Start { call, .. } = revoked.observe(revision, Some(&scan), IDLE) else {
            panic!("expected a start");
        };
        revoked.started(&call, Some("session-2".to_string()), revision);
        revoked.apply_settings(settings(Mode::Auto, false));
        let revision = revoked.revision();
        for _ in 0..4 {
            assert_eq!(
                revoked.observe(revision, Some(&snapshot(&[], &[], &[])), running),
                Decision::Nothing
            );
        }
    }

    #[test]
    fn a_scan_started_under_old_settings_is_discarded() {
        let mut tracker = Tracker::new(settings(Mode::Auto, false));
        let old = tracker.revision();
        tracker.apply_settings(settings(Mode::Auto, true));
        let scan = snapshot(&[meet()], &["9-a"], &[]);
        assert_eq!(tracker.observe(old, Some(&scan), IDLE), Decision::Nothing);
        assert!(matches!(
            tracker.observe(tracker.revision(), Some(&scan), IDLE),
            Decision::Start { .. }
        ));
    }

    #[test]
    fn a_prompt_waits_for_an_answer_and_rechecks_before_starting() {
        let mut tracker = Tracker::new(settings(Mode::Prompt, false));
        let revision = tracker.revision();
        let scan = snapshot(&[meet()], &["9-a"], &[]);
        assert_eq!(
            tracker.observe(revision, Some(&scan), IDLE),
            Decision::Prompt
        );
        assert_eq!(
            tracker.prompt().map(|p| p.call.key.as_str()),
            Some("room-a")
        );
        // Seen calls are not prompted again.
        assert_eq!(
            tracker.observe(revision, Some(&scan), IDLE),
            Decision::Nothing
        );
        assert!(tracker.confirm("another-room").is_none());
        let key = tracker.prompt_view().unwrap().key;
        let (call, serial, confirmed_revision) = tracker.confirm(&key).unwrap();
        assert!(tracker.prompt().is_none());
        assert!(tracker.may_begin(&call, serial, confirmed_revision, true, true));
        // Automatic starts are not allowed in prompt mode.
        assert!(!tracker.may_begin(&call, serial, confirmed_revision, true, false));
        // Switching detection off makes the stale answer void.
        tracker.apply_settings(settings(Mode::Off, false));
        assert!(!tracker.may_begin(&call, serial, confirmed_revision, true, true));
    }

    #[test]
    fn a_prompt_goes_away_with_its_call_and_with_new_settings() {
        let mut tracker = Tracker::new(settings(Mode::Prompt, false));
        let revision = tracker.revision();
        let scan = snapshot(&[meet()], &["9-a"], &[]);
        tracker.observe(revision, Some(&scan), IDLE);
        assert!(tracker.prompt().is_some());
        tracker.observe(revision, Some(&snapshot(&[], &["9-a"], &[])), IDLE);
        assert!(tracker.prompt().is_none());

        let mut other = Tracker::new(settings(Mode::Prompt, false));
        let revision = other.revision();
        other.observe(revision, Some(&scan), IDLE);
        other.apply_settings(settings(Mode::Prompt, true));
        assert!(other.prompt().is_none());
    }

    #[test]
    fn a_busy_start_is_looked_at_again_and_a_failed_one_can_be_retried() {
        let mut tracker = Tracker::new(settings(Mode::Auto, false));
        let revision = tracker.revision();
        let scan = snapshot(&[meet()], &["9-a"], &[]);
        let Decision::Start { call, serial } = tracker.observe(revision, Some(&scan), IDLE) else {
            panic!("expected a start");
        };
        tracker.start_deferred(&call, serial);
        assert!(matches!(
            tracker.observe(revision, Some(&scan), IDLE),
            Decision::Start { .. }
        ));

        let mut failing = Tracker::new(settings(Mode::Auto, false));
        let revision = failing.revision();
        let Decision::Start { call, serial } = failing.observe(revision, Some(&scan), IDLE) else {
            panic!("expected a start");
        };
        failing.start_failed(&call, serial, revision, false);
        assert!(failing.prompt().is_none(), "a full disk cannot be retried");
        failing.start_failed(&call, serial, revision, true);
        let prompt = failing.prompt().expect("retry offered");
        assert!(prompt.retry);
        let key = prompt.key.clone();
        let (call, serial, revision) = failing.confirm(&key).unwrap();
        assert!(failing.may_begin(&call, serial, revision, true, true));
    }

    #[test]
    fn next_teams_call_stays_eligible_while_the_owned_call_closes_and_saves() {
        let mut tracker = Tracker::new(CallSettings {
            mode: Mode::Auto,
            providers: vec![Provider::Teams],
            auto_stop: true,
        });
        let revision = tracker.revision();
        let first = snapshot(
            &[call(Provider::Teams, "first-window", "first-call")],
            &["first-window"],
            &[],
        );
        let Decision::Start {
            call: first_call, ..
        } = tracker.observe(revision, Some(&first), IDLE)
        else {
            panic!("expected a start");
        };
        tracker.started(&first_call, Some("first-session".into()), revision);
        let next_call = call(Provider::Teams, "second-window", "second-call");
        let next = snapshot(&[next_call.clone()], &["second-window"], &[]);
        let running = Context {
            idle: false,
            recording: true,
        };
        for _ in 1..MISSES_TO_END {
            assert_eq!(
                tracker.observe(revision, Some(&next), running),
                Decision::Nothing
            );
        }
        assert!(
            matches!(tracker.observe(revision, Some(&next), running), Decision::Stop(owner)
            if owner.session == "first-session")
        );
        let saving = Context {
            idle: false,
            recording: false,
        };
        assert_eq!(
            tracker.observe(revision, Some(&next), saving),
            Decision::Nothing
        );
        assert!(
            matches!(tracker.observe(revision, Some(&next), IDLE), Decision::Start { call, .. }
            if call == next_call)
        );
    }

    #[test]
    fn a_failed_stop_is_retried_after_more_evidence() {
        let mut tracker = Tracker::new(settings(Mode::Auto, true));
        let revision = tracker.revision();
        let scan = snapshot(&[meet()], &["9-a"], &[]);
        let Decision::Start { call, .. } = tracker.observe(revision, Some(&scan), IDLE) else {
            panic!("expected a start");
        };
        tracker.started(&call, Some("session-3".to_string()), revision);
        let busy = Context {
            idle: false,
            recording: true,
        };
        let gone = snapshot(&[], &[], &[]);
        for _ in 0..2 {
            tracker.observe(revision, Some(&gone), busy);
        }
        let Decision::Stop(owner) = tracker.observe(revision, Some(&gone), busy) else {
            panic!("expected a stop");
        };
        tracker.stop_failed(owner, revision);
        for _ in 0..2 {
            assert_eq!(
                tracker.observe(revision, Some(&gone), busy),
                Decision::Nothing
            );
        }
        assert!(matches!(
            tracker.observe(revision, Some(&gone), busy),
            Decision::Stop(_)
        ));
    }

    #[test]
    fn two_active_calls_are_ambiguous_and_nothing_starts() {
        let mut tracker = Tracker::new(settings(Mode::Auto, false));
        let revision = tracker.revision();
        let other = call(Provider::GoogleMeet, "9-b", "room-b");
        let scan = snapshot(&[meet(), other], &["9-a", "9-b"], &[]);
        assert_eq!(
            tracker.observe(revision, Some(&scan), IDLE),
            Decision::Nothing
        );
    }
}
