use crate::api::TranscriptSegment as ApiTranscriptSegment;
use crate::audio::constants::AUDIO_EXTENSIONS;
use crate::audio::decoder::decode_audio_file;
use crate::state::AppState;
use anyhow::{anyhow, Result};
use chrono::Utc;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sherpa_onnx::{
    FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
    OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    SpeakerEmbeddingExtractorConfig,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

const FLOAT_TIE_EPSILON: f64 = 1e-9;
const MIN_SPLIT_SPEAKER_OVERLAP_SECONDS: f64 = 0.25;
const SAME_SPEAKER_MERGE_GAP_SECONDS: f64 = 0.25;
const SPLIT_BOUNDARY_SEARCH_WORDS: usize = 8;
const DIARIZATION_SAMPLE_RATE: i32 = 16_000;
const SEGMENTATION_MODEL_DIR: &str = "sherpa-onnx-pyannote-segmentation-3-0";
const EMBEDDING_MODEL_DIR: &str =
    "sherpa-onnx-3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k";
const SEGMENTATION_MODEL_URL: &str =
    "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.int8.onnx";
const EMBEDDING_MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx";
const MODEL_DOWNLOAD_CONNECT_TIMEOUT_SECS: u64 = 30;
const MODEL_DOWNLOAD_TIMEOUT_SECS: u64 = 20 * 60;
const SHORT_CLIP_OVERCLUSTER_RETRY_MAX_MINUTES: f64 = 10.0;
const SHORT_CLIP_OVERCLUSTERED_SPEAKERS: usize = 4;
const SHORT_CLIP_RETRY_NUM_SPEAKERS: i32 = 2;
const DIRECTML_PROBE_SECONDS: usize = 5;
const DIRECTML_REQUIRED_SPEEDUP: f64 = 1.10;
const SHERPA_RUNTIME_DLLS: &[&str] = &[
    "onnxruntime.dll",
    "onnxruntime_providers_shared.dll",
    "DirectML.dll",
    "sherpa-onnx-c-api.dll",
    "sherpa-onnx-cxx-api.dll",
];

static DIARIZATION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static DIARIZATION_DIRECTML_UNAVAILABLE: AtomicBool = AtomicBool::new(false);
static DIARIZATION_DIRECTML_SLOW: AtomicBool = AtomicBool::new(false);

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
    pub timestamp: Option<String>,
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
    pub duration: Option<f64>,
    pub speaker: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TranscriptSpeakerSpan {
    start_time: f64,
    end_time: f64,
    speaker: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WordToken {
    start_byte: usize,
    end_byte: usize,
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
        .flat_map(|segment| {
            if should_preserve_existing_speaker(segment, options.existing_speaker_policy) {
                return vec![segment.clone()];
            }

            split_or_label_transcript_segment(segment, diarization_turns, options.mode)
        })
        .collect()
}

fn split_or_label_transcript_segment(
    segment: &TranscriptSegment,
    diarization_turns: &[DiarizationTurn],
    mode: DiarizationMappingMode,
) -> Vec<TranscriptSegment> {
    let spans = speaker_spans_for_segment(segment, diarization_turns);
    if spans.len() > 1 {
        let split_segments = split_transcript_segment_by_speaker_spans(segment, &spans);
        if split_segments.len() > 1 {
            return split_segments;
        }
    }

    let mut mapped = segment.clone();
    mapped.speaker = assign_speaker(segment, diarization_turns, mode)
        .map(speaker_label)
        .or_else(|| spans.first().map(|span| speaker_label(span.speaker)))
        .or_else(|| segment.speaker.clone());
    vec![mapped]
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

fn speaker_spans_for_segment(
    segment: &TranscriptSegment,
    diarization_turns: &[DiarizationTurn],
) -> Vec<TranscriptSpeakerSpan> {
    let Some(start) = segment.audio_start_time else {
        return Vec::new();
    };
    let Some(end) = segment.audio_end_time else {
        return Vec::new();
    };
    if !is_valid_interval(start, end) {
        return Vec::new();
    }

    let mut spans: Vec<TranscriptSpeakerSpan> = diarization_turns
        .iter()
        .filter(|turn| is_valid_interval(turn.start_time, turn.end_time))
        .filter_map(|turn| {
            let overlap = overlap_seconds(start, end, turn.start_time, turn.end_time);
            if overlap < MIN_SPLIT_SPEAKER_OVERLAP_SECONDS {
                return None;
            }

            Some(TranscriptSpeakerSpan {
                start_time: turn.start_time.max(start),
                end_time: turn.end_time.min(end),
                speaker: turn.speaker,
            })
        })
        .collect();

    spans.sort_by(|a, b| {
        a.start_time
            .partial_cmp(&b.start_time)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.speaker.cmp(&b.speaker))
    });

    collapse_adjacent_same_speaker_spans(spans)
}

fn collapse_adjacent_same_speaker_spans(
    spans: Vec<TranscriptSpeakerSpan>,
) -> Vec<TranscriptSpeakerSpan> {
    let mut collapsed: Vec<TranscriptSpeakerSpan> = Vec::new();
    for span in spans {
        let Some(last) = collapsed.last_mut() else {
            collapsed.push(span);
            continue;
        };

        if last.speaker == span.speaker
            && span.start_time <= last.end_time + SAME_SPEAKER_MERGE_GAP_SECONDS
        {
            last.end_time = last.end_time.max(span.end_time);
        } else {
            collapsed.push(span);
        }
    }

    collapsed
}

fn split_transcript_segment_by_speaker_spans(
    segment: &TranscriptSegment,
    spans: &[TranscriptSpeakerSpan],
) -> Vec<TranscriptSegment> {
    let Some(segment_start) = segment.audio_start_time else {
        return vec![segment.clone()];
    };
    let Some(segment_end) = segment.audio_end_time else {
        return vec![segment.clone()];
    };
    if spans.len() <= 1 || !is_valid_interval(segment_start, segment_end) {
        return vec![segment.clone()];
    }

    let boundaries: Vec<f64> = spans
        .iter()
        .take(spans.len().saturating_sub(1))
        .map(|span| span.end_time.clamp(segment_start, segment_end))
        .collect();
    let text_parts =
        split_text_by_time_boundaries(&segment.text, segment_start, segment_end, &boundaries);
    if text_parts.len() != spans.len() || text_parts.iter().any(|part| part.trim().is_empty()) {
        return vec![segment.clone()];
    }

    spans
        .iter()
        .enumerate()
        .map(|(index, span)| {
            let audio_start_time = if index == 0 {
                segment_start
            } else {
                span.start_time.max(segment_start)
            };
            let audio_end_time = if index + 1 == spans.len() {
                segment_end
            } else {
                span.end_time.min(segment_end)
            };

            TranscriptSegment {
                id: if index == 0 {
                    segment.id.clone()
                } else {
                    format!("transcript-{}", Uuid::new_v4())
                },
                text: text_parts[index].clone(),
                timestamp: segment.timestamp.clone(),
                audio_start_time: Some(audio_start_time),
                audio_end_time: Some(audio_end_time),
                duration: Some((audio_end_time - audio_start_time).max(0.0)),
                speaker: Some(speaker_label(span.speaker)),
            }
        })
        .collect()
}

fn split_text_by_time_boundaries(
    text: &str,
    segment_start: f64,
    segment_end: f64,
    boundaries: &[f64],
) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || boundaries.is_empty() || !is_valid_interval(segment_start, segment_end)
    {
        return vec![trimmed.to_string()];
    }

    let tokens = word_tokens(trimmed);
    let piece_count = boundaries.len() + 1;
    if tokens.len() < piece_count {
        return vec![trimmed.to_string()];
    }

    let mut split_indices = Vec::<usize>::with_capacity(boundaries.len());
    let mut previous_index = 0usize;
    for (boundary_index, boundary_time) in boundaries.iter().enumerate() {
        let remaining_boundaries = boundaries.len() - boundary_index - 1;
        let lower = previous_index + 1;
        let upper = tokens.len().saturating_sub(remaining_boundaries + 1);
        if lower > upper {
            return vec![trimmed.to_string()];
        }

        let ratio =
            ((*boundary_time - segment_start) / (segment_end - segment_start)).clamp(0.0, 1.0);
        let target = ((ratio * tokens.len() as f64).round() as usize).clamp(lower, upper);
        let Some(split_index) = best_text_split_index(trimmed, &tokens, target, lower, upper)
        else {
            return vec![trimmed.to_string()];
        };
        split_indices.push(split_index);
        previous_index = split_index;
    }

    let mut parts = Vec::with_capacity(piece_count);
    let mut start_token_index = 0usize;
    for end_token_index in split_indices
        .into_iter()
        .chain(std::iter::once(tokens.len()))
    {
        let start_byte = tokens[start_token_index].start_byte;
        let end_byte = tokens[end_token_index - 1].end_byte;
        parts.push(trimmed[start_byte..end_byte].trim().to_string());
        start_token_index = end_token_index;
    }

    parts
}

fn word_tokens(text: &str) -> Vec<WordToken> {
    let mut tokens = Vec::new();
    let mut token_start = None;

    for (index, character) in text.char_indices() {
        if character.is_whitespace() {
            if let Some(start_byte) = token_start.take() {
                tokens.push(WordToken {
                    start_byte,
                    end_byte: index,
                });
            }
        } else if token_start.is_none() {
            token_start = Some(index);
        }
    }

    if let Some(start_byte) = token_start {
        tokens.push(WordToken {
            start_byte,
            end_byte: text.len(),
        });
    }

    tokens
}

fn best_text_split_index(
    text: &str,
    tokens: &[WordToken],
    target: usize,
    lower: usize,
    upper: usize,
) -> Option<usize> {
    let search_lower = lower.max(target.saturating_sub(SPLIT_BOUNDARY_SEARCH_WORDS));
    let search_upper = upper.min(target.saturating_add(SPLIT_BOUNDARY_SEARCH_WORDS));

    // We do not have word-level timestamps, only a diarization timestamp and a
    // full transcript row. Splitting at arbitrary proportional word offsets
    // creates mid-sentence fragments, so only accept nearby sentence endings.
    (search_lower..=search_upper)
        .filter(|candidate| is_sentence_split_boundary(text, tokens, *candidate))
        .min_by_key(|candidate| candidate.abs_diff(target))
}

fn is_sentence_split_boundary(text: &str, tokens: &[WordToken], boundary: usize) -> bool {
    let previous_word = &text[tokens[boundary - 1].start_byte..tokens[boundary - 1].end_byte];
    word_ends_sentence(previous_word)
}

fn word_ends_sentence(word: &str) -> bool {
    word.trim_end_matches(|character: char| matches!(character, '"' | '\'' | ')' | ']' | '}'))
        .ends_with(['.', '?', '!'])
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
            provider: preferred_diarization_provider().to_string(),
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

        let turns = result
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
            .collect();

        Ok(compact_diarization_speakers(turns))
    }
}

