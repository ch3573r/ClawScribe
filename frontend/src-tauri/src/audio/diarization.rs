use crate::api::TranscriptSegment as ApiTranscriptSegment;
use crate::audio::constants::AUDIO_EXTENSIONS;
use crate::audio::decoder::decode_audio_file;
use crate::state::AppState;
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sherpa_onnx::{
    FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
    OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    SpeakerEmbeddingExtractorConfig,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, Runtime};

const FLOAT_TIE_EPSILON: f64 = 1e-9;
const DIARIZATION_SAMPLE_RATE: i32 = 16_000;
const SEGMENTATION_MODEL_DIR: &str = "sherpa-onnx-pyannote-segmentation-3-0";
const EMBEDDING_MODEL_DIR: &str =
    "sherpa-onnx-3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k";
const SEGMENTATION_MODEL_URL: &str =
    "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.int8.onnx";
const EMBEDDING_MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx";

static DIARIZATION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiarizationTurn {
    pub start_time: f64,
    pub end_time: f64,
    /// Zero-based speaker index from sherpa-onnx.
    pub speaker: usize,
}

pub type DiarizationSegment = DiarizationTurn;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub text: String,
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
    pub speaker: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiarizationMappingMode {
    /// Assign the speaker with the highest total overlap with the transcript segment.
    Overlap,
    /// Assign the speaker whose diarization turn contains the transcript midpoint.
    Midpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingSpeakerPolicy {
    PreserveNonEmpty,
    Overwrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiarizationMappingOptions {
    pub mode: DiarizationMappingMode,
    pub existing_speaker_policy: ExistingSpeakerPolicy,
}

impl Default for DiarizationMappingOptions {
    fn default() -> Self {
        Self {
            mode: DiarizationMappingMode::Overlap,
            existing_speaker_policy: ExistingSpeakerPolicy::PreserveNonEmpty,
        }
    }
}

pub fn speaker_label(speaker: usize) -> String {
    format!("Speaker {}", speaker + 1)
}

pub fn map_diarization_to_transcript_segments(
    transcript_segments: &[TranscriptSegment],
    diarization_turns: &[DiarizationTurn],
    options: DiarizationMappingOptions,
) -> Vec<TranscriptSegment> {
    transcript_segments
        .iter()
        .map(|segment| {
            if should_preserve_existing_speaker(segment, options.existing_speaker_policy) {
                return segment.clone();
            }

            let mut mapped = segment.clone();
            mapped.speaker = assign_speaker(segment, diarization_turns, options.mode)
                .map(speaker_label)
                .or_else(|| segment.speaker.clone());
            mapped
        })
        .collect()
}

pub fn assign_speaker(
    transcript_segment: &TranscriptSegment,
    diarization_turns: &[DiarizationTurn],
    mode: DiarizationMappingMode,
) -> Option<usize> {
    let start = transcript_segment.audio_start_time?;
    let end = transcript_segment.audio_end_time?;
    if !is_valid_interval(start, end) {
        return None;
    }

    match mode {
        DiarizationMappingMode::Overlap => best_speaker_by_overlap(start, end, diarization_turns),
        DiarizationMappingMode::Midpoint => best_speaker_by_midpoint(start, end, diarization_turns),
    }
}

fn best_speaker_by_overlap(
    start: f64,
    end: f64,
    diarization_turns: &[DiarizationTurn],
) -> Option<usize> {
    let mut overlap_by_speaker = BTreeMap::<usize, f64>::new();

    for turn in diarization_turns
        .iter()
        .filter(|turn| is_valid_interval(turn.start_time, turn.end_time))
    {
        let overlap = overlap_seconds(start, end, turn.start_time, turn.end_time);
        if overlap > 0.0 {
            *overlap_by_speaker.entry(turn.speaker).or_insert(0.0) += overlap;
        }
    }

    overlap_by_speaker
        .into_iter()
        .fold(None, |best: Option<(usize, f64)>, current| match best {
            Some((best_speaker, best_overlap)) if current.1 <= best_overlap + FLOAT_TIE_EPSILON => {
                Some((best_speaker, best_overlap))
            }
            _ => Some(current),
        })
        .map(|(speaker, _)| speaker)
}

fn best_speaker_by_midpoint(
    start: f64,
    end: f64,
    diarization_turns: &[DiarizationTurn],
) -> Option<usize> {
    let midpoint = start + ((end - start) / 2.0);

    diarization_turns
        .iter()
        .filter(|turn| {
            is_valid_interval(turn.start_time, turn.end_time)
                && midpoint >= turn.start_time
                && midpoint < turn.end_time
        })
        .map(|turn| turn.speaker)
        .min()
}

fn overlap_seconds(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> f64 {
    (a_end.min(b_end) - a_start.max(b_start)).max(0.0)
}

fn is_valid_interval(start: f64, end: f64) -> bool {
    start.is_finite() && end.is_finite() && end > start
}

fn should_preserve_existing_speaker(
    transcript_segment: &TranscriptSegment,
    policy: ExistingSpeakerPolicy,
) -> bool {
    matches!(policy, ExistingSpeakerPolicy::PreserveNonEmpty)
        && transcript_segment
            .speaker
            .as_deref()
            .map(|speaker| !speaker.trim().is_empty())
            .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct SherpaDiarizationConfig {
    pub segmentation_model_path: PathBuf,
    pub embedding_model_path: PathBuf,
    pub num_threads: i32,
    pub provider: String,
    pub num_clusters: Option<i32>,
    pub clustering_threshold: f32,
    pub min_duration_on: f32,
    pub min_duration_off: f32,
    pub debug: bool,
}

impl SherpaDiarizationConfig {
    pub fn new(
        segmentation_model_path: impl Into<PathBuf>,
        embedding_model_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            segmentation_model_path: segmentation_model_path.into(),
            embedding_model_path: embedding_model_path.into(),
            num_threads: 1,
            provider: "cpu".to_string(),
            num_clusters: None,
            clustering_threshold: 0.5,
            min_duration_on: 0.3,
            min_duration_off: 0.5,
            debug: false,
        }
    }
}

pub struct SherpaOfflineDiarizer {
    diarizer: OfflineSpeakerDiarization,
}

impl SherpaOfflineDiarizer {
    pub fn new(config: SherpaDiarizationConfig) -> Result<Self> {
        ensure_model_file(&config.segmentation_model_path, "segmentation")?;
        ensure_model_file(&config.embedding_model_path, "embedding")?;

        let segmentation_model = path_to_string(&config.segmentation_model_path)?;
        let embedding_model = path_to_string(&config.embedding_model_path)?;
        let num_threads = config.num_threads.max(1);
        let provider = config.provider.trim();
        let provider = if provider.is_empty() { "cpu" } else { provider };

        let sherpa_config = OfflineSpeakerDiarizationConfig {
            segmentation: OfflineSpeakerSegmentationModelConfig {
                pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                    model: Some(segmentation_model),
                },
                num_threads,
                debug: config.debug,
                provider: Some(provider.to_string()),
            },
            embedding: SpeakerEmbeddingExtractorConfig {
                model: Some(embedding_model),
                num_threads,
                debug: config.debug,
                provider: Some(provider.to_string()),
            },
            clustering: FastClusteringConfig {
                num_clusters: config.num_clusters.unwrap_or(-1),
                threshold: config.clustering_threshold,
            },
            min_duration_on: config.min_duration_on,
            min_duration_off: config.min_duration_off,
        };

        let diarizer = OfflineSpeakerDiarization::create(&sherpa_config)
            .ok_or_else(|| anyhow!("failed to create sherpa-onnx offline speaker diarizer"))?;

        Ok(Self { diarizer })
    }

    pub fn sample_rate(&self) -> i32 {
        self.diarizer.sample_rate()
    }

    pub fn diarize(&self, mono_samples: &[f32]) -> Result<Vec<DiarizationTurn>> {
        if mono_samples.is_empty() {
            return Ok(Vec::new());
        }

        let result = self
            .diarizer
            .process(mono_samples)
            .ok_or_else(|| anyhow!("sherpa-onnx speaker diarization failed"))?;

        Ok(result
            .sort_by_start_time()
            .into_iter()
            .filter_map(|segment| {
                let speaker = usize::try_from(segment.speaker).ok()?;
                let turn = DiarizationTurn {
                    start_time: f64::from(segment.start),
                    end_time: f64::from(segment.end),
                    speaker,
                };
                is_valid_interval(turn.start_time, turn.end_time).then_some(turn)
            })
            .collect())
    }
}

fn ensure_model_file(path: &Path, model_name: &str) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(anyhow!(
            "{} diarization model file not found: {}",
            model_name,
            path.display()
        ))
    }
}

