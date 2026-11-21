//! Native inference owns its permit until the native call actually returns.
//! Cancelling an async waiter cannot release the model underneath a running FFI call.
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use std::future::{poll_fn, Future};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

static NATIVE: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(1)));
static JOBS: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(1)));
static FOREGROUND_DONE: Notify = Notify::const_new();

#[cfg(test)]
pub(crate) static GLOBAL_JOB_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
pub(crate) static NATIVE_TEARDOWN_TEST_PAUSE: std::sync::Mutex<
    Option<(
        tokio::sync::oneshot::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    )>,
> = std::sync::Mutex::new(None);

/// Why a claim failed: another holder owns the slot, or the slot is free but a
/// cancelled native call is still finishing.
enum ClaimBlock {
    SlotHeld,
    NativeBusy,
}

fn claim_job_checked() -> Result<OwnedSemaphorePermit, (ClaimBlock, String)> {
    let permit = JOBS.clone().try_acquire_owned().map_err(|_| {
        let message = if super::diarization::active_speaker_diarization_command().is_some() {
            "Speaker detection is running. Cancel speaker detection before starting a recording.".to_string()
        } else {
            "Another recording or transcription job is active. Stop or cancel it before starting another.".to_string()
        };
        (ClaimBlock::SlotHeld, message)
    })?;
    if NATIVE.available_permits() == 0 {
        return Err((ClaimBlock::NativeBusy, "The previous speech engine is still finishing a native call. Wait for it to finish before starting another job or changing models.".into()));
    }
    Ok(permit)
}

pub(crate) fn claim_job() -> Result<OwnedSemaphorePermit, String> {
    claim_job_checked().map_err(|(_, message)| message)
}

/// Wait for foreground work to release both permits before restarting a job.
pub(crate) async fn wait_for_job(
    token: &tokio_util::sync::CancellationToken,
) -> Result<(), String> {
    wait_for_slot(token, |_, _| ()).await
}

async fn wait_for_slot<T>(
    token: &tokio_util::sync::CancellationToken,
    on_claim: impl FnOnce(&mut SlotRegistration, OwnedSemaphorePermit) -> T,
) -> Result<T, String> {
    crate::summary::llm_client::with_cancellation(Some(token), async {
        enum SlotWait<T> {
            Claimed(T),
            Foreground,
            Native,
        }
        let mut on_claim = Some(on_claim);
        loop {
            let foreground_done = FOREGROUND_DONE.notified();
            let state = {
                let acquire = JOBS.clone().acquire_owned();
                tokio::pin!(acquire);
                poll_fn(|cx| {
                    let mut registration = slot_registration();
                    if registration.foreground_waiters > 0 {
                        return std::task::Poll::Ready(Ok(SlotWait::Foreground));
                    }
                    let permit = std::task::ready!(acquire.as_mut().poll(cx))
                        .map_err(|_| "The inference queue is unavailable")?;
                    let state = if NATIVE.available_permits() > 0 {
                        let register = on_claim.take().expect("slot claimed once");
                        SlotWait::Claimed(register(&mut registration, permit))
                    } else {
                        SlotWait::Native
                    };
                    std::task::Poll::Ready(Ok::<_, String>(state))
                })
                .await?
            };
            match state {
                SlotWait::Claimed(permit) => return Ok(permit),
                SlotWait::Foreground => foreground_done.await,
                SlotWait::Native => wait_for_native_idle().await,
            }
        }
    })
    .await
}

pub(crate) struct LocalSummaryPermit(Option<OwnedSemaphorePermit>);

impl Drop for LocalSummaryPermit {
    fn drop(&mut self) {
        let mut registration = slot_registration();
        registration.local_summary = false;
        drop(self.0.take());
    }
}

/// Only a summary holding a permit can delay foreground preemption.
pub(crate) async fn claim_local_summary(
    wait: Option<&tokio_util::sync::CancellationToken>,
) -> Result<LocalSummaryPermit, String> {
    let register = |registration: &mut SlotRegistration, permit| {
        registration.local_summary = true;
        LocalSummaryPermit(Some(permit))
    };
    if let Some(token) = wait {
        wait_for_slot(token, register).await
    } else {
        let mut registration = slot_registration();
        if registration.foreground_waiters > 0 {
            return Err("Another recording or transcription job is starting.".into());
        }
        let permit = claim_job()?;
        Ok(register(&mut registration, permit))
    }
}

