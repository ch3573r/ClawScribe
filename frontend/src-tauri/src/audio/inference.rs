//! Native inference owns its permit until the native call actually returns.
//! Cancelling an async waiter cannot release the model underneath a running FFI call.
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

static NATIVE: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(1)));
static JOBS: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(1)));

#[cfg(test)]
pub(crate) static GLOBAL_JOB_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) fn claim_job() -> Result<OwnedSemaphorePermit, String> {
    let permit = JOBS.clone().try_acquire_owned().map_err(|_| {
        if super::diarization::active_speaker_diarization_command().is_some() {
            "Speaker detection is running. Cancel speaker detection before starting a recording.".to_string()
        } else {
            "Another recording or transcription job is active. Stop or cancel it before starting another.".to_string()
        }
    })?;
    if NATIVE.available_permits() == 0 {
        return Err("The previous speech engine is still finishing a native call. Wait for it to finish before starting another job or changing models.".into());
    }
    Ok(permit)
}

pub(crate) async fn claim_job_preempting_local_summary(
    reason: &'static str,
) -> Result<OwnedSemaphorePermit, String> {
    // Block new indexing claims before cancelling the current input. Native FFI
    // retains both permits until it actually completes.
    let priority = crate::knowledge::scheduler::ForegroundPriority::enter();
    if priority.preempted {
        if let Ok(permit) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(permit) = claim_job() {
                    break permit;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        {
            return Ok(permit);
        }
    }
    let original = match claim_job() {
        Ok(permit) => return Ok(permit),
        Err(error) => error,
    };
    if !crate::summary::SummaryService::has_active_local_summary() {
        return Err(original);
    }
    crate::summary::SummaryService::cancel_local_summaries(reason);
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        JOBS.clone().acquire_owned(),
    )
    .await
    {
        Ok(Ok(permit)) if NATIVE.available_permits() > 0 => Ok(permit),
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

pub(crate) async fn run<T: Send + 'static>(
    work: impl FnOnce(Arc<AtomicBool>) -> Result<T> + Send + 'static,
) -> Result<T> {
    run_cancellable(Arc::new(AtomicBool::new(false)), work).await
}

pub(crate) async fn run_cancellable<T: Send + 'static>(
    cancelled: Arc<AtomicBool>,
    work: impl FnOnce(Arc<AtomicBool>) -> Result<T> + Send + 'static,
) -> Result<T> {
    let permit = NATIVE
        .clone()
        .try_acquire_owned()
        .map_err(|_| anyhow!("Speech engine is still busy with a previous native call"))?;
    let mut cancel_on_drop = CancelOnDrop(Some(cancelled.clone()));
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if cancelled.load(Ordering::Acquire) {
            return Err(anyhow!("Transcription cancelled"));
        }
        let result = work(cancelled.clone())?;
        if cancelled.load(Ordering::Acquire) {
            return Err(anyhow!("Transcription cancelled"));
        }
        Ok(result)
    })
    .await
    .map_err(|_| anyhow!("Native speech engine task failed"))?;
    cancel_on_drop.0 = None;
    result
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
}
