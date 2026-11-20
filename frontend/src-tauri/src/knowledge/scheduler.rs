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
    path::{Path, PathBuf},
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
pub(crate) fn foreground_waiting() -> bool {
    PRIORITY.lock().unwrap().foreground > 0
}
pub(crate) fn claim_snapshot(token: Arc<AtomicBool>) -> Result<ActiveCall, KnowledgeError> {
    let mut priority = PRIORITY.lock().unwrap();
    if priority.foreground > 0 {
        return Err(KnowledgeError::Busy);
    }
    let job = inference::claim_job().map_err(|_| KnowledgeError::Busy)?;
    priority.active = Some(Arc::downgrade(&token));
    Ok(ActiveCall {
        registry: None,
        token,
        job: Some(job),
    })
}
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
pub(crate) struct ActiveCall {
    registry: Option<Arc<CancellationRegistry>>,
    token: Arc<AtomicBool>,
    job: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl Drop for ActiveCall {
    fn drop(&mut self) {
        let mut priority = PRIORITY.lock().unwrap();
        // The native call or synchronous BLOB read has completed. Keep
        // admission and its priority marker synchronized through permit release,
        // including when the async waiter was aborted.
        drop(self.job.take());
        if let Some(registry) = &self.registry {
            let mut active = registry.active.lock().unwrap();
            if active.as_ref().is_some_and(|v| Arc::ptr_eq(v, &self.token)) {
                *active = None;
            }
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
#[cfg(test)]
pub async fn run_indexing<T: Send + 'static>(
    registry: Arc<CancellationRegistry>,
    work: impl FnOnce() -> Result<T, KnowledgeError> + Send + 'static,
) -> Result<T, KnowledgeError> {
    run_indexing_with_token(registry, move |_| work()).await
}

async fn run_indexing_with_token<T: Send + 'static>(
    registry: Arc<CancellationRegistry>,
    work: impl FnOnce(&AtomicBool) -> Result<T, KnowledgeError> + Send + 'static,
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
        registry: Some(registry),
        token: token.clone(),
        job: Some(job),
    };
    let result = inference::run_cancellable_with_guard(token.clone(), active, move |cancelled| {
        // Preserve typed failures; the shared inference layer owns cancellation.
        Ok(work(&cancelled))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerificationStatus {
    Inactive,
    Pending,
    Verifying,
    Verified,
    Unavailable,
    Updating,
}

enum Activation {
    Inactive,
    Pending(PathBuf),
    Verifying,
    Verified(VerifiedModel),
    Unavailable,
    Updating,
}

impl Activation {
    fn check_available(&self) -> Result<(), KnowledgeError> {
        match self {
            Self::Inactive => Err(KnowledgeError::Disabled),
            Self::Unavailable => Err(KnowledgeError::ModelUnavailable),
            Self::Updating => Err(KnowledgeError::Busy),
            _ => Ok(()),
        }
    }
}

/// Keep new inference out while the explicit transfer may replace model files.
pub(super) struct ModelUpdate {
    scheduler: Arc<Scheduler>,
    enabled: bool,
    root: PathBuf,
}

impl ModelUpdate {
    pub(super) async fn drain(&self) {
        self.scheduler.reset_worker().await;
    }
    /// Call after verification completes. Busy and Cancelled are retryable; any other
    /// error leaves the model unavailable until the next explicit recheck.
    pub(super) fn finish(self, result: &Result<VerifiedModel, KnowledgeError>) {
        if self.enabled {
            let mut activation = self.scheduler.activation.lock().unwrap();
            *activation = match result {
                Ok(verified) => Activation::Verified(verified.clone()),
                Err(KnowledgeError::Cancelled) | Err(KnowledgeError::Busy) => {
                    Activation::Pending(self.root.clone())
                }
                Err(_) => Activation::Unavailable,
            };
        }
    }
}

impl Drop for ModelUpdate {
    fn drop(&mut self) {
        // Use into_inner on poison so a panic during the update does not strand
        // callers in Updating forever.
        let mut activation = self
            .scheduler
            .activation
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if matches!(*activation, Activation::Updating) {
            // Unwind without finish: leave a retryable Pending rather than
            // Unavailable so the next enable/download can recover.
            *activation = if self.enabled {
                Activation::Pending(self.root.clone())
            } else {
                Activation::Inactive
            };
        }
    }
}

pub struct Scheduler {
    timer_started: AtomicBool,
    activation: Mutex<Activation>,
    worker: Mutex<Worker>,
    pub notifications: Mutex<NotificationQueue>,
    pub cancellation: Arc<CancellationRegistry>,
}
impl Scheduler {
    pub fn new(cancellation: Arc<CancellationRegistry>) -> Arc<Self> {
        Arc::new(Self {
            timer_started: AtomicBool::new(false),
            activation: Mutex::new(Activation::Inactive),
            worker: Mutex::new(Worker {
                model: None,
                last_used: Instant::now(),
            }),
            notifications: Mutex::new(NotificationQueue::default()),
            cancellation,
        })
    }
    pub(super) fn enable(self: &Arc<Self>, root: PathBuf) {
        // Repeated enable is idempotent and cannot launch extra idle timers.
        let mut activation = self.activation.lock().unwrap();
        if matches!(*activation, Activation::Inactive) {
            *activation = Activation::Pending(root);
            self.start_idle_timer();
        }
    }
    fn start_idle_timer(self: &Arc<Self>) {
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
    }
    fn unload_idle(&self) -> Duration {
        let idle = Duration::from_secs(60);
        if let Ok(mut worker) = self.worker.try_lock() {
            let elapsed = worker.last_used.elapsed();
            if !self.accepts_work() || elapsed >= idle {
                worker.model = None;
            } else if worker.model.is_some() {
                return idle - elapsed;
            }
        }
        idle
    }
    pub async fn disable(self: &Arc<Self>) {
        *self.activation.lock().unwrap() = Activation::Inactive;
        self.reset_worker().await;
    }
    pub(super) fn begin_model_update(
        self: &Arc<Self>,
        enabled: bool,
        root: PathBuf,
    ) -> ModelUpdate {
        let update = ModelUpdate {
            scheduler: self.clone(),
            enabled,
            root,
        };
        *self.activation.lock().unwrap() = Activation::Updating;
        if enabled {
            self.start_idle_timer();
        }
        update
    }
    async fn reset_worker(self: &Arc<Self>) {
        self.cancellation.cancel();
        self.notifications.lock().unwrap().clear();
        let scheduler = self.clone();
        // A running native call retains its model. Cleanup happens off the runtime.
        let _ = tokio::task::spawn_blocking(move || {
            scheduler.lock_worker().model = None;
        })
        .await;
    }

    fn lock_worker(&self) -> std::sync::MutexGuard<'_, Worker> {
        self.worker.lock().unwrap_or_else(|poisoned| {
            let mut worker = poisoned.into_inner();
            worker.model = None;
            self.worker.clear_poison();
            worker
        })
    }
    pub fn notify(&self, source_id: String, revision: i64) -> Result<(), KnowledgeError> {
        self.check_available()?;
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
    pub(super) fn check_available(&self) -> Result<(), KnowledgeError> {
        self.activation.lock().unwrap().check_available()
    }
    pub(super) fn accepts_work(&self) -> bool {
        self.check_available().is_ok()
    }
    pub(crate) fn verification_status(&self) -> VerificationStatus {
        match *self.activation.lock().unwrap() {
            Activation::Inactive => VerificationStatus::Inactive,
            Activation::Pending(_) => VerificationStatus::Pending,
            Activation::Verifying => VerificationStatus::Verifying,
            Activation::Verified(_) => VerificationStatus::Verified,
            Activation::Unavailable => VerificationStatus::Unavailable,
            Activation::Updating => VerificationStatus::Updating,
        }
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
        self.check_available()?;
        let scheduler = self.clone();
        run_indexing_with_token(self.cancellation.clone(), move |cancelled| {
            let mut worker = scheduler.lock_worker();
            scheduler.prepare_backend(
                &mut worker,
                cancelled,
                VerifiedModel::verify_blocking,
                |model| Ok(Box::new(OnnxEmbedding::load(model)?)),
            )?;
            if cancelled.load(Ordering::Acquire) {
                return Err(KnowledgeError::Cancelled);
            }
            let result = work(worker.model.as_mut().unwrap().as_mut());
            worker.last_used = Instant::now();
            result
        })
        .await
    }

    fn prepare_backend(
        &self,
        worker: &mut Worker,
        cancelled: &AtomicBool,
        verify: impl FnOnce(&Path, &AtomicBool) -> Result<VerifiedModel, KnowledgeError>,
        load: impl FnOnce(&VerifiedModel) -> Result<Box<dyn EmbeddingBackend>, KnowledgeError>,
    ) -> Result<(), KnowledgeError> {
        let root = {
            let mut activation = self.activation.lock().unwrap();
            activation.check_available()?;
            match &*activation {
                Activation::Verified(_) if worker.model.is_some() => return Ok(()),
                Activation::Verified(_) => None,
                Activation::Pending(root) => {
                    worker.model = None;
                    let root = root.clone();
                    *activation = Activation::Verifying;
                    Some(root)
                }
                _ => return Err(KnowledgeError::Busy),
            }
        };
        if let Some(root) = root {
            struct VerificationAttempt<'a> {
                activation: &'a Mutex<Activation>,
                pending_root: Option<&'a Path>,
            }
            impl Drop for VerificationAttempt<'_> {
                fn drop(&mut self) {
                    if let Some(root) = self.pending_root {
                        let mut state = self.activation.lock().unwrap_or_else(|p| p.into_inner());
                        if matches!(*state, Activation::Verifying) {
                            *state = Activation::Pending(root.to_owned());
                        }
                    }
                }
            }
            let result = {
                let mut attempt = VerificationAttempt {
                    activation: &self.activation,
                    pending_root: Some(&root),
                };
                let checked = verify(&root, cancelled);
                attempt.pending_root = None;
                checked
            };
            let mut activation = self.activation.lock().unwrap();
            if !matches!(*activation, Activation::Verifying) {
                return Err(KnowledgeError::Cancelled);
            }
            *activation = match result {
                Ok(verified) => Activation::Verified(verified),
                Err(error) => {
                    *activation = if error == KnowledgeError::Cancelled {
                        Activation::Pending(root)
                    } else {
                        Activation::Unavailable
                    };
                    return Err(error);
                }
            };
        }
        if cancelled.load(Ordering::Acquire) {
            return Err(KnowledgeError::Cancelled);
        }
        let verified = {
            let activation = self.activation.lock().unwrap();
            match &*activation {
                Activation::Verified(verified) => verified.clone(),
                _ => return Err(KnowledgeError::Cancelled),
            }
        };
        match load(&verified) {
            Ok(model) => {
                worker.model = Some(model);
                worker.last_used = Instant::now();
                Ok(())
            }
            Err(error) => {
                let mut activation = self.activation.lock().unwrap();
                if matches!(*activation, Activation::Verified(_)) {
                    *activation = Activation::Unavailable;
                }
                Err(error)
            }
        }
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
    *scheduler.activation.lock().unwrap() = Activation::Verified(VerifiedModel::synthetic());
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
    use super::super::model::{ModelFile, ModelPins};
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{cell::Cell, path::Path};

    fn verification_fixture() -> (tempfile::TempDir, ModelPins) {
        let root = tempfile::tempdir().unwrap();
        let mut pins: ModelPins =
            serde_json::from_str(include_str!("embedding-model-pins.json")).unwrap();
        pins.files = [("model", b"abcdef"), ("tokenizer", b"uvwxyz")]
            .into_iter()
            .map(|(path, body)| {
                std::fs::write(root.path().join(path), body).unwrap();
                ModelFile {
                    path: path.into(),
                    size: body.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(body)),
                }
            })
            .collect();
        (root, pins)
    }

    fn synthetic_loader(_: &VerifiedModel) -> Result<Box<dyn EmbeddingBackend>, KnowledgeError> {
        Ok(synthetic_query_scheduler()
            .worker
            .lock()
            .unwrap()
            .model
            .take()
            .unwrap())
    }

    #[tokio::test]
    async fn activation_hashes_only_on_demand_and_reuses_proof_after_idle_unload() {
        let scheduler = Scheduler::new(Arc::new(CancellationRegistry::default()));
        let (dir, pins) = verification_fixture();
        let attempts = Cell::new(0);
        let bytes = Cell::new(0);
        let loads = Cell::new(0);
        let cancelled = AtomicBool::new(false);
        let demand = || {
            let mut worker = scheduler.worker.lock().unwrap();
            scheduler.prepare_backend(
                &mut worker,
                &cancelled,
                |root, token| {
                    assert_eq!(
                        scheduler.verification_status(),
                        VerificationStatus::Verifying
                    );
                    attempts.set(attempts.get() + 1);
                    VerifiedModel::verify_pinned(root, &pins, token, |n| bytes.set(bytes.get() + n))
                },
                |proof| {
                    loads.set(loads.get() + 1);
                    synthetic_loader(proof)
                },
            )
        };

        scheduler.enable(dir.path().into());
        scheduler.enable(dir.path().into());
        assert_eq!(scheduler.verification_status(), VerificationStatus::Pending);
        assert_eq!((attempts.get(), bytes.get(), loads.get()), (0, 0, 0));
        demand().unwrap();
        assert_eq!(
            scheduler.verification_status(),
            VerificationStatus::Verified
        );
        assert_eq!((attempts.get(), bytes.get(), loads.get()), (1, 12, 1));

        scheduler.enable(dir.path().into());
        scheduler.worker.lock().unwrap().last_used = Instant::now() - Duration::from_secs(61);
        scheduler.unload_idle();
        demand().unwrap();
        assert_eq!((attempts.get(), bytes.get(), loads.get()), (1, 12, 2));

        scheduler.disable().await;
        scheduler.enable(dir.path().into());
        assert_eq!(scheduler.verification_status(), VerificationStatus::Pending);
        demand().unwrap();
        assert_eq!((attempts.get(), bytes.get(), loads.get()), (2, 24, 3));
    }

    #[tokio::test]
    async fn same_size_tampering_and_cancelled_verification_cannot_load_a_backend() {
        let scheduler = Scheduler::new(Arc::new(CancellationRegistry::default()));
        let (dir, pins) = verification_fixture();
        let cancelled = AtomicBool::new(false);
        let demand =
            |verify: &dyn Fn(&Path, &AtomicBool) -> Result<VerifiedModel, KnowledgeError>| {
                scheduler.prepare_backend(
                    &mut scheduler.worker.lock().unwrap(),
                    &cancelled,
                    verify,
                    |_| panic!("verification must succeed before the backend is loaded"),
                )
            };

        scheduler.enable(dir.path().into());
        let bytes = Cell::new(0);
        assert_eq!(
            demand(&|root, token| {
                VerifiedModel::verify_pinned(root, &pins, token, |n| {
                    bytes.set(bytes.get() + n);
                    if n > 0 {
                        token.store(true, Ordering::Release);
                    }
                })
            }),
            Err(KnowledgeError::Cancelled)
        );
        assert_eq!(bytes.get(), 6);
        assert_eq!(scheduler.verification_status(), VerificationStatus::Pending);

        cancelled.store(false, Ordering::Release);
        std::fs::write(dir.path().join("tokenizer"), b"UVWXYZ").unwrap();
        let bytes = Cell::new(0);
        assert_eq!(
            demand(&|root, token| {
                VerifiedModel::verify_pinned(root, &pins, token, |n| bytes.set(bytes.get() + n))
            }),
            Err(KnowledgeError::ModelUnavailable)
        );
        assert_eq!(bytes.get(), 12);
        assert_eq!(
            scheduler.verification_status(),
            VerificationStatus::Unavailable
        );
        scheduler.enable(dir.path().into());
        assert_eq!(
            demand(&|_, _| panic!("redundant enable must not retry verification")),
            Err(KnowledgeError::ModelUnavailable)
        );
    }

    #[tokio::test]
    async fn explicit_recheck_replaces_proof_only_if_the_files_pass() {
        let scheduler = synthetic_query_scheduler();
        let (dir, pins) = verification_fixture();
        let root = dir.path();
        let cancelled = AtomicBool::new(false);
        std::fs::write(root.join("model"), b"wrong!").unwrap();

        let update = scheduler.begin_model_update(true, root.into());
        assert_eq!(
            scheduler.verification_status(),
            VerificationStatus::Updating
        );
        assert_eq!(scheduler.check_available(), Err(KnowledgeError::Busy));
        update.drain().await;
        assert!(scheduler.worker.lock().unwrap().model.is_none());
        let checked = VerifiedModel::verify_pinned(root, &pins, &cancelled, |_| {});
        assert!(matches!(&checked, Err(KnowledgeError::ModelUnavailable)));
        update.finish(&checked);
        assert_eq!(
            scheduler.verification_status(),
            VerificationStatus::Unavailable
        );

        let update = scheduler.begin_model_update(true, root.into());
        update.drain().await;
        std::fs::write(root.join("model"), b"abcdef").unwrap();
        let checked = VerifiedModel::verify_pinned(root, &pins, &cancelled, |_| {});
        assert!(checked.is_ok());
        update.finish(&checked);
        assert_eq!(
            scheduler.verification_status(),
            VerificationStatus::Verified
        );
    }
    #[tokio::test]
    async fn update_drop_without_finish_leaves_a_retryable_pending_state() {
        let scheduler = synthetic_query_scheduler();
        let (dir, _) = verification_fixture();
        let update = scheduler.begin_model_update(true, dir.path().into());
        assert_eq!(
            scheduler.verification_status(),
            VerificationStatus::Updating
        );
        drop(update);
        assert_eq!(scheduler.verification_status(), VerificationStatus::Pending);
    }

    #[tokio::test]
    async fn update_finish_with_busy_error_is_retryable_not_unavailable() {
        let scheduler = synthetic_query_scheduler();
        let (dir, _) = verification_fixture();
        let update = scheduler.begin_model_update(true, dir.path().into());
        update.drain().await;
        update.finish(&Err(KnowledgeError::Busy));
        assert_eq!(scheduler.verification_status(), VerificationStatus::Pending);
    }

    #[tokio::test]
    async fn panicking_verifier_leaves_pending_for_retry() {
        let scheduler = Scheduler::new(Arc::new(CancellationRegistry::default()));
        let (dir, pins) = verification_fixture();
        scheduler.enable(dir.path().into());
        assert_eq!(scheduler.verification_status(), VerificationStatus::Pending);
        let cancelled = AtomicBool::new(false);

        {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut worker = scheduler.lock_worker();
                scheduler.prepare_backend(
                    &mut worker,
                    &cancelled,
                    |_, _| panic!("synthetic verifier panic"),
                    |_| unreachable!("load must not be reached"),
                )
            }));
            assert!(result.is_err(), "caught panic must propagate as Err");
        }
        assert_eq!(
            scheduler.verification_status(),
            VerificationStatus::Pending,
            "stuck Verifying after panic would prevent all future use"
        );

        // Second demand: real verifier should complete and leave Verified.
        let bytes = Cell::new(0);
        {
            assert!(scheduler.worker.is_poisoned());
            let mut worker = scheduler.lock_worker();
            assert!(!scheduler.worker.is_poisoned());
            scheduler
                .prepare_backend(
                    &mut worker,
                    &cancelled,
                    |root, token| {
                        VerifiedModel::verify_pinned(root, &pins, token, |n| {
                            bytes.set(bytes.get() + n)
                        })
                    },
                    synthetic_loader,
                )
                .unwrap();
        }
        assert_eq!(
            scheduler.verification_status(),
            VerificationStatus::Verified
        );
        assert!(bytes.get() > 0, "real verifier must have read the files");
    }

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
        *scheduler.activation.lock().unwrap() = Activation::Verified(VerifiedModel::synthetic());
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