fn path_to_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("model path is not valid UTF-8: {}", path.display()))
}

#[derive(Debug, Clone, Serialize)]
pub struct SpeakerDiarizationProgress {
    pub meeting_id: String,
    pub stage: String,
    pub progress_percentage: u32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpeakerDiarizationStartResponse {
    pub started: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpeakerDiarizationComplete {
    pub meeting_id: String,
    pub speaker_count: usize,
    pub updated_segments: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpeakerDiarizationError {
    pub meeting_id: String,
    pub error: String,
}

#[derive(Debug, Clone)]
struct DiarizationModelPaths {
    segmentation_model: PathBuf,
    embedding_model: PathBuf,
    can_download_segmentation: bool,
    can_download_embedding: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct StoredTranscriptSegment {
    id: String,
    transcript: String,
    timestamp: String,
    audio_start_time: Option<f64>,
    audio_end_time: Option<f64>,
    duration: Option<f64>,
    speaker: Option<String>,
}

struct DiarizationRunGuard;

impl DiarizationRunGuard {
    fn acquire() -> std::result::Result<Self, String> {
        DIARIZATION_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self)
            .map_err(|_| "Speaker diarization is already running".to_string())
    }
}

impl Drop for DiarizationRunGuard {
    fn drop(&mut self) {
        DIARIZATION_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

#[tauri::command]
pub async fn start_speaker_diarization_command<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
    segmentation_model_path: Option<String>,
    embedding_model_path: Option<String>,
    num_speakers: Option<i32>,
    preserve_existing_labels: Option<bool>,
) -> std::result::Result<SpeakerDiarizationStartResponse, String> {
    let guard = DiarizationRunGuard::acquire()?;
    let meeting_id_for_task = meeting_id.clone();

    tauri::async_runtime::spawn(async move {
        let _guard = guard;
        let result = run_speaker_diarization_for_meeting(
            app.clone(),
            meeting_id_for_task.clone(),
            meeting_folder_path,
            segmentation_model_path,
            embedding_model_path,
            num_speakers,
            preserve_existing_labels.unwrap_or(true),
        )
        .await;

        match result {
            Ok(complete) => {
                let _ = app.emit("speaker-diarization-complete", complete);
            }
            Err(error) => {
                let _ = app.emit(
                    "speaker-diarization-error",
                    SpeakerDiarizationError {
                        meeting_id: meeting_id_for_task,
                        error: error.to_string(),
                    },
                );
            }
        }
    });

    Ok(SpeakerDiarizationStartResponse { started: true })
}

#[tauri::command]
pub fn is_speaker_diarization_in_progress_command() -> bool {
    DIARIZATION_IN_PROGRESS.load(Ordering::SeqCst)
}

async fn run_speaker_diarization_for_meeting<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
    segmentation_model_path: Option<String>,
    embedding_model_path: Option<String>,
    num_speakers: Option<i32>,
    preserve_existing_labels: bool,
) -> Result<SpeakerDiarizationComplete> {
    let folder_path = PathBuf::from(&meeting_folder_path);
    if !folder_path.is_dir() {
        return Err(anyhow!(
            "Meeting folder is not available: {}",
            folder_path.display()
        ));
    }

    emit_progress(
        &app,
        &meeting_id,
        "locating_audio",
        5,
        "Finding meeting audio...",
    );
    let audio_path = find_audio_file(&folder_path)?;
    let model_paths = resolve_model_paths(&app, segmentation_model_path, embedding_model_path)?;

    ensure_model_available(
        &app,
        &meeting_id,
        &model_paths.segmentation_model,
        "segmentation",
        SEGMENTATION_MODEL_URL,
        model_paths.can_download_segmentation,
    )
    .await?;
    ensure_model_available(
        &app,
        &meeting_id,
        &model_paths.embedding_model,
        "embedding",
        EMBEDDING_MODEL_URL,
        model_paths.can_download_embedding,
    )
    .await?;

    emit_progress(
        &app,
        &meeting_id,
        "decoding",
        15,
        "Decoding meeting audio...",
    );
    let decode_path = audio_path.clone();
    let decoded = tokio::task::spawn_blocking(move || decode_audio_file(&decode_path))
        .await
        .map_err(|e| anyhow!("Audio decode task failed: {}", e))??;

    emit_progress(
        &app,
        &meeting_id,
        "preparing_audio",
        30,
        "Preparing 16 kHz mono audio...",
    );
    let samples = tokio::task::spawn_blocking(move || decoded.to_whisper_format())
        .await
        .map_err(|e| anyhow!("Audio preparation task failed: {}", e))?;
    if samples.is_empty() {
        return Err(anyhow!("Meeting audio did not contain decodable samples"));
    }

    emit_progress(
        &app,
        &meeting_id,
        "diarizing",
        45,
        "Detecting speaker turns...",
    );
    let mut config = SherpaDiarizationConfig::new(
        model_paths.segmentation_model.clone(),
        model_paths.embedding_model.clone(),
    );
    config.num_threads = default_diarization_threads();
    config.num_clusters = num_speakers.filter(|value| *value > 0);

    let turns = tokio::task::spawn_blocking(move || {
        let diarizer = SherpaOfflineDiarizer::new(config)?;
        let sample_rate = diarizer.sample_rate();
        if sample_rate != DIARIZATION_SAMPLE_RATE {
            return Err(anyhow!(
                "Diarization model expects {} Hz audio, but prepared audio is {} Hz",
                sample_rate,
                DIARIZATION_SAMPLE_RATE
            ));
        }
        diarizer.diarize(&samples)
    })
    .await
    .map_err(|e| anyhow!("Speaker diarization task failed: {}", e))??;

    if turns.is_empty() {
        return Err(anyhow!(
            "No speaker turns were detected in this meeting audio"
        ));
    }

    emit_progress(
        &app,
        &meeting_id,
        "saving",
        82,
        "Applying speaker labels to transcripts...",
    );

    let app_state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("Application database is not initialized"))?;
    let pool = app_state.db_manager.pool().clone();

    let stored_segments = sqlx::query_as::<_, StoredTranscriptSegment>(
        "SELECT id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker
         FROM transcripts
         WHERE meeting_id = ?
         ORDER BY COALESCE(audio_start_time, 999999999.0), timestamp, id",
    )
    .bind(&meeting_id)
    .fetch_all(&pool)
    .await?;

    if stored_segments.is_empty() {
        return Err(anyhow!("No transcript segments found for this meeting"));
    }

    let transcript_segments: Vec<TranscriptSegment> = stored_segments
        .iter()
        .map(|segment| TranscriptSegment {
            id: segment.id.clone(),
            text: segment.transcript.clone(),
            audio_start_time: segment.audio_start_time,
            audio_end_time: segment.audio_end_time,
            speaker: segment.speaker.clone(),
        })
        .collect();

    let mapped_segments = map_diarization_to_transcript_segments(
        &transcript_segments,
        &turns,
        DiarizationMappingOptions {
            mode: DiarizationMappingMode::Overlap,
            existing_speaker_policy: if preserve_existing_labels {
                ExistingSpeakerPolicy::PreserveNonEmpty
            } else {
                ExistingSpeakerPolicy::Overwrite
            },
        },
    );

    let mut updated_segments = 0usize;
    let mut tx = pool.begin().await?;
    for (stored, mapped) in stored_segments.iter().zip(mapped_segments.iter()) {
        if stored.speaker == mapped.speaker {
            continue;
        }

        let result =
            sqlx::query("UPDATE transcripts SET speaker = ? WHERE meeting_id = ? AND id = ?")
                .bind(&mapped.speaker)
                .bind(&meeting_id)
                .bind(&stored.id)
                .execute(&mut *tx)
                .await?;

        if result.rows_affected() > 0 {
            updated_segments += 1;
        }
    }

    if updated_segments > 0 {
        sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
            .bind(Utc::now())
            .bind(&meeting_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let transcript_file_segments: Vec<ApiTranscriptSegment> = stored_segments
        .iter()
        .zip(mapped_segments.iter())
        .map(|(stored, mapped)| ApiTranscriptSegment {
            id: stored.id.clone(),
            text: stored.transcript.clone(),
            timestamp: stored.timestamp.clone(),
            audio_start_time: stored.audio_start_time,
            audio_end_time: stored.audio_end_time,
            duration: stored.duration,
            speaker: mapped.speaker.clone(),
        })
        .collect();
    super::common::write_transcripts_json(&folder_path, &transcript_file_segments)?;

    let speaker_count = mapped_segments
        .iter()
        .filter_map(|segment| segment.speaker.as_deref())
        .filter(|speaker| !speaker.trim().is_empty())
        .collect::<BTreeSet<_>>()
        .len();

    emit_progress(
        &app,
        &meeting_id,
        "complete",
        100,
        "Speaker labels applied.",
    );

    Ok(SpeakerDiarizationComplete {
        meeting_id,
        speaker_count,
        updated_segments,
    })
}

fn emit_progress<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    stage: &str,
    progress_percentage: u32,
    message: &str,
) {
    let _ = app.emit(
        "speaker-diarization-progress",
        SpeakerDiarizationProgress {
            meeting_id: meeting_id.to_string(),
            stage: stage.to_string(),
            progress_percentage,
            message: message.to_string(),
        },
    );
}

fn default_diarization_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|threads| threads.get().clamp(1, 4) as i32)
        .unwrap_or(2)
}

fn resolve_model_paths<R: Runtime>(
    app: &AppHandle<R>,
    segmentation_model_path: Option<String>,
    embedding_model_path: Option<String>,
) -> Result<DiarizationModelPaths> {
    let models_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| anyhow!("Failed to resolve app data directory: {}", e))?
        .join("models")
        .join("diarization");

