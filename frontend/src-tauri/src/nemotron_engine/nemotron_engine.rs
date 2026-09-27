// nemotron_engine/nemotron_engine.rs
//
// Engine wrapper for the Nemotron streaming RNN-T model: model catalog
// (discover_models), download (HF, with resume/cancel/progress), load/unload,
// and transcription. Mirrors ParakeetEngine for parity; reuses Parakeet's
// DownloadProgress/ModelStatus value types to avoid divergence.

use crate::nemotron_engine::model::NemotronModel;
use crate::parakeet_engine::parakeet_engine::{DownloadProgress, ModelStatus};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::fs;
use tokio::sync::RwLock;

/// A downloadable Nemotron export. Two ship: fp16 (default, CPU-capable) and
/// int8 (smaller, GPU-only). They share the streaming RNN-T interface but differ
/// in repo, file layout, and — critically — whether the encoder can run on CPU.
pub struct NemotronVariant {
    pub id: &'static str,
    base_url: &'static str,
    /// Files to download with exact sizes; SHA-256 pins live in the shared manifest.
    /// Each fp16 .onnx has a sibling .onnx.data; the int8 encoder is a single
    /// inline file with no .data.
    files: &'static [(&'static str, u64)],
    pub size_mb: u32,
    speed: &'static str,
    description: &'static str,
    /// fp16 runs correctly on the CPU EP, so it gets a CPU baseline, a
    /// CPU-vs-DirectML self-test, and CPU fallback. The int8 encoder uses
    /// `ConvInteger`, which has no Rust CPU kernel — it is DirectML(GPU)-only,
    /// validated by output magnitude instead, with no CPU fallback.
    pub cpu_capable: bool,
}

const FP16: NemotronVariant = NemotronVariant {
    id: "nemotron-streaming-0.6b-fp16",
    base_url: "https://huggingface.co/soniqo/Nemotron-3.5-ASR-Streaming-Multilingual-0.6B-ONNX-FP16/resolve/76daabfd0aaf5ec6ef1e6640eae3b364af6c9970",
    files: &[
        ("encoder.onnx", 22_131_503),
        ("encoder.onnx.data", 1_236_396_032),
        ("decoder.onnx", 7_040),
        ("decoder.onnx.data", 29_880_320),
        ("joint.onnx", 3_207),
        ("joint.onnx.data", 18_911_296),
        ("vocab.json", 236_127),
        ("config.json", 602),
        ("languages.json", 2_020),
    ],
    size_mb: 1310,
    speed: "Streaming (FP16)",
    description:
        "NVIDIA Nemotron 3.5 ASR — streaming, multilingual (incl. German). FP16; tries DirectML GPU and falls back to CPU if the encoder self-test fails. Beta.",
    cpu_capable: true,
};

const INT8: NemotronVariant = NemotronVariant {
    id: "nemotron-streaming-0.6b-int8",
    base_url: "https://huggingface.co/soniqo/Nemotron-3.5-ASR-Streaming-Multilingual-0.6B-ONNX-INT8/resolve/1ce4daedd303e01d4e603634a72c28562f1a6855",
    files: &[
        ("encoder.onnx", 657_558_932),
        ("decoder.onnx", 4_345),
        ("decoder.onnx.data", 59_760_640),
        ("joint.onnx", 2_023),
        ("joint.onnx.data", 37_822_592),
        ("vocab.json", 236_127),
        ("config.json", 602),
        ("languages.json", 2_020),
    ],
    size_mb: 755,
    speed: "Streaming (INT8, GPU-only)",
    description:
        "NVIDIA Nemotron 3.5 ASR — streaming, multilingual (incl. German). INT8; smaller and GPU-only (DirectML), no CPU fallback. Beta.",
    cpu_capable: false,
};

/// Every selectable variant, in display order (fp16 first as the default).
pub const VARIANTS: &[NemotronVariant] = &[FP16, INT8];

/// Default model id when nothing is configured — fp16, which runs everywhere.
pub const NEMOTRON_MODEL: &str = FP16.id;

