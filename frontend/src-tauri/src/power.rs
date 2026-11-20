//! Keeps the PC from going to sleep on idle while ClawScribe captures audio.
//!
//! In a meeting the user is idle (listening), so Windows may suspend the machine
//! mid-recording. The request is deliberately narrow:
//!
//! * it asks only that the system stay awake; the display may still turn off;
//! * it is held only while audio is being captured or a recording is being
//!   finished, not while a recording is paused and not while idle;
//! * it is bounded: after `MAX_LEASE` it is let go even if the recording state
//!   is stuck, and is not taken again until the recording state has been idle;
//! * it is released on every way out. The state is reconciled once a second from
//!   the recorder's own flags, so a stop, a capture error, a failed start and an
//!   exit all end it, and the request lives on a thread of its own, so the
//!   system drops it with the process if the app dies.
//!
//! A recording cut off by a power loss or a killed process is found again by the
//! recovery scan at the next start; this module does not change that.
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The longest a single request may be held.
pub const MAX_LEASE: Duration = Duration::from_secs(12 * 60 * 60);

/// Whatever actually tells the system to stay awake.
pub trait WakeBackend {
    /// Takes the request. Returns false when the system refused. Taking it twice
    /// is harmless.
    fn acquire(&mut self) -> bool;
    /// Lets the request go. Releasing without holding it is harmless.
    fn release(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    Acquired,
    Released,
    /// The bound was reached and the request was let go.
    Expired,
    /// The system refused the request (reported once).
    Unavailable,
}

pub struct WakeLease<B: WakeBackend> {
    backend: B,
    held_since: Option<Instant>,
    expired: bool,
    refused: bool,
    max: Duration,
}

impl<B: WakeBackend> WakeLease<B> {
    pub fn new(backend: B, max: Duration) -> Self {
        Self {
            backend,
            held_since: None,
            expired: false,
            refused: false,
            max,
        }
    }

    pub fn is_held(&self) -> bool {
        self.held_since.is_some()
    }

    /// Brings the request in line with whether capture needs it right now.
    pub fn reconcile(&mut self, wanted: bool, now: Instant) -> Change {
        if !wanted {
            self.expired = false;
            self.refused = false;
            if self.held_since.take().is_some() {
                self.backend.release();
                return Change::Released;
            }
            return Change::None;
        }
        if self.expired {
            return Change::None;
        }
        match self.held_since {
            Some(since) => {
                if now.duration_since(since) >= self.max {
                    self.held_since = None;
                    self.expired = true;
                    self.backend.release();
                    Change::Expired
                } else {
                    Change::None
                }
            }
            None => {
                if self.backend.acquire() {
                    self.held_since = Some(now);
                    self.refused = false;
                    Change::Acquired
                } else if self.refused {
                    Change::None
                } else {
                    self.refused = true;
                    Change::Unavailable
                }
            }
        }
    }

    /// Lets the request go whatever the state (app exit).
    pub fn release(&mut self) {
        self.held_since = None;
        self.expired = false;
        self.refused = false;
        self.backend.release();
    }
}

#[cfg(windows)]
mod system {
    use super::WakeBackend;
    use std::sync::mpsc::{channel, Sender};
    use std::thread::JoinHandle;
    use windows::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED, EXECUTION_STATE,
    };

    /// `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)` lasts as long
    /// as the thread that made it, so a thread of its own holds it. The thread
    /// ends, and with it the request, when it is told to or when this owner is
    /// gone.
    #[derive(Default)]
    pub struct SystemBackend {
        holder: Option<(Sender<()>, JoinHandle<()>)>,
    }

    impl WakeBackend for SystemBackend {
        fn acquire(&mut self) -> bool {
            if self.holder.is_some() {
                return true;
            }
            let (stop, wait) = channel::<()>();
            let (ready_sender, ready) = channel::<bool>();
            let spawned = std::thread::Builder::new()
                .name("capture-wake".to_string())
                .spawn(move || {
                    let previous = unsafe {
                        SetThreadExecutionState(EXECUTION_STATE(
                            ES_CONTINUOUS.0 | ES_SYSTEM_REQUIRED.0,
                        ))
                    };
                    let held = previous.0 != 0;
                    let _ = ready_sender.send(held);
                    if held {
                        // Returns when told to stop, or when the sender is gone.
                        let _ = wait.recv();
                        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
                    }
                });
            let Ok(handle) = spawned else {
                return false;
            };
            if ready.recv().unwrap_or(false) {
                self.holder = Some((stop, handle));
                true
            } else {
                let _ = handle.join();
                false
            }
        }

        fn release(&mut self) {
            if let Some((stop, handle)) = self.holder.take() {
                drop(stop);
                let _ = handle.join();
            }
        }
    }
}

#[cfg(not(windows))]
mod system {
    use super::WakeBackend;

    /// Other platforms keep their own power policy for now.
    #[derive(Default)]
    pub struct SystemBackend;

    impl WakeBackend for SystemBackend {
        fn acquire(&mut self) -> bool {
            true
        }

        fn release(&mut self) {}
    }
}

