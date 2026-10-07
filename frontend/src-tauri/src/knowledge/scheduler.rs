use super::types::KnowledgeError;
use crate::audio::inference;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[derive(Default)]
pub struct CancellationRegistry {
    active: std::sync::Mutex<Option<Arc<AtomicBool>>>,
}
impl CancellationRegistry {
    pub fn cancel(&self) {
        if let Some(token) = self.active.lock().unwrap().as_ref() {
            token.store(true, Ordering::Release);
        }
    }
}
pub async fn run_indexing<T: Send + 'static>(
    registry: Arc<CancellationRegistry>,
    work: impl FnOnce() -> Result<T, KnowledgeError> + Send + 'static,
) -> Result<T, KnowledgeError> {
    let job = inference::claim_job().map_err(|_| KnowledgeError::Busy)?;
    let token = Arc::new(AtomicBool::new(false));
    *registry.active.lock().unwrap() = Some(token.clone());
    inference::run_cancellable(token, move |_| {
        let _job = job;
        work().map_err(anyhow::Error::new)
    })
    .await
    .map_err(|_| KnowledgeError::Cancelled)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancel_retains_native_permit() {
        let _serial = inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let registry = Arc::new(CancellationRegistry::default());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task = tokio::spawn(run_indexing(registry.clone(), move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        }));
        started_rx.await.unwrap();
        registry.cancel();
        task.abort();
        let _ = task.await;
        assert!(inference::claim_job().is_err());
        assert!(
            inference::run(|_| Ok(())).await.is_err(),
            "native calls must not overlap"
        );
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while inference::claim_job().is_err() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn recording_preempts_indexing() {
        let _serial = inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let registry = Arc::new(CancellationRegistry::default());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task = tokio::spawn(run_indexing(registry.clone(), move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        }));
        started_rx.await.unwrap();
        let foreground = tokio::spawn(inference::claim_job_preempting_local_summary("recording"));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cancelled = registry
            .active
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .load(Ordering::Acquire);
        release_tx.send(()).unwrap();
        let _ = task.await;
        let acquired = foreground.await.unwrap();
        assert!(
            cancelled,
            "recording must cancel indexing before claiming its job"
        );
        assert!(acquired.is_ok());
    }
}
