// Model manager for built-in AI models - handles downloads and lifecycle
// Follows the same pattern as whisper_engine/whisper_engine.rs for consistency

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::RwLock;

use super::models::{get_available_models, get_model_by_name};

// ============================================================================
// Model Status Types
// ============================================================================

/// Detailed download progress info (MB-based with speed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    /// Bytes downloaded so far
    pub downloaded_bytes: u64,
    /// Total file size in bytes
    pub total_bytes: u64,
    /// Downloaded in MB (for display)
    pub downloaded_mb: f64,
    /// Total size in MB (for display)
    pub total_mb: f64,
    /// Download speed in MB/s
    pub speed_mbps: f64,
    /// Percentage complete (0-100)
    pub percent: u8,
}

impl DownloadProgress {
    pub fn new(downloaded: u64, total: u64, speed_mbps: f64) -> Self {
        let percent = if total > 0 {
            ((downloaded as f64 / total as f64) * 100.0) as u8
        } else {
            0
        };
        Self {
            downloaded_bytes: downloaded,
            total_bytes: total,
            downloaded_mb: downloaded as f64 / (1024.0 * 1024.0),
            total_mb: total as f64 / (1024.0 * 1024.0),
            speed_mbps,
            percent,
        }
    }
}

/// Model status in the system
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelStatus {
    /// Model is not yet downloaded
    NotDownloaded,

    /// Model is currently being downloaded (progress 0-100)
    Downloading { progress: u8 },

    /// Model is downloaded and ready to use
    Available,

    /// Model file is corrupted and needs redownload
    Corrupted {
        file_size: u64,
        expected_min_size: u64,
    },

    /// Error occurred with the model
    Error(String),
}

/// Model information for UI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model name (e.g., "gemma3:1b")
    pub name: String,

    /// Display name for UI
    pub display_name: String,

    /// Current status
    pub status: ModelStatus,

    /// File path (if available)
    pub path: PathBuf,

    /// Size in MB
    pub size_mb: u64,

    /// Context window size in tokens
    pub context_size: u32,

    /// Description
    pub description: String,

    /// GGUF filename on disk
    pub gguf_file: String,
}

// ============================================================================
// Model Manager
// ============================================================================

pub struct ModelManager {
    /// Directory where models are stored
    models_dir: PathBuf,

    /// Currently available models with their status
    available_models: Arc<RwLock<HashMap<String, ModelInfo>>>,

    /// Active downloads (model names)
    active_downloads: crate::model_download::Downloads,
    verified: RwLock<HashMap<PathBuf, (u64, std::time::SystemTime, String)>>,
    #[cfg(test)]
    verify_calls: std::sync::atomic::AtomicUsize,
}

#[derive(Serialize, Deserialize)]
struct VerifiedFile {
    file: String,
    size: u64,
    mtime: std::time::SystemTime,
    sha256: String,
}

impl ModelManager {
    /// Create a new model manager with default models directory
    pub fn new() -> Result<Self> {
        Self::new_with_models_dir(None)
    }