    let can_download_segmentation = segmentation_model_path
        .as_deref()
        .map(|path| path.trim().is_empty())
        .unwrap_or(true);
    let segmentation_model = match segmentation_model_path {
        Some(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => first_existing_path(&[
            models_dir
                .join(SEGMENTATION_MODEL_DIR)
                .join("model.int8.onnx"),
            models_dir.join(SEGMENTATION_MODEL_DIR).join("model.onnx"),
        ])
        .unwrap_or_else(|| {
            models_dir
                .join(SEGMENTATION_MODEL_DIR)
                .join("model.int8.onnx")
        }),
    };

    let can_download_embedding = embedding_model_path
        .as_deref()
        .map(|path| path.trim().is_empty())
        .unwrap_or(true);
    let embedding_model = match embedding_model_path {
        Some(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => first_existing_path(&[
            models_dir.join(EMBEDDING_MODEL_DIR).join("model.onnx"),
            models_dir.join("3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx"),
        ])
        .unwrap_or_else(|| models_dir.join(EMBEDDING_MODEL_DIR).join("model.onnx")),
    };

    if !can_download_segmentation {
        ensure_model_file(&segmentation_model, "segmentation")?;
    }
    if !can_download_embedding {
        ensure_model_file(&embedding_model, "embedding")?;
    }

    Ok(DiarizationModelPaths {
        segmentation_model,
        embedding_model,
        can_download_segmentation,
        can_download_embedding,
    })
}

async fn ensure_model_available<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    model_path: &Path,
    model_name: &str,
    download_url: &str,
    allow_download: bool,
) -> Result<()> {
    if model_path.is_file() {
        return Ok(());
    }

    if !allow_download {
        ensure_model_file(model_path, model_name)?;
    }

    emit_progress(
        app,
        meeting_id,
        "downloading_models",
        8,
        &format!("Downloading {model_name} diarization model..."),
    );

    let parent = model_path
        .parent()
        .ok_or_else(|| anyhow!("Model path has no parent: {}", model_path.display()))?;
    tokio::fs::create_dir_all(parent).await?;

    let response = reqwest::get(download_url).await.map_err(|e| {
        anyhow!(
            "Failed to download {} diarization model from {}: {}",
            model_name,
            download_url,
            e
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!(
            "Failed to download {} diarization model from {}: HTTP {}",
            model_name,
            download_url,
            status
        ));
    }

    let bytes = response.bytes().await.map_err(|e| {
        anyhow!(
            "Failed to read {} diarization model download from {}: {}",
            model_name,
            download_url,
            e
        )
    })?;
    if bytes.is_empty() {
        return Err(anyhow!(
            "{} diarization model download was empty: {}",
            model_name,
            download_url
        ));
    }

    let temp_path = model_path.with_extension("download");
    tokio::fs::write(&temp_path, &bytes).await?;
    tokio::fs::rename(&temp_path, model_path).await?;

    Ok(())
}

fn first_existing_path(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|path| path.is_file()).cloned()
}

fn find_audio_file(folder: &Path) -> Result<PathBuf> {
    let candidates = [
        "audio.mp4",
        "audio.m4a",
        "audio.wav",
        "audio.mp3",
        "audio.flac",
        "audio.ogg",
        "recording.mp4",
        "audio.mkv",
        "audio.webm",
        "audio.wma",
    ];

    for name in candidates {
        let path = folder.join(name);
        if path.is_file() {
            return Ok(path);
        }
    }

    for entry in std::fs::read_dir(folder)
        .map_err(|e| anyhow!("Failed to scan meeting folder {}: {}", folder.display(), e))?
    {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }

        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };

