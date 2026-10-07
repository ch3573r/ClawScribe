//! One bounded input owns both native and job permits until FFI returns.
use super::{
    embedding::{EmbeddingBackend, OnnxEmbedding},
    model::VerifiedModel,
    types::{EmbeddingPurpose, KnowledgeError},
};
use crate::audio::inference;
use once_cell::sync::Lazy;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct CancellationRegistry {
    active: Mutex<Option<Arc<AtomicBool>>>,
}
impl CancellationRegistry {
    pub fn cancel(&self) {
        if let Some(token) = self.active.lock().unwrap().as_ref() {
            token.store(true, Ordering::Release);
        }
    }
}
#[derive(Default)]
struct Priority {
    foreground: usize,
    active: Option<Weak<AtomicBool>>,
}
static PRIORITY: Lazy<Mutex<Priority>> = Lazy::new(|| Mutex::new(Priority::default()));
pub(crate) struct ForegroundPriority {
    pub preempted: bool,
}
impl ForegroundPriority {
    pub fn enter() -> Self {
        let mut priority = PRIORITY.lock().unwrap();
        priority.foreground += 1;
        let active = priority.active.as_ref().and_then(Weak::upgrade);
        if let Some(token) = &active {
            token.store(true, Ordering::Release);
        }
        Self {
            preempted: active.is_some(),
        }
    }
}
impl Drop for ForegroundPriority {
    fn drop(&mut self) {
        PRIORITY.lock().unwrap().foreground -= 1;
    }
}
struct ActiveCall {
    registry: Arc<CancellationRegistry>,
    token: Arc<AtomicBool>,
    job: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl Drop for ActiveCall {
    fn drop(&mut self) {
        let mut priority = PRIORITY.lock().unwrap();
        // NATIVE has already released. Keep admission and the priority marker
        // synchronized so foreground callers never observe a false idle gap.
        drop(self.job.take());
        let mut active = self.registry.active.lock().unwrap();
        if active.as_ref().is_some_and(|v| Arc::ptr_eq(v, &self.token)) {
            *active = None;
        }
        if priority
            .active
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|v| Arc::ptr_eq(&v, &self.token))
        {
            priority.active = None;
        }
    }
}
pub async fn run_indexing<T: Send + 'static>(
    registry: Arc<CancellationRegistry>,
    work: impl FnOnce() -> Result<T, KnowledgeError> + Send + 'static,
) -> Result<T, KnowledgeError> {
    let token = Arc::new(AtomicBool::new(false));
    let job = {
        let mut priority = PRIORITY.lock().unwrap();
        if priority.foreground > 0 {
            return Err(KnowledgeError::Busy);
        }
        let job = inference::claim_job().map_err(|_| KnowledgeError::Busy)?;
        *registry.active.lock().unwrap() = Some(token.clone());
        priority.active = Some(Arc::downgrade(&token));
        job
    };
    let active = ActiveCall {
        registry,
        token: token.clone(),
        job: Some(job),
    };
    let result = inference::run_cancellable_with_guard(token.clone(), active, move |_| {
        // Preserve typed failures; the shared inference layer owns cancellation.
        Ok(work())
    })
    .await;
    if token.load(Ordering::Acquire) {
        return Err(KnowledgeError::Cancelled);
    }
    result.map_err(
        |error| match error.downcast_ref::<inference::NativeRunFailure>() {
            Some(inference::NativeRunFailure::Busy) => KnowledgeError::Busy,
            _ => KnowledgeError::ProviderFailure,
        },
    )?
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexNotification {
    pub source_id: String,
    pub revision: i64,
}
#[derive(Default)]
pub struct NotificationQueue {
    pending: VecDeque<IndexNotification>,
}
impl NotificationQueue {
    pub fn notify(&mut self, source_id: String, revision: i64) -> Result<(), KnowledgeError> {
        if source_id.is_empty() || source_id.len() > 128 || revision < 0 {
            return Err(KnowledgeError::InvalidInput);
        }
        if let Some(existing) = self.pending.iter_mut().find(|n| n.source_id == source_id) {
            existing.revision = existing.revision.max(revision);
            return Ok(());
        }
        if self.pending.len() >= 32 {
            return Err(KnowledgeError::Busy);
        }
        self.pending.push_back(IndexNotification {
            source_id,
            revision,
        });
        Ok(())
    }
    pub fn next(&mut self) -> Option<IndexNotification> {
        self.pending.pop_front()
    }
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}
struct Worker {
    model: Option<Box<dyn EmbeddingBackend>>,
    last_used: Instant,
}
pub struct Scheduler {
    enabled: AtomicBool,
    timer_started: AtomicBool,
    verified: Mutex<Option<VerifiedModel>>,
    worker: Mutex<Worker>,
    pub notifications: Mutex<NotificationQueue>,
    pub cancellation: Arc<CancellationRegistry>,
}
impl Scheduler {
    pub fn new(cancellation: Arc<CancellationRegistry>) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(false),
            timer_started: AtomicBool::new(false),
            verified: Mutex::new(None),
            worker: Mutex::new(Worker {
                model: None,
                last_used: Instant::now(),
            }),
            notifications: Mutex::new(NotificationQueue::default()),
            cancellation,
        })
    }
    pub fn enable(self: &Arc<Self>, verified: VerifiedModel) -> Result<(), KnowledgeError> {
        // Repeated enable is idempotent and cannot launch extra idle timers.
        let mut slot = self.verified.lock().unwrap();
        if slot.is_some() {
            return if self.enabled.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err(KnowledgeError::Busy)
            };
        }
        *slot = Some(verified);
        self.enabled.store(true, Ordering::Release);
        if !self.timer_started.swap(true, Ordering::AcqRel) {
            let weak = Arc::downgrade(self);
            tokio::spawn(async move {
                let mut delay = Duration::from_secs(60);
                loop {
                    tokio::time::sleep(delay).await;
                    let Some(scheduler) = weak.upgrade() else {
                        break;
                    };
                    delay = tokio::task::spawn_blocking(move || scheduler.unload_idle())
                        .await
                        .unwrap_or(Duration::from_secs(60));
                }
            });
        }
        Ok(())
    }
    fn unload_idle(&self) -> Duration {
        let idle = Duration::from_secs(60);
        if let Ok(mut worker) = self.worker.try_lock() {
            let elapsed = worker.last_used.elapsed();
            if !self.enabled.load(Ordering::Acquire) || elapsed >= idle {
                worker.model = None;
            } else if worker.model.is_some() {
                return idle - elapsed;
            }
        }
        idle
    }
    pub async fn disable(self: &Arc<Self>) {
        self.enabled.store(false, Ordering::Release);
        self.cancellation.cancel();
        self.notifications.lock().unwrap().clear();
        let scheduler = self.clone();
        // A running native call retains its model. Cleanup happens off the runtime.
        let _ = tokio::task::spawn_blocking(move || {
            scheduler.worker.lock().unwrap().model = None;
            *scheduler.verified.lock().unwrap() = None;
        })
        .await;
    }
    pub fn notify(&self, source_id: String, revision: i64) -> Result<(), KnowledgeError> {
        if !self.enabled.load(Ordering::Acquire) {
            return Err(KnowledgeError::Disabled);
        }
        self.notifications
            .lock()
            .unwrap()
            .notify(source_id, revision)
    }
    pub async fn embed(
        self: &Arc<Self>,
        text: String,
        purpose: EmbeddingPurpose,
    ) -> Result<Vec<f32>, KnowledgeError> {
        super::embedding::prefixed_input(&text, purpose)?;
        self.with_backend(move |backend| backend.embed(&text, purpose))
            .await
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
    pub async fn spans(
        self: &Arc<Self>,
        id: String,
        text: String,
    ) -> Result<Vec<super::types::TextSpan>, KnowledgeError> {
        if text.len() > 16 * 1024 {
            return Err(KnowledgeError::InvalidInput);
        }
        self.with_backend(move |backend| backend.spans(&id, &text))
            .await
    }
    async fn with_backend<T: Send + 'static>(
        self: &Arc<Self>,
        work: impl FnOnce(&mut dyn EmbeddingBackend) -> Result<T, KnowledgeError> + Send + 'static,
    ) -> Result<T, KnowledgeError> {
        if !self.enabled.load(Ordering::Acquire) {
            return Err(KnowledgeError::Disabled);
        }
        let scheduler = self.clone();
        run_indexing(self.cancellation.clone(), move || {
            if !scheduler.enabled.load(Ordering::Acquire) {
                return Err(KnowledgeError::Disabled);
            }
            let mut worker = scheduler.worker.lock().unwrap();
            if worker.model.is_none() {
                let verified = scheduler
                    .verified
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or(KnowledgeError::ModelUnavailable)?;
                worker.model = Some(Box::new(OnnxEmbedding::load(&verified)?));
                if scheduler
                    .cancellation
                    .active
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|token| token.load(Ordering::Acquire))
                {
                    return Err(KnowledgeError::Cancelled);
                }
            }
            let result = work(worker.model.as_mut().unwrap().as_mut());
            worker.last_used = Instant::now();
            result
        })
        .await
    }
}