fn compact_diarization_speakers(turns: Vec<DiarizationTurn>) -> Vec<DiarizationTurn> {
    let mut remapped_speakers = BTreeMap::<usize, usize>::new();

    turns
        .into_iter()
        .map(|mut turn| {
            let compacted_speaker = match remapped_speakers.get(&turn.speaker).copied() {
                Some(compacted_speaker) => compacted_speaker,
                None => {
                    let compacted_speaker = remapped_speakers.len();
                    remapped_speakers.insert(turn.speaker, compacted_speaker);
                    compacted_speaker
                }
            };
            turn.speaker = compacted_speaker;
            turn
        })
        .collect()
}

fn audio_duration_minutes(sample_count: usize) -> f64 {
    sample_count as f64 / f64::from(DIARIZATION_SAMPLE_RATE) / 60.0
}

fn speaker_count_from_turns(turns: &[DiarizationTurn]) -> usize {
    turns
        .iter()
        .map(|turn| turn.speaker)
        .collect::<BTreeSet<_>>()
        .len()
}

fn should_retry_short_clip_with_two_speakers(
    sample_count: usize,
    turns: &[DiarizationTurn],
    explicit_num_speakers: Option<i32>,
) -> bool {
    explicit_num_speakers.is_none()
        && audio_duration_minutes(sample_count) <= SHORT_CLIP_OVERCLUSTER_RETRY_MAX_MINUTES
        && speaker_count_from_turns(turns) > SHORT_CLIP_OVERCLUSTERED_SPEAKERS
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

#[derive(Debug, Clone, Serialize)]
struct DiarizationRuntimeDll {
    name: String,
    present: bool,
    bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct DiarizationProfileEvent {
    stage: String,
    provider: String,
    sample_count: usize,
    elapsed_ms: Option<u64>,
    turns: Option<usize>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DiarizationProfile {
    meeting_id: String,
    audio_seconds: f64,
    sample_count: usize,
    num_threads: i32,
    explicit_num_speakers: Option<i32>,
    directml_feature: bool,
    preferred_provider_before_probe: String,
    selected_provider: Option<String>,
    decision: Option<String>,
    runtime_dlls: Vec<DiarizationRuntimeDll>,
    events: Vec<DiarizationProfileEvent>,
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

    let sample_count = samples.len();
    let audio_minutes = audio_duration_minutes(sample_count);
    let samples = Arc::new(samples);
    let explicit_num_speakers = num_speakers.filter(|value| *value > 0);
    let mut profile = DiarizationProfile::new(&meeting_id, sample_count, explicit_num_speakers);
    let provider = select_diarization_provider(
        &app,
        &meeting_id,
        &model_paths,
        explicit_num_speakers,
        Arc::clone(&samples),
        &mut profile,
    )
    .await?;
    let diarization_message = if audio_minutes >= 1.0 {
        format!(
            "Detecting speaker turns in {:.1} min of audio{}...",
            audio_minutes,
            diarization_provider_message_suffix(provider)
        )
    } else {
        format!(
            "Detecting speaker turns{}...",
            diarization_provider_message_suffix(provider)
        )
    };
    emit_progress(&app, &meeting_id, "diarizing", 45, &diarization_message);

    let config = diarization_config_for_provider(&model_paths, explicit_num_speakers, provider);

    let mut turns = run_sherpa_diarization_with_fallback(
        &app,
        &meeting_id,
        config,
        Arc::clone(&samples),
        &mut profile,
    )
    .await?;
    if should_retry_short_clip_with_two_speakers(sample_count, &turns, explicit_num_speakers) {
        emit_progress(
            &app,
            &meeting_id,
            "diarizing",
            65,
            "Auto speaker detection split this short clip into too many speakers; retrying with 2 speakers...",
        );

        let mut retry_config =
            diarization_config_for_provider(&model_paths, explicit_num_speakers, provider);
        retry_config.num_clusters = Some(SHORT_CLIP_RETRY_NUM_SPEAKERS);
        turns = run_sherpa_diarization_with_fallback(
            &app,
            &meeting_id,
            retry_config,
            Arc::clone(&samples),
            &mut profile,
        )
        .await?;
    }

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
            timestamp: Some(segment.timestamp.clone()),
            audio_start_time: segment.audio_start_time,
            audio_end_time: segment.audio_end_time,
            duration: segment.duration,
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

    let updated_segments = count_changed_transcript_segments(&stored_segments, &mapped_segments);
    if updated_segments > 0 {
        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
            .bind(&meeting_id)
            .execute(&mut *tx)
            .await?;

        for mapped in &mapped_segments {
            sqlx::query(
                "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&mapped.id)
            .bind(&meeting_id)
            .bind(&mapped.text)
            .bind(
                mapped
                    .timestamp
                    .as_deref()
                    .unwrap_or_else(|| stored_segments[0].timestamp.as_str()),
            )
            .bind(mapped.audio_start_time)
            .bind(mapped.audio_end_time)
            .bind(mapped.duration)
            .bind(&mapped.speaker)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
            .bind(Utc::now())
            .bind(&meeting_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }

    let fallback_timestamp = stored_segments[0].timestamp.as_str();
    let transcript_file_segments: Vec<ApiTranscriptSegment> = mapped_segments
        .iter()
        .map(|mapped| ApiTranscriptSegment {
            id: mapped.id.clone(),
            text: mapped.text.clone(),
            timestamp: mapped
                .timestamp
                .clone()
                .unwrap_or_else(|| fallback_timestamp.to_string()),
            audio_start_time: mapped.audio_start_time,
            audio_end_time: mapped.audio_end_time,
            duration: mapped.duration,
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
    write_diarization_profile(&app, &profile);

    Ok(SpeakerDiarizationComplete {
        meeting_id,
        speaker_count,
        updated_segments,
    })
}

fn count_changed_transcript_segments(
    stored_segments: &[StoredTranscriptSegment],
    mapped_segments: &[TranscriptSegment],
) -> usize {
    if stored_segments.len() != mapped_segments.len() {
        return mapped_segments.len();
    }

    stored_segments
        .iter()
        .zip(mapped_segments.iter())
        .filter(|(stored, mapped)| {
            stored.id != mapped.id
                || stored.transcript != mapped.text
                || Some(stored.timestamp.as_str()) != mapped.timestamp.as_deref()
                || !optional_seconds_equal(stored.audio_start_time, mapped.audio_start_time)
                || !optional_seconds_equal(stored.audio_end_time, mapped.audio_end_time)
                || !optional_seconds_equal(stored.duration, mapped.duration)
                || stored.speaker != mapped.speaker
        })
        .count()
}

fn optional_seconds_equal(left: Option<f64>, right: Option<f64>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => (left - right).abs() <= FLOAT_TIE_EPSILON,
        (None, None) => true,
        _ => false,
    }
}

impl DiarizationProfile {
    fn new(meeting_id: &str, sample_count: usize, explicit_num_speakers: Option<i32>) -> Self {
        let preferred_provider = preferred_diarization_provider().to_string();
        let profile = Self {
            meeting_id: meeting_id.to_string(),
            audio_seconds: sample_count as f64 / DIARIZATION_SAMPLE_RATE as f64,
            sample_count,
            num_threads: default_diarization_threads(),
            explicit_num_speakers,
            directml_feature: cfg!(all(target_os = "windows", feature = "directml")),
            preferred_provider_before_probe: preferred_provider,
            selected_provider: None,
            decision: None,
            runtime_dlls: collect_sherpa_runtime_dlls(),
            events: Vec::new(),
        };
        log_profile_snapshot("start", &profile);
        profile
    }

    fn set_decision(&mut self, selected_provider: &str, decision: impl Into<String>) {
        self.selected_provider = Some(selected_provider.to_string());
        self.decision = Some(decision.into());
        log_profile_snapshot("decision", self);
    }

    fn record_success(
        &mut self,
        stage: &str,
        provider: &str,
        sample_count: usize,
        elapsed: Duration,
        turns: usize,
    ) {
        self.record(DiarizationProfileEvent {
            stage: stage.to_string(),
            provider: provider.to_string(),
            sample_count,
            elapsed_ms: Some(duration_millis(elapsed)),
            turns: Some(turns),
            error: None,
        });
    }

    fn record_error(
        &mut self,
        stage: &str,
        provider: &str,
        sample_count: usize,
        elapsed: Duration,
        error: &anyhow::Error,
    ) {
        self.record(DiarizationProfileEvent {
            stage: stage.to_string(),
            provider: provider.to_string(),
            sample_count,
            elapsed_ms: Some(duration_millis(elapsed)),
            turns: None,
            error: Some(error.to_string()),
        });
    }

    fn record(&mut self, event: DiarizationProfileEvent) {
        log_profile_event(&event);
        self.events.push(event);
    }
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn log_profile_snapshot(stage: &str, profile: &DiarizationProfile) {
    match serde_json::to_string(profile) {
        Ok(json) => log::info!("speaker_diarization_profile_snapshot stage={stage} {json}"),
        Err(error) => log::warn!("Failed to serialize speaker diarization profile: {error}"),
    }
}

fn log_profile_event(event: &DiarizationProfileEvent) {
    match serde_json::to_string(event) {
        Ok(json) => log::info!("speaker_diarization_profile_event {json}"),
        Err(error) => log::warn!("Failed to serialize speaker diarization profile event: {error}"),
    }
}

fn collect_sherpa_runtime_dlls() -> Vec<DiarizationRuntimeDll> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));

    SHERPA_RUNTIME_DLLS
        .iter()
        .map(|name| {
            let metadata = exe_dir
                .as_ref()
                .map(|dir| dir.join(name))
                .and_then(|path| fs::metadata(path).ok());
            DiarizationRuntimeDll {
                name: (*name).to_string(),
                present: metadata.is_some(),
                bytes: metadata.map(|metadata| metadata.len()),
            }
        })
        .collect()
}

fn write_diarization_profile<R: Runtime>(app: &AppHandle<R>, profile: &DiarizationProfile) {
    let result = (|| -> Result<PathBuf> {
        let profile_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| anyhow!("Failed to resolve app data directory: {error}"))?
            .join("logs")
            .join("diarization");
        fs::create_dir_all(&profile_dir)?;
        let safe_meeting_id = sanitize_profile_file_stem(&profile.meeting_id);
        let file_name = format!(
            "{}-{}.json",
            safe_meeting_id,
            Utc::now().format("%Y%m%dT%H%M%SZ")
        );
        let path = profile_dir.join(file_name);
        fs::write(&path, serde_json::to_vec_pretty(profile)?)?;
        Ok(path)
    })();

    match result {
        Ok(path) => log::info!(
            "speaker_diarization_profile_written path={}",
            path.display()
        ),
        Err(error) => log::warn!("Failed to write speaker diarization profile: {error}"),
    }
}

fn sanitize_profile_file_stem(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();

    let sanitized = sanitized.trim_matches('-');
    if sanitized.is_empty() {
        "meeting".to_string()
    } else {
        sanitized.chars().take(96).collect()
    }
}

async fn select_diarization_provider<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    model_paths: &DiarizationModelPaths,
    explicit_num_speakers: Option<i32>,
    samples: Arc<Vec<f32>>,
    profile: &mut DiarizationProfile,
) -> Result<&'static str> {
    let provider = preferred_diarization_provider();
    if provider != "directml" {
        profile.set_decision(
            provider,
            "DirectML is not preferred for this build or app session",
        );
        return Ok(provider);
    }

    emit_progress(
        app,
        meeting_id,
        "checking_directml",
        40,
        "Checking whether DirectML accelerates speaker detection...",
    );

    let probe_samples = Arc::new(diarization_probe_samples(samples.as_ref().as_slice()));
    let directml_config =
        diarization_config_for_provider(model_paths, explicit_num_speakers, "directml");
    let directml_probe = time_sherpa_diarization(directml_config, Arc::clone(&probe_samples)).await;
    let directml_sample_count = probe_samples.len();
    let directml_result = match directml_probe {
        Ok(result) => {
            profile.record_success(
                "probe",
                "directml",
                directml_sample_count,
                result.elapsed,
                result.turns.len(),
            );
            result
        }
        Err(directml_error) => {
            profile.record_error(
                "probe",
                "directml",
                directml_sample_count,
                directml_error.elapsed,
                &directml_error.error,
            );
            DIARIZATION_DIRECTML_UNAVAILABLE.store(true, Ordering::SeqCst);
            log::warn!(
                "DirectML speaker diarization probe failed; falling back to CPU: {}",
                directml_error.error
            );
            emit_progress(
                app,
                meeting_id,
                "checking_directml",
                42,
                "DirectML speaker detection is unavailable; using CPU...",
            );
            profile.set_decision("cpu", "DirectML probe failed");
            return Ok("cpu");
        }
    };

    let cpu_config = diarization_config_for_provider(model_paths, explicit_num_speakers, "cpu");
    let cpu_probe = time_sherpa_diarization(cpu_config, probe_samples).await;
    let cpu_result = match cpu_probe {
        Ok(result) => {
            profile.record_success(
                "probe",
                "cpu",
                directml_sample_count,
                result.elapsed,
                result.turns.len(),
            );
            result
        }
        Err(cpu_error) => {
            profile.record_error(
                "probe",
                "cpu",
                directml_sample_count,
                cpu_error.elapsed,
                &cpu_error.error,
            );
            log::warn!(
                "CPU speaker diarization probe failed; using DirectML probe result instead: {}",
                cpu_error.error
            );
            profile.set_decision(
                "directml",
                "CPU probe failed after DirectML probe succeeded",
            );
            return Ok("directml");
        }
    };

    if directml_is_fast_enough(cpu_result.elapsed, directml_result.elapsed) {
        log::info!(
            "DirectML speaker diarization selected: directml={:?} ({} turns), cpu={:?} ({} turns)",
            directml_result.elapsed,
            directml_result.turns.len(),
            cpu_result.elapsed,
            cpu_result.turns.len()
        );
        emit_progress(
            app,
            meeting_id,
            "checking_directml",
            42,
            "DirectML speaker detection is faster on this machine; using DirectML...",
        );
        profile.set_decision("directml", "DirectML probe was faster than CPU");
        return Ok("directml");
    }

    DIARIZATION_DIRECTML_SLOW.store(true, Ordering::SeqCst);
    log::warn!(
        "DirectML speaker diarization rejected as slower: directml={:?} ({} turns), cpu={:?} ({} turns)",
        directml_result.elapsed,
        directml_result.turns.len(),
        cpu_result.elapsed,
        cpu_result.turns.len()
    );
    emit_progress(
        app,
        meeting_id,
        "checking_directml",
        42,
        "DirectML is slower for this diarization model on this machine; using CPU...",
    );
    profile.set_decision("cpu", "DirectML probe was not faster than CPU");
    Ok("cpu")
}

fn diarization_config_for_provider(
    model_paths: &DiarizationModelPaths,
    explicit_num_speakers: Option<i32>,
    provider: &str,
) -> SherpaDiarizationConfig {
    let mut config = SherpaDiarizationConfig::new(
        model_paths.segmentation_model.clone(),
        model_paths.embedding_model.clone(),
    );
    config.num_threads = default_diarization_threads();
    config.num_clusters = explicit_num_speakers;
    config.provider = provider.to_string();
    config.debug = sherpa_diarization_debug_enabled(provider);
    config
}

fn sherpa_diarization_debug_enabled(provider: &str) -> bool {
    match std::env::var("CLAWSCRIBE_SHERPA_DIARIZATION_DEBUG") {
        Ok(value) => matches_truthy(&value),
        Err(_) => provider == "directml",
    }
}

fn matches_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn diarization_probe_samples(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let probe_sample_count = (DIARIZATION_SAMPLE_RATE as usize * DIRECTML_PROBE_SECONDS)
        .min(samples.len())
        .max(1);
    samples[..probe_sample_count].to_vec()
}

struct TimedDiarizationRun {
    elapsed: Duration,
    turns: Vec<DiarizationTurn>,
}

struct TimedDiarizationError {
    elapsed: Duration,
    error: anyhow::Error,
}

async fn time_sherpa_diarization(
    config: SherpaDiarizationConfig,
    samples: Arc<Vec<f32>>,
) -> std::result::Result<TimedDiarizationRun, TimedDiarizationError> {
    let started = Instant::now();
    match run_sherpa_diarization(config, samples).await {
        Ok(turns) => Ok(TimedDiarizationRun {
            elapsed: started.elapsed(),
            turns,
        }),
        Err(error) => Err(TimedDiarizationError {
            elapsed: started.elapsed(),
            error,
        }),
    }
}

fn directml_is_fast_enough(cpu_elapsed: Duration, directml_elapsed: Duration) -> bool {
    let cpu = cpu_elapsed.as_secs_f64();
    let directml = directml_elapsed.as_secs_f64();
    directml > 0.0 && cpu / directml >= DIRECTML_REQUIRED_SPEEDUP
}

async fn run_sherpa_diarization(
    config: SherpaDiarizationConfig,
    samples: Arc<Vec<f32>>,
) -> Result<Vec<DiarizationTurn>> {
    tokio::task::spawn_blocking(move || {
        let diarizer = SherpaOfflineDiarizer::new(config)?;
        let sample_rate = diarizer.sample_rate();
        if sample_rate != DIARIZATION_SAMPLE_RATE {
            return Err(anyhow!(
                "Diarization model expects {} Hz audio, but prepared audio is {} Hz",
                sample_rate,
                DIARIZATION_SAMPLE_RATE
            ));
        }
        diarizer.diarize(samples.as_ref().as_slice())
    })
    .await
    .map_err(|e| anyhow!("Speaker diarization task failed: {}", e))?
}

async fn run_sherpa_diarization_with_fallback<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    config: SherpaDiarizationConfig,
    samples: Arc<Vec<f32>>,
    profile: &mut DiarizationProfile,
) -> Result<Vec<DiarizationTurn>> {
    let provider = normalized_diarization_provider(&config.provider).to_string();
    if provider != "directml" {
        let sample_count = samples.len();
        return match time_sherpa_diarization(config, samples).await {
            Ok(result) => {
                profile.record_success(
                    "full",
                    &provider,
                    sample_count,
                    result.elapsed,
                    result.turns.len(),
                );
                Ok(result.turns)
            }
            Err(error) => {
                profile.record_error("full", &provider, sample_count, error.elapsed, &error.error);
                Err(error.error)
            }
        };
    }

    let directml_sample_count = samples.len();
    match time_sherpa_diarization(config.clone(), Arc::clone(&samples)).await {
        Ok(result) => {
            profile.record_success(
                "full",
                "directml",
                directml_sample_count,
                result.elapsed,
                result.turns.len(),
            );
            Ok(result.turns)
        }
        Err(directml_error) => {
            profile.record_error(
                "full",
                "directml",
                directml_sample_count,
                directml_error.elapsed,
                &directml_error.error,
            );
            DIARIZATION_DIRECTML_UNAVAILABLE.store(true, Ordering::SeqCst);
            log::warn!(
                "DirectML speaker diarization failed; falling back to CPU: {}",
                directml_error.error
            );
            emit_progress(
                app,
                meeting_id,
                "diarizing",
                55,
                "DirectML speaker diarization is unavailable; falling back to CPU...",
            );

            let mut cpu_config = config;
            cpu_config.provider = "cpu".to_string();
            match time_sherpa_diarization(cpu_config, samples).await {
                Ok(result) => {
                    profile.record_success(
                        "fallback",
                        "cpu",
                        directml_sample_count,
                        result.elapsed,
                        result.turns.len(),
                    );
                    Ok(result.turns)
                }
                Err(cpu_error) => {
                    profile.record_error(
                        "fallback",
                        "cpu",
                        directml_sample_count,
                        cpu_error.elapsed,
                        &cpu_error.error,
                    );
                    Err(anyhow!(
                        "DirectML speaker diarization failed ({}); CPU fallback also failed ({})",
                        directml_error.error,
                        cpu_error.error
                    ))
                }
            }
        }
    }
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

fn preferred_diarization_provider() -> &'static str {
    if cfg!(all(target_os = "windows", feature = "directml"))
        && !DIARIZATION_DIRECTML_UNAVAILABLE.load(Ordering::SeqCst)
        && !DIARIZATION_DIRECTML_SLOW.load(Ordering::SeqCst)
    {
        "directml"
    } else {
        "cpu"
    }
}

fn normalized_diarization_provider(provider: &str) -> &str {
    let provider = provider.trim();
    if provider.is_empty() {
        "cpu"
    } else {
        provider
    }
}

fn diarization_provider_message_suffix(provider: &str) -> &'static str {
    if provider == "directml" {
        " with DirectML"
    } else {
        ""
    }
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

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(MODEL_DOWNLOAD_CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_secs(MODEL_DOWNLOAD_TIMEOUT_SECS))
        .build()
        .map_err(|e| anyhow!("Failed to configure diarization model downloader: {}", e))?;

    let response = client.get(download_url).send().await.map_err(|e| {
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

    let temp_path = model_path.with_extension("download");
    let mut file = tokio::fs::File::create(&temp_path).await?;
    let total_bytes = response.content_length();
    let mut downloaded_bytes = 0u64;
    let mut last_reported_percent = 0u32;
    let mut last_reported_mebibytes = 0u64;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            anyhow!(
                "Failed to read {} diarization model download from {}: {}",
                model_name,
                download_url,
                e
            )
        })?;
        if chunk.is_empty() {
            continue;
        }

        file.write_all(&chunk).await?;
        downloaded_bytes += chunk.len() as u64;

        if let Some(total) = total_bytes.filter(|value| *value > 0) {
            let percent = ((downloaded_bytes.saturating_mul(100)) / total).min(100) as u32;
            if percent == 100 || percent >= last_reported_percent.saturating_add(10) {
                last_reported_percent = percent;
                emit_progress(
                    app,
                    meeting_id,
                    "downloading_models",
                    8 + ((percent.min(100) * 6) / 100),
                    &format!("Downloading {model_name} diarization model ({percent}%)..."),
                );
            }
        } else {
            let mebibytes = downloaded_bytes / (1024 * 1024);
            if mebibytes >= last_reported_mebibytes.saturating_add(10) {
                last_reported_mebibytes = mebibytes;
                emit_progress(
                    app,
                    meeting_id,
                    "downloading_models",
                    8,
                    &format!("Downloading {model_name} diarization model ({mebibytes} MiB)..."),
                );
            }
        }
    }

    file.flush().await?;

    if downloaded_bytes == 0 {
        return Err(anyhow!(
            "{} diarization model download was empty: {}",
            model_name,
            download_url
        ));
    }

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
            text: format!("segment-{id}"),
            timestamp: Some("2026-06-22T12:00:00Z".to_string()),
            audio_start_time: start,
            audio_end_time: end,
            duration: start.zip(end).map(|(start, end)| end - start),
            speaker: None,
        }
    }

    fn transcript_with_text(
        id: &str,
        text: &str,
        start: Option<f64>,
        end: Option<f64>,
    ) -> TranscriptSegment {
        let mut segment = transcript(id, start, end);
        segment.text = text.to_string();
        segment
    }

    fn turn(start: f64, end: f64, speaker: usize) -> DiarizationTurn {
        DiarizationTurn {
            start_time: start,
            end_time: end,
            speaker,
        }
    }

    fn sample_count_for_minutes(minutes: usize) -> usize {
        DIARIZATION_SAMPLE_RATE as usize * 60 * minutes
    }

    #[test]
    fn compacts_sparse_sherpa_speaker_ids_by_first_turn() {
        let turns = vec![
            turn(0.0, 1.0, 59),
            turn(1.0, 2.0, 3),
            turn(2.0, 3.0, 59),
            turn(3.0, 4.0, 13),
        ];

        let compacted = compact_diarization_speakers(turns);
        let speakers: Vec<usize> = compacted.iter().map(|turn| turn.speaker).collect();

        assert_eq!(speakers, vec![0, 1, 0, 2]);
    }

    #[test]
    fn retries_short_auto_runs_that_overcluster_speakers() {
        let turns = vec![
            turn(0.0, 1.0, 0),
            turn(1.0, 2.0, 1),
            turn(2.0, 3.0, 2),
            turn(3.0, 4.0, 3),
            turn(4.0, 5.0, 4),
        ];

        assert!(should_retry_short_clip_with_two_speakers(
            sample_count_for_minutes(8),
            &turns,
            None,
        ));
        assert!(!should_retry_short_clip_with_two_speakers(
            sample_count_for_minutes(8),
            &turns,
            Some(2),
        ));
        assert!(!should_retry_short_clip_with_two_speakers(
            sample_count_for_minutes(20),
            &turns,
            None,
        ));
        assert!(!should_retry_short_clip_with_two_speakers(
            sample_count_for_minutes(8),
            &turns[..SHORT_CLIP_OVERCLUSTERED_SPEAKERS],
            None,
        ));
    }

    #[test]
    fn directml_probe_uses_short_audio_slice() {
        let samples = vec![0.0; DIARIZATION_SAMPLE_RATE as usize * (DIRECTML_PROBE_SECONDS + 3)];
        let probe = diarization_probe_samples(&samples);

        assert_eq!(
            probe.len(),
            DIARIZATION_SAMPLE_RATE as usize * DIRECTML_PROBE_SECONDS
        );
        assert!(diarization_probe_samples(&[]).is_empty());
    }

    #[test]
    fn directml_probe_requires_measurable_speedup() {
        assert!(directml_is_fast_enough(
            Duration::from_millis(2200),
            Duration::from_millis(1000),
        ));
        assert!(!directml_is_fast_enough(
            Duration::from_millis(1000),
            Duration::from_millis(950),
        ));
        assert!(!directml_is_fast_enough(
            Duration::from_millis(1000),
            Duration::from_millis(2000),
        ));
    }

    #[test]
    fn profile_file_stem_is_filesystem_safe() {
        assert_eq!(
            sanitize_profile_file_stem("meeting:abc/def?ghi"),
            "meeting-abc-def-ghi"
        );
        assert_eq!(sanitize_profile_file_stem("???"), "meeting");
        assert!(sanitize_profile_file_stem(&"a".repeat(150)).len() <= 96);
    }

    #[test]
    fn truthy_debug_env_values_are_explicit() {
        assert!(matches_truthy("1"));
        assert!(matches_truthy(" true "));
        assert!(matches_truthy("YES"));
        assert!(matches_truthy("on"));
        assert!(!matches_truthy("0"));
        assert!(!matches_truthy("false"));
        assert!(!matches_truthy("directml"));
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
    fn splits_segment_when_speaker_changes_inside_transcript_row() {
        let transcripts = vec![transcript_with_text(
            "a",
            "Rogan opens the topic. Guest adds the drone contract point. Rogan asks who made money?",
            Some(0.0),
            Some(12.0),
        )];
        let turns = vec![turn(0.0, 5.0, 0), turn(5.0, 9.0, 1), turn(9.0, 12.0, 0)];

        let mapped = map_diarization_to_transcript_segments(
            &transcripts,
            &turns,
            DiarizationMappingOptions {
                existing_speaker_policy: ExistingSpeakerPolicy::Overwrite,
                ..Default::default()
            },
        );

        assert_eq!(mapped.len(), 3);
        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 1"));
        assert_eq!(mapped[0].text, "Rogan opens the topic.");
        assert_eq!(mapped[0].audio_start_time, Some(0.0));
        assert_eq!(mapped[0].audio_end_time, Some(5.0));
        assert_eq!(mapped[1].speaker.as_deref(), Some("Speaker 2"));
        assert_eq!(mapped[1].text, "Guest adds the drone contract point.");
        assert_eq!(mapped[1].audio_start_time, Some(5.0));
        assert_eq!(mapped[1].audio_end_time, Some(9.0));
        assert_eq!(mapped[2].speaker.as_deref(), Some("Speaker 1"));
        assert_eq!(mapped[2].text, "Rogan asks who made money?");
        assert_eq!(mapped[2].audio_start_time, Some(9.0));
        assert_eq!(mapped[2].audio_end_time, Some(12.0));
    }

    #[test]
    fn does_not_split_transcript_row_inside_a_sentence() {
        let transcripts = vec![transcript_with_text(
            "a",
            "Just like the Patriot Act until you listen to people who explain how nonprofits become a fucking scam.",
            Some(0.0),
            Some(16.0),
        )];
        let turns = vec![turn(0.0, 8.0, 0), turn(8.0, 12.0, 1), turn(12.0, 16.0, 0)];

        let mapped = map_diarization_to_transcript_segments(
            &transcripts,
            &turns,
            DiarizationMappingOptions {
                existing_speaker_policy: ExistingSpeakerPolicy::Overwrite,
                ..Default::default()
            },
        );

        assert_eq!(mapped.len(), 1);
        assert_eq!(
            mapped[0].text,
            "Just like the Patriot Act until you listen to people who explain how nonprofits become a fucking scam."
        );
        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 1"));
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
