//! Opt-in local knowledge resources; meeting persistence remains in AppState.
pub mod chunking;
pub mod conversations;
pub mod embedding;
pub mod indexer;
pub mod model;
pub mod retrieval;
pub mod scheduler;
pub mod store;
pub mod types;
use std::sync::Arc;
pub struct KnowledgeState {
    pub index_worker: indexer::IndexWorker,
    pub configuration: Arc<tokio::sync::Mutex<()>>,
    pub cancellation: Arc<scheduler::CancellationRegistry>,
    pub scheduler: Arc<scheduler::Scheduler>,
    pub downloads: model::ModelDownloads,
}
impl Default for KnowledgeState {
    fn default() -> Self {
        let cancellation = Arc::new(scheduler::CancellationRegistry::default());
        Self {
            index_worker: indexer::IndexWorker::default(),
            configuration: Arc::new(tokio::sync::Mutex::new(())),
            scheduler: scheduler::Scheduler::new(cancellation.clone()),
            cancellation,
            downloads: model::ModelDownloads::default(),
        }
    }
}