fn variant_for(id: &str) -> Option<&'static NemotronVariant> {
    VARIANTS.iter().find(|v| v.id == id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub path: PathBuf,
    pub size_mb: u32,
    pub speed: String,
    pub status: ModelStatus,
    pub description: String,
    /// True for the int8 variant, whose encoder ops only have a DirectML (GPU)
    /// implementation. On a build without the DirectML EP it isn't listed at
    /// all; on a DirectML build it's listed but still requires an actual GPU at
    /// load time — the UI can use this to badge/disable it.
    pub requires_gpu: bool,
}

pub struct NemotronEngine {
    models_dir: PathBuf,
    current_model: Arc<RwLock<Option<NemotronModel>>>,
    current_model_name: Arc<RwLock<Option<String>>>,
    pub(crate) available_models: Arc<RwLock<HashMap<String, ModelInfo>>>,
    active_downloads: crate::model_download::Downloads,
}

async fn verify_variant(variant: &NemotronVariant, folder: &std::path::Path) -> Result<()> {
    for (filename, _) in variant.files {
        crate::model_download::verify_pinned_file(
            &folder.join(filename),
            &format!("{}/{filename}", variant.base_url),
        )
        .await?;
    }
    Ok(())
}

impl NemotronEngine {
    pub fn new_with_models_dir(models_dir: Option<PathBuf>) -> Result<Self> {
        let models_dir = if let Some(dir) = models_dir {
            dir.join("nemotron")
        } else {
            let current_dir = std::env::current_dir()
                .map_err(|e| anyhow!("Failed to get current directory: {}", e))?;
            if cfg!(debug_assertions) {
                current_dir.join("models").join("nemotron")
            } else {
                dirs::data_dir()
                    .or_else(|| dirs::home_dir())
                    .ok_or_else(|| anyhow!("Could not find system data directory"))?
                    .join("ClawScribe")
                    .join("models")
                    .join("nemotron")
            }
        };

        log::info!("Nemotron model storage initialized");
        if !models_dir.exists() {
            std::fs::create_dir_all(&models_dir)?;
        }

        Ok(Self {
            models_dir,
            current_model: Arc::new(RwLock::new(None)),
            current_model_name: Arc::new(RwLock::new(None)),
            available_models: Arc::new(RwLock::new(HashMap::new())),
            active_downloads: crate::model_download::Downloads::default(),
        })
    }

    pub async fn discover_models(&self) -> Result<Vec<ModelInfo>> {
        let mut infos = Vec::with_capacity(VARIANTS.len());

        for v in VARIANTS {
            // The int8 encoder's ops only have a DirectML (GPU) kernel. On a
            // build compiled without the DirectML EP it can never load, so don't
            // even offer it; otherwise it's selectable but fails at load with a
            // confusing error. On a DirectML build it stays listed (still GPU-
            // gated at load) and is flagged via `requires_gpu`.
            if !v.cpu_capable && !cfg!(feature = "directml") {
                continue;
            }
            let model_path = self.models_dir.join(v.id);
            let status = if self.active_downloads.is_active(v.id) {
                ModelStatus::Downloading { progress: 0 }
            } else if model_path.exists() {
                if verify_variant(v, &model_path).await.is_ok() {
                    ModelStatus::Available
                } else {
                    ModelStatus::Corrupted {
                        file_size: 0,
                        expected_min_size: v.files.iter().map(|(_, size)| *size).sum(),
                    }
                }
            } else {
                ModelStatus::Missing
            };

            infos.push(ModelInfo {
                name: v.id.to_string(),
                path: model_path,
                size_mb: v.size_mb,
                speed: v.speed.to_string(),
                status,
                description: v.description.to_string(),
                requires_gpu: !v.cpu_capable,
            });
        }

        let mut cache = self.available_models.write().await;
        cache.clear();
        for info in &infos {
            cache.insert(info.name.clone(), info.clone());
        }
        Ok(infos)
    }