/// A caller already holding the exclusive job may need to restore a model
/// after cancelling a native load/decode. The native permit outlives its waiter.
pub(crate) async fn wait_for_native_idle() {
    drop(NATIVE.acquire().await);
}

/// Batch jobs that hold the job slot and can be cancelled without corrupting
/// data. A recording may pre-empt them; they never pre-empt each other or a
/// recording. The cancel paths leave the meeting library unchanged: see each
/// job's own module for what a cancel discards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BatchJob {
    Retranscription,
    Import,
    SpeakerDetection,
    PcTest,
}

impl BatchJob {
    /// Identifier sent to the UI in the `batch-job-preempted` event.
    pub(crate) fn event_name(self) -> &'static str {
        match self {
            Self::Retranscription => "retranscription",
            Self::Import => "import",
            Self::SpeakerDetection => "speaker-detection",
            Self::PcTest => "pc-test",
        }
    }

    fn still_stopping_message(self) -> &'static str {
        match self {
            Self::Retranscription => {
                "Retranscription is still stopping. Try starting the recording again in a moment."
            }
            Self::Import => {
                "The audio import is still stopping. Try starting the recording again in a moment."
            }
            Self::SpeakerDetection => {
                "Speaker detection is still stopping. Try starting the recording again in a moment."
            }
            Self::PcTest => {
                "The PC test is still stopping. Try starting the recording again in a moment."
            }
        }
    }
}

/// Requests cancellation of a running batch job and reports whether the
/// request can still take effect (`false` once the job is past its last
/// cancel check, for example while it is saving). It must be cheap, idempotent
/// and non-blocking: it is re-issued while a recording waits for the slot.
pub(crate) type CancelBatchJob = Arc<dyn Fn() -> bool + Send + Sync>;

struct BatchRegistration {
    id: u64,
    job: BatchJob,
    cancel: CancelBatchJob,
    /// The UI was already told this job is being stopped.
    announced: bool,
}

struct RunningBatch {
    id: u64,
    job: BatchJob,
    cancel: CancelBatchJob,
}

#[derive(Default)]
struct SlotRegistration {
    batch: Option<BatchRegistration>,
    local_summary: bool,
    foreground_waiters: usize,
}

static SLOT_REGISTRATION: std::sync::LazyLock<std::sync::Mutex<SlotRegistration>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(SlotRegistration::default()));
static NEXT_BATCH_ID: AtomicU64 = AtomicU64::new(1);

fn slot_registration() -> std::sync::MutexGuard<'static, SlotRegistration> {
    SLOT_REGISTRATION
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

struct ForegroundClaim;

impl ForegroundClaim {
    fn enter() -> Self {
        // Keep a queued summary from retaking the slot from a cancelled owner.
        slot_registration().foreground_waiters += 1;
        Self
    }
}

impl Drop for ForegroundClaim {
    fn drop(&mut self) {
        let mut registration = slot_registration();
        registration.foreground_waiters -= 1;
        if registration.foreground_waiters == 0 {
            FOREGROUND_DONE.notify_waiters();
        }
    }
}

/// The job slot plus, for a batch job, its registration. Dropping clears the
/// registration and releases the slot under the registry lock, so no observer
/// can see a registration without its holder, or a holder that has already
/// been released but is still registered.
pub(crate) struct BatchPermit {
    id: u64,
    permit: Option<OwnedSemaphorePermit>,
}

impl Drop for BatchPermit {
    fn drop(&mut self) {
        let mut registration = slot_registration();
        if registration
            .batch
            .as_ref()
            .is_some_and(|current| current.id == self.id)
        {
            registration.batch = None;
        }
        drop(self.permit.take());
    }
}

/// How long a recording waits for a cancelled batch job to release the slot.
/// A native decode (one VAD segment of at most 25 s, or the speaker-detection
/// call) cannot be interrupted, so this covers the call in flight plus the
/// job's own cleanup (engine unload, previous-model restore).
const BATCH_PREEMPT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
const LOCAL_SUMMARY_PREEMPT_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

enum Poll {
    Claimed(OwnedSemaphorePermit, u64),
    Blocked {
        message: String,
        block: ClaimBlock,
        /// The registered batch job holding the slot, read under the same
        /// lock as the failed claim.
        batch: Option<RunningBatch>,
        local_summary: bool,
    },
}

