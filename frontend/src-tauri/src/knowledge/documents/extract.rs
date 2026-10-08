//! Parser contract; extraction runs only in the bounded child in the app.
use super::{DocumentError, DocumentFormat, ExtractedDocument};

pub fn extract(_format: DocumentFormat, _bytes: &[u8]) -> Result<ExtractedDocument, DocumentError> {
    Err(DocumentError::Malformed)
}