        if AUDIO_EXTENSIONS.contains(&extension.to_lowercase().as_str()) {
            return Ok(path);
        }
    }

    Err(anyhow!("No audio file found in: {}", folder.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(id: &str, start: Option<f64>, end: Option<f64>) -> TranscriptSegment {
        TranscriptSegment {
            id: id.to_string(),
            text: format!("segment {id}"),
            audio_start_time: start,
            audio_end_time: end,
            speaker: None,
        }
    }

    fn turn(start: f64, end: f64, speaker: usize) -> DiarizationTurn {
        DiarizationTurn {
            start_time: start,
            end_time: end,
            speaker,
        }
    }

    #[test]
    fn maps_segments_by_largest_overlap() {
        let transcripts = vec![
            transcript("a", Some(0.5), Some(1.5)),
            transcript("b", Some(2.25), Some(3.5)),
        ];
        let turns = vec![turn(0.0, 2.0, 0), turn(2.0, 4.0, 1)];

        let mapped = map_diarization_to_transcript_segments(
            &transcripts,
            &turns,
            DiarizationMappingOptions::default(),
        );

        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 1"));
        assert_eq!(mapped[1].speaker.as_deref(), Some("Speaker 2"));
    }

    #[test]
    fn leaves_speaker_empty_when_there_is_no_overlap() {
        let transcripts = vec![transcript("a", Some(5.0), Some(6.0))];
        let turns = vec![turn(0.0, 2.0, 0), turn(2.0, 4.0, 1)];

        let mapped = map_diarization_to_transcript_segments(
            &transcripts,
            &turns,
            DiarizationMappingOptions::default(),
        );

        assert_eq!(mapped[0].speaker, None);
    }

    #[test]
    fn overlap_tie_chooses_lowest_speaker_index() {
        let transcripts = vec![transcript("a", Some(0.0), Some(2.0))];
        let turns = vec![turn(0.0, 1.0, 1), turn(1.0, 2.0, 0)];

        let mapped = map_diarization_to_transcript_segments(
            &transcripts,
            &turns,
            DiarizationMappingOptions::default(),
        );

        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 1"));
    }

    #[test]
    fn can_map_by_midpoint_instead_of_overlap() {
        let transcripts = vec![transcript("a", Some(0.0), Some(2.0))];
        let turns = vec![turn(0.0, 1.0, 0), turn(1.0, 3.0, 1)];

        let mapped = map_diarization_to_transcript_segments(
            &transcripts,
            &turns,
            DiarizationMappingOptions {
                mode: DiarizationMappingMode::Midpoint,
                ..Default::default()
            },
        );

        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 2"));
    }

    #[test]
    fn preserves_existing_manual_labels_by_default() {
        let mut segment = transcript("a", Some(0.0), Some(2.0));
        segment.speaker = Some("Alice".to_string());

        let mapped = map_diarization_to_transcript_segments(
            &[segment],
            &[turn(0.0, 2.0, 0)],
            DiarizationMappingOptions::default(),
        );

        assert_eq!(mapped[0].speaker.as_deref(), Some("Alice"));
    }

    #[test]
    fn can_overwrite_existing_labels() {
        let mut segment = transcript("a", Some(0.0), Some(2.0));
        segment.speaker = Some("Alice".to_string());

        let mapped = map_diarization_to_transcript_segments(
            &[segment],
            &[turn(0.0, 2.0, 1)],
            DiarizationMappingOptions {
                existing_speaker_policy: ExistingSpeakerPolicy::Overwrite,
                ..Default::default()
            },
        );

        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 2"));
    }

    #[test]
    fn missing_or_invalid_timestamps_do_not_assign_speakers() {
        let transcripts = vec![
            transcript("missing", None, Some(1.0)),
            transcript("invalid", Some(2.0), Some(2.0)),
        ];
        let turns = vec![turn(0.0, 3.0, 0)];

        let mapped = map_diarization_to_transcript_segments(
            &transcripts,
            &turns,
            DiarizationMappingOptions::default(),
        );

        assert_eq!(mapped[0].speaker, None);
        assert_eq!(mapped[1].speaker, None);
    }
}
