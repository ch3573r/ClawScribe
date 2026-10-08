//! Resource limits and path-free reference-document values.
use serde::{Deserialize, Serialize};

pub const INPUT_BYTES: usize = 25 * 1024 * 1024;
pub const TEXT_BYTES: usize = 2 * 1024 * 1024;
pub const PAGE_LIMIT: usize = 500;
pub const ZIP_BYTES: usize = 32 * 1024 * 1024;
pub const XML_BYTES: usize = 5 * 1024 * 1024;
pub const WORKER_BYTES: usize = 512 * 1024 * 1024;
pub const EXTRACTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFormat {
    Pdf,
    Docx,
    Text,
    Markdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocumentBlock {
    pub page: Option<u32>,
    pub paragraph: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtractedDocument {
    pub format: DocumentFormat,
    pub blocks: Vec<DocumentBlock>,
}

#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum DocumentError {
    #[error("Choose a PDF, DOCX, TXT or Markdown file.")]
    UnsupportedFormat,
    #[error("The file content does not match its extension.")]
    FormatMismatch,
    #[error("Reference files must be 25 MiB or smaller.")]
    InputLimit,
    #[error("Extracted reference text exceeds 2 MiB. Choose a shorter document.")]
    TextLimit,
    #[error("PDF references must contain at most 500 pages.")]
    PageLimit,
    #[error("The DOCX expands beyond 32 MiB. Reduce embedded content and try again.")]
    ZipLimit,
    #[error("A DOCX XML part exceeds 5 MiB. Split the document and try again.")]
    XmlLimit,
    #[error("Encrypted PDFs are unsupported. Choose an unencrypted copy.")]
    EncryptedPdf,
    #[error("No readable text was found. Scanned PDFs need a text layer; OCR is unavailable.")]
    NoText,
    #[error("The reference file is malformed or contains unsupported XML.")]
    Malformed,
    #[error("TXT and Markdown references must use valid UTF-8 text.")]
    InvalidUtf8,
    #[error("Document extraction exceeded 10 seconds or its memory limit. Try a smaller file.")]
    WorkerLimit,
    #[error("Document import was cancelled.")]
    Cancelled,
    #[error("Finish recording or transcription before importing a reference document.")]
    Busy,
    #[error("The local document worker could not start safely. Try again.")]
    WorkerUnavailable,
    #[error("The reference file could not be read. Check that it is a regular readable file.")]
    FileUnavailable,
    #[error("Reference document storage failed. Check free disk space and try again.")]
    Storage,
    #[error("The meeting or reference attachment is unavailable.")]
    NotFound,
}
