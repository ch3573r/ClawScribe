//! Opt-in local knowledge resources; meeting persistence remains in AppState.
pub mod chunking;
pub mod embedding;
pub mod model;
pub mod retrieval;
pub mod scheduler;
pub mod store;
pub mod types;
use std::sync::Arc;
pub struct KnowledgeState {
    pub cancellation: Arc<scheduler::CancellationRegistry>,
    pub scheduler: Arc<scheduler::Scheduler>,
    pub downloads: model::ModelDownloads,
}
impl Default for KnowledgeState {
    fn default() -> Self {
        let cancellation = Arc::new(scheduler::CancellationRegistry::default());
        Self {
            scheduler: scheduler::Scheduler::new(cancellation.clone()),
            cancellation,
            downloads: model::ModelDownloads::default(),
        }
    }
}
