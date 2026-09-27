use crate::parakeet_engine::model::{ParakeetModel, TimestampedResult};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::fs;
use tokio::sync::RwLock;

/// Whether to route Parakeet inference through the DirectML (Windows GPU)
/// execution provider. Beta, opt-in: set via `set_parakeet_use_directml`, read
/// when a model is (re)loaded. No effect unless the `directml` feature is built.
pub static USE_PARAKEET_DIRECTML: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Quantization type for Parakeet models
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum QuantizationType {
    FP32, // Full precision
    FP16, // Half precision for GPU-oriented ONNX exports
    Int8, // 8-bit integer quantization (faster)
}

impl Default for QuantizationType {
    fn default() -> Self {
        QuantizationType::Int8 // Default to int8 for best performance
    }
}

impl QuantizationType {
    fn model_suffix(&self) -> Option<&'static str> {
        match self {
            QuantizationType::FP32 => None,
            QuantizationType::FP16 => Some("fp16"),
            QuantizationType::Int8 => Some("int8"),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            QuantizationType::FP32 => "FP32",
            QuantizationType::FP16 => "FP16",
            QuantizationType::Int8 => "Int8 quantized",
        }
    }
}

/// Model status for Parakeet models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelStatus {
    Available,
    Missing,
    Downloading {
        progress: u8,
    },
    Error(String),
    Corrupted {
        file_size: u64,
        expected_min_size: u64,
    },
}

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
            ((downloaded as f64 / total as f64) * 100.0).min(100.0) as u8
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

/// Information about a Parakeet model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub path: PathBuf,
    pub size_mb: u32,
    pub quantization: QuantizationType,
    pub speed: String, // Performance description
    pub status: ModelStatus,
    pub description: String,
}

#[derive(Debug)]
pub enum ParakeetEngineError {
    ModelNotLoaded,
    ModelNotFound(String),
    TranscriptionFailed(String),
    DownloadFailed(String),
    IoError(std::io::Error),
    Other(String),
}

impl std::fmt::Display for ParakeetEngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParakeetEngineError::ModelNotLoaded => write!(f, "No Parakeet model loaded"),
            ParakeetEngineError::ModelNotFound(name) => write!(f, "Model '{}' not found", name),
            ParakeetEngineError::TranscriptionFailed(err) => {
                write!(f, "Transcription failed: {}", err)
            }
            ParakeetEngineError::DownloadFailed(err) => write!(f, "Download failed: {}", err),
            ParakeetEngineError::IoError(err) => write!(f, "IO error: {}", err),
            ParakeetEngineError::Other(err) => write!(f, "Error: {}", err),
        }
    }
}

impl std::error::Error for ParakeetEngineError {}

impl From<std::io::Error> for ParakeetEngineError {
    fn from(err: std::io::Error) -> Self {
        ParakeetEngineError::IoError(err)
    }
}

fn parakeet_base_url(model_name: &str) -> &'static str {
    if model_name.contains("smoothquant") {
        "https://huggingface.co/Olicorne/parakeet-tdt-0.6b-v3-smoothquant-onnx/resolve/2748538802098c611cf00e7c9a959a1515f695f7"
    } else if model_name.contains("-fp16") {
        "https://huggingface.co/grikdotnet/parakeet-tdt-0.6b-fp16/resolve/dc9871ec5ad84a420940077e76e8741b3609bf8b"
    } else if model_name.contains("-v2-") {
        "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/0bbb45a3365852604aef28b538a8f066f4ccaa85"
    } else {
        // Default to v3 for v3 models
        "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce"
    }
}

pub struct ParakeetEngine {
    models_dir: PathBuf,
    current_model: Arc<RwLock<Option<ParakeetModel>>>,
    current_model_name: Arc<RwLock<Option<String>>>,
    pub(crate) available_models: Arc<RwLock<HashMap<String, ModelInfo>>>,
    downloads: crate::model_download::Downloads,
}

