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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KnowledgeScope {
    Meeting { meeting_id: String },
    Library { filter: MeetingFilter },
    Live { session_id: String },
}
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MeetingFilter {
    pub all_meetings: bool,
    pub meeting_ids: Vec<String>,
    pub tags: Vec<String>,
    pub tag_mode: TagMatch,
    pub untagged: bool,
    pub from: Option<String>,
    pub to: Option<String>,
}
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TagMatch {
    #[default]
    Any,
    All,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchRequest {
    pub scope: KnowledgeScope,
    pub query: String,
    pub document_ids: Vec<String>,
    pub mode: SearchMode,
}
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Keyword,
    Hybrid,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct EvidenceRef {
    pub source_id: String,
    pub source_revision: i64,
    pub chunk_id: String,
    pub fingerprint: String,
    pub locator: EvidenceLocator,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceLocator {
    Transcript {
        meeting_id: String,
        transcript_ids: Vec<String>,
        spans: Vec<TextSpan>,
        start_seconds: Option<f64>,
    },
    Document {
        document_id: String,
        page: Option<u32>,
        paragraph: u32,
    },
    Live {
        session_id: String,
        sequence_ids: Vec<u64>,
    },
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TextSpan {
    pub transcript_id: String,
    pub start_byte: usize,
    pub end_byte: usize,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Passage {
    pub evidence: EvidenceRef,
    pub meeting_id: String,
    pub title: String,
    pub date: String,
    pub speaker: Option<String>,
    pub text: String,
    pub rank: f64,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexStatus {
    pub keyword_ready: bool,
    pub semantic_enabled: bool,
    pub semantic_ready: usize,
    pub pending: usize,
    pub failed: usize,
    pub reason: Option<String>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchResponse {
    pub passages: Vec<Passage>,
    pub mode: SearchMode,
    pub index_status: IndexStatus,
}

impl From<sqlx::Error> for KnowledgeError {
    fn from(_: sqlx::Error) -> Self {
        Self::Storage
    }
}
