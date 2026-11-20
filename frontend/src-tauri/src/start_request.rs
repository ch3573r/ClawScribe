//! One gate for every start of a recording that the core asks the window to carry
//! out: the tray, the global shortcut, the command line and call detection.
//!
//! The window knows the chosen audio devices and the calendar title for the next
//! recording, so it performs the start. The core only asks, in the background:
//! nothing navigates, nothing takes the focus, and the page the user is on and
//! any draft in it stay as they are.
//!
//! At most one request is open at a time, and none is issued while a recording is
//! running or being stopped, so a repeated shortcut, a second command-line start
//! or a detected call arriving together cannot start a recording twice. The
//! window can decline a request before it is claimed. After claim, the core
//! supplies the actual result. Unclaimed requests expire after `START_TIMEOUT`.
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::oneshot;

use crate::teams_detection::calls::Provider;

/// How long the window has to deliver an unclaimed request to the recorder.
pub const START_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartSource {
    Tray,
    Shortcut,
    Cli,
    Call(Provider),
}

impl StartSource {
    fn name(self) -> &'static str {
        match self {
            StartSource::Tray => "tray",
            StartSource::Shortcut => "shortcut",
            StartSource::Cli => "cli",
            StartSource::Call(_) => "call",
        }
    }

    fn provider(self) -> Option<Provider> {
        match self {
            StartSource::Call(provider) => Some(provider),
            _ => None,
        }
    }
}

/// How the window answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartOutcome {
    /// The recording is running; `session_id` is set for a detected call.
    Started { session_id: Option<String> },
    /// The window was busy (starting, stopping or saving); nothing was started.
    Busy,
    /// The start failed. `retryable` is false when trying again cannot help
    /// until the user acts, for example with a full disk.
    Failed { retryable: bool },
    /// Nobody answered in time.
    TimedOut,
}

/// What the window is told.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequestView {
    pub request_id: String,
    pub source: &'static str,
    pub provider: Option<Provider>,
}

struct Pending {
    view: StartRequestView,
    source: StartSource,
    issued: Instant,
    call: Option<crate::teams_detection::monitor::CallStartEvidence>,
    reply: Option<oneshot::Sender<StartOutcome>>,
}

pub(crate) struct ClaimedStart {
    source: StartSource,
    call: Option<crate::teams_detection::monitor::CallStartEvidence>,
    reply: oneshot::Sender<StartOutcome>,
}

pub(crate) const EXPIRED: &str = "recording_request_expired";

impl ClaimedStart {
    pub(crate) fn is_call(&self) -> bool {
        matches!(self.source, StartSource::Call(_))
    }

    pub(crate) fn check_call(&self) -> Result<(), String> {
        if self
            .call
            .as_ref()
            .is_some_and(|evidence| !evidence.is_current())
        {
            return Err(EXPIRED.into());
        }
        Ok(())
    }

    pub(crate) async fn verify_call(&self) -> Result<(), String> {
        if let Some(evidence) = &self.call {
            if !evidence.verify().await {
                return Err(EXPIRED.into());
            }
        }
        Ok(())
    }

    pub(crate) fn complete(self, result: &Result<String, String>) {
        let outcome = match result {
            Ok(session) => StartOutcome::Started {
                session_id: Some(session.clone()),
            },
            Err(error) if error == EXPIRED => StartOutcome::TimedOut,
            Err(error) if error == "Recording already in progress" => StartOutcome::Busy,
            Err(error) => StartOutcome::Failed {
                retryable: !error.starts_with("Not enough free disk space to record.")
                    && !error.starts_with("live_model:")
                    && error != "recording_setup_incomplete",
            },
        };
        let _ = self.reply.send(outcome);
    }
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// The lock is never held across an await.
fn slot() -> MutexGuard<'static, Option<Pending>> {
    PENDING.lock().unwrap_or_else(|error| error.into_inner())
}

/// Whether a request is open (its start is under way or about to be).
pub fn is_pending() -> bool {
    slot().is_some()
}

/// Opens a request and tells the window. `None` when another request is open or
/// a recording is running or stopping.
async fn issue<R: Runtime>(
    app: &AppHandle<R>,
    source: StartSource,
    call: Option<crate::teams_detection::monitor::CallStartEvidence>,
) -> Option<(StartRequestView, oneshot::Receiver<StartOutcome>)> {
    use crate::audio::recording_commands::{capture_active, is_starting, start_allowed};
    if start_allowed().is_err()
        || !crate::tray::check_can_record(app).await
        || capture_active()
        || is_starting()
    {
        return None;
    }
    let (reply, receiver) = oneshot::channel();
    let view = StartRequestView {
        request_id: uuid::Uuid::new_v4().to_string(),
        source: source.name(),
        provider: source.provider(),
    };
    {
        let mut pending = slot();
        if pending.is_some() {
            return None;
        }
        *pending = Some(Pending {
            view: view.clone(),
            source,
            issued: Instant::now(),
            call,
            reply: Some(reply),
        });
    }
    if let Err(error) = app.emit("recording-start-requested", view.clone()) {
        // The request stays open and ends by timeout; the window may still pick
        // it up when it next loads.
        log::warn!("Could not tell the window about a recording request: {error}");
    }
    Some((view, receiver))
}

fn close(request_id: &str) {
    let mut pending = slot();
    if pending
        .as_ref()
        .is_some_and(|open| open.view.request_id == request_id)
    {
        *pending = None;
    }
}

