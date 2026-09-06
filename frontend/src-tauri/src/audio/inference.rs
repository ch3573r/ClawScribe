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

pub(crate) fn claim_job() -> Result<OwnedSemaphorePermit, String> {
    let permit = JOBS.clone().try_acquire_owned().map_err(|_| "Another recording or transcription job is active. Stop or cancel it before starting another.".to_string())?;
    if NATIVE.available_permits() == 0 {
        return Err("The previous speech engine is still finishing a native call. Wait for it to finish before starting another job or changing models.".into());
    }
    Ok(permit)
}

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(crate) async fn run<T: Send + 'static>(
    work: impl FnOnce(Arc<AtomicBool>) -> Result<T> + Send + 'static,
) -> Result<T> {
    let permit = NATIVE
        .clone()
        .try_acquire_owned()
        .map_err(|_| anyhow!("Speech engine is still busy with a previous native call"))?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = CancelOnDrop(cancelled.clone());
    tokio::task::spawn_blocking(move || {
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
    .map_err(|_| anyhow!("Native speech engine task failed"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn jobs_are_exclusive_and_release_on_error() {
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