impl ParakeetEngine {
    /// Create a new Parakeet engine with optional custom models directory
    pub fn new_with_models_dir(models_dir: Option<PathBuf>) -> Result<Self> {
        let models_dir = if let Some(dir) = models_dir {
            dir.join("parakeet") // Parakeet models in subdirectory
        } else {
            // Fallback to default location
            let current_dir = std::env::current_dir()
                .map_err(|e| anyhow!("Failed to get current directory: {}", e))?;

            if cfg!(debug_assertions) {
                // Development mode
                current_dir.join("models").join("parakeet")
            } else {
                // Production mode
                dirs::data_dir()
                    .or_else(|| dirs::home_dir())
                    .ok_or_else(|| anyhow!("Could not find system data directory"))?
                    .join("ClawScribe")
                    .join("models")
                    .join("parakeet")
            }
        };

        log::info!("Parakeet model storage initialized");

        // Create directory if it doesn't exist
        if !models_dir.exists() {
            std::fs::create_dir_all(&models_dir)?;
        }

        Ok(Self {
            models_dir,
            current_model: Arc::new(RwLock::new(None)),
            current_model_name: Arc::new(RwLock::new(None)),
            available_models: Arc::new(RwLock::new(HashMap::new())),
            downloads: crate::model_download::Downloads::default(),
        })
    }

    /// Discover available Parakeet models
    pub async fn discover_models(&self) -> Result<Vec<ModelInfo>> {
        loop {
            let generation = self.downloads.generation();
            let models_dir = &self.models_dir;
            let mut models = Vec::new();

            // Parakeet model configurations
            // Model name format: parakeet-tdt-0.6b-v{version}-{quantization}
            // Sizes match actual download sizes (encoder + decoder + preprocessor + vocab)
            let model_configs = [
            (
                "parakeet-tdt-0.6b-v3-int8",
                670,
                QuantizationType::Int8,
                "Ultra Fast (v3)",
                "Fastest default. Stock v3 int8; measured ~22x realtime on the 9070 XT.",
            ),
            (
                "parakeet-tdt-0.6b-v3-smoothquant-int8",
                813,
                QuantizationType::Int8,
                "Experimental (SmoothQuant)",
                "Larger SmoothQuant int8 export intended to improve long-audio accuracy versus stock int8.",
            ),
            (
                "parakeet-tdt-0.6b-v2-int8",
                661,
                QuantizationType::Int8,
                "Fast (v2)",
                "Previous version with int8 quantization, good balance of speed and accuracy",
            ),
        ];

            // Get active downloads to override status

            for (name, size_mb, quantization, speed, description) in model_configs {
                let model_path = models_dir.join(name);

                // Check if model is currently downloading
                let status = if self.downloads.is_active(name) {
                    // If downloading, preserve that status regardless of file system
                    // We don't know the exact progress here without more state, but 0 is safe fallback
                    // The progress events will update the UI
                    ModelStatus::Downloading { progress: 0 }
                } else if model_path.exists() {
                    // Check for required ONNX files
                    let required_files = match quantization {
                        QuantizationType::Int8 => vec![
                            "encoder-model.int8.onnx",
                            "decoder_joint-model.int8.onnx",
                            "nemo128.onnx",
                            "vocab.txt",
                        ],
                        QuantizationType::FP16 => vec![
                            "encoder-model.fp16.onnx",
                            "decoder_joint-model.fp16.onnx",
                            "nemo128.onnx",
                            "vocab.txt",
                        ],
                        QuantizationType::FP32 => vec![
                            "encoder-model.onnx",
                            "decoder_joint-model.onnx",
                            "nemo128.onnx",
                            "vocab.txt",
                        ],
                    };

                    let all_files_exist = required_files
                        .iter()
                        .all(|file| model_path.join(file).exists());

                    if all_files_exist {
                        // Validate model by checking file sizes
                        match self.validate_model_directory(&model_path).await {
                            Ok(_) => ModelStatus::Available,
                            Err(_) => {
                                log::warn!("Model directory appears corrupted");
                                // Calculate total size of existing files
                                let mut total_size = 0u64;
                                for file in required_files {
                                    if let Ok(metadata) = std::fs::metadata(model_path.join(file)) {
                                        total_size += metadata.len();
                                    }
                                }
                                ModelStatus::Corrupted {
                                    file_size: total_size,
                                    expected_min_size: (size_mb as u64) * 1024 * 1024,
                                }
                            }
                        }
                    } else {
                        ModelStatus::Missing
                    }
                } else {
                    ModelStatus::Missing
                };

                let model_info = ModelInfo {
                    name: name.to_string(),
                    path: model_path,
                    size_mb: size_mb as u32,
                    quantization: quantization.clone(),
                    speed: speed.to_string(),
                    status,
                    description: description.to_string(),
                };

                models.push(model_info);
            }

            // Update internal cache
            let mut available_models = self.available_models.write().await;
            if generation != self.downloads.generation() {
                continue;
            }
            available_models.clear();
            for model in &models {
                available_models.insert(model.name.clone(), model.clone());
            }

            return Ok(models);
        }
    }