    /// Create a new model manager with custom models directory
    pub fn new_with_models_dir(models_dir: Option<PathBuf>) -> Result<Self> {
        let models_dir = if let Some(dir) = models_dir {
            dir
        } else {
            // Fallback: Use current directory in development
            let current_dir = std::env::current_dir()
                .map_err(|e| anyhow!("Failed to get current directory: {}", e))?;

            if cfg!(debug_assertions) {
                // Development mode
                current_dir.join("models").join("summary")
            } else {
                // Production mode fallback (caller should provide path)
                log::warn!("ModelManager: No models directory provided, using fallback path");
                dirs::data_dir()
                    .or_else(|| dirs::home_dir())
                    .ok_or_else(|| anyhow!("Could not find system data directory"))?
                    .join("ClawScribe")
                    .join("models")
                    .join("summary")
            }
        };

        log::info!(
            "Built-in AI ModelManager using directory: {}",
            models_dir.display()
        );

        Ok(Self {
            models_dir,
            available_models: Arc::new(RwLock::new(HashMap::new())),
            active_downloads: Default::default(),
            verified: RwLock::new(HashMap::new()),
            #[cfg(test)]
            verify_calls: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Initialize and scan for existing models
    pub async fn init(&self) -> Result<()> {
        // Create models directory if it doesn't exist
        if !self.models_dir.exists() {
            fs::create_dir_all(&self.models_dir).await?;
            log::info!("Created models directory: {}", self.models_dir.display());
        }

        if let Ok(bytes) = fs::read(self.models_dir.join("verified.json")).await {
            if let Ok(entries) = serde_json::from_slice::<Vec<VerifiedFile>>(&bytes) {
                let mut verified = self.verified.write().await;
                for entry in entries {
                    if std::path::Path::new(&entry.file)
                        .file_name()
                        .and_then(|name| name.to_str())
                        == Some(entry.file.as_str())
                    {
                        verified.insert(
                            self.models_dir.join(entry.file),
                            (entry.size, entry.mtime, entry.sha256),
                        );
                    }
                }
            }
        }
        // Scan for existing models
        self.scan_models().await?;

        Ok(())
    }

    /// Scan models directory and update status
    pub async fn scan_models(&self) -> Result<()> {
        let start = std::time::Instant::now();

        log::info!(
            "Starting model scan in directory: {}",
            self.models_dir.display()
        );

        let model_defs = get_available_models();
        let mut models_map = HashMap::new();

        for model_def in model_defs {
            let model_path = self.models_dir.join(&model_def.gguf_file);
            log::debug!(
                "Checking model '{}' at path: {}",
                model_def.name,
                model_path.display()
            );

            let is_actively_downloading = self.active_downloads.is_active(&model_def.name);

            // If actively downloading, preserve existing status from memory
            if is_actively_downloading {
                let existing_info = {
                    let models = self.available_models.read().await;
                    models.get(&model_def.name).cloned()
                };

                if let Some(info) = existing_info {
                    // Preserve existing status (should be Downloading)
                    models_map.insert(model_def.name.clone(), info);
                    log::debug!(
                        "Model '{}': Preserving Downloading status during scan",
                        model_def.name
                    );
                    continue;
                }
            }

            let status = if self.verified_model(&model_path, &model_def).await {
                ModelStatus::Available
            } else if let Ok(metadata) = fs::metadata(&model_path).await {
                ModelStatus::Corrupted {
                    file_size: metadata.len() / (1024 * 1024),
                    expected_min_size: model_def.size_mb,
                }
            } else {
                ModelStatus::NotDownloaded
            };

            let model_info = ModelInfo {
                name: model_def.name.clone(),
                display_name: model_def.display_name.clone(),
                status,
                path: model_path,
                size_mb: model_def.size_mb,
                context_size: model_def.context_size,
                description: model_def.description.clone(),
                gguf_file: model_def.gguf_file.clone(),
            };

            models_map.insert(model_def.name.clone(), model_info);
        }

        let model_count = models_map.len();

        let mut models = self.available_models.write().await;
        *models = models_map;

        let elapsed = start.elapsed();
        log::info!(
            "Model scan complete: {} models checked in {:?}",
            model_count,
            elapsed
        );
        Ok(())
    }

    /// Get list of all models with their status
    pub async fn list_models(&self) -> Vec<ModelInfo> {
        self.available_models
            .read()
            .await
            .values()
            .cloned()
            .collect()
    }

    /// Get info for a specific model
    pub async fn get_model_info(&self, model_name: &str) -> Option<ModelInfo> {
        self.available_models.read().await.get(model_name).cloned()
    }

    /// Check if a model is ready to use
    /// If refresh=true, scans filesystem before checking (slower but accurate)
    pub async fn is_model_ready(&self, model_name: &str, refresh: bool) -> bool {
        if refresh {
            if let Err(e) = self.scan_models().await {
                log::error!("Failed to scan models: {}", e);
                return false;
            }
        }

        if let Some(info) = self.get_model_info(model_name).await {
            info.status == ModelStatus::Available
        } else {
            false
        }
    }

    /// Download a model with simple percentage callback (backward compatible)
    pub async fn download_model(
        &self,
        model_name: &str,
        progress_callback: Option<Box<dyn Fn(u8) + Send>>,
    ) -> Result<()> {
        // Wrap the simple callback to use detailed progress internally
        let detailed_callback: Option<Box<dyn Fn(DownloadProgress) + Send>> = progress_callback
            .map(|cb| {
                Box::new(move |p: DownloadProgress| cb(p.percent))
                    as Box<dyn Fn(DownloadProgress) + Send>
            });
        self.download_model_detailed(model_name, detailed_callback)
            .await
    }

    /// Download a model with detailed progress (MB, speed, etc.)
    pub async fn download_model_detailed(
        &self,
        model_name: &str,
        progress_callback: Option<Box<dyn Fn(DownloadProgress) + Send>>,
    ) -> Result<()> {
        let model_def =
            get_model_by_name(model_name).ok_or_else(|| anyhow!("Unknown summary model"))?;
        let reservation = self.active_downloads.start(model_name)?;
        let path = self.models_dir.join(&model_def.gguf_file);
        if self.verified_model(&path, &model_def).await {
            if let Some(callback) = progress_callback {
                callback(DownloadProgress::new(
                    model_def.size_bytes,
                    model_def.size_bytes,
                    0.0,
                ));
            }
            return Ok(());
        }
        self.invalidate_verified(&path).await;
        let progress_callback = std::sync::Mutex::new(progress_callback);
        if let Some(info) = self.available_models.write().await.get_mut(model_name) {
            info.status = ModelStatus::Downloading { progress: 0 };
        }
        let result = async {
            fs::create_dir_all(&self.models_dir).await?;
            let start = std::time::Instant::now();
            let mut initial = None;
            crate::model_download::download_file_checked(
                &crate::model_download::client()?,
                &model_def.download_url,
                &path,
                reservation.token(),
                Some((model_def.size_bytes, &model_def.sha256)),
                |downloaded, total| {
                    let base = *initial.get_or_insert(downloaded);
                    let speed = downloaded.saturating_sub(base) as f64
                        / (1024.0 * 1024.0)
                        / start.elapsed().as_secs_f64().max(0.001);
                    let mut progress = DownloadProgress::new(downloaded, total, speed);
                    progress.percent = progress.percent.min(99);
                    if let Ok(mut models) = self.available_models.try_write() {
                        if let Some(info) = models.get_mut(model_name) {
                            info.status = ModelStatus::Downloading {
                                progress: progress.percent.min(99),
                            };
                        }
                    }
                    if let Some(callback) = &*progress_callback.lock().unwrap() {
                        callback(progress);
                    }
                },
            )
            .await?;
            self.cache_verified(&path, &model_def.sha256).await;
            if let Some(callback) = &*progress_callback.lock().unwrap() {
                callback(DownloadProgress::new(
                    model_def.size_bytes,
                    model_def.size_bytes,
                    0.0,
                ));
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if let Some(info) = self.available_models.write().await.get_mut(model_name) {
            info.status = match &result {
                Ok(()) => ModelStatus::Available,
                Err(error) if error.is::<crate::model_download::Cancelled>() => {
                    ModelStatus::NotDownloaded
                }
                Err(error) => ModelStatus::Error(error.to_string()),
            };
        }
        result.map_err(|error| {
            if error.is::<crate::model_download::Cancelled>() {
                anyhow!("CANCELLED: Download cancelled by user")
            } else {
                error
            }
        })
    }

    async fn cache_verified(&self, path: &PathBuf, hash: &str) {
        if let Ok(meta) = fs::metadata(path).await {
            if let Ok(modified) = meta.modified() {
                let mut verified = self.verified.write().await;
                verified.insert(path.clone(), (meta.len(), modified, hash.into()));
                self.persist_verified(&verified).await;
            }
        }
    }

    async fn persist_verified(
        &self,
        verified: &HashMap<PathBuf, (u64, std::time::SystemTime, String)>,
    ) {
        let entries: Vec<_> = verified
            .iter()
            .filter_map(|(path, (size, mtime, sha256))| {
                Some(VerifiedFile {
                    file: path.file_name()?.to_str()?.to_owned(),
                    size: *size,
                    mtime: *mtime,
                    sha256: sha256.clone(),
                })
            })
            .collect();
        // An interrupted or unwritable receipt only causes re-verification.
        if let Ok(bytes) = serde_json::to_vec(&entries) {
            let _ = fs::write(self.models_dir.join("verified.json"), bytes).await;
        }
    }

    async fn invalidate_verified(&self, path: &PathBuf) {
        let mut verified = self.verified.write().await;
        verified.remove(path);
        self.persist_verified(&verified).await;
    }

    async fn verified_model(&self, path: &PathBuf, model: &super::models::ModelDef) -> bool {
        let Ok(meta) = fs::metadata(path).await else {
            return false;
        };
        if meta.len() != model.size_bytes {
            return false;
        }
        if let Ok(modified) = meta.modified() {
            if self.verified.read().await.get(path)
                == Some(&(meta.len(), modified, model.sha256.clone()))
            {
                return true;
            }
        }
        #[cfg(test)]
        self.verify_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if crate::model_download::verify_file(path, model.size_bytes, &model.sha256)
            .await
            .is_err()
        {
            return false;
        }
        if fs::metadata(path).await.is_ok_and(|after| {
            after.len() == meta.len() && after.modified().ok() == meta.modified().ok()
        }) {
            self.cache_verified(path, &model.sha256).await;
        } else {
            return false;
        }
        true
    }

    /// Cancellation keeps the reservation until the transfer has closed its files.
    pub async fn cancel_download(&self, model_name: &str) -> Result<()> {
        self.active_downloads.cancel(model_name).await
    }

    /// Delete a corrupted or available model file
    pub async fn delete_model(&self, model_name: &str) -> Result<()> {
        log::info!("Deleting model: {}", model_name);

        let model_def = get_model_by_name(model_name)
            .ok_or_else(|| anyhow!("Unknown model: {}", model_name))?;

        self.active_downloads.cancel(model_name).await?;
        let _reservation = self.active_downloads.start(model_name)?;
        let file_path = self.models_dir.join(&model_def.gguf_file);
        let partial = crate::model_download::partial_path(&file_path);
        if fs::try_exists(&partial).await? {
            fs::remove_file(partial).await?;
        }
        self.invalidate_verified(&file_path).await;

        if file_path.exists() {
            fs::remove_file(&file_path).await?;
            log::info!("Deleted model file: {}", file_path.display());
        }

        // Update status
        {
            let mut models = self.available_models.write().await;
            if let Some(model_info) = models.get_mut(model_name) {
                model_info.status = ModelStatus::NotDownloaded;
            }
        }

        Ok(())
    }

    /// Get models directory path
    pub fn get_models_directory(&self) -> PathBuf {
        self.models_dir.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn verification_survives_restart_and_invalidates_changed_files() {
        use sha2::{Digest, Sha256};
        use std::sync::atomic::Ordering;
        let directory = tempfile::tempdir().unwrap();
        let mut model = get_available_models().remove(0);
        model.gguf_file = "synthetic.gguf".into();
        model.size_bytes = 8;
        model.sha256 = format!("{:x}", Sha256::digest(b"GGUFtest"));
        let path = directory.path().join(&model.gguf_file);
        fs::write(&path, b"GGUFtest").await.unwrap();
        let first = ModelManager::new_with_models_dir(Some(directory.path().into())).unwrap();
        assert!(first.verified_model(&path, &model).await);
        assert_eq!(first.verify_calls.load(Ordering::SeqCst), 1);
        let second = ModelManager::new_with_models_dir(Some(directory.path().into())).unwrap();
        second.init().await.unwrap();
        assert!(second.verified_model(&path, &model).await);
        assert_eq!(second.verify_calls.load(Ordering::SeqCst), 0);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2)),
            )
            .unwrap();
        assert!(second.verified_model(&path, &model).await);
        assert_eq!(second.verify_calls.load(Ordering::SeqCst), 1);
        second.invalidate_verified(&path).await;
        let third = ModelManager::new_with_models_dir(Some(directory.path().into())).unwrap();
        third.init().await.unwrap();
        assert!(third.verified_model(&path, &model).await);
        assert_eq!(third.verify_calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn readiness_requires_complete_hash_and_ignores_partial_files() {
        use sha2::{Digest, Sha256};
        let temp = tempfile::tempdir().unwrap();
        let manager = ModelManager::new_with_models_dir(Some(temp.path().into())).unwrap();
        let mut model = get_available_models().remove(0);
        model.size_bytes = 8;
        model.sha256 = format!("{:x}", Sha256::digest(b"GGUFtest"));
        let path = temp.path().join(&model.gguf_file);
        fs::write(crate::model_download::partial_path(&path), b"GGUFtes")
            .await
            .unwrap();
        assert!(!manager.verified_model(&path, &model).await);
        fs::write(&path, b"GGUFtes").await.unwrap();
        assert!(!manager.verified_model(&path, &model).await);
        fs::write(&path, b"GGUFbad!").await.unwrap();
        assert!(!manager.verified_model(&path, &model).await);
        fs::write(&path, b"GGUFtest").await.unwrap();
        assert!(manager.verified_model(&path, &model).await);
        assert!(manager.verified.read().await.contains_key(&path));
        assert!(manager.verified_model(&path, &model).await);
    }
}
