#[derive(Debug, Clone, Copy)]
pub enum EmbeddingPurpose {
    Query,
    Passage,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingSpace {
    pub id: String,
    pub dimensions: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KnowledgeError {
    #[error("Semantic indexing is disabled")]
    Disabled,
    #[error("Local embedding model is unavailable")]
    ModelUnavailable,
    #[error("Recording or another inference job is active")]
    Busy,
    #[error("Knowledge work was cancelled")]
    Cancelled,
    #[error("Invalid embedding input")]
    InvalidInput,
    #[error("Document extraction failed")]
    ExtractionFailure,
    #[error("Knowledge storage failed")]
    Storage,
    #[error("Knowledge provider failed")]
    ProviderFailure,
}