    /// Validate model directory by checking if all required files exist AND have valid sizes
    async fn validate_model_directory(&self, model_dir: &PathBuf) -> Result<()> {
        // Check if vocab.txt exists and is readable
        let vocab_path = model_dir.join("vocab.txt");
        if !vocab_path.exists() {
            return Err(anyhow!("vocab.txt not found"));
        }

        // Determine which files to check based on what exists
        let is_int8 = model_dir.join("encoder-model.int8.onnx").exists();
        let is_fp16 = model_dir.join("encoder-model.fp16.onnx").exists();
        let is_fp32 = model_dir.join("encoder-model.onnx").exists();
        let is_smoothquant = model_dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains("smoothquant"));

        if !is_int8 && !is_fp16 && !is_fp32 {
            return Err(anyhow!("No ONNX model files found"));
        }

        // Check preprocessor
        if !model_dir.join("nemo128.onnx").exists() {
            return Err(anyhow!("Preprocessor (nemo128.onnx) not found"));
        }

        // Define minimum file sizes (90% of expected to allow some variance)
        // These are critical to catch partial downloads that would crash on load
        let expected_sizes: Vec<(&str, u64)> = if is_int8 && is_smoothquant {
            vec![
                ("encoder-model.int8.onnx", 720_000_000), // ~794 MB, min 720 MB
                ("decoder_joint-model.int8.onnx", 16_000_000), // ~18 MB, min 16 MB
                ("nemo128.onnx", 100_000),                // ~140 KB, min 100 KB
                ("vocab.txt", 5_000),                     // ~94 KB, min 5 KB
            ]
        } else if is_int8 {
            vec![
                ("encoder-model.int8.onnx", 580_000_000), // ~652 MB, min 580 MB (89%)
                ("decoder_joint-model.int8.onnx", 8_000_000), // ~18 MB, min 8 MB
                ("nemo128.onnx", 100_000),                // ~140 KB, min 100 KB
                ("vocab.txt", 5_000),                     // ~94 KB, min 5 KB
            ]
        } else if is_fp16 {
            vec![
                ("encoder-model.fp16.onnx", 1_100_000_000), // ~1.24 GB, min 1.1 GB
                ("decoder_joint-model.fp16.onnx", 30_000_000), // ~36 MB, min 30 MB
                ("nemo128.onnx", 100_000),                  // ~140 KB, min 100 KB
                ("vocab.txt", 5_000),                       // ~94 KB, min 5 KB
            ]
        } else {
            vec![
                ("encoder-model.onnx", 2_200_000_000), // ~2.44 GB, min 2.2 GB
                ("decoder_joint-model.onnx", 65_000_000), // ~72 MB, min 65 MB
                ("nemo128.onnx", 100_000),             // ~140 KB, min 100 KB
                ("vocab.txt", 5_000),                  // ~94 KB, min 5 KB
            ]
        };

        // Validate each file exists AND has sufficient size
        for (filename, min_size) in expected_sizes {
            let model_name = model_dir
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| anyhow!("Invalid model folder"))?;
            crate::model_download::verify_pinned_file(
                &model_dir.join(filename),
                &format!("{}/{filename}", parakeet_base_url(model_name)),
            )
            .await?;
            let file_path = model_dir.join(filename);
            if !file_path.exists() {
                return Err(anyhow!("{} not found", filename));
            }