#[cfg(test)]
pub(crate) fn synthetic_query_scheduler() -> Arc<Scheduler> {
    // Test utility for retrieval control flow, never used by model acceptance.
    struct UnitVector {
        tokenizer: tokenizers::Tokenizer,
    }
    impl EmbeddingBackend for UnitVector {
        fn space(&self) -> super::types::EmbeddingSpace {
            super::model::PINS.space()
        }
        fn embed(&mut self, _: &str, _: EmbeddingPurpose) -> Result<Vec<f32>, KnowledgeError> {
            let mut vector = vec![0.; 384];
            vector[0] = 1.;
            Ok(vector)
        }
        fn spans(
            &self,
            id: &str,
            text: &str,
        ) -> Result<Vec<super::types::TextSpan>, KnowledgeError> {
            super::chunking::semantic_spans(&self.tokenizer, id, text)
        }
    }
    let scheduler = Scheduler::new(Arc::new(CancellationRegistry::default()));
    scheduler.enabled.store(true, Ordering::Release);
    let mut tokenizer = tokenizers::Tokenizer::new(
        tokenizers::models::wordlevel::WordLevel::builder()
            .vocab([("[UNK]".into(), 0)].into_iter().collect())
            .unk_token("[UNK]".into())
            .build()
            .unwrap(),
    );
    tokenizer.with_pre_tokenizer(Some(
        tokenizers::pre_tokenizers::whitespace::WhitespaceSplit,
    ));
    scheduler.worker.lock().unwrap().model = Some(Box::new(UnitVector { tokenizer }));
    scheduler
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_unload_releases_model_only_after_sixty_seconds() {
        struct DropProbe(Arc<AtomicBool>);
        impl EmbeddingBackend for DropProbe {
            fn space(&self) -> super::super::types::EmbeddingSpace {
                super::super::model::PINS.space()
            }
            fn embed(&mut self, _: &str, _: EmbeddingPurpose) -> Result<Vec<f32>, KnowledgeError> {
                unreachable!()
            }
        }
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let scheduler = Scheduler::new(Arc::new(CancellationRegistry::default()));
        scheduler.enabled.store(true, Ordering::Release);
        let dropped = Arc::new(AtomicBool::new(false));
        {
            let mut worker = scheduler.worker.lock().unwrap();
            worker.model = Some(Box::new(DropProbe(dropped.clone())));
            worker.last_used = Instant::now() - Duration::from_secs(59);
        }
        assert!(scheduler.unload_idle() <= Duration::from_secs(1));
        assert!(!dropped.load(Ordering::Acquire));
        scheduler.worker.lock().unwrap().last_used = Instant::now() - Duration::from_secs(61);
        scheduler.unload_idle();
        assert!(dropped.load(Ordering::Acquire));
    }
    #[test]
    fn notifications_coalesce_with_bounded_backpressure() {
        let mut queue = NotificationQueue::default();
        for n in 0..32 {
            queue.notify(n.to_string(), 1).unwrap();
        }
        queue.notify("0".into(), 5).unwrap();
        queue.notify("0".into(), 2).unwrap();
        assert_eq!(
            queue.notify("overflow".into(), 1),
            Err(KnowledgeError::Busy)
        );
        assert_eq!(
            queue.next().unwrap(),
            IndexNotification {
                source_id: "0".into(),
                revision: 5
            }
        );
        queue.notify("overflow".into(), 1).unwrap();
        assert_eq!(queue.pending.len(), 32);
    }
    #[tokio::test]
    async fn disabled_scheduler_never_loads_a_model() {
        let scheduler = Scheduler::new(Arc::new(CancellationRegistry::default()));
        assert_eq!(
            scheduler.embed("Ja.".into(), EmbeddingPurpose::Query).await,
            Err(KnowledgeError::Disabled)
        );
        assert_eq!(
            scheduler.notify("public-fixture".into(), 1),
            Err(KnowledgeError::Disabled)
        );
    }
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
    async fn recording_waits_through_native_teardown() {
        let _serial = inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        for abort_waiter in [false, true] {
            let registry = Arc::new(CancellationRegistry::default());
            let (paused_tx, paused_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            *inference::NATIVE_TEARDOWN_TEST_PAUSE.lock().unwrap() = Some((paused_tx, release_rx));
            let indexing = tokio::spawn(run_indexing(registry, || Ok(())));
            paused_rx.await.unwrap();
            if abort_waiter {
                indexing.abort();
            }
            let foreground =
                tokio::spawn(inference::claim_job_preempting_local_summary("recording"));
            tokio::time::sleep(Duration::from_millis(50)).await;
            let returned_before_native_release = foreground.is_finished();
            // Always release the blocking thread before assertions, including RED.
            release_tx.send(()).unwrap();
            let _ = indexing.await;
            let acquired = foreground.await.unwrap();
            assert!(
                !returned_before_native_release,
                "recording must wait through indexing's native teardown"
            );
            assert!(acquired.is_ok());
        }
    }
    #[tokio::test]
    async fn indexing_panic_releases_permits_and_priority() {
        let _serial = inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let registry = Arc::new(CancellationRegistry::default());
        let result = run_indexing(registry.clone(), || -> Result<(), KnowledgeError> {
            panic!("synthetic indexing panic");
        })
        .await;
        assert_eq!(result, Err(KnowledgeError::ProviderFailure));
        assert!(registry.active.lock().unwrap().is_none());
        let priority = ForegroundPriority::enter();
        assert!(!priority.preempted);
        drop(priority);
        assert!(inference::claim_job().is_ok());
        assert!(inference::run(|_| Ok(())).await.is_ok());
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