    pub async fn load_model(&self, model_name: &str) -> Result<()> {
        let variant = variant_for(model_name).ok_or_else(|| anyhow!("Unknown Nemotron model"))?;
        verify_variant(variant, &self.models_dir.join(model_name)).await?;
        let path = {
            let models = self.available_models.read().await;
            let info = models
                .get(model_name)
                .ok_or_else(|| anyhow!("Nemotron model {} not found", model_name))?;
            if !matches!(info.status, ModelStatus::Available) {
                return Err(anyhow!("Nemotron model {} is not available", model_name));
            }
            info.path.clone()
        };

        if let Some(cur) = self.current_model_name.read().await.as_ref() {
            if cur == model_name {
                return Ok(());
            }
        }
        self.unload_model().await;

        log::info!("Loading Nemotron model: {}", model_name);
        // fp16 tries DirectML then falls back to CPU; int8 is GPU-only.
        let cpu_capable = variant_for(model_name)
            .map(|v| v.cpu_capable)
            .unwrap_or(true);
        let model = crate::audio::inference::run(move |_cancelled| {
            NemotronModel::new(&path, cpu_capable).map_err(anyhow::Error::from)
        })
        .await
        .map_err(|e| anyhow!("Failed to load Nemotron model {}: {}", model_name, e))?;

        *self.current_model.write().await = Some(model);
        *self.current_model_name.write().await = Some(model_name.to_string());
        log::info!("Successfully loaded Nemotron model: {}", model_name);
        Ok(())
    }

    pub async fn unload_model(&self) -> bool {
        let unloaded = self.current_model.write().await.take().is_some();
        self.current_model_name.write().await.take();
        if unloaded {
            log::info!("Nemotron model unloaded");
        }
        unloaded
    }

    pub async fn get_current_model(&self) -> Option<String> {
        self.current_model_name.read().await.clone()
    }

    pub async fn is_model_loaded(&self) -> bool {
        self.current_model.read().await.is_some()
    }

    pub async fn transcribe_audio(
        &self,
        samples: Vec<f32>,
        language: Option<String>,
    ) -> Result<String> {
        // Resolve once at the engine boundary so live, import and retranscription
        // apply the same Auto/translation policy.
        let requested_language =
            crate::audio::transcription::nemotron_provider::resolve_requested_language(
                language.as_deref(),
                sys_locale::get_locale().as_deref(),
            )?;
        let mut guard = self.current_model.clone().write_owned().await;
        crate::audio::inference::run(move |cancelled| {
            let model = guard
                .as_mut()
                .ok_or_else(|| anyhow!("No Nemotron model loaded"))?;
            let slot = model
                .resolve_lang_slot(Some(&requested_language))
                .ok_or_else(|| {
                    anyhow!(
                        "Nemotron does not have a prompt slot for language '{}'",
                        requested_language
                    )
                })?;
            model
                .transcribe_samples_cancellable(samples, slot, &cancelled)
                .map_err(|e| anyhow!("Nemotron transcription failed: {}", e))
        })
        .await
    }

    pub async fn cancel_download(&self, model_name: &str) {
        if self.active_downloads.cancel(model_name).await.is_err() {
            log::warn!("Nemotron download cancellation is still pending");
        }
    }

    pub async fn download_model_detailed(
        &self,
        model_name: &str,
        mut progress_callback: Option<Box<dyn Fn(DownloadProgress) + Send>>,
    ) -> Result<()> {
        let variant = variant_for(model_name).ok_or_else(|| anyhow!("Unknown Nemotron model"))?;
        let reservation = self.active_downloads.start(model_name)?;
        let model_dir = self.models_dir.join(model_name);
        let operation = async {
            fs::create_dir_all(&model_dir).await?;
            let client = crate::model_download::client()?;
            let total: u64 = variant.files.iter().map(|(_, size)| *size).sum();
            let start = Instant::now();
            let mut completed = 0;
            for (filename, _) in variant.files {
                let bytes = crate::model_download::download_file(
                    &client,
                    &format!("{}/{filename}", variant.base_url),
                    &model_dir.join(filename),
                    reservation.token(),
                    |bytes, _| {
                        if let Some(callback) = progress_callback.as_mut() {
                            let mut progress = DownloadProgress::new(
                                completed + bytes,
                                total,
                                bytes as f64
                                    / 1_048_576.0
                                    / start.elapsed().as_secs_f64().max(0.001),
                            );
                            progress.percent = progress.percent.min(99);
                            callback(progress);
                        }
                    },
                )
                .await?;
                completed += bytes;
            }
            verify_variant(variant, &model_dir).await
        }
        .await;
        drop(reservation);
        let _ = self.discover_models().await;
        if operation.is_ok() {
            if let Some(callback) = progress_callback {
                let total = variant.files.iter().map(|(_, size)| *size).sum();
                callback(DownloadProgress::new(total, total, 0.0));
            }
        }
        operation
    }
}