/// Takes the slot and, when `register` is given, records the holder while the
/// registration lock is held. A failed claim reads the registration under that
/// same lock, so a recording never observes a batch job as an unregistered
/// holder, nor a released job as still holding the slot.
fn poll_claim(register: Option<(BatchJob, &CancelBatchJob)>) -> Poll {
    let mut registration = slot_registration();
    match claim_job_checked() {
        Ok(permit) => {
            let id = match register {
                Some((job, cancel)) => {
                    let id = NEXT_BATCH_ID.fetch_add(1, Ordering::Relaxed);
                    registration.batch = Some(BatchRegistration {
                        id,
                        job,
                        cancel: cancel.clone(),
                        announced: false,
                    });
                    id
                }
                None => 0,
            };
            Poll::Claimed(permit, id)
        }
        Err((block, message)) => Poll::Blocked {
            message,
            block,
            batch: registration.batch.as_ref().map(|current| RunningBatch {
                id: current.id,
                job: current.job,
                cancel: current.cancel.clone(),
            }),
            local_summary: registration.local_summary,
        },
    }
}

fn try_claim(
    register: Option<(BatchJob, &CancelBatchJob)>,
) -> Result<(OwnedSemaphorePermit, u64), String> {
    match poll_claim(register) {
        Poll::Claimed(permit, id) => Ok((permit, id)),
        Poll::Blocked { message, .. } => Err(message),
    }
}

