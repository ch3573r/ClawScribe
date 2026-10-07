//! Pin verification and explicit opt-in downloads reuse the shared transfer registry.
use super::types::{EmbeddingSpace, KnowledgeError};
use crate::model_download::{self, Downloads};
use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

#[derive(serde::Deserialize)]
pub struct ModelPins {
    pub model: String,
    pub revision: String,
    pub dimensions: usize,
    pub preprocessing: String,
    pub pooling: String,
    pub tokenizer_version: String,
    pub files: Vec<ModelFile>,
}
#[derive(serde::Deserialize)]
pub struct ModelFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}
pub static PINS: Lazy<ModelPins> = Lazy::new(|| {
    serde_json::from_str(include_str!("embedding-model-pins.json")).expect("embedding pins")
});
impl ModelPins {
    pub fn space(&self) -> EmbeddingSpace {
        EmbeddingSpace {
            id: format!(
                "{}@{}:{}:{}:tokenizers-{}:{}:{}",
                self.model,
                self.revision,
                self.preprocessing,
                self.pooling,
                self.tokenizer_version,
                self.files[0].sha256,
                self.files[1].sha256
            ),
            dimensions: self.dimensions,
        }
    }
    fn url(&self, file: &ModelFile) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.model, self.revision, file.path
        )
    }
}
#[derive(Clone)]
pub struct VerifiedModel {
    root: PathBuf,
}
impl VerifiedModel {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub async fn verify(root: &Path) -> Result<Self, KnowledgeError> {
        for file in &PINS.files {
            model_download::verify_file(&root.join(&file.path), file.size, &file.sha256)
                .await
                .map_err(|_| KnowledgeError::ModelUnavailable)?;
        }
        Ok(Self { root: root.into() })
    }
}
#[derive(Default)]
pub struct ModelDownloads {
    downloads: Downloads,
}
impl ModelDownloads {
    pub async fn download(
        &self,
        root: &Path,
        cancelled: CancellationToken,
    ) -> Result<VerifiedModel, KnowledgeError> {
        let reservation = self
            .downloads
            .start("knowledge-e5")
            .map_err(|_| KnowledgeError::Busy)?;
        let operation = async {
            let client = model_download::client().map_err(|_| KnowledgeError::ModelUnavailable)?;
            for file in &PINS.files {
                let path = root.join(&file.path);
                tokio::fs::create_dir_all(path.parent().ok_or(KnowledgeError::ModelUnavailable)?)
                    .await
                    .map_err(|_| KnowledgeError::ModelUnavailable)?;
                model_download::download_file_checked(
                    &client,
                    &PINS.url(file),
                    &path,
                    reservation.token(),
                    Some((file.size, &file.sha256)),
                    |_, _| {},
                )
                .await
                .map_err(|_| {
                    if reservation.token().is_cancelled() {
                        KnowledgeError::Cancelled
                    } else {
                        KnowledgeError::ModelUnavailable
                    }
                })?;
            }
            VerifiedModel::verify(root).await
        };
        tokio::pin!(operation);
        tokio::select! {
            result = &mut operation => result,
            _ = cancelled.cancelled() => {
                reservation.token().cancel();
                // Keep the reservation until the transfer closes its file.
                let _ = operation.await;
                Err(KnowledgeError::Cancelled)
            }
        }
    }
    pub async fn cancel(&self) -> Result<(), KnowledgeError> {
        self.downloads
            .cancel("knowledge-e5")
            .await
            .map_err(|_| KnowledgeError::Busy)
    }
}
