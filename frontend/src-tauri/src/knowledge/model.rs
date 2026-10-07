//! Pin verification and explicit opt-in downloads reuse the shared transfer registry.
use super::types::{EmbeddingSpace, KnowledgeError};
use crate::model_download::{self, Downloads};
use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Conservatively distinguish vectors computed by a different loading policy.
pub const LOADING_POLICY: &str = "cpu-no-graph-optimization-no-prepacking-v1";

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
                "{}@{}:{}:{}:tokenizers-{}:{}:{}:{LOADING_POLICY}",
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
    status: Mutex<DownloadStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStage {
    Idle,
    Checking,
    Downloading,
    Verifying,
    Cancelling,
    Ready,
    Cancelled,
    Error,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct DownloadStatus {
    #[serde(skip)]
    generation: u64,
    pub stage: DownloadStage,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub current_file: Option<String>,
    pub error: Option<String>,
}
impl Default for DownloadStatus {
    fn default() -> Self {
        Self {
            generation: 0,
            stage: DownloadStage::Idle,
            downloaded_bytes: 0,
            total_bytes: PINS.files.iter().map(|file| file.size).sum(),
            current_file: None,
            error: None,
        }
    }
}

/// Presence is only a presentation hint. Loading still requires pinned SHA verification.
pub async fn files_present(root: &Path) -> bool {
    for file in &PINS.files {
        if tokio::fs::metadata(root.join(&file.path))
            .await
            .map_or(true, |metadata| metadata.len() != file.size)
        {
            return false;
        }
    }
    true
}
impl ModelDownloads {
    pub fn snapshot(&self) -> DownloadStatus {
        self.status.lock().unwrap().clone()
    }
    pub async fn download(
        &self,
        root: &Path,
        cancelled: CancellationToken,
    ) -> Result<VerifiedModel, KnowledgeError> {
        let urls: Vec<_> = PINS.files.iter().map(|file| PINS.url(file)).collect();
        self.download_files(root, &PINS, &urls, cancelled).await
    }
    async fn download_files(
        &self,
        root: &Path,
        pins: &ModelPins,
        urls: &[String],
        cancelled: CancellationToken,
    ) -> Result<VerifiedModel, KnowledgeError> {
        if pins.files.len() != urls.len() {
            return Err(KnowledgeError::ModelUnavailable);
        }
        let reservation = self
            .downloads
            .start("knowledge-e5")
            .map_err(|_| KnowledgeError::Busy)?;
        *self.status.lock().unwrap() = DownloadStatus {
            generation: reservation.generation(),
            stage: DownloadStage::Checking,
            total_bytes: pins.files.iter().map(|file| file.size).sum(),
            ..DownloadStatus::default()
        };
        let operation = async {
            let client = model_download::client().map_err(|_| KnowledgeError::ModelUnavailable)?;
            let mut completed = 0;
            for (file, url) in pins.files.iter().zip(urls) {
                if reservation.token().is_cancelled() {
                    return Err(KnowledgeError::Cancelled);
                }
                {
                    let mut status = self.status.lock().unwrap();
                    status.stage = DownloadStage::Checking;
                    status.current_file = Some(file.path.clone());
                }
                let path = root.join(&file.path);
                tokio::fs::create_dir_all(path.parent().ok_or(KnowledgeError::ModelUnavailable)?)
                    .await
                    .map_err(|_| KnowledgeError::ModelUnavailable)?;
                model_download::download_file_checked_with_verification(
                    &client,
                    url,
                    &path,
                    reservation.token(),
                    Some((file.size, &file.sha256)),
                    |bytes, _| {
                        let mut status = self.status.lock().unwrap();
                        if status.stage != DownloadStage::Cancelling {
                            status.stage = if bytes >= file.size {
                                DownloadStage::Verifying
                            } else {
                                DownloadStage::Downloading
                            };
                        }
                        status.downloaded_bytes = completed + bytes.min(file.size);
                    },
                    || {
                        let mut status = self.status.lock().unwrap();
                        if status.stage != DownloadStage::Cancelling {
                            status.stage = DownloadStage::Verifying;
                        }
                    },
                )
                .await
                .map_err(|_| {
                    if reservation.token().is_cancelled() {
                        KnowledgeError::Cancelled
                    } else {
                        KnowledgeError::ModelUnavailable
                    }
                })?;
                completed += file.size;
            }
            if reservation.token().is_cancelled() {
                return Err(KnowledgeError::Cancelled);
            }
            // Each checked transfer verifies before returning, including healthy
            // installed files. No second full-model hash or network request is needed.
            Ok(VerifiedModel { root: root.into() })
        };
        tokio::pin!(operation);
        let result = tokio::select! {
            result = &mut operation => result,
            _ = cancelled.cancelled() => {
                reservation.token().cancel();
                // Keep the reservation until the transfer closes its file.
                let _ = operation.await;
                Err(KnowledgeError::Cancelled)
            }
        };
        {
            let mut status = self.status.lock().unwrap();
            match &result {
                Ok(_) => {
                    status.stage = DownloadStage::Ready;
                    status.downloaded_bytes = status.total_bytes;
                    status.current_file = None;
                }
                Err(KnowledgeError::Cancelled) => status.stage = DownloadStage::Cancelled,
                Err(error) => {
                    status.stage = DownloadStage::Error;
                    status.error = Some(error.to_string());
                }
            }
        }
        result
    }
    pub async fn cancel(&self) -> Result<(), KnowledgeError> {
        let generation = {
            let mut status = self.status.lock().unwrap();
            if !matches!(
                status.stage,
                DownloadStage::Checking
                    | DownloadStage::Downloading
                    | DownloadStage::Verifying
                    | DownloadStage::Cancelling
            ) {
                return Ok(());
            }
            status.stage = DownloadStage::Cancelling;
            status.generation
        };
        self.downloads
            .cancel_generation("knowledge-e5", generation)
            .await
            .map_err(|_| KnowledgeError::Busy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn cancellation_during_completion_preserves_terminal_ready_state() {
        let downloads = ModelDownloads::default();
        let reservation = downloads.downloads.start("knowledge-e5").unwrap();
        downloads.status.lock().unwrap().stage = DownloadStage::Ready;
        // The completed worker publishes Ready just before its reservation drops.
        // Poll cancellation first while the registry still contains that worker.
        let (result, ()) = tokio::join!(downloads.cancel(), async move {
            tokio::task::yield_now().await;
            drop(reservation);
        });
        result.unwrap();
        assert_eq!(downloads.snapshot().stage, DownloadStage::Ready);
    }

    fn pins() -> ModelPins {
        ModelPins {
            model: "synthetic".into(),
            revision: "synthetic".into(),
            dimensions: 1,
            preprocessing: "synthetic".into(),
            pooling: "synthetic".into(),
            tokenizer_version: "synthetic".into(),
            files: [("model", b"abcdef"), ("tokenizer", b"uvwxyz")]
                .into_iter()
                .map(|(path, body)| ModelFile {
                    path: path.into(),
                    size: body.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(body)),
                })
                .collect(),
        }
    }
    async fn request(socket: &mut tokio::net::TcpStream) {
        let mut request = Vec::new();
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let mut bytes = [0; 1024];
            let count = socket.read(&mut bytes).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&bytes[..count]);
        }
    }
    #[tokio::test]
    async fn real_transfer_updates_aggregate_progress_and_retains_cancelled_state() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            drop(socket);
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nab")
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            socket.write_all(b"cd").await.unwrap();
            std::future::pending::<()>().await;
        });
        let downloads = Arc::new(ModelDownloads::default());
        let transfer = downloads.clone();
        let worker = tokio::spawn(async move {
            transfer
                .download_files(
                    &root,
                    &pins(),
                    &[url.clone(), url],
                    CancellationToken::new(),
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let status = downloads.snapshot();
                if status.downloaded_bytes == 4 {
                    assert_eq!(status.stage, DownloadStage::Downloading);
                    assert_eq!(status.total_bytes, 12);
                    assert_eq!(status.current_file.as_deref(), Some("model"));
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        downloads.cancel().await.unwrap();
        assert!(matches!(
            worker.await.unwrap(),
            Err(KnowledgeError::Cancelled)
        ));
        server.abort();
        let status = downloads.snapshot();
        assert_eq!(status.stage, DownloadStage::Cancelled);
        assert_eq!(status.downloaded_bytes, 4);
        assert_eq!(
            tokio::fs::read(model_download::partial_path(&dir.path().join("model")))
                .await
                .unwrap(),
            b"abcd"
        );
        assert!(!downloads.downloads.is_active("knowledge-e5"));
    }
    #[tokio::test]
    async fn healthy_files_are_rehashed_without_network_or_rewriting_and_become_ready() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("model"), b"abcdef")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("tokenizer"), b"uvwxyz")
            .await
            .unwrap();
        let before = std::fs::metadata(dir.path().join("model"))
            .unwrap()
            .modified()
            .unwrap();
        let downloads = ModelDownloads::default();
        let urls = vec!["http://127.0.0.1:1/model".into(); 2];
        downloads
            .download_files(dir.path(), &pins(), &urls, CancellationToken::new())
            .await
            .unwrap();
        let status = downloads.snapshot();
        assert_eq!(status.stage, DownloadStage::Ready);
        assert_eq!(status.downloaded_bytes, 12);
        assert_eq!(status.total_bytes, 12);
        assert!(status.current_file.is_none());
        assert_eq!(
            std::fs::metadata(dir.path().join("model"))
                .unwrap()
                .modified()
                .unwrap(),
            before
        );
        // Corruption must fail verification and enter repair rather than reuse a prior receipt.
        tokio::fs::write(dir.path().join("model"), b"wrong!")
            .await
            .unwrap();
        assert!(downloads
            .download_files(dir.path(), &pins(), &urls, CancellationToken::new())
            .await
            .is_err());
        assert_eq!(downloads.snapshot().stage, DownloadStage::Error);
        assert!(downloads.snapshot().error.is_some());
        assert!(!dir.path().join("model").exists());
    }
}