static LEASE: LazyLock<Mutex<WakeLease<system::SystemBackend>>> =
    LazyLock::new(|| Mutex::new(WakeLease::new(system::SystemBackend::default(), MAX_LEASE)));

fn lease() -> MutexGuard<'static, WakeLease<system::SystemBackend>> {
    LEASE.lock().unwrap_or_else(|error| error.into_inner())
}

/// Whether capture needs the system awake right now: while audio is captured
/// and while a recording is being finished, but not while it is paused.
pub fn needs_wake(recording: bool, paused: bool, stopping: bool) -> bool {
    stopping || (recording && !paused)
}

/// Applies the current need. Cheap; called about once a second.
pub fn reconcile(wanted: bool) {
    let change = lease().reconcile(wanted, Instant::now());
    match change {
        Change::None => {}
        Change::Acquired => log::info!("Keeping the system awake while recording"),
        Change::Released => log::info!("No longer keeping the system awake"),
        Change::Expired => log::warn!(
            "Stopped keeping the system awake after {} hours; the recording state did not change",
            MAX_LEASE.as_secs() / 3600
        ),
        Change::Unavailable => log::warn!("Windows did not allow keeping the system awake"),
    }
}

/// Lets the request go at exit.
pub fn release() {
    lease().release();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Fake {
        held: Rc<Cell<bool>>,
        acquires: Rc<Cell<u32>>,
        refuse: Rc<Cell<bool>>,
    }

    impl WakeBackend for Fake {
        fn acquire(&mut self) -> bool {
            if self.refuse.get() {
                return false;
            }
            self.acquires.set(self.acquires.get() + 1);
            self.held.set(true);
            true
        }

        fn release(&mut self) {
            self.held.set(false);
        }
    }

    fn lease_with(
        max: Duration,
    ) -> (
        WakeLease<Fake>,
        Rc<Cell<bool>>,
        Rc<Cell<u32>>,
        Rc<Cell<bool>>,
    ) {
        let fake = Fake::default();
        let held = fake.held.clone();
        let acquires = fake.acquires.clone();
        let refuse = fake.refuse.clone();
        (WakeLease::new(fake, max), held, acquires, refuse)
    }

    #[test]
    fn it_is_held_exactly_while_capture_needs_it() {
        let (mut lease, held, acquires, _) = lease_with(MAX_LEASE);
        let start = Instant::now();
        assert_eq!(lease.reconcile(false, start), Change::None);
        assert!(!held.get());
        assert_eq!(lease.reconcile(true, start), Change::Acquired);
        assert_eq!(
            lease.reconcile(true, start + Duration::from_secs(1)),
            Change::None
        );
        assert!(held.get() && lease.is_held());
        assert_eq!(acquires.get(), 1, "one request, however long it is held");
        assert_eq!(
            lease.reconcile(false, start + Duration::from_secs(2)),
            Change::Released
        );
        assert!(!held.get() && !lease.is_held());
    }

    #[test]
    fn it_is_bounded_and_not_retaken_until_the_state_has_been_idle() {
        let (mut lease, held, acquires, _) = lease_with(Duration::from_secs(10));
        let start = Instant::now();
        assert_eq!(lease.reconcile(true, start), Change::Acquired);
        assert_eq!(
            lease.reconcile(true, start + Duration::from_secs(9)),
            Change::None
        );
        assert_eq!(
            lease.reconcile(true, start + Duration::from_secs(10)),
            Change::Expired
        );
        assert!(!held.get());
        assert_eq!(
            lease.reconcile(true, start + Duration::from_secs(11)),
            Change::None
        );
        assert_eq!(acquires.get(), 1);
        assert_eq!(
            lease.reconcile(false, start + Duration::from_secs(12)),
            Change::None
        );
        assert_eq!(
            lease.reconcile(true, start + Duration::from_secs(13)),
            Change::Acquired
        );
        assert_eq!(acquires.get(), 2);
    }

    #[test]
    fn a_refusal_is_reported_once_and_retried() {
        let (mut lease, held, _, refuse) = lease_with(MAX_LEASE);
        let start = Instant::now();
        refuse.set(true);
        assert_eq!(lease.reconcile(true, start), Change::Unavailable);
        assert_eq!(
            lease.reconcile(true, start + Duration::from_secs(1)),
            Change::None
        );
        refuse.set(false);
        assert_eq!(
            lease.reconcile(true, start + Duration::from_secs(2)),
            Change::Acquired
        );
        assert!(held.get());
    }

    #[test]
    fn releasing_at_exit_lets_go_in_any_state() {
        let (mut lease, held, _, _) = lease_with(MAX_LEASE);
        lease.release();
        assert!(!held.get());
        lease.reconcile(true, Instant::now());
        assert!(held.get());
        lease.release();
        assert!(!held.get() && !lease.is_held());
    }

    #[test]
    fn capture_needs_wake_while_recording_or_finishing_but_not_while_paused_or_idle() {
        assert!(needs_wake(true, false, false));
        assert!(needs_wake(true, false, true));
        assert!(needs_wake(false, false, true));
        assert!(!needs_wake(true, true, false));
        assert!(!needs_wake(false, false, false));
    }
}
