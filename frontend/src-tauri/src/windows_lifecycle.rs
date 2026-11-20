//! Best-effort Windows power and session notifications for the app's own HWND.
use crate::audio::recording_commands;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::Notify;
use windows::Win32::{
    Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM},
    System::Power::{
        RegisterSuspendResumeNotification, UnregisterSuspendResumeNotification, HPOWERNOTIFY,
    },
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetSystemMetrics, DEVICE_NOTIFY_WINDOW_HANDLE, PBT_APMRESUMEAUTOMATIC,
            PBT_APMRESUMECRITICAL, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND, SM_SHUTTINGDOWN,
            WM_ENDSESSION, WM_NCDESTROY, WM_POWERBROADCAST, WM_QUERYENDSESSION,
        },
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Signal {
    Suspend,
    Resume,
    QueryEnd,
    ConfirmEnd,
    CancelEnd,
}

fn decode(message: u32, wparam: usize) -> Option<Signal> {
    match message {
        WM_QUERYENDSESSION => Some(Signal::QueryEnd),
        WM_ENDSESSION if wparam != 0 => Some(Signal::ConfirmEnd),
        WM_ENDSESSION => Some(Signal::CancelEnd),
        WM_POWERBROADCAST => match wparam as u32 {
            PBT_APMSUSPEND => Some(Signal::Suspend),
            PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND | PBT_APMRESUMECRITICAL => {
                Some(Signal::Resume)
            }
            _ => None,
        },
        _ => None,
    }
}

struct Signals {
    start_attempt: AtomicU64,
    suspending: AtomicBool,
    session_ending: AtomicBool,
    checkpoint: AtomicU64,
    suspend_stop: AtomicU64,
    wake: Notify,
}

impl Signals {
    const fn new() -> Self {
        Self {
            start_attempt: AtomicU64::new(0),
            suspending: AtomicBool::new(false),
            session_ending: AtomicBool::new(false),
            checkpoint: AtomicU64::new(0),
            suspend_stop: AtomicU64::new(0),
            wake: Notify::const_new(),
        }
    }

    fn begin_start(&self) -> Result<u64, &'static str> {
        const SUSPENDING: &str = "Windows is going to sleep. Start recording after it wakes.";
        if self.suspending.load(Ordering::SeqCst) {
            return Err(SUSPENDING);
        }
        let attempt = self.start_attempt.fetch_add(1, Ordering::SeqCst) + 1;
        // A suspend racing this reservation either sees the new attempt or
        // closes admission before the start can open any audio streams.
        if self.suspending.load(Ordering::SeqCst) {
            return Err(SUSPENDING);
        }
        Ok(attempt)
    }

    fn receive(&self, signal: Signal) {
        match signal {
            Signal::Suspend => {
                if !self.suspending.swap(true, Ordering::SeqCst) {
                    let attempt = self.start_attempt.load(Ordering::SeqCst);
                    self.suspend_stop.store(attempt, Ordering::Release);
                    self.checkpoint.store(attempt, Ordering::Release);
                    self.wake.notify_one();
                }
            }
            Signal::Resume => self.suspending.store(false, Ordering::SeqCst),
            Signal::QueryEnd | Signal::ConfirmEnd => {
                if !self.session_ending.swap(true, Ordering::AcqRel) {
                    let attempt = self.start_attempt.load(Ordering::SeqCst);
                    self.checkpoint.store(attempt, Ordering::Release);
                    self.wake.notify_one();
                }
            }
            Signal::CancelEnd => self.session_ending.store(false, Ordering::Release),
        }
    }

    fn is_current(&self, attempt: u64) -> bool {
        attempt != 0 && self.start_attempt.load(Ordering::SeqCst) == attempt
    }

    fn take_checkpoint(&self) -> u64 {
        self.checkpoint.swap(0, Ordering::AcqRel)
    }

    fn stop_ready(&self, dispatched: u64, recording_ready: impl FnOnce() -> bool) -> Option<u64> {
        let attempt = self.suspend_stop.load(Ordering::Acquire);
        (attempt != dispatched && self.is_current(attempt) && recording_ready()).then_some(attempt)
    }
}

static SIGNALS: Signals = Signals::new();

thread_local! {
    static POWER_NOTIFICATION: Cell<Option<HPOWERNOTIFY>> = const { Cell::new(None) };
}

pub(crate) fn begin_start() -> Result<(), &'static str> {
    SIGNALS.begin_start().map(drop)
}

pub(crate) fn start_settled() {
    SIGNALS.wake.notify_one();
}

pub(crate) fn is_current(attempt: u64) -> bool {
    SIGNALS.is_current(attempt)
}

pub(crate) fn suspend_requested() -> bool {
    SIGNALS.is_current(SIGNALS.suspend_stop.load(Ordering::Acquire))
}

pub(crate) fn session_ending() -> bool {
    // Tao's hidden HWND can receive WM_ENDSESSION ahead of the main window.
    SIGNALS.session_ending.load(Ordering::Acquire)
        || unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
}