            match std::fs::metadata(&file_path) {
                Ok(metadata) => {
                    let actual_size = metadata.len();
                    if actual_size < min_size {
                        return Err(anyhow!(
                            "{} is incomplete: {} bytes (expected at least {} bytes)",
                            filename,
                            actual_size,
                            min_size
                        ));
                    }
                }
                Err(e) => {
                    return Err(anyhow!("Failed to read {} metadata: {}", filename, e));
                }
            }
        }

        Ok(())
    }

    /// Load a Parakeet model
    pub async fn load_model(&self, model_name: &str) -> Result<()> {
        let model_info = self
            .available_models
            .read()
            .await
            .get(model_name)
            .cloned()
            .ok_or_else(|| anyhow!("Model {} not found", model_name))?;

        match model_info.status {
            ModelStatus::Available => {
                // Check if this model is already loaded
                // Drop the read guard before unload_model takes the write lock.
                let loaded_model = self.get_current_model().await;
                if let Some(current_model) = loaded_model.as_deref() {
                    if current_model == model_name {
                        log::info!(
                            "Parakeet model {} is already loaded, skipping reload",
                            model_name
                        );
                        return Ok(());
                    }

                    // Unload current model before loading new one
                    log::info!(
                        "Unloading current Parakeet model '{}' before loading '{}'",
                        current_model,
                        model_name
                    );
                    self.unload_model().await;
                }

                log::info!("Loading Parakeet model: {}", model_name);
                self.validate_model_directory(&model_info.path).await?;

                // Load model based on precision/quantization type.
                let precision_suffix = model_info.quantization.model_suffix();
                // Beta opt-in: route inference through DirectML (Windows GPU) when enabled.
                let use_directml = USE_PARAKEET_DIRECTML.load(std::sync::atomic::Ordering::Relaxed);
                let path = model_info.path.clone();
                let model = crate::audio::inference::run(move |_cancelled| {
                    ParakeetModel::new(&path, precision_suffix, use_directml)
                        .map_err(anyhow::Error::from)
                })
                .await
                .map_err(|e| anyhow!("Failed to load Parakeet model {}: {}", model_name, e))?;

                // Update current model and model name
                *self.current_model.write().await = Some(model);
                *self.current_model_name.write().await = Some(model_name.to_string());

                log::info!(
                    "Successfully loaded Parakeet model: {} ({})",
                    model_name,
                    model_info.quantization.label()
                );
                Ok(())
            }
            ModelStatus::Missing => Err(anyhow!("Parakeet model {} is not downloaded", model_name)),
            ModelStatus::Downloading { .. } => Err(anyhow!(
                "Parakeet model {} is currently downloading",
                model_name
            )),
            ModelStatus::Error(ref err) => {
                Err(anyhow!("Parakeet model {} has error: {}", model_name, err))
            }
            ModelStatus::Corrupted { .. } => Err(anyhow!(
                "Parakeet model {} is corrupted and cannot be loaded",
                model_name
            )),
        }
    }

    /// Unload the current model
    pub async fn unload_model(&self) -> bool {
        let mut model_guard = self.current_model.write().await;
        let unloaded = model_guard.take().is_some();
        if unloaded {
            log::info!("Parakeet model unloaded");
        }

        let mut model_name_guard = self.current_model_name.write().await;
        model_name_guard.take();

        unloaded
    }

    /// Get the currently loaded model name
    pub async fn get_current_model(&self) -> Option<String> {
        self.current_model_name.read().await.clone()
    }

    /// Check if a model is loaded
    pub async fn is_model_loaded(&self) -> bool {
        self.current_model.read().await.is_some()
    }

    /// Transcribe audio samples using the loaded Parakeet model
    pub async fn transcribe_audio(&self, audio_data: Vec<f32>) -> Result<String> {
        Ok(self.transcribe_audio_timestamped(audio_data).await?.text)
    }

    /// Transcribe audio samples using the loaded Parakeet model and retain
    /// provider-native token timestamps for downstream word alignment.
    pub async fn transcribe_audio_timestamped(
        &self,
        audio_data: Vec<f32>,
    ) -> Result<TimestampedResult> {
        let mut model_guard = self.current_model.clone().write_owned().await;
        crate::audio::inference::run(move |_cancelled| {
            let model = model_guard
                .as_mut()
                .ok_or_else(|| anyhow!("No Parakeet model loaded. Please load a model first."))?;

            let duration_seconds = audio_data.len() as f64 / 16000.0; // Assuming 16kHz
            log::debug!(
                "Parakeet transcribing {} samples ({:.1}s duration)",
                audio_data.len(),
                duration_seconds
            );

            // Transcribe using Parakeet model
            let start = Instant::now();
            let result = model
                .transcribe_samples(audio_data)
                .map_err(|e| anyhow!("Parakeet transcription failed: {}", e))?;
            let elapsed = start.elapsed();
            let elapsed_seconds = elapsed.as_secs_f64();
            let speed = if elapsed_seconds > 0.0 {
                duration_seconds / elapsed_seconds
            } else {
                0.0
            };

            log::info!(
                "Parakeet segment: audio={:.2}s elapsed={}ms speed={:.2}x chars={}",
                duration_seconds,
                elapsed.as_millis(),
                speed,
                result.text.chars().count()
            );

            Ok(result)
        })
        .await
    }

    /// Get the models directory path
    pub async fn get_models_directory(&self) -> PathBuf {
        self.models_dir.clone()
    }

    /// Delete a corrupted model
    pub async fn delete_model(&self, model_name: &str) -> Result<String> {
        let _reservation = self.downloads.start(model_name)?;
        log::info!("Attempting to delete Parakeet model: {}", model_name);

        // Get model info to find the directory path
        let model_info = {
            let models = self.available_models.read().await;
            models.get(model_name).cloned()
        };

        let model_info =
            model_info.ok_or_else(|| anyhow!("Parakeet model '{}' not found", model_name))?;

        log::info!(
            "Parakeet model '{}' has status: {:?}",
            model_name,
            model_info.status
        );

        // Allow deletion of corrupted or available models
        match &model_info.status {
            ModelStatus::Corrupted { .. } | ModelStatus::Available => {
                // Delete the entire model directory
                if model_info.path.exists() {
                    fs::remove_dir_all(&model_info.path).await
                        .map_err(|e| anyhow!("Failed to delete directory '{}': {}", model_info.path.display(), e))?;
                    log::info!("Successfully deleted Parakeet model directory");
                } else {
                    log::warn!("Model directory already absent");
                }

                // Update model status to Missing
                {
                    let mut models = self.available_models.write().await;
                    if let Some(model) = models.get_mut(model_name) {
                        model.status = ModelStatus::Missing;
                    }
                }

                Ok(format!("Successfully deleted Parakeet model '{}'", model_name))
            }
            _ => {
                Err(anyhow!(
                    "Can only delete corrupted or available Parakeet models. Model '{}' has status: {:?}",
                    model_name,
                    model_info.status
                ))
            }
        }
    }

    /// Download a Parakeet model from HuggingFace (backward-compatible wrapper)
    pub async fn download_model(
        &self,
        model_name: &str,
        progress_callback: Option<Box<dyn Fn(u8) + Send>>,
    ) -> Result<()> {
        // Wrap simple callback to use detailed version
        let detailed_callback: Option<Box<dyn Fn(DownloadProgress) + Send>> = progress_callback
            .map(|cb| {
                Box::new(move |p: DownloadProgress| cb(p.percent))
                    as Box<dyn Fn(DownloadProgress) + Send>
            });
        self.download_model_detailed(model_name, detailed_callback)
            .await
    }

    /// Download a Parakeet model with detailed progress (MB/speed/resume support)
    pub async fn download_model_detailed(
        &self,
        model_name: &str,
        mut progress_callback: Option<Box<dyn Fn(DownloadProgress) + Send>>,
    ) -> Result<()> {
        let reservation = self.downloads.start(model_name)?;
        let model_info = self
            .available_models
            .read()
            .await
            .get(model_name)
            .cloned()
            .ok_or_else(|| anyhow!("Unknown Parakeet model"))?;
        if let Some(model) = self.available_models.write().await.get_mut(model_name) {
            model.status = ModelStatus::Downloading { progress: 0 };
        }
        // Source URL for Parakeet models (variant-specific).
        let base_url = parakeet_base_url(model_name);

        // Determine which files to download based on quantization
        let files_to_download = match model_info.quantization {
            QuantizationType::Int8 => vec![
                "encoder-model.int8.onnx",
                "decoder_joint-model.int8.onnx",
                "nemo128.onnx",
                "vocab.txt",
            ],
            QuantizationType::FP16 => vec![
                "encoder-model.fp16.onnx",
                "decoder_joint-model.fp16.onnx",
                "nemo128.onnx",
                "vocab.txt",
            ],
            QuantizationType::FP32 => vec![
                "encoder-model.onnx",
                "decoder_joint-model.onnx",
                "nemo128.onnx",
                "vocab.txt",
            ],
        };

        let operation = async {
            fs::create_dir_all(&model_info.path).await?;
            let client = crate::model_download::client()?;
            let started = Instant::now();
            let mut completed_bytes = 0;
            let mut transferred_bytes = 0;
            let count = files_to_download.len() as u64;
            for (index, filename) in files_to_download.iter().enumerate() {
                let mut previous_bytes = None;
                let size = crate::model_download::download_file(
                    &client,
                    &format!("{base_url}/{filename}"),
                    &model_info.path.join(filename),
                    reservation.token(),
                    |bytes, total| {
                        if let Some(previous) = previous_bytes {
                            transferred_bytes += bytes.saturating_sub(previous);
                        }
                        previous_bytes = Some(bytes);
                        if let Some(callback) = progress_callback.as_mut() {
                            let mut progress = DownloadProgress::new(
                                completed_bytes + bytes,
                                completed_bytes + total,
                                transferred_bytes as f64
                                    / 1_048_576.0
                                    / started.elapsed().as_secs_f64().max(0.001),
                            );
                            // Unknown remaining variant sizes: report monotonic per-file
                            // progress, reserving 100% for validation and worker completion.
                            progress.percent = ((index as u64 * 100
                                + bytes.saturating_mul(100) / total.max(1))
                                / count)
                                .min(99) as u8;
                            callback(progress);
                        }
                    },
                )
                .await?;
                completed_bytes += size;
            }
            self.validate_model_directory(&model_info.path).await?;
            Ok::<_, anyhow::Error>(DownloadProgress::new(
                completed_bytes,
                completed_bytes,
                transferred_bytes as f64 / 1_048_576.0 / started.elapsed().as_secs_f64().max(0.001),
            ))
        };
        let result = operation.await;
        let mut models = self.available_models.write().await;
        if let Some(model) = models.get_mut(model_name) {
            model.status = if result.is_ok() {
                ModelStatus::Available
            } else {
                ModelStatus::Missing
            };
        }
        if let Ok(progress) = &result {
            if let Some(callback) = progress_callback.as_mut() {
                callback(progress.clone());
            }
        }
        // Reservation is released only after file handles and status writes finish.
        // Hold the cache lock through release so discovery cannot restore a
        // stale Downloading status between the status write and generation bump.
        drop(reservation);
        result.map(|_| ())
    }

    pub async fn cancel_download(&self, model_name: &str) -> Result<()> {
        self.downloads.cancel(model_name).await
    }
}

#[cfg(test)]
mod model_switch_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn switching_model_releases_name_lock_before_unloading() {
        let directory = tempfile::tempdir().unwrap();
        let engine =
            ParakeetEngine::new_with_models_dir(Some(directory.path().to_path_buf())).unwrap();
        *engine.current_model_name.write().await = Some("previous".to_string());
        engine.available_models.write().await.insert(
            "replacement".to_string(),
            ModelInfo {
                name: "replacement".to_string(),
                path: directory.path().join("missing"),
                size_mb: 0,
                quantization: QuantizationType::Int8,
                speed: String::new(),
                status: ModelStatus::Available,
                description: String::new(),
            },
        );

        let result = tokio::time::timeout(Duration::from_secs(5), engine.load_model("replacement"))
            .await
            .expect("model switching must not deadlock");
        assert!(result.is_err());
        assert_eq!(engine.get_current_model().await, None);
        assert!(!engine.is_model_loaded().await);
    }
}
