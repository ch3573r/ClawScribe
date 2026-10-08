//! Canonical local reference documents and bounded extraction.
pub mod extract;
pub mod import;
pub mod store;
mod types;
pub mod worker;
pub use types::*;
#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
#[derive(Default)]
pub struct Imports {
    active: Arc<Mutex<Option<(String, CancellationToken)>>>,
}
pub struct ImportLease {
    registry: Arc<Mutex<Option<(String, CancellationToken)>>>,
    id: String,
    pub cancel: CancellationToken,
}
impl Imports {
    pub fn begin(&self, id: &str) -> Result<ImportLease, DocumentError> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(DocumentError::Malformed);
        }
        let mut active = self.active.lock().map_err(|_| DocumentError::Busy)?;
        if active.is_some() {
            return Err(DocumentError::Busy);
        }
        let cancel = CancellationToken::new();
        *active = Some((id.to_owned(), cancel.clone()));
        Ok(ImportLease {
            registry: self.active.clone(),
            id: id.to_owned(),
            cancel,
        })
    }
    pub fn cancel(&self, id: &str) {
        if let Ok(active) = self.active.lock() {
            if let Some((key, cancel)) = active.as_ref().filter(|(key, _)| key == id) {
                let _ = key;
                cancel.cancel();
            }
        }
    }
}
impl Drop for ImportLease {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Ok(mut active) = self.registry.lock() {
            if active.as_ref().is_some_and(|(id, _)| id == &self.id) {
                *active = None;
            }
        }
    }
}

pub fn original_path(
    root: &std::path::Path,
    name: &str,
) -> Result<std::path::PathBuf, DocumentError> {
    let (id, extension) = name.rsplit_once('.').ok_or(DocumentError::NotFound)?;
    let parsed = uuid::Uuid::parse_str(id).map_err(|_| DocumentError::NotFound)?;
    let format = DocumentFormat::from_extension(extension)?;
    if format.extension() != extension || parsed.to_string() != id {
        return Err(DocumentError::NotFound);
    }
    Ok(root.join(name))
}