/// Install on the HWND's owner thread from Tauri setup. A tray-hidden main
/// window still owns the subclass and the power registration.
pub(crate) fn install<R: Runtime>(app: &AppHandle<R>) {
    let Some(hwnd) = app
        .get_webview_window("main")
        .and_then(|window| window.hwnd().ok())
    else {
        log::warn!("Windows recording lifecycle notifications need a main window");
        return;
    };
    let hwnd = HWND(hwnd.0 as isize);
    // SAFETY: Setup runs on the thread that owns this HWND. The subclass has
    // no borrowed data and is removed in WM_NCDESTROY.
    if !unsafe { SetWindowSubclass(hwnd, Some(window_proc), 0, 0) }.as_bool() {
        log::warn!("Could not attach Windows recording lifecycle notifications");
        return;
    }
    match unsafe { RegisterSuspendResumeNotification(HANDLE(hwnd.0), DEVICE_NOTIFY_WINDOW_HANDLE) }
    {
        Ok(handle) => POWER_NOTIFICATION.with(|notification| notification.set(Some(handle))),
        Err(_) => log::warn!("Windows power registration failed; using window broadcasts"),
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut dispatched = 0;
        loop {
            SIGNALS.wake.notified().await;
            let checkpoint = SIGNALS.take_checkpoint();
            if SIGNALS.is_current(checkpoint) {
                recording_commands::request_recovery_snapshot(checkpoint);
            }
            if let Some(attempt) = SIGNALS.stop_ready(dispatched, || {
                !recording_commands::is_starting()
                    && !recording_commands::is_stopping()
                    && recording_commands::capture_active()
            }) {
                dispatched = attempt;
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    recording_commands::stop_after_windows_suspend(app, attempt).await;
                });
            }
        }
    });
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    if let Some(signal) = decode(message, wparam.0) {
        SIGNALS.receive(signal);
        if message == WM_QUERYENDSESSION || message == WM_POWERBROADCAST {
            return LRESULT(1);
        }
    }
    if message == WM_NCDESTROY {
        POWER_NOTIFICATION.with(|notification| {
            if let Some(handle) = notification.take() {
                let _ = unsafe { UnregisterSuspendResumeNotification(handle) };
            }
        });
        let _ = unsafe { RemoveWindowSubclass(hwnd, Some(window_proc), id) };
    }
    // Keep Tao/WebView2's handling for all other messages.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_suspend_and_resume_do_not_schedule_another_stop() {
        let signals = Signals::new();
        let attempt = signals.begin_start().unwrap();
        signals.receive(Signal::Suspend);
        assert!(signals.begin_start().is_err());
        assert_eq!(signals.take_checkpoint(), attempt);
        assert_eq!(signals.stop_ready(0, || false), None);
        assert_eq!(signals.stop_ready(0, || true), Some(attempt));

        signals.receive(Signal::Suspend);
        signals.receive(Signal::Resume);
        signals.receive(Signal::Resume);
        assert_eq!(signals.take_checkpoint(), 0);
        assert_eq!(signals.stop_ready(attempt, || true), None);

        signals.receive(Signal::Suspend);
        assert_eq!(signals.stop_ready(attempt, || true), None);
        signals.receive(Signal::Resume);
        assert!(signals.begin_start().is_ok());
    }

    #[test]
    fn a_delayed_suspend_does_not_stop_a_later_start() {
        let signals = Signals::new();
        let previous = signals.begin_start().unwrap();
        signals.receive(Signal::Suspend);
        signals.receive(Signal::Resume);
        let current = signals.begin_start().unwrap();
        assert!(!signals.is_current(previous));
        assert_eq!(signals.stop_ready(0, || true), None);
        assert_eq!(signals.take_checkpoint(), previous);

        signals.receive(Signal::Suspend);
        assert_eq!(signals.stop_ready(0, || true), Some(current));
    }

    #[test]
    fn session_query_requests_only_a_checkpoint_and_cancel_rearms_it() {
        let signals = Signals::new();
        let attempt = signals.begin_start().unwrap();
        signals.receive(Signal::QueryEnd);
        signals.receive(Signal::QueryEnd);
        assert!(signals.session_ending.load(Ordering::Relaxed));
        assert_eq!(signals.take_checkpoint(), attempt);
        signals.receive(Signal::ConfirmEnd);
        assert_eq!(signals.stop_ready(0, || true), None);
        assert_eq!(signals.take_checkpoint(), 0);

        signals.receive(Signal::CancelEnd);
        assert!(!signals.session_ending.load(Ordering::Relaxed));
        signals.receive(Signal::ConfirmEnd);
        assert_eq!(signals.take_checkpoint(), attempt);
        signals.receive(Signal::ConfirmEnd);
        assert_eq!(signals.take_checkpoint(), 0);
        assert_eq!(signals.stop_ready(0, || true), None);
    }

    #[test]
    fn idle_signals_do_not_claim_a_future_recording() {
        let signals = Signals::new();
        signals.receive(Signal::QueryEnd);
        signals.receive(Signal::Suspend);
        assert_eq!(signals.take_checkpoint(), 0);
        assert_eq!(signals.stop_ready(0, || true), None);
        signals.receive(Signal::Resume);
        assert!(signals.begin_start().is_ok());
        assert_eq!(signals.stop_ready(0, || true), None);
    }

    #[test]
    fn native_message_decoding_ignores_unrelated_power_and_window_messages() {
        assert_eq!(
            decode(WM_POWERBROADCAST, PBT_APMSUSPEND as usize),
            Some(Signal::Suspend)
        );
        assert_eq!(
            decode(WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC as usize),
            Some(Signal::Resume)
        );
        assert_eq!(
            decode(WM_POWERBROADCAST, PBT_APMRESUMESUSPEND as usize),
            Some(Signal::Resume)
        );
        assert_eq!(decode(WM_QUERYENDSESSION, 0), Some(Signal::QueryEnd));
        assert_eq!(decode(WM_ENDSESSION, 0), Some(Signal::CancelEnd));
        assert_eq!(decode(WM_ENDSESSION, 1), Some(Signal::ConfirmEnd));
        assert_eq!(decode(WM_POWERBROADCAST, 0), None);
        assert_eq!(decode(WM_NCDESTROY, 0), None);
    }
}