/// True for the first caller only: one `batch-job-preempted` event per job.
fn announce_once(id: u64) -> bool {
    match slot_registration().batch.as_mut() {
        Some(current) if current.id == id && !current.announced => {
            current.announced = true;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
fn running_batch_job() -> Option<(BatchJob, CancelBatchJob)> {
    slot_registration()
        .batch
        .as_ref()
        .map(|current| (current.job, current.cancel.clone()))
}

/// Pre-empts only a local summary. Production callers use [`claim_batch_job`]
/// or [`claim_job_for_recording`]; the knowledge and summary tests still drive
/// the plain form.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn claim_job_preempting_local_summary(
    reason: &'static str,
) -> Result<OwnedSemaphorePermit, String> {
    claim_with_preemption(
        reason,
        None,
        None,
        LOCAL_SUMMARY_PREEMPT_WAIT,
        BATCH_PREEMPT_WAIT,
    )
    .await
    .map(|(permit, _)| permit)
}

/// A batch job takes the slot, pre-empting only a local summary, and
/// registers so that a starting recording can cancel it.
pub(crate) async fn claim_batch_job(
    job: BatchJob,
    cancel: CancelBatchJob,
    reason: &'static str,
) -> Result<BatchPermit, String> {
    claim_with_preemption(
        reason,
        Some((job, &cancel)),
        None,
        LOCAL_SUMMARY_PREEMPT_WAIT,
        BATCH_PREEMPT_WAIT,
    )
    .await
    .map(|(permit, id)| BatchPermit {
        id,
        permit: Some(permit),
    })
}

/// A batch job that, unlike [`claim_batch_job`], does not pre-empt anything.
pub(crate) fn claim_batch_job_without_preemption(
    job: BatchJob,
    cancel: CancelBatchJob,
) -> Result<BatchPermit, String> {
    let (permit, id) = try_claim(Some((job, &cancel)))?;
    Ok(BatchPermit {
        id,
        permit: Some(permit),
    })
}

/// A recording takes the slot, pre-empting a local summary or a running batch
/// job. `on_preempt` is called at most once per batch job, when it has been
/// asked to stop and the request can still take effect.
/// A running recording holds an unregistered permit, so it is never pre-empted.
pub(crate) async fn claim_job_for_recording<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    reason: &'static str,
) -> Result<OwnedSemaphorePermit, String> {
    claim_job_for_recording_with(reason, BATCH_PREEMPT_WAIT, &|job| {
        emit_batch_preempted(app, job)
    })
    .await
}

async fn claim_job_for_recording_with(
    reason: &'static str,
    batch_wait: std::time::Duration,
    on_preempt: &(dyn Fn(BatchJob) + Sync),
) -> Result<OwnedSemaphorePermit, String> {
    let (permit, _) = claim_with_preemption(
        reason,
        None,
        Some(on_preempt),
        LOCAL_SUMMARY_PREEMPT_WAIT,
        batch_wait,
    )
    .await?;
    // A summary can still be active between native requests. Stop it before
    // it begins another prompt against a recording-owned inference slot.
    crate::summary::SummaryService::cancel_local_summaries(reason);
    Ok(permit)
}

fn emit_batch_preempted<R: tauri::Runtime>(app: &tauri::AppHandle<R>, job: BatchJob) {
    use tauri::Emitter;
    log::info!(
        "Stopping {} because a recording is starting",
        job.event_name()
    );
    let _ = app.emit(
        "batch-job-preempted",
        serde_json::json!({ "job": job.event_name(), "reason": "recording" }),
    );
}

async fn claim_with_preemption(
    reason: &'static str,
    register: Option<(BatchJob, &CancelBatchJob)>,
    preempt_batch: Option<&(dyn Fn(BatchJob) + Sync)>,
    summary_wait: std::time::Duration,
    batch_wait: std::time::Duration,
) -> Result<(OwnedSemaphorePermit, u64), String> {
    let _foreground = ForegroundClaim::enter();
    // Block new indexing claims before cancelling the current input. Native FFI
    // retains both permits until it actually completes.
    let priority = crate::knowledge::scheduler::ForegroundPriority::enter();
    if priority.preempted {
        if let Ok(claimed) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(claimed) = try_claim(register) {
                    break claimed;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        {
            return Ok(claimed);
        }
    }
    let (original, first_batch, local_summary) = match poll_claim(register) {
        Poll::Claimed(permit, id) => return Ok((permit, id)),
        Poll::Blocked {
            message,
            batch,
            local_summary,
            ..
        } => (message, batch, local_summary),
    };
    // Only a recording pre-empts a batch job.
    if let (Some(on_preempt), Some(first)) = (preempt_batch, first_batch) {
        let mut stopping = first.job;
        let deadline = tokio::time::Instant::now() + batch_wait;
        loop {
            // Re-issued each round: a job that has claimed the slot but not
            // yet reset its own cancel state would otherwise swallow it.
            match poll_claim(register) {
                Poll::Claimed(permit, id) => return Ok((permit, id)),
                Poll::Blocked {
                    batch: Some(batch), ..
                } => {
                    stopping = batch.job;
                    // Tell the UI once, and only when the cancel can take effect.
                    if (batch.cancel)() && announce_once(batch.id) {
                        on_preempt(batch.job);
                    }
                }
                // The batch job is gone and someone else (another recording)
                // took the slot: nothing left to pre-empt.
                Poll::Blocked {
                    batch: None,
                    block: ClaimBlock::SlotHeld,
                    message,
                    ..
                } => return Err(message),
                // The slot is free; a cancelled native call is still finishing.
                Poll::Blocked {
                    batch: None,
                    block: ClaimBlock::NativeBusy,
                    ..
                } => {}
            }
            if tokio::time::Instant::now() >= deadline {
                log::warn!(
                    "{} did not release the job slot in time",
                    stopping.event_name()
                );
                return Err(stopping.still_stopping_message().to_string());
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }
    if !local_summary {
        return Err(original);
    }
    crate::summary::SummaryService::cancel_local_summaries(reason);
    match tokio::time::timeout(summary_wait, JOBS.clone().acquire_owned()).await {
        Ok(Ok(permit)) if NATIVE.available_permits() > 0 => {
            if let Some((job, cancel)) = register {
                let id = NEXT_BATCH_ID.fetch_add(1, Ordering::Relaxed);
                slot_registration().batch = Some(BatchRegistration {
                    id,
                    job,
                    cancel: cancel.clone(),
                    announced: false,
                });
                return Ok((permit, id));
            }
            Ok((permit, 0))
        }
        _ => Err(original),
    }
}

struct CancelOnDrop(Option<Arc<AtomicBool>>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(cancelled) = &self.0 {
            cancelled.store(true, Ordering::Release);
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum NativeRunFailure {
    #[error("Speech engine is still busy with a previous native call")]
    Busy,
    #[error("Native speech engine task failed")]
    WorkerFailed,
}

pub(crate) async fn run<T: Send + 'static>(
    work: impl FnOnce(Arc<AtomicBool>) -> Result<T> + Send + 'static,
) -> Result<T> {
    run_cancellable(Arc::new(AtomicBool::new(false)), work).await
}

pub(crate) async fn run_cancellable<T: Send + 'static>(
    cancelled: Arc<AtomicBool>,
    work: impl FnOnce(Arc<AtomicBool>) -> Result<T> + Send + 'static,
) -> Result<T> {
    run_cancellable_with_guard(cancelled, (), work).await
}

/// The completion guard outlives the native permit, including cancellation and
/// unwinding. Indexing uses it to keep foreground preemption visible until all
/// of its resources are available again.
pub(crate) async fn run_cancellable_with_guard<T: Send + 'static>(
    cancelled: Arc<AtomicBool>,
    completion: impl Send + 'static,
    work: impl FnOnce(Arc<AtomicBool>) -> Result<T> + Send + 'static,
) -> Result<T> {
    let permit = NATIVE
        .clone()
        .try_acquire_owned()
        .map_err(|_| NativeRunFailure::Busy)?;
    let mut cancel_on_drop = CancelOnDrop(Some(cancelled.clone()));
    let result = tokio::task::spawn_blocking(move || {
        // Locals drop in reverse order: release NATIVE before completion.
        let _completion = completion;
        let _permit = permit;
        if cancelled.load(Ordering::Acquire) {
            return Err(anyhow!("Transcription cancelled"));
        }
        let result = work(cancelled.clone())?;
        #[cfg(test)]
        {
            let pause = NATIVE_TEARDOWN_TEST_PAUSE.lock().unwrap().take();
            if let Some((started, release)) = pause {
                let _ = started.send(());
                let _ = release.recv();
            }
        }
        if cancelled.load(Ordering::Acquire) {
            return Err(anyhow!("Transcription cancelled"));
        }
        Ok(result)
    })
    .await;
    // A completed worker failure is not cancellation of the async waiter.
    cancel_on_drop.0 = None;
    result.map_err(|_| NativeRunFailure::WorkerFailed)?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn jobs_are_exclusive_and_release_on_error() {
        let _test_lock = GLOBAL_JOB_TEST_LOCK.lock().await;
        let first = claim_job().unwrap();
        assert!(claim_job().is_err());
        drop(first);
        assert!(claim_job().is_ok());

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let native = tokio::spawn(run(move |cancelled| {
            let _ = started_tx.send(cancelled.clone());
            release_rx.recv().unwrap();
            Ok(())
        }));
        let cancelled = started_rx.await.unwrap();
        native.abort();
        assert!(native.await.unwrap_err().is_cancelled());
        assert!(cancelled.load(Ordering::Acquire));
        assert!(claim_job().is_err(), "cancelled FFI still owns its permit");
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while NATIVE.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(claim_job().is_ok());
    }

    const BUSY: &str =
        "Another recording or transcription job is active. Stop or cancel it before starting another.";
    const SHORT_WAIT: std::time::Duration = std::time::Duration::from_millis(200);

    /// A batch job that releases the slot `release_after` once cancelled, or
    /// never when `release_after` is `None`.
    struct FakeBatchJob {
        cancelled: Arc<AtomicBool>,
        cancel_calls: Arc<AtomicU64>,
        permit: Option<BatchPermit>,
    }

    impl FakeBatchJob {
        fn start(job: BatchJob) -> Self {
            let cancelled = Arc::new(AtomicBool::new(false));
            let cancel_calls = Arc::new(AtomicU64::new(0));
            let cancel: CancelBatchJob = {
                let (cancelled, cancel_calls) = (cancelled.clone(), cancel_calls.clone());
                Arc::new(move || {
                    cancelled.store(true, Ordering::Release);
                    cancel_calls.fetch_add(1, Ordering::Relaxed);
                    true
                })
            };
            let permit = claim_batch_job_without_preemption(job, cancel).unwrap();
            Self {
                cancelled,
                cancel_calls,
                permit: Some(permit),
            }
        }

        /// Releases the slot after the job has been cancelled, like a job that
        /// notices its cancel flag at the next checkpoint.
        fn release_when_cancelled(mut self) -> tokio::task::JoinHandle<Arc<AtomicU64>> {
            tokio::spawn(async move {
                while !self.cancelled.load(Ordering::Acquire) {
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                }
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                drop(self.permit.take());
                self.cancel_calls.clone()
            })
        }
    }

    fn preempted_jobs() -> (
        Arc<std::sync::Mutex<Vec<BatchJob>>>,
        impl Fn(BatchJob) + Sync,
    ) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = seen.clone();
        (seen, move |job| record.lock().unwrap().push(job))
    }

    #[tokio::test]
    async fn recording_preempts_each_cancellable_batch_job() {
        let _test_lock = GLOBAL_JOB_TEST_LOCK.lock().await;
        for job in [
            BatchJob::Retranscription,
            BatchJob::Import,
            BatchJob::SpeakerDetection,
            BatchJob::PcTest,
        ] {
            let batch = FakeBatchJob::start(job);
            let flag = batch.cancelled.clone();
            let released = batch.release_when_cancelled();
            let (seen, record) = preempted_jobs();
            let recording = claim_job_for_recording_with(
                "recording",
                std::time::Duration::from_secs(5),
                &record,
            )
            .await
            .expect("recording must pre-empt a batch job");
            assert!(flag.load(Ordering::Acquire), "{job:?} was not cancelled");
            assert_eq!(*seen.lock().unwrap(), vec![job], "reported once");
            assert!(released.await.unwrap().load(Ordering::Relaxed) >= 1);
            assert!(
                running_batch_job().is_none(),
                "the pre-empted job's registration is gone"
            );
            assert_eq!(
                claim_job().unwrap_err(),
                BUSY,
                "the recording holds the slot"
            );
            drop(recording);
            assert!(claim_job().is_ok());
        }
    }

    #[tokio::test]
    async fn event_names_match_the_ui_contract() {
        assert_eq!(BatchJob::Retranscription.event_name(), "retranscription");
        assert_eq!(BatchJob::Import.event_name(), "import");
        assert_eq!(BatchJob::SpeakerDetection.event_name(), "speaker-detection");
        assert_eq!(BatchJob::PcTest.event_name(), "pc-test");
    }

    #[tokio::test]
    async fn batch_job_does_not_preempt_a_running_recording() {
        let _test_lock = GLOBAL_JOB_TEST_LOCK.lock().await;
        // A recording holds a plain, unregistered permit for its whole duration.
        let recording = claim_job().unwrap();
        let cancel_calls = Arc::new(AtomicU64::new(0));
        let cancel: CancelBatchJob = {
            let cancel_calls = cancel_calls.clone();
            Arc::new(move || {
                cancel_calls.fetch_add(1, Ordering::Relaxed);
                true
            })
        };
        assert_eq!(
            claim_batch_job(BatchJob::Import, cancel.clone(), "summary")
                .await
                .err()
                .as_deref(),
            Some(BUSY)
        );
        assert!(claim_batch_job_without_preemption(BatchJob::PcTest, cancel).is_err());
        assert_eq!(cancel_calls.load(Ordering::Relaxed), 0);
        assert!(running_batch_job().is_none());
        drop(recording);
    }

    #[tokio::test]
    async fn recording_does_not_preempt_a_running_recording() {
        let _test_lock = GLOBAL_JOB_TEST_LOCK.lock().await;
        let first_recording = claim_job().unwrap();
        let (seen, record) = preempted_jobs();
        let started = std::time::Instant::now();
        let second =
            claim_job_for_recording_with("recording", std::time::Duration::from_secs(5), &record)
                .await;
        assert_eq!(second.err().as_deref(), Some(BUSY));
        assert!(seen.lock().unwrap().is_empty());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "no bounded wait when nothing can be pre-empted"
        );
        drop(first_recording);
    }

    #[tokio::test]
    async fn batch_job_does_not_preempt_another_batch_job() {
        let _test_lock = GLOBAL_JOB_TEST_LOCK.lock().await;
        let running = FakeBatchJob::start(BatchJob::Retranscription);
        let other_cancel: CancelBatchJob = Arc::new(|| true);
        assert_eq!(
            claim_batch_job(BatchJob::SpeakerDetection, other_cancel.clone(), "summary")
                .await
                .err()
                .as_deref(),
            Some(BUSY)
        );
        assert!(claim_batch_job_without_preemption(BatchJob::PcTest, other_cancel).is_err());
        assert!(!running.cancelled.load(Ordering::Acquire));
        assert_eq!(
            running_batch_job().map(|(job, _)| job),
            Some(BatchJob::Retranscription)
        );
    }

    #[tokio::test]
    async fn wait_for_a_stuck_batch_job_is_bounded_and_fails_cleanly() {
        let _test_lock = GLOBAL_JOB_TEST_LOCK.lock().await;
        let stuck = FakeBatchJob::start(BatchJob::SpeakerDetection);
        let (seen, record) = preempted_jobs();
        let started = std::time::Instant::now();
        let result = claim_job_for_recording_with("recording", SHORT_WAIT, &record).await;
        let message = result.err().expect("the slot never became free");
        assert!(
            message.contains("Speaker detection is still stopping"),
            "{message}"
        );
        assert!(started.elapsed() >= SHORT_WAIT);
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        assert_eq!(*seen.lock().unwrap(), vec![BatchJob::SpeakerDetection]);
        assert!(
            stuck.cancel_calls.load(Ordering::Relaxed) > 1,
            "cancel is re-issued while waiting"
        );
        // Nothing was taken or corrupted: once the job lets go the slot is usable.
        drop(stuck);
        assert!(running_batch_job().is_none());
        assert!(claim_job().is_ok());
    }

    #[tokio::test]
    async fn two_recording_starts_racing_one_batch_job_one_wins_and_one_event() {
        let _test_lock = GLOBAL_JOB_TEST_LOCK.lock().await;
        let batch = FakeBatchJob::start(BatchJob::Retranscription);
        let released = batch.release_when_cancelled();
        let (seen, record) = preempted_jobs();
        let started = std::time::Instant::now();
        let (first, second) = tokio::join!(
            claim_job_for_recording_with("recording", std::time::Duration::from_secs(5), &record),
            claim_job_for_recording_with("recording", std::time::Duration::from_secs(5), &record),
        );
        released.await.unwrap();
        let (winner, loser) = match (first, second) {
            (Ok(permit), Err(message)) | (Err(message), Ok(permit)) => (permit, message),
            (first, second) => panic!(
                "exactly one start must win: {:?} / {:?}",
                first.err(),
                second.err()
            ),
        };
        assert_eq!(loser, BUSY, "the loser reports the normal busy error");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "the loser fails fast instead of waiting out the pre-emption window"
        );
        assert_eq!(*seen.lock().unwrap(), vec![BatchJob::Retranscription]);
        drop(winner);
    }

    #[tokio::test]
    async fn no_event_when_the_cancel_cannot_take_effect() {
        let _test_lock = GLOBAL_JOB_TEST_LOCK.lock().await;
        // A job past its last cancel check: the request is refused, the job
        // finishes by itself and releases the slot.
        let refusals = Arc::new(AtomicU64::new(0));
        let cancel: CancelBatchJob = {
            let refusals = refusals.clone();
            Arc::new(move || {
                refusals.fetch_add(1, Ordering::Relaxed);
                false
            })
        };
        let saving = claim_batch_job_without_preemption(BatchJob::Retranscription, cancel).unwrap();
        let finish = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            drop(saving);
        });
        let (seen, record) = preempted_jobs();
        let recording =
            claim_job_for_recording_with("recording", std::time::Duration::from_secs(5), &record)
                .await
                .expect("the recording waits for the job to finish saving");
        finish.await.unwrap();
        assert!(refusals.load(Ordering::Relaxed) >= 1);
        assert!(seen.lock().unwrap().is_empty());
        drop(recording);
    }

    #[tokio::test]
    async fn registration_follows_the_slot_holder() {
        let _test_lock = GLOBAL_JOB_TEST_LOCK.lock().await;
        let first = FakeBatchJob::start(BatchJob::Import);
        let first_id = first.permit.as_ref().unwrap().id;
        assert_eq!(
            running_batch_job().map(|(job, _)| job),
            Some(BatchJob::Import)
        );
        drop(first);
        assert!(running_batch_job().is_none());
        let second = FakeBatchJob::start(BatchJob::PcTest);
        assert_ne!(second.permit.as_ref().unwrap().id, first_id);
        assert_eq!(
            running_batch_job().map(|(job, _)| job),
            Some(BatchJob::PcTest)
        );
    }
}
