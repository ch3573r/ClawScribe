//! Opt-in local knowledge resources; meeting persistence remains in AppState.
pub mod answers;
pub mod chunking;
pub mod commands;
pub mod conversations;
pub mod document_context;
#[cfg(test)]
mod document_contract_tests;
pub mod documents;
pub mod embedding;
pub mod evidence;
pub mod indexer;
pub mod live;
pub mod live_context;
pub mod model;
pub mod retrieval;
pub mod scheduler;
pub mod store;
pub mod types;
use std::sync::Arc;
pub struct KnowledgeState {
    pub live: Arc<live::LiveState>,
    pub documents: documents::Imports,
    pub answers: Arc<answers::AnswerRegistry>,
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
            live: Arc::new(live::LiveState::default()),
            documents: documents::Imports::default(),
            answers: Arc::new(answers::AnswerRegistry::default()),
            index_worker: indexer::IndexWorker::default(),
            configuration: Arc::new(tokio::sync::Mutex::new(())),
            scheduler: scheduler::Scheduler::new(cancellation.clone()),
            cancellation,
            downloads: model::ModelDownloads::default(),
        }
    }
}