/// Asks the window to start a recording and waits for its answer. `None` when no
/// request could be opened (one is already open, or a recording is running).
pub(crate) async fn request_and_wait<R: Runtime>(
    app: &AppHandle<R>,
    source: StartSource,
    call: Option<crate::teams_detection::monitor::CallStartEvidence>,
) -> Option<StartOutcome> {
    let (view, mut receiver) = issue(app, source, call).await?;
    log::info!("Recording start requested from {}", view.source);
    crate::tray::update_tray_menu(app);
    let outcome = match tokio::time::timeout(START_TIMEOUT, &mut receiver).await {
        Ok(outcome) => outcome.unwrap_or(StartOutcome::TimedOut),
        Err(_) if expire_unclaimed(&view.request_id) => StartOutcome::TimedOut,
        // Once claimed, native code owns completion and the actual session ID.
        Err(_) => receiver.await.unwrap_or(StartOutcome::TimedOut),
    };
    close(&view.request_id);
    crate::tray::update_tray_menu(app);
    Some(outcome)
}

fn expire_unclaimed(request_id: &str) -> bool {
    let mut pending = slot();
    if pending
        .as_ref()
        .is_some_and(|open| open.view.request_id == request_id && open.reply.is_some())
    {
        *pending = None;
        return true;
    }
    false
}

pub(crate) fn claim(request_id: &str) -> Result<ClaimedStart, String> {
    let mut pending = slot();
    let open = pending
        .as_mut()
        .filter(|open| open.view.request_id == request_id && open.issued.elapsed() < START_TIMEOUT)
        .ok_or(EXPIRED)?;
    let reply = open.reply.take().ok_or(EXPIRED)?;
    Ok(ClaimedStart {
        source: open.source,
        call: open.call.take(),
        reply,
    })
}

/// Delivers the window's answer. Returns false when nothing was waiting for it
/// (a late or repeated answer).
pub fn finish(request_id: &str, outcome: StartOutcome) -> bool {
    let reply = {
        let mut pending = slot();
        match pending.as_mut() {
            Some(open) if open.view.request_id == request_id => open.reply.take(),
            _ => None,
        }
    };
    match reply {
        Some(reply) => reply.send(outcome).is_ok(),
        None => false,
    }
}

fn outcome_from_status(status: &str) -> StartOutcome {
    match status {
        "busy" => StartOutcome::Busy,
        "refused" => StartOutcome::Failed { retryable: false },
        _ => StartOutcome::Failed { retryable: true },
    }
}

/// The window's answer to a recording request.
#[tauri::command]
pub fn finish_recording_start(request_id: String, status: String) -> bool {
    finish(&request_id, outcome_from_status(&status))
}

/// The open request, if any. The window asks when it loads, in case a request
/// arrived before it was listening.
#[tauri::command]
pub fn get_pending_start_request() -> Option<StartRequestView> {
    let pending = slot();
    pending
        .as_ref()
        .filter(|open| open.reply.is_some() && open.issued.elapsed() < START_TIMEOUT)
        .map(|open| open.view.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_status_maps_to_one_outcome() {
        assert_eq!(outcome_from_status("busy"), StartOutcome::Busy);
        assert_eq!(
            outcome_from_status("refused"),
            StartOutcome::Failed { retryable: false }
        );
        assert_eq!(
            outcome_from_status("failed"),
            StartOutcome::Failed { retryable: true }
        );
        assert_eq!(
            outcome_from_status("started"),
            StartOutcome::Failed { retryable: true }
        );
    }

    #[test]
    fn an_expired_request_cannot_open_capture_but_claimed_work_keeps_ownership() {
        fn install(issued: Instant) -> oneshot::Receiver<StartOutcome> {
            let (reply, receiver) = oneshot::channel();
            *slot() = Some(Pending {
                view: StartRequestView {
                    request_id: "request-a".into(),
                    source: "call",
                    provider: Some(Provider::Zoom),
                },
                source: StartSource::Call(Provider::Zoom),
                issued,
                call: None,
                reply: Some(reply),
            });
            receiver
        }
        let _expired_receiver = install(Instant::now() - START_TIMEOUT);
        assert!(get_pending_start_request().is_none());
        assert!(claim("request-a").is_err());
        assert!(expire_unclaimed("request-a"));
        assert!(!is_pending());

        let mut receiver = install(Instant::now());
        assert!(claim("request-b").is_err());
        let owner = claim("request-a").unwrap();
        assert!(owner.is_call());
        assert!(claim("request-a").is_err());
        assert!(!finish("request-a", StartOutcome::Busy));
        assert!(!expire_unclaimed("request-a"));
        owner.complete(&Ok("owned-session".into()));
        assert_eq!(
            receiver.try_recv().ok(),
            Some(StartOutcome::Started {
                session_id: Some("owned-session".into()),
            })
        );
        close("request-b");
        assert!(is_pending());
        close("request-a");
        assert!(!is_pending());
    }

    #[test]
    fn a_request_carries_its_source_and_app() {
        assert_eq!(StartSource::Shortcut.name(), "shortcut");
        assert_eq!(StartSource::Cli.provider(), None);
        assert_eq!(
            StartSource::Call(Provider::Zoom).provider(),
            Some(Provider::Zoom)
        );
        assert_eq!(StartSource::Call(Provider::Zoom).name(), "call");
    }
}
