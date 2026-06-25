use crate::api::{
    TranscriptSegment as ApiTranscriptSegment, TranscriptWord, TranscriptWordTimestampSource,
};
use crate::audio::decoder::decode_audio_file;
use crate::state::AppState;
use crate::summary::language_detection::detect_summary_language;
use crate::summary::metadata::{
    read_detected_summary_language_from_metadata, read_transcription_source_language_from_metadata,
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sherpa_onnx::{
    FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
    OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    SpeakerEmbeddingExtractorConfig,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
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
const MIN_SPLIT_SPEAKER_OVERLAP_SECONDS: f64 = 0.2;
const MIN_SPLIT_PART_SECONDS: f64 = 1.25;
const SAME_SPEAKER_MERGE_GAP_SECONDS: f64 = 0.25;
const SAME_SPEAKER_TRANSCRIPT_MERGE_GAP_SECONDS: f64 = 1.0;
const SHORT_UTTERANCE_HINT_MAX_SECONDS: f64 = 0.75;
const SHORT_UTTERANCE_EXPANSION_MAX_SECONDS: f64 = 2.5;
const SHORT_UTTERANCE_EXPANSION_MAX_WORDS: usize = 8;
const SHORT_SPEAKER_ISLAND_MAX_SECONDS: f64 = 0.8;
const SHORT_SPEAKER_ISLAND_MIN_CONTEXT_SECONDS: f64 = 2.0;
const SPLIT_BOUNDARY_SEARCH_WORDS: usize = 8;
const LEGACY_ESTIMATED_WORD_TIMESTAMP_TOLERANCE_SECONDS: f64 = 0.05;
const MIN_EXPLICIT_RETRY_SPEAKER_SECONDS: f64 = 2.0;
const AUTO_MAX_CONFIDENT_SPEAKER_LANES: usize = 4;
const AUTO_SIGNIFICANT_SPEAKER_MIN_SECONDS: f64 = 6.0;
const AUTO_SIGNIFICANT_SPEAKER_DOMINANT_SHARE: f64 = 0.06;
const DEFAULT_MIN_DURATION_ON_SECONDS: f32 = 0.3;
const DEFAULT_MIN_DURATION_OFF_SECONDS: f32 = 0.5;
const FIXED_COUNT_MIN_DURATION_ON_SECONDS: f32 = 0.25;
const FIXED_COUNT_MIN_DURATION_OFF_SECONDS: f32 = 0.25;
const DIARIZATION_SAMPLE_RATE: i32 = 16_000;
const DEFAULT_CLUSTERING_THRESHOLD: f32 = 0.5;
const DEFAULT_SEGMENTATION_MODEL_ID: &str = "pyannote-segmentation-3-0-int8";
const DEFAULT_EMBEDDING_MODEL_ID: &str = "wespeaker-camplusplus-en";
const ZH_CN_EMBEDDING_MODEL_ID: &str = "3dspeaker-eres2net-base-zh-cn";
const SEGMENTATION_MODEL_DIR: &str = "sherpa-onnx-pyannote-segmentation-3-0";
const DEFAULT_EMBEDDING_MODEL_DIR: &str = "sherpa-onnx-wespeaker_en_voxceleb_CAMplusplus";
const ZH_CN_EMBEDDING_MODEL_DIR: &str =
    "sherpa-onnx-3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k";
const SEGMENTATION_MODEL_URL: &str =
    "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.int8.onnx";
const ZH_CN_EMBEDDING_MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx";
const MODEL_DOWNLOAD_CONNECT_TIMEOUT_SECS: u64 = 30;
const MODEL_DOWNLOAD_TIMEOUT_SECS: u64 = 20 * 60;
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
static DIARIZATION_DIRECTML_FAST: AtomicBool = AtomicBool::new(false);

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_timestamps: Option<Vec<TranscriptWord>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TranscriptSpeakerSpan {
    start_time: f64,
    end_time: f64,
    speaker: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordTimestampEligibility {
    None,
    Real,
    Estimated,
    LegacyPrecise,
    LegacyEstimated,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WordToken {
    start_byte: usize,
    end_byte: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiarizationMappingDiagnostics {
    transcript_rows: usize,
    rows_with_word_timestamps: usize,
    rows_with_real_word_timestamps: usize,
    rows_with_estimated_word_timestamps: usize,
    rows_with_legacy_precise_word_timestamps: usize,
    rows_with_legacy_estimated_word_timestamps: usize,
    rows_with_invalid_word_timestamps: usize,
    prepared_turns: usize,
    short_prepared_turns: usize,
    shortest_turn_seconds: Option<f64>,
    rows_with_multiple_speaker_spans: usize,
    rows_split_by_speaker_spans: usize,
    rows_blocked_by_short_part_gate: usize,
    rows_blocked_by_word_timestamp_alignment: usize,
    short_speaker_islands_smoothed: usize,
    short_speaker_spans: usize,
}

impl DiarizationMappingDiagnostics {
    fn from_inputs(
        transcript_segments: &[TranscriptSegment],
        diarization_turns: &[DiarizationTurn],
    ) -> Self {
        let shortest_turn_seconds = diarization_turns
            .iter()
            .filter_map(|turn| {
                is_valid_interval(turn.start_time, turn.end_time)
                    .then_some(turn.end_time - turn.start_time)
            })
            .min_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));

        let mut diagnostics = Self {
            transcript_rows: transcript_segments.len(),
            prepared_turns: diarization_turns.len(),
            short_prepared_turns: diarization_turns
                .iter()
                .filter(|turn| {
                    is_valid_interval(turn.start_time, turn.end_time)
                        && turn.end_time - turn.start_time < MIN_SPLIT_PART_SECONDS
                })
                .count(),
            shortest_turn_seconds,
            ..Default::default()
        };

        for segment in transcript_segments {
            diagnostics.record_word_timestamp_eligibility(word_timestamp_eligibility(segment));
        }

        diagnostics
    }

    fn record_word_timestamp_eligibility(&mut self, eligibility: WordTimestampEligibility) {
        match eligibility {
            WordTimestampEligibility::None => {}
            WordTimestampEligibility::Real => {
                self.rows_with_word_timestamps += 1;
                self.rows_with_real_word_timestamps += 1;
            }
            WordTimestampEligibility::Estimated => {
                self.rows_with_word_timestamps += 1;
                self.rows_with_estimated_word_timestamps += 1;
            }
            WordTimestampEligibility::LegacyPrecise => {
                self.rows_with_word_timestamps += 1;
                self.rows_with_legacy_precise_word_timestamps += 1;
            }
            WordTimestampEligibility::LegacyEstimated => {
                self.rows_with_word_timestamps += 1;
                self.rows_with_legacy_estimated_word_timestamps += 1;
            }
            WordTimestampEligibility::Invalid => {
                self.rows_with_word_timestamps += 1;
                self.rows_with_invalid_word_timestamps += 1;
            }
        }
    }

    fn record_split_candidate(
        &mut self,
        segment: &TranscriptSegment,
        spans: &[TranscriptSpeakerSpan],
        split_segment_count: usize,
    ) {
        self.rows_with_multiple_speaker_spans += 1;
        let short_span_count = spans
            .iter()
            .filter(|span| span.end_time - span.start_time < MIN_SPLIT_PART_SECONDS)
            .count();
        self.short_speaker_spans += short_span_count;

        if split_segment_count > 1 {
            self.rows_split_by_speaker_spans += 1;
            return;
        }

        match word_timestamp_eligibility(segment) {
            WordTimestampEligibility::Real | WordTimestampEligibility::LegacyPrecise => {
                self.rows_blocked_by_word_timestamp_alignment += 1;
            }
            _ if short_span_count > 0 => {
                self.rows_blocked_by_short_part_gate += 1;
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone)]
struct MappedDiarizationQuality {
    speaker_count: usize,
    labelled_seconds: f64,
    dominant_speaker: Option<String>,
    dominant_seconds: f64,
    dominant_share: f64,
    min_speaker_seconds: Option<f64>,
}

#[derive(Debug, Clone)]
struct DiarizationMappingAttempt {
    model_paths: DiarizationModelPaths,
    profile: DiarizationProfile,
    provider: &'static str,
    turns: Vec<DiarizationTurn>,
    mapped_segments: Vec<TranscriptSegment>,
    prepared_speaker_count: usize,
    quality: MappedDiarizationQuality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiarizationModelKind {
    Segmentation,
    Embedding,
}

impl DiarizationModelKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Segmentation => "segmentation",
            Self::Embedding => "embedding",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DiarizationModelDescriptor {
    kind: DiarizationModelKind,
    id: &'static str,
    display_name: &'static str,
    family: &'static str,
    language: Option<&'static str>,
    cache_dir: &'static str,
    cache_file: &'static str,
    source_file: &'static str,
    download_url: &'static str,
    expected_sha256: Option<&'static str>,
    expected_bytes: Option<u64>,
    default_clustering_threshold: f32,
    is_default: bool,
    legacy_flat_file: Option<&'static str>,
}

const SEGMENTATION_MODEL_CATALOG: &[DiarizationModelDescriptor] = &[
    DiarizationModelDescriptor {
        kind: DiarizationModelKind::Segmentation,
        id: DEFAULT_SEGMENTATION_MODEL_ID,
        display_name: "Pyannote segmentation 3.0 INT8",
        family: "pyannote",
        language: None,
        cache_dir: SEGMENTATION_MODEL_DIR,
        cache_file: "model.int8.onnx",
        source_file: "model.int8.onnx",
        download_url: SEGMENTATION_MODEL_URL,
        expected_sha256: Some(
            "d582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d",
        ),
        expected_bytes: Some(1_540_506),
        default_clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
        is_default: true,
        legacy_flat_file: None,
    },
    DiarizationModelDescriptor {
        kind: DiarizationModelKind::Segmentation,
        id: "pyannote-segmentation-3-0-fp32",
        display_name: "Pyannote segmentation 3.0 FP32",
        family: "pyannote",
        language: None,
        cache_dir: SEGMENTATION_MODEL_DIR,
        cache_file: "model.onnx",
        source_file: "model.onnx",
        download_url:
            "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.onnx",
        expected_sha256: Some(
            "220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079",
        ),
        expected_bytes: Some(5_992_913),
        default_clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
        is_default: false,
        legacy_flat_file: None,
    },
];

const EMBEDDING_MODEL_CATALOG: &[DiarizationModelDescriptor] = &[
    DiarizationModelDescriptor {
        kind: DiarizationModelKind::Embedding,
        id: ZH_CN_EMBEDDING_MODEL_ID,
        display_name: "3D-Speaker ERes2Net base zh-cn",
        family: "3D-Speaker ERes2Net",
        language: Some("zh-cn"),
        cache_dir: ZH_CN_EMBEDDING_MODEL_DIR,
        cache_file: "model.onnx",
        source_file: "3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx",
        download_url: ZH_CN_EMBEDDING_MODEL_URL,
        expected_sha256: Some(
            "1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b",
        ),
        expected_bytes: Some(39_593_761),
        default_clustering_threshold: 0.90,
        is_default: false,
        legacy_flat_file: Some("3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx"),
    },
    DiarizationModelDescriptor {
        kind: DiarizationModelKind::Embedding,
        id: "3dspeaker-campplus-en",
        display_name: "3D-Speaker CAM++ English VoxCeleb",
        family: "3D-Speaker CAM++",
        language: Some("en"),
        cache_dir: "sherpa-onnx-3dspeaker_speech_campplus_sv_en_voxceleb_16k",
        cache_file: "model.onnx",
        source_file: "3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx",
        download_url:
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx",
        expected_sha256: Some(
            "357a834f702b80161e5b981182c038e18553c1f2ca752ed6cec2052365d4129b",
        ),
        expected_bytes: Some(29_596_978),
        default_clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
        is_default: false,
        legacy_flat_file: None,
    },
    DiarizationModelDescriptor {
        kind: DiarizationModelKind::Embedding,
        id: "3dspeaker-eres2net-en",
        display_name: "3D-Speaker ERes2Net English VoxCeleb",
        family: "3D-Speaker ERes2Net",
        language: Some("en"),
        cache_dir: "sherpa-onnx-3dspeaker_speech_eres2net_sv_en_voxceleb_16k",
        cache_file: "model.onnx",
        source_file: "3dspeaker_speech_eres2net_sv_en_voxceleb_16k.onnx",
        download_url:
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_eres2net_sv_en_voxceleb_16k.onnx",
        expected_sha256: Some(
            "c59158379255ad66e161679cca6af8d52d51e389e3224ab7d7a7baae295c2db5",
        ),
        expected_bytes: Some(26_485_263),
        default_clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
        is_default: false,
        legacy_flat_file: None,
    },
    DiarizationModelDescriptor {
        kind: DiarizationModelKind::Embedding,
        id: DEFAULT_EMBEDDING_MODEL_ID,
        display_name: "WeSpeaker CAM++ English VoxCeleb",
        family: "WeSpeaker CAM++",
        language: Some("en"),
        cache_dir: DEFAULT_EMBEDDING_MODEL_DIR,
        cache_file: "model.onnx",
        source_file: "wespeaker_en_voxceleb_CAM++.onnx",
        download_url:
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_CAM%2B%2B.onnx",
        expected_sha256: Some(
            "c46fad10b5f81e1aa4a60c162714208577093655076c5450f8c469e522ec54ef",
        ),
        expected_bytes: Some(29_292_684),
        default_clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
        is_default: true,
        legacy_flat_file: None,
    },
    DiarizationModelDescriptor {
        kind: DiarizationModelKind::Embedding,
        id: "nemo-titanet-small-en",
        display_name: "NeMo TitaNet small English",
        family: "NeMo TitaNet",
        language: Some("en"),
        cache_dir: "sherpa-onnx-nemo_en_titanet_small",
        cache_file: "model.onnx",
        source_file: "nemo_en_titanet_small.onnx",
        download_url:
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_small.onnx",
        expected_sha256: Some(
            "ad4a1802485d8b34c722d2a9d04249662f2ece5d28a7a039063ca22f515a789e",
        ),
        expected_bytes: Some(40_257_283),
        default_clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
        is_default: false,
        legacy_flat_file: None,
    },
];

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
    map_diarization_to_transcript_segments_with_diagnostics(
        transcript_segments,
        diarization_turns,
        options,
    )
    .0
}

fn map_diarization_to_transcript_segments_with_diagnostics(
    transcript_segments: &[TranscriptSegment],
    diarization_turns: &[DiarizationTurn],
    options: DiarizationMappingOptions,
) -> (Vec<TranscriptSegment>, DiarizationMappingDiagnostics) {
    let mut diagnostics =
        DiarizationMappingDiagnostics::from_inputs(transcript_segments, diarization_turns);
    let mapped_segments: Vec<TranscriptSegment> = transcript_segments
        .iter()
        .flat_map(|segment| {
            if should_preserve_existing_speaker(segment, options.existing_speaker_policy) {
                return vec![segment.clone()];
            }

            split_or_label_transcript_segment(
                segment,
                diarization_turns,
                options.mode,
                options.existing_speaker_policy,
                Some(&mut diagnostics),
            )
        })
        .collect();
    let (mapped_segments, smoothed_islands) =
        smooth_short_transcript_speaker_islands(mapped_segments);
    diagnostics.short_speaker_islands_smoothed = smoothed_islands;

    (
        merge_adjacent_transcript_segments_by_speaker(mapped_segments),
        diagnostics,
    )
}

fn split_or_label_transcript_segment(
    segment: &TranscriptSegment,
    diarization_turns: &[DiarizationTurn],
    mode: DiarizationMappingMode,
    existing_policy: ExistingSpeakerPolicy,
    diagnostics: Option<&mut DiarizationMappingDiagnostics>,
) -> Vec<TranscriptSegment> {
    let spans = speaker_spans_for_segment(segment, diarization_turns);
    if spans.len() > 1 {
        let split_segments = split_transcript_segment_by_speaker_spans(segment, &spans);
        if let Some(diagnostics) = diagnostics {
            diagnostics.record_split_candidate(segment, &spans, split_segments.len());
        }
        if split_segments.len() > 1 {
            return split_segments;
        }
    }

    let mut mapped = segment.clone();
    let assigned_speaker = assign_speaker(segment, diarization_turns, mode)
        .map(speaker_label)
        .or_else(|| spans.first().map(|span| speaker_label(span.speaker)));
    mapped.speaker = match existing_policy {
        ExistingSpeakerPolicy::PreserveNonEmpty => {
            assigned_speaker.or_else(|| segment.speaker.clone())
        }
        ExistingSpeakerPolicy::Overwrite => assigned_speaker,
    };
    if let Some(words) = mapped.word_timestamps.as_mut() {
        for word in words {
            word.speaker = mapped.speaker.clone();
        }
    }
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
    if spans
        .iter()
        .any(|span| !is_valid_interval(span.start_time, span.end_time))
    {
        return vec![segment.clone()];
    }

    let spans = expand_short_utterance_speaker_hints(segment, spans);
    if spans.len() <= 1 {
        return vec![segment.clone()];
    }

    let has_real_aligned_word_timestamps = has_real_aligned_word_timestamps(segment);
    if !has_real_aligned_word_timestamps
        && spans
            .iter()
            .any(|span| span.end_time - span.start_time < MIN_SPLIT_PART_SECONDS)
    {
        return vec![segment.clone()];
    }

    let boundaries: Vec<f64> = spans
        .iter()
        .take(spans.len().saturating_sub(1))
        .map(|span| span.end_time.clamp(segment_start, segment_end))
        .collect();
    let text_parts =
        match split_text_by_word_timestamps(&segment.text, &segment.word_timestamps, &boundaries) {
            Some(parts) => parts,
            None if has_real_aligned_word_timestamps => return vec![segment.clone()],
            None => split_text_by_time_boundaries(
                &segment.text,
                segment_start,
                segment_end,
                &boundaries,
            ),
        };
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

            let speaker = speaker_label(span.speaker);
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
                speaker: Some(speaker.clone()),
                word_timestamps: word_timestamps_for_interval(
                    &segment.word_timestamps,
                    audio_start_time,
                    audio_end_time,
                    Some(&speaker),
                ),
            }
        })
        .collect()
}

fn expand_short_utterance_speaker_hints(
    segment: &TranscriptSegment,
    spans: &[TranscriptSpeakerSpan],
) -> Vec<TranscriptSpeakerSpan> {
    if spans.len() <= 1 || !has_real_aligned_word_timestamps(segment) {
        return spans.to_vec();
    }

    let Some(words) = segment.word_timestamps.as_ref() else {
        return spans.to_vec();
    };
    if words.is_empty() {
        return spans.to_vec();
    }

    let mut expanded = spans.to_vec();
    for index in 0..expanded.len() {
        let span = expanded[index];
        if span.end_time - span.start_time > SHORT_UTTERANCE_HINT_MAX_SECONDS {
            continue;
        }

        let Some((utterance_start, utterance_end)) = compact_utterance_bounds_for_span(words, span)
        else {
            continue;
        };
        let start_time = words[utterance_start].start;
        let end_time = words[utterance_end].end;
        if !is_valid_interval(start_time, end_time)
            || end_time - start_time > SHORT_UTTERANCE_EXPANSION_MAX_SECONDS
        {
            continue;
        }

        let text = words[utterance_start..=utterance_end]
            .iter()
            .map(|word| word.text.trim())
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if !is_short_standalone_utterance(&text, utterance_end - utterance_start + 1) {
            continue;
        }

        expanded[index].start_time = start_time.max(segment.audio_start_time.unwrap_or(start_time));
        expanded[index].end_time = end_time.min(segment.audio_end_time.unwrap_or(end_time));
        if index > 0
            && expanded[index - 1].speaker != expanded[index].speaker
            && expanded[index - 1].end_time > expanded[index].start_time
        {
            let expanded_start = expanded[index].start_time;
            expanded[index - 1].end_time = expanded_start;
        }
        if index + 1 < expanded.len()
            && expanded[index + 1].speaker != expanded[index].speaker
            && expanded[index + 1].start_time < expanded[index].end_time
        {
            let expanded_end = expanded[index].end_time;
            expanded[index + 1].start_time = expanded_end;
        }
    }

    trim_overlapping_expanded_spans(expanded)
}

fn compact_utterance_bounds_for_span(
    words: &[TranscriptWord],
    span: TranscriptSpeakerSpan,
) -> Option<(usize, usize)> {
    let first = words.iter().position(|word| {
        word_midpoint(word) >= span.start_time && word_midpoint(word) < span.end_time
    });
    let last = words.iter().rposition(|word| {
        word_midpoint(word) >= span.start_time && word_midpoint(word) < span.end_time
    });
    let (first, last) = match (first, last) {
        (Some(first), Some(last)) => (first, last),
        _ => {
            let (anchor, distance) = words
                .iter()
                .enumerate()
                .map(|(index, word)| (index, word_distance_to_span(word, span)))
                .filter(|(_, distance)| distance.is_finite())
                .min_by(|left, right| {
                    left.1
                        .partial_cmp(&right.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })?;
            if distance > SHORT_UTTERANCE_HINT_MAX_SECONDS {
                return None;
            }
            (anchor, anchor)
        }
    };

    let mut start = first;
    while start > 0 && !word_ends_sentence(&words[start - 1].text) {
        start -= 1;
    }

    let mut end = last;
    while end + 1 < words.len() && !word_ends_sentence(&words[end].text) {
        end += 1;
    }

    if end < start || end - start + 1 > SHORT_UTTERANCE_EXPANSION_MAX_WORDS {
        return None;
    }

    Some((start, end))
}

fn word_distance_to_span(word: &TranscriptWord, span: TranscriptSpeakerSpan) -> f64 {
    if word.end >= span.start_time && word.start <= span.end_time {
        0.0
    } else if word.end < span.start_time {
        span.start_time - word.end
    } else {
        word.start - span.end_time
    }
}

fn is_short_standalone_utterance(text: &str, word_count: usize) -> bool {
    if word_count == 0 || word_count > SHORT_UTTERANCE_EXPANSION_MAX_WORDS {
        return false;
    }

    let trimmed = text.trim();
    is_standalone_backchannel(trimmed)
        || trimmed.ends_with('?')
        || (word_count <= 4 && trimmed.ends_with(['.', '!', '?']))
}

fn trim_overlapping_expanded_spans(
    mut spans: Vec<TranscriptSpeakerSpan>,
) -> Vec<TranscriptSpeakerSpan> {
    spans.sort_by(|a, b| {
        a.start_time
            .partial_cmp(&b.start_time)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.end_time
                    .partial_cmp(&b.end_time)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.speaker.cmp(&b.speaker))
    });

    for index in 0..spans.len() {
        if index > 0 && spans[index - 1].end_time > spans[index].start_time {
            if spans[index - 1].speaker == spans[index].speaker {
                let merged_end = spans[index - 1].end_time.max(spans[index].end_time);
                spans[index - 1].end_time = merged_end;
                let current_end = spans[index].end_time;
                spans[index].start_time = current_end;
            } else {
                let current_start = spans[index].start_time;
                spans[index - 1].end_time = current_start;
            }
        }
        if index + 1 < spans.len() && spans[index].end_time > spans[index + 1].start_time {
            if spans[index].speaker == spans[index + 1].speaker {
                let merged_end = spans[index].end_time.max(spans[index + 1].end_time);
                spans[index].end_time = merged_end;
                let next_end = spans[index + 1].end_time;
                spans[index + 1].start_time = next_end;
            } else {
                let current_end = spans[index].end_time;
                spans[index + 1].start_time = current_end;
            }
        }
    }

    collapse_adjacent_same_speaker_spans(
        spans
            .into_iter()
            .filter(|span| is_valid_interval(span.start_time, span.end_time))
            .collect(),
    )
}

fn has_real_aligned_word_timestamps(segment: &TranscriptSegment) -> bool {
    matches!(
        word_timestamp_eligibility(segment),
        WordTimestampEligibility::Real | WordTimestampEligibility::LegacyPrecise
    )
}

fn smooth_short_transcript_speaker_islands(
    mut segments: Vec<TranscriptSegment>,
) -> (Vec<TranscriptSegment>, usize) {
    if segments.len() < 3 {
        return (segments, 0);
    }

    let mut smoothed = 0usize;
    for index in 1..segments.len() - 1 {
        let Some(previous_speaker) =
            normalized_speaker_label(segments[index - 1].speaker.as_deref())
        else {
            continue;
        };
        let Some(current_speaker) = normalized_speaker_label(segments[index].speaker.as_deref())
        else {
            continue;
        };
        let Some(next_speaker) = normalized_speaker_label(segments[index + 1].speaker.as_deref())
        else {
            continue;
        };
        if previous_speaker != next_speaker || current_speaker == previous_speaker {
            continue;
        }

        let Some(current_duration) = transcript_segment_duration(&segments[index]) else {
            continue;
        };
        if current_duration > SHORT_SPEAKER_ISLAND_MAX_SECONDS {
            continue;
        }

        let context_seconds = transcript_segment_duration(&segments[index - 1]).unwrap_or(0.0)
            + transcript_segment_duration(&segments[index + 1]).unwrap_or(0.0);
        if context_seconds < SHORT_SPEAKER_ISLAND_MIN_CONTEXT_SECONDS {
            continue;
        }

        if is_standalone_backchannel(&segments[index].text) {
            continue;
        }

        set_transcript_segment_speaker(&mut segments[index], &previous_speaker);
        smoothed += 1;
    }

    (segments, smoothed)
}

fn transcript_segment_duration(segment: &TranscriptSegment) -> Option<f64> {
    segment
        .duration
        .or_else(|| Some((segment.audio_end_time? - segment.audio_start_time?).max(0.0)))
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
}

fn set_transcript_segment_speaker(segment: &mut TranscriptSegment, speaker: &str) {
    let speaker = speaker.to_string();
    segment.speaker = Some(speaker.clone());
    if let Some(words) = segment.word_timestamps.as_mut() {
        for word in words {
            word.speaker = Some(speaker.clone());
        }
    }
}

fn is_standalone_backchannel(text: &str) -> bool {
    let normalized = text
        .trim()
        .trim_matches(|character: char| {
            matches!(
                character,
                '.' | ',' | '?' | '!' | ':' | ';' | '"' | '\'' | '(' | ')' | '[' | ']'
            )
        })
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }

    matches!(
        normalized.as_str(),
        "yeah"
            | "yep"
            | "yes"
            | "no"
            | "nah"
            | "right"
            | "ok"
            | "okay"
            | "exactly"
            | "definitely"
            | "sure"
            | "mm-hmm"
            | "mhm"
            | "uh-huh"
            | "of course"
            | "for sure"
    )
}

fn word_timestamp_eligibility(segment: &TranscriptSegment) -> WordTimestampEligibility {
    let Some(words) = segment.word_timestamps.as_ref() else {
        return WordTimestampEligibility::None;
    };
    let text = segment.text.trim();
    let tokens = word_tokens(text);
    if tokens.is_empty()
        || tokens.len() != words.len()
        || words
            .iter()
            .any(|word| !is_valid_interval(word.start, word.end))
    {
        return WordTimestampEligibility::Invalid;
    }

    if words
        .iter()
        .any(|word| word.timestamp_source == Some(TranscriptWordTimestampSource::Estimated))
    {
        return WordTimestampEligibility::Estimated;
    }

    if words
        .iter()
        .all(|word| word.timestamp_source == Some(TranscriptWordTimestampSource::Real))
    {
        return WordTimestampEligibility::Real;
    }

    if words.iter().any(|word| word.timestamp_source.is_none()) {
        return if matches_weighted_estimated_word_timestamps(segment, text, &tokens, words) {
            WordTimestampEligibility::LegacyEstimated
        } else {
            WordTimestampEligibility::LegacyPrecise
        };
    }

    WordTimestampEligibility::Invalid
}

fn matches_weighted_estimated_word_timestamps(
    segment: &TranscriptSegment,
    text: &str,
    tokens: &[WordToken],
    words: &[TranscriptWord],
) -> bool {
    let Some(segment_start) = segment.audio_start_time else {
        return false;
    };
    let Some(segment_end) = segment.audio_end_time else {
        return false;
    };
    if tokens.is_empty()
        || tokens.len() != words.len()
        || !is_valid_interval(segment_start, segment_end)
    {
        return false;
    }

    // Keep this in lockstep with crate::audio::common::estimate_word_timestamps.
    // It distinguishes legacy estimated anchors from legacy precise Parakeet anchors
    // that were saved before timestamp_source existed.
    let total_weight = tokens
        .iter()
        .map(|token| word_token_weight(text, token))
        .sum::<f64>()
        .max(1.0);
    let duration = segment_end - segment_start;
    let mut cursor = segment_start;

    for (index, (token, word)) in tokens.iter().zip(words.iter()).enumerate() {
        let expected_start = cursor;
        let expected_end = if index + 1 == tokens.len() {
            segment_end
        } else {
            (cursor + duration * (word_token_weight(text, token) / total_weight)).min(segment_end)
        };
        cursor = expected_end;

        if (word.start - expected_start).abs() > LEGACY_ESTIMATED_WORD_TIMESTAMP_TOLERANCE_SECONDS
            || (word.end - expected_end).abs() > LEGACY_ESTIMATED_WORD_TIMESTAMP_TOLERANCE_SECONDS
        {
            return false;
        }
    }

    true
}

fn word_token_weight(text: &str, token: &WordToken) -> f64 {
    text[token.start_byte..token.end_byte]
        .chars()
        .filter(|character| !character.is_whitespace())
        .count()
        .max(1) as f64
}

fn merge_adjacent_transcript_segments_by_speaker(
    segments: Vec<TranscriptSegment>,
) -> Vec<TranscriptSegment> {
    let mut merged: Vec<TranscriptSegment> = Vec::with_capacity(segments.len());

    for segment in segments {
        if let Some(previous) = merged.last_mut() {
            if can_merge_transcript_segments(previous, &segment) {
                previous.text = join_transcript_text(&previous.text, &segment.text);
                if let Some(end_time) = segment.audio_end_time {
                    previous.audio_end_time = Some(end_time);
                }
                if let (Some(start), Some(end)) =
                    (previous.audio_start_time, previous.audio_end_time)
                {
                    previous.duration = Some((end - start).max(0.0));
                }
                match (&mut previous.word_timestamps, segment.word_timestamps) {
                    (Some(existing), Some(mut next)) => existing.append(&mut next),
                    (None, Some(next)) => previous.word_timestamps = Some(next),
                    _ => {}
                }
                continue;
            }
        }

        merged.push(segment);
    }

    merged
}

fn word_timestamps_for_interval(
    word_timestamps: &Option<Vec<TranscriptWord>>,
    start_time: f64,
    end_time: f64,
    speaker: Option<&str>,
) -> Option<Vec<TranscriptWord>> {
    let words = word_timestamps.as_ref()?;
    let speaker = speaker.map(str::to_string);
    let filtered = words
        .iter()
        .filter(|word| {
            let midpoint = word_midpoint(word);
            midpoint >= start_time - FLOAT_TIE_EPSILON && midpoint < end_time
        })
        .map(|word| TranscriptWord {
            text: word.text.clone(),
            start: word.start.max(start_time),
            end: word.end.min(end_time).max(word.start.max(start_time)),
            confidence: word.confidence,
            speaker: speaker.clone().or_else(|| word.speaker.clone()),
            timestamp_source: word.timestamp_source,
        })
        .collect::<Vec<_>>();

    if filtered.is_empty() {
        None
    } else {
        Some(filtered)
    }
}

fn parse_word_timestamps(value: Option<&str>) -> Option<Vec<TranscriptWord>> {
    value.and_then(|json| serde_json::from_str::<Vec<TranscriptWord>>(json).ok())
}

fn can_merge_transcript_segments(left: &TranscriptSegment, right: &TranscriptSegment) -> bool {
    let Some(left_speaker) = normalized_speaker_label(left.speaker.as_deref()) else {
        return false;
    };
    if Some(left_speaker) != normalized_speaker_label(right.speaker.as_deref()) {
        return false;
    }

    let (Some(left_end), Some(right_start)) = (left.audio_end_time, right.audio_start_time) else {
        return false;
    };

    let gap = right_start - left_end;
    gap >= -FLOAT_TIE_EPSILON && gap <= SAME_SPEAKER_TRANSCRIPT_MERGE_GAP_SECONDS
}

fn normalized_speaker_label(speaker: Option<&str>) -> Option<String> {
    let label = speaker?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!label.is_empty()).then_some(label)
}

fn join_transcript_text(left: &str, right: &str) -> String {
    let left = left.trim();
    let right = right.trim();
    if left.is_empty() {
        return right.to_string();
    }
    if right.is_empty() {
        return left.to_string();
    }

    format!("{left} {right}")
}

fn split_text_by_word_timestamps(
    text: &str,
    word_timestamps: &Option<Vec<TranscriptWord>>,
    boundaries: &[f64],
) -> Option<Vec<String>> {
    let trimmed = text.trim();
    let words = word_timestamps.as_ref()?;
    if trimmed.is_empty() || boundaries.is_empty() {
        return None;
    }

    let tokens = word_tokens(trimmed);
    let piece_count = boundaries.len() + 1;
    if tokens.len() < piece_count || tokens.len() != words.len() {
        return None;
    }
    if words
        .iter()
        .any(|word| !is_valid_interval(word.start, word.end))
    {
        return None;
    }

    let mut split_indices = Vec::<usize>::with_capacity(boundaries.len());
    let mut previous_index = 0usize;
    for (boundary_index, boundary_time) in boundaries.iter().enumerate() {
        if !boundary_time.is_finite() {
            return None;
        }

        let remaining_boundaries = boundaries.len() - boundary_index - 1;
        let lower = previous_index + 1;
        let upper = tokens.len().saturating_sub(remaining_boundaries + 1);
        if lower > upper {
            return None;
        }

        let midpoint_split = words
            .iter()
            .take(tokens.len())
            .take_while(|word| word_midpoint(word) < *boundary_time)
            .count();
        let split_index = midpoint_split.clamp(lower, upper);
        split_indices.push(split_index);
        previous_index = split_index;
    }

    Some(split_text_by_token_indices(
        trimmed,
        &tokens,
        &split_indices,
    ))
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

    split_text_by_token_indices(trimmed, &tokens, &split_indices)
}

fn split_text_by_token_indices(
    text: &str,
    tokens: &[WordToken],
    split_indices: &[usize],
) -> Vec<String> {
    let mut parts = Vec::with_capacity(split_indices.len() + 1);
    let mut start_token_index = 0usize;
    for end_token_index in split_indices
        .iter()
        .copied()
        .chain(std::iter::once(tokens.len()))
    {
        let start_byte = tokens[start_token_index].start_byte;
        let end_byte = tokens[end_token_index - 1].end_byte;
        parts.push(text[start_byte..end_byte].trim().to_string());
        start_token_index = end_token_index;
    }

    parts
}

fn word_midpoint(word: &TranscriptWord) -> f64 {
    word.start + ((word.end - word.start) / 2.0)
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
            clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
            min_duration_on: DEFAULT_MIN_DURATION_ON_SECONDS,
            min_duration_off: DEFAULT_MIN_DURATION_OFF_SECONDS,
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

fn prepare_diarization_turns_for_mapping(
    _sample_count: usize,
    turns: &[DiarizationTurn],
    _explicit_num_speakers: Option<i32>,
) -> Vec<DiarizationTurn> {
    let mut prepared = turns
        .iter()
        .filter(|turn| is_valid_interval(turn.start_time, turn.end_time))
        .cloned()
        .collect::<Vec<_>>();
    prepared.sort_by(|a, b| {
        a.start_time
            .partial_cmp(&b.start_time)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.end_time
                    .partial_cmp(&b.end_time)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.speaker.cmp(&b.speaker))
    });

    prepared = resolve_overlapping_diarization_turns(prepared);
    compact_diarization_speakers(collapse_adjacent_diarization_turns(prepared))
}

fn resolve_overlapping_diarization_turns(turns: Vec<DiarizationTurn>) -> Vec<DiarizationTurn> {
    if turns.len() < 2 {
        return turns;
    }

    let mut boundaries = turns
        .iter()
        .flat_map(|turn| [turn.start_time, turn.end_time])
        .filter(|time| time.is_finite())
        .collect::<Vec<_>>();
    boundaries.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    boundaries.dedup_by(|left, right| (*left - *right).abs() <= FLOAT_TIE_EPSILON);
    if boundaries.len() < 2 {
        return turns;
    }

    let mut resolved = Vec::new();
    for window in boundaries.windows(2) {
        let start_time = window[0];
        let end_time = window[1];
        if !is_valid_interval(start_time, end_time) {
            continue;
        }

        let Some(active) = dominant_turn_for_atomic_interval(&turns, start_time, end_time) else {
            continue;
        };
        resolved.push(DiarizationTurn {
            start_time,
            end_time,
            speaker: active.speaker,
        });
    }

    if resolved.is_empty() {
        turns
    } else {
        collapse_adjacent_diarization_turns(resolved)
    }
}

fn dominant_turn_for_atomic_interval(
    turns: &[DiarizationTurn],
    start_time: f64,
    end_time: f64,
) -> Option<&DiarizationTurn> {
    let interval_duration = end_time - start_time;
    turns
        .iter()
        .filter(|turn| {
            overlap_seconds(start_time, end_time, turn.start_time, turn.end_time)
                >= interval_duration - FLOAT_TIE_EPSILON
        })
        .min_by(|left, right| {
            turn_duration(*left)
                .partial_cmp(&turn_duration(*right))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    right
                        .start_time
                        .partial_cmp(&left.start_time)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| left.speaker.cmp(&right.speaker))
        })
}

fn turn_duration(turn: &DiarizationTurn) -> f64 {
    (turn.end_time - turn.start_time).max(0.0)
}

fn collapse_adjacent_diarization_turns(turns: Vec<DiarizationTurn>) -> Vec<DiarizationTurn> {
    let mut collapsed: Vec<DiarizationTurn> = Vec::new();
    for turn in turns {
        let Some(last) = collapsed.last_mut() else {
            collapsed.push(turn);
            continue;
        };

        if last.speaker == turn.speaker
            && turn.start_time <= last.end_time + SAME_SPEAKER_MERGE_GAP_SECONDS
        {
            last.end_time = last.end_time.max(turn.end_time);
        } else {
            collapsed.push(turn);
        }
    }

    collapsed
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelFileValidation {
    actual_sha256: String,
    actual_bytes: u64,
    sha256_matches: bool,
    bytes_match: bool,
}

impl ModelFileValidation {
    fn is_valid(&self) -> bool {
        self.sha256_matches && self.bytes_match
    }

    fn error_message(&self, descriptor: &DiarizationModelDescriptor, path: &Path) -> String {
        let mut errors = Vec::new();
        if !self.sha256_matches {
            errors.push(format!(
                "expected sha256 {}, got {}",
                descriptor.expected_sha256.unwrap_or("<not pinned>"),
                self.actual_sha256
            ));
        }
        if !self.bytes_match {
            errors.push(format!(
                "expected {} bytes, got {}",
                descriptor
                    .expected_bytes
                    .map(|bytes| bytes.to_string())
                    .unwrap_or_else(|| "<not pinned>".to_string()),
                self.actual_bytes
            ));
        }

        format!(
            "{} diarization model failed validation at {} ({})",
            descriptor.kind.as_str(),
            path.display(),
            errors.join("; ")
        )
    }
}

fn validate_model_file(
    path: &Path,
    descriptor: &DiarizationModelDescriptor,
) -> Result<ModelFileValidation> {
    ensure_model_file(path, descriptor.kind.as_str())?;
    let actual_bytes = fs::metadata(path)?.len();
    let actual_sha256 = sha256_file(path)?;
    let sha256_matches = descriptor
        .expected_sha256
        .map(|expected| actual_sha256.eq_ignore_ascii_case(expected))
        .unwrap_or(true);
    let bytes_match = descriptor
        .expected_bytes
        .map(|expected| actual_bytes == expected)
        .unwrap_or(true);

    Ok(ModelFileValidation {
        actual_sha256,
        actual_bytes,
        sha256_matches,
        bytes_match,
    })
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn quarantine_invalid_default_model(path: &Path) -> Result<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("model");
    let quarantine_name = format!(
        "{}.invalid-{}",
        file_name,
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    let quarantine_path = path.with_file_name(quarantine_name);

    match fs::rename(path, &quarantine_path) {
        Ok(()) => Ok(Some(quarantine_path)),
        Err(rename_error) => {
            log::warn!(
                "Failed to quarantine invalid diarization model {}: {}; deleting it instead",
                path.display(),
                rename_error
            );
            fs::remove_file(path)?;
            Ok(None)
        }
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
pub struct SpeakerDiarizationComplete {
    pub meeting_id: String,
    pub speaker_count: usize,
    pub updated_segments: usize,
    pub duration_seconds: f64,
    pub processing_seconds: f64,
    pub provider: String,
    pub embedding_model: String,
    pub turn_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpeakerDiarizationError {
    pub meeting_id: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiarizationRuntimeDll {
    pub name: String,
    pub present: bool,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerDiarizationRuntimeStatus {
    pub in_progress: bool,
    pub directml_compiled: bool,
    pub directml_fast: bool,
    pub directml_slow: bool,
    pub directml_unavailable: bool,
    pub preferred_provider: String,
    pub runtime_dlls: Vec<DiarizationRuntimeDll>,
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
struct DiarizationProfileModel {
    kind: String,
    id: Option<String>,
    display_name: String,
    family: Option<String>,
    language: Option<String>,
    path: String,
    source_file: Option<String>,
    download_url: Option<String>,
    expected_sha256: Option<String>,
    expected_bytes: Option<u64>,
    is_default: bool,
    custom_path: bool,
}

#[derive(Debug, Clone, Serialize)]
struct DiarizationProfile {
    meeting_id: String,
    audio_seconds: f64,
    sample_count: usize,
    num_threads: i32,
    explicit_num_speakers: Option<i32>,
    clustering_threshold: f32,
    segmentation_model: DiarizationProfileModel,
    embedding_model: DiarizationProfileModel,
    directml_feature: bool,
    preferred_provider_before_probe: String,
    selected_provider: Option<String>,
    decision: Option<String>,
    runtime_dlls: Vec<DiarizationRuntimeDll>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mapping_diagnostics: Option<DiarizationMappingDiagnostics>,
    events: Vec<DiarizationProfileEvent>,
}

#[derive(Debug, Clone)]
struct DiarizationModelPaths {
    segmentation_model: PathBuf,
    embedding_model: PathBuf,
    segmentation_descriptor: Option<&'static DiarizationModelDescriptor>,
    embedding_descriptor: Option<&'static DiarizationModelDescriptor>,
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
    word_timestamps_json: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiarizationEmbeddingChoice {
    Default,
    Model(&'static str),
}

impl DiarizationEmbeddingChoice {
    fn model_id(self) -> Option<String> {
        match self {
            Self::Default => None,
            Self::Model(id) => Some(id.to_string()),
        }
    }
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
    embedding_model_id: Option<String>,
    num_speakers: Option<i32>,
    preserve_existing_labels: Option<bool>,
) -> std::result::Result<SpeakerDiarizationComplete, String> {
    let guard = DiarizationRunGuard::acquire()?;
    let meeting_id_for_task = meeting_id.clone();

    let _guard = guard;
    let result = run_speaker_diarization_for_meeting(
        app.clone(),
        meeting_id_for_task.clone(),
        meeting_folder_path,
        segmentation_model_path,
        embedding_model_path,
        embedding_model_id,
        num_speakers,
        preserve_existing_labels.unwrap_or(true),
    )
    .await;

    match result {
        Ok(complete) => {
            let _ = app.emit("speaker-diarization-complete", complete.clone());
            Ok(complete)
        }
        Err(error) => {
            let message = error.to_string();
            let _ = app.emit(
                "speaker-diarization-error",
                SpeakerDiarizationError {
                    meeting_id: meeting_id_for_task,
                    error: message.clone(),
                },
            );
            Err(message)
        }
    }
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
    embedding_model_id: Option<String>,
    num_speakers: Option<i32>,
    preserve_existing_labels: bool,
) -> Result<SpeakerDiarizationComplete> {
    let run_started = Instant::now();
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
    let audio_path =
        crate::audio::incremental_saver::find_or_recover_audio_file(&folder_path).await?;
    let app_state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("Application database is not initialized"))?;
    let pool = app_state.db_manager.pool().clone();

    let stored_segments = sqlx::query_as::<_, StoredTranscriptSegment>(
        "SELECT id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker, word_timestamps_json
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

    let requested_segmentation_model_path = segmentation_model_path.clone();
    let requested_embedding_model_path = embedding_model_path.clone();
    let custom_embedding_requested = embedding_model_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .is_some()
        || embedding_model_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .is_some();

    let resolved_embedding_model_id = resolve_embedding_model_id_for_meeting(
        &folder_path,
        embedding_model_path.as_deref(),
        embedding_model_id,
        &stored_segments,
    );
    let allow_zh_cn_fallback =
        resolved_embedding_model_id.as_deref() == Some(ZH_CN_EMBEDDING_MODEL_ID);
    let model_paths = resolve_model_paths_for_embedding(
        &app,
        segmentation_model_path,
        embedding_model_path,
        resolved_embedding_model_id,
    )?;

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
    let samples = Arc::new(samples);
    let explicit_num_speakers = num_speakers.filter(|value| *value > 0);
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
            word_timestamps: parse_word_timestamps(segment.word_timestamps_json.as_deref()),
        })
        .collect();

    let mut attempt = run_diarization_mapping_attempt(
        &app,
        &meeting_id,
        model_paths,
        sample_count,
        explicit_num_speakers,
        Arc::clone(&samples),
        &transcript_segments,
        preserve_existing_labels,
        45,
        None,
    )
    .await?;

    if let Some(expected_speakers) = explicit_num_speakers
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 1)
    {
        if explicit_speaker_mapping_needs_embedding_retry(expected_speakers, &attempt.quality)
            && !custom_embedding_requested
        {
            attempt = retry_unreliable_mapping_with_fallback_embeddings(
                &app,
                &meeting_id,
                attempt,
                expected_speakers,
                explicit_num_speakers,
                sample_count,
                Arc::clone(&samples),
                &transcript_segments,
                preserve_existing_labels,
                requested_segmentation_model_path.clone(),
                requested_embedding_model_path.clone(),
                allow_zh_cn_fallback,
                65,
            )
            .await?;
        }

        if explicit_speaker_mapping_is_collapsed(expected_speakers, &attempt.quality) {
            write_diarization_profile(&app, &attempt.profile);
            return Err(anyhow!(explicit_mapping_failure_message(
                expected_speakers,
                attempt.prepared_speaker_count,
                &attempt.quality,
            )));
        }
    } else if let Some(recovery) = auto_diarization_recovery_target(&attempt) {
        log::warn!(
            "speaker_diarization_auto_overfragmented_retry meeting_id={} reason=\"{}\" retry_speakers={} prepared_speakers={} mapped_speakers={}",
            meeting_id,
            recovery.reason,
            recovery.speaker_count,
            attempt.prepared_speaker_count,
            attempt.quality.speaker_count
        );
        emit_progress(
            &app,
            &meeting_id,
            "diarizing_retry",
            65,
            &format!(
                "Auto speaker detection found {} meaningful speakers after {} lanes; retrying with {} speakers...",
                recovery.speaker_count,
                attempt.prepared_speaker_count,
                recovery.speaker_count
            ),
        );

        let mut recovered_attempt = run_diarization_mapping_attempt(
            &app,
            &meeting_id,
            attempt.model_paths.clone(),
            sample_count,
            Some(recovery.speaker_count),
            Arc::clone(&samples),
            &transcript_segments,
            preserve_existing_labels,
            65,
            None,
        )
        .await?;

        let expected_speakers = usize::try_from(recovery.speaker_count).unwrap_or(0);
        if expected_speakers > 1
            && explicit_speaker_mapping_needs_embedding_retry(
                expected_speakers,
                &recovered_attempt.quality,
            )
            && !custom_embedding_requested
        {
            recovered_attempt = retry_unreliable_mapping_with_fallback_embeddings(
                &app,
                &meeting_id,
                recovered_attempt,
                expected_speakers,
                Some(recovery.speaker_count),
                sample_count,
                Arc::clone(&samples),
                &transcript_segments,
                preserve_existing_labels,
                requested_segmentation_model_path.clone(),
                requested_embedding_model_path.clone(),
                allow_zh_cn_fallback,
                65,
            )
            .await?;
        }

        if explicit_speaker_mapping_is_collapsed(expected_speakers, &recovered_attempt.quality) {
            write_diarization_profile(&app, &recovered_attempt.profile);
            return Err(anyhow!(explicit_mapping_failure_message(
                expected_speakers,
                recovered_attempt.prepared_speaker_count,
                &recovered_attempt.quality,
            )));
        }

        attempt = recovered_attempt;
    } else if let Some(reason) = auto_diarization_low_confidence_reason(&attempt) {
        write_diarization_profile(&app, &attempt.profile);
        return Err(anyhow!(auto_mapping_failure_message(
            &reason,
            attempt.prepared_speaker_count,
            &attempt.quality,
        )));
    }

    if diarization_mapping_has_zero_mapped_speakers(&attempt) {
        write_diarization_profile(&app, &attempt.profile);
        return Err(anyhow!(zero_speaker_mapping_failure_message(
            attempt.prepared_speaker_count
        )));
    }

    emit_progress(
        &app,
        &meeting_id,
        "saving",
        82,
        "Applying speaker labels to transcripts...",
    );

    let mapped_segments = attempt.mapped_segments;
    let updated_segments = count_changed_transcript_segments(&stored_segments, &mapped_segments);
    if updated_segments > 0 {
        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
            .bind(&meeting_id)
            .execute(&mut *tx)
            .await?;

        for mapped in &mapped_segments {
            let word_timestamps_json = mapped
                .word_timestamps
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| anyhow!("Invalid word timestamps: {}", e))?;
            sqlx::query(
                "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker, word_timestamps_json)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
            .bind(word_timestamps_json)
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
            word_timestamps: mapped.word_timestamps.clone(),
        })
        .collect();
    super::common::write_transcripts_json(&folder_path, &transcript_file_segments)?;

    let speaker_count = transcript_speaker_count(&mapped_segments);
    let turn_count = attempt.turns.len();
    let embedding_model = attempt
        .model_paths
        .embedding_descriptor
        .map(|descriptor| descriptor.display_name.to_string())
        .unwrap_or_else(|| "Custom embedding model".to_string());

    emit_progress(
        &app,
        &meeting_id,
        "complete",
        100,
        "Speaker labels applied.",
    );
    write_diarization_profile(&app, &attempt.profile);

    Ok(SpeakerDiarizationComplete {
        meeting_id,
        speaker_count,
        updated_segments,
        duration_seconds: sample_count as f64 / f64::from(DIARIZATION_SAMPLE_RATE),
        processing_seconds: run_started.elapsed().as_secs_f64(),
        provider: attempt.provider.to_string(),
        embedding_model,
        turn_count,
    })
}

async fn run_diarization_mapping_attempt<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    model_paths: DiarizationModelPaths,
    sample_count: usize,
    explicit_num_speakers: Option<i32>,
    samples: Arc<Vec<f32>>,
    transcript_segments: &[TranscriptSegment],
    preserve_existing_labels: bool,
    progress_percentage: u32,
    retry_embedding_name: Option<&str>,
) -> Result<DiarizationMappingAttempt> {
    ensure_model_available(
        app,
        meeting_id,
        &model_paths.segmentation_model,
        model_paths.segmentation_descriptor,
        "segmentation",
        model_paths.can_download_segmentation,
    )
    .await?;
    ensure_model_available(
        app,
        meeting_id,
        &model_paths.embedding_model,
        model_paths.embedding_descriptor,
        "embedding",
        model_paths.can_download_embedding,
    )
    .await?;

    let clustering_threshold = default_clustering_threshold(&model_paths);
    let mut profile = DiarizationProfile::new(
        meeting_id,
        sample_count,
        explicit_num_speakers,
        clustering_threshold,
        &model_paths,
    );
    let provider = select_diarization_provider(
        app,
        meeting_id,
        &model_paths,
        explicit_num_speakers,
        clustering_threshold,
        Arc::clone(&samples),
        &mut profile,
    )
    .await?;
    let audio_minutes = audio_duration_minutes(sample_count);
    let diarization_message = match retry_embedding_name {
        Some(embedding_name) => format!(
            "Retrying speaker turns with {embedding_name}{}...",
            diarization_provider_message_suffix(provider)
        ),
        None if audio_minutes >= 1.0 => format!(
            "Detecting speaker turns in {:.1} min of audio{}...",
            audio_minutes,
            diarization_provider_message_suffix(provider)
        ),
        None => format!(
            "Detecting speaker turns{}...",
            diarization_provider_message_suffix(provider)
        ),
    };
    emit_progress(
        app,
        meeting_id,
        "diarizing",
        progress_percentage,
        &diarization_message,
    );

    let config = diarization_config_for_provider(
        &model_paths,
        explicit_num_speakers,
        provider,
        clustering_threshold,
    );
    let mut turns =
        run_sherpa_diarization_with_fallback(app, meeting_id, config, samples, &mut profile)
            .await?;

    if turns.is_empty() {
        return Err(anyhow!(
            "No speaker turns were detected in this meeting audio"
        ));
    }
    turns = prepare_diarization_turns_for_mapping(sample_count, &turns, explicit_num_speakers);
    let prepared_speaker_count = speaker_count_from_turns(&turns);
    let (mut mapped_segments, mut mapping_diagnostics) =
        map_diarization_to_transcript_segments_with_diagnostics(
            transcript_segments,
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
    let mut mapped_speaker_count = transcript_speaker_count(&mapped_segments);
    if prepared_speaker_count > 1 && mapped_speaker_count <= 1 {
        let (midpoint_segments, midpoint_diagnostics) =
            map_diarization_to_transcript_segments_with_diagnostics(
                transcript_segments,
                &turns,
                DiarizationMappingOptions {
                    mode: DiarizationMappingMode::Midpoint,
                    existing_speaker_policy: if preserve_existing_labels {
                        ExistingSpeakerPolicy::PreserveNonEmpty
                    } else {
                        ExistingSpeakerPolicy::Overwrite
                    },
                },
            );
        let midpoint_speaker_count = transcript_speaker_count(&midpoint_segments);
        if midpoint_speaker_count > mapped_speaker_count {
            log::info!(
                "speaker_diarization_mapping_fallback meeting_id={} overlap_speakers={} midpoint_speakers={} prepared_speakers={}",
                meeting_id,
                mapped_speaker_count,
                midpoint_speaker_count,
                prepared_speaker_count
            );
            mapped_segments = midpoint_segments;
            mapping_diagnostics = midpoint_diagnostics;
            mapped_speaker_count = midpoint_speaker_count;
        }
    }
    let quality = mapped_diarization_quality(&mapped_segments);
    log::info!(
        "speaker_diarization_mapping_quality meeting_id={} embedding_model={} provider={} prepared_speakers={} mapped_speakers={} labelled_seconds={:.1} dominant_speaker={} dominant_seconds={:.1} dominant_share={:.3} min_speaker_seconds={:.1}",
        meeting_id,
        model_paths
            .embedding_descriptor
            .map(|descriptor| descriptor.id)
            .unwrap_or("custom"),
        provider,
        prepared_speaker_count,
        mapped_speaker_count,
        quality.labelled_seconds,
        quality
            .dominant_speaker
            .as_deref()
            .unwrap_or("<none>"),
        quality.dominant_seconds,
        quality.dominant_share,
        quality.min_speaker_seconds.unwrap_or(0.0),
    );
    profile.set_mapping_diagnostics(mapping_diagnostics);

    Ok(DiarizationMappingAttempt {
        model_paths,
        profile,
        provider,
        turns,
        mapped_segments,
        prepared_speaker_count,
        quality,
    })
}

#[allow(clippy::too_many_arguments)]
async fn retry_unreliable_mapping_with_fallback_embeddings<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    mut attempt: DiarizationMappingAttempt,
    expected_speakers: usize,
    explicit_num_speakers: Option<i32>,
    sample_count: usize,
    samples: Arc<Vec<f32>>,
    transcript_segments: &[TranscriptSegment],
    preserve_existing_labels: bool,
    requested_segmentation_model_path: Option<String>,
    requested_embedding_model_path: Option<String>,
    allow_zh_cn_fallback: bool,
    progress_percentage: u32,
) -> Result<DiarizationMappingAttempt> {
    if !explicit_speaker_mapping_needs_embedding_retry(expected_speakers, &attempt.quality) {
        return Ok(attempt);
    }

    let current_embedding_id = attempt
        .model_paths
        .embedding_descriptor
        .map(|descriptor| descriptor.id);
    let mut best_attempt = attempt.clone();

    for embedding_model_id in
        fallback_embedding_model_ids(current_embedding_id, allow_zh_cn_fallback)
    {
        let candidate_model_paths = resolve_model_paths_for_embedding(
            app,
            requested_segmentation_model_path.clone(),
            requested_embedding_model_path.clone(),
            Some(embedding_model_id.to_string()),
        )?;
        let embedding_name = candidate_model_paths
            .embedding_descriptor
            .map(|descriptor| descriptor.display_name)
            .unwrap_or("custom embedding model");
        emit_progress(
            app,
            meeting_id,
            "diarizing_retry",
            progress_percentage,
            &format!("Speaker labels look unreliable; retrying with {embedding_name}..."),
        );

        let candidate = match run_diarization_mapping_attempt(
            app,
            meeting_id,
            candidate_model_paths,
            sample_count,
            explicit_num_speakers,
            Arc::clone(&samples),
            transcript_segments,
            preserve_existing_labels,
            progress_percentage,
            Some(embedding_name),
        )
        .await
        {
            Ok(candidate) => candidate,
            Err(error) => {
                log::warn!(
                    "speaker_diarization_embedding_retry_failed meeting_id={} embedding_model_id={} error={}",
                    meeting_id,
                    embedding_model_id,
                    error
                );
                continue;
            }
        };

        if is_better_explicit_mapping_attempt(expected_speakers, &candidate, &best_attempt) {
            best_attempt = candidate.clone();
        }

        if !explicit_speaker_mapping_needs_embedding_retry(expected_speakers, &candidate.quality) {
            attempt = candidate;
            break;
        }
    }

    if explicit_speaker_mapping_needs_embedding_retry(expected_speakers, &attempt.quality)
        && is_better_explicit_mapping_attempt(expected_speakers, &best_attempt, &attempt)
    {
        attempt = best_attempt;
    }

    Ok(attempt)
}

fn is_better_explicit_mapping_attempt(
    expected_speakers: usize,
    candidate: &DiarizationMappingAttempt,
    current: &DiarizationMappingAttempt,
) -> bool {
    let candidate_meets_count = candidate.quality.speaker_count >= expected_speakers;
    let current_meets_count = current.quality.speaker_count >= expected_speakers;
    if candidate_meets_count != current_meets_count {
        return candidate_meets_count;
    }

    if candidate.quality.speaker_count != current.quality.speaker_count {
        return candidate.quality.speaker_count > current.quality.speaker_count;
    }

    let candidate_has_weak_speaker =
        explicit_speaker_mapping_has_weak_requested_speaker(expected_speakers, &candidate.quality);
    let current_has_weak_speaker =
        explicit_speaker_mapping_has_weak_requested_speaker(expected_speakers, &current.quality);
    if candidate_has_weak_speaker != current_has_weak_speaker {
        return !candidate_has_weak_speaker;
    }

    if candidate_has_weak_speaker && current_has_weak_speaker {
        let candidate_min = candidate.quality.min_speaker_seconds.unwrap_or(0.0);
        let current_min = current.quality.min_speaker_seconds.unwrap_or(0.0);
        if (candidate_min - current_min).abs() > FLOAT_TIE_EPSILON {
            return candidate_min > current_min;
        }
    }

    candidate.quality.dominant_share + FLOAT_TIE_EPSILON < current.quality.dominant_share
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct DiarizationEvaluationOptions {
    pub meeting_id: String,
    pub meeting_folder_path: String,
    pub embedding_model_id: Option<String>,
    pub clustering_threshold: Option<f32>,
    pub num_speakers: Option<i32>,
    pub output_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct DiarizationEvaluationArtifacts {
    pub profile_path: PathBuf,
    pub turns_path: PathBuf,
    pub speaker_count: usize,
    pub turn_count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct DiarizationEvaluationTurnsFile {
    meeting_id: String,
    created_at: String,
    embedding_model_id: Option<String>,
    clustering_threshold: f32,
    num_speakers: Option<i32>,
    speaker_count: usize,
    turns: Vec<DiarizationTurn>,
}

#[allow(dead_code)]
pub(crate) async fn run_speaker_diarization_evaluation<R: Runtime>(
    app: AppHandle<R>,
    options: DiarizationEvaluationOptions,
) -> Result<DiarizationEvaluationArtifacts> {
    let folder_path = PathBuf::from(&options.meeting_folder_path);
    if !folder_path.is_dir() {
        return Err(anyhow!(
            "Meeting folder is not available: {}",
            folder_path.display()
        ));
    }

    let audio_path =
        crate::audio::incremental_saver::find_or_recover_audio_file(&folder_path).await?;
    let model_paths =
        resolve_model_paths_for_embedding(&app, None, None, options.embedding_model_id.clone())?;

    ensure_model_available(
        &app,
        &options.meeting_id,
        &model_paths.segmentation_model,
        model_paths.segmentation_descriptor,
        "segmentation",
        model_paths.can_download_segmentation,
    )
    .await?;
    ensure_model_available(
        &app,
        &options.meeting_id,
        &model_paths.embedding_model,
        model_paths.embedding_descriptor,
        "embedding",
        model_paths.can_download_embedding,
    )
    .await?;

    let decode_path = audio_path.clone();
    let decoded = tokio::task::spawn_blocking(move || decode_audio_file(&decode_path))
        .await
        .map_err(|e| anyhow!("Audio decode task failed: {}", e))??;
    let samples = tokio::task::spawn_blocking(move || decoded.to_whisper_format())
        .await
        .map_err(|e| anyhow!("Audio preparation task failed: {}", e))?;
    if samples.is_empty() {
        return Err(anyhow!("Meeting audio did not contain decodable samples"));
    }

    let sample_count = samples.len();
    let samples = Arc::new(samples);
    let explicit_num_speakers = options.num_speakers.filter(|value| *value > 0);
    let clustering_threshold = options
        .clustering_threshold
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or_else(|| default_clustering_threshold(&model_paths));
    let mut profile = DiarizationProfile::new(
        &options.meeting_id,
        sample_count,
        explicit_num_speakers,
        clustering_threshold,
        &model_paths,
    );

    let provider = select_diarization_provider(
        &app,
        &options.meeting_id,
        &model_paths,
        explicit_num_speakers,
        clustering_threshold,
        Arc::clone(&samples),
        &mut profile,
    )
    .await?;
    let config = diarization_config_for_provider(
        &model_paths,
        explicit_num_speakers,
        provider,
        clustering_threshold,
    );
    let turns = run_sherpa_diarization_with_fallback(
        &app,
        &options.meeting_id,
        config,
        samples,
        &mut profile,
    )
    .await?;

    let output_dir = match options.output_dir {
        Some(output_dir) => output_dir,
        None => diarization_log_dir(&app)?,
    };
    fs::create_dir_all(&output_dir)?;
    let safe_meeting_id = sanitize_profile_file_stem(&options.meeting_id);
    let created_at = Utc::now();
    let turns_path = output_dir.join(format!(
        "{}-{}-turns.json",
        safe_meeting_id,
        created_at.format("%Y%m%dT%H%M%SZ")
    ));
    let speaker_count = speaker_count_from_turns(&turns);
    let turn_count = turns.len();
    let turns_file = DiarizationEvaluationTurnsFile {
        meeting_id: options.meeting_id,
        created_at: created_at.to_rfc3339(),
        embedding_model_id: model_paths
            .embedding_descriptor
            .map(|descriptor| descriptor.id.to_string()),
        clustering_threshold,
        num_speakers: explicit_num_speakers,
        speaker_count,
        turns,
    };
    fs::write(&turns_path, serde_json::to_vec_pretty(&turns_file)?)?;
    let profile_path = write_diarization_profile_to_dir(&output_dir, &profile)?;

    Ok(DiarizationEvaluationArtifacts {
        profile_path,
        turns_path,
        speaker_count,
        turn_count,
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

fn transcript_speaker_count(segments: &[TranscriptSegment]) -> usize {
    segments
        .iter()
        .filter_map(|segment| segment.speaker.as_deref())
        .filter(|speaker| !speaker.trim().is_empty())
        .collect::<BTreeSet<_>>()
        .len()
}

fn mapped_diarization_quality(segments: &[TranscriptSegment]) -> MappedDiarizationQuality {
    let mut seconds_by_speaker = BTreeMap::<String, f64>::new();
    for segment in segments {
        let Some(speaker) = segment
            .speaker
            .as_deref()
            .map(|speaker| speaker.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|speaker| !speaker.is_empty())
        else {
            continue;
        };

        let seconds = segment
            .duration
            .or_else(|| Some((segment.audio_end_time? - segment.audio_start_time?).max(0.0)))
            .unwrap_or(0.0);
        if seconds > 0.0 {
            *seconds_by_speaker.entry(speaker).or_insert(0.0) += seconds;
        }
    }

    let labelled_seconds = seconds_by_speaker.values().sum::<f64>();
    let min_speaker_seconds = seconds_by_speaker
        .values()
        .copied()
        .min_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let (dominant_speaker, dominant_seconds) = seconds_by_speaker
        .iter()
        .max_by(|left, right| {
            left.1
                .partial_cmp(right.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.0.cmp(left.0))
        })
        .map(|(speaker, seconds)| (Some(speaker.clone()), *seconds))
        .unwrap_or((None, 0.0));
    let dominant_share = if labelled_seconds > 0.0 {
        dominant_seconds / labelled_seconds
    } else {
        0.0
    };

    MappedDiarizationQuality {
        speaker_count: seconds_by_speaker.len(),
        labelled_seconds,
        dominant_speaker,
        dominant_seconds,
        dominant_share,
        min_speaker_seconds,
    }
}

fn auto_diarization_low_confidence_reason(attempt: &DiarizationMappingAttempt) -> Option<String> {
    let turn_quality = turn_speaker_quality(&attempt.turns)?;
    if turn_quality.total_speakers <= 1 {
        return None;
    }

    if turn_quality.total_speakers > AUTO_MAX_CONFIDENT_SPEAKER_LANES
        && turn_quality.total_speakers > turn_quality.significant_speakers + 2
    {
        return Some(format!(
            "auto-detect produced {} speaker lanes, but only {} had meaningful duration ({} micro-lanes below {:.1}s)",
            turn_quality.total_speakers,
            turn_quality.significant_speakers,
            turn_quality.micro_speakers,
            turn_quality.significant_threshold_seconds
        ));
    }

    if attempt.quality.speaker_count > AUTO_MAX_CONFIDENT_SPEAKER_LANES
        && attempt.quality.speaker_count > turn_quality.significant_speakers + 2
    {
        return Some(format!(
            "auto-detect mapped {} transcript speakers from only {} meaningful diarization lane{}",
            attempt.quality.speaker_count,
            turn_quality.significant_speakers,
            if turn_quality.significant_speakers == 1 {
                ""
            } else {
                "s"
            }
        ));
    }

    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AutoDiarizationRecovery {
    speaker_count: i32,
    reason: String,
}

fn auto_diarization_recovery_target(
    attempt: &DiarizationMappingAttempt,
) -> Option<AutoDiarizationRecovery> {
    let reason = auto_diarization_low_confidence_reason(attempt)?;
    let turn_quality = turn_speaker_quality(&attempt.turns)?;
    let meaningful_speakers = turn_quality.significant_speakers;
    if !(2..=AUTO_MAX_CONFIDENT_SPEAKER_LANES).contains(&meaningful_speakers) {
        return None;
    }

    let speaker_count = i32::try_from(meaningful_speakers).ok()?;
    Some(AutoDiarizationRecovery {
        speaker_count,
        reason,
    })
}

#[derive(Debug, Clone)]
struct TurnSpeakerQuality {
    total_speakers: usize,
    significant_speakers: usize,
    micro_speakers: usize,
    significant_threshold_seconds: f64,
}

fn turn_speaker_quality(turns: &[DiarizationTurn]) -> Option<TurnSpeakerQuality> {
    let mut seconds_by_speaker = BTreeMap::<usize, f64>::new();
    for turn in turns {
        if !is_valid_interval(turn.start_time, turn.end_time) {
            continue;
        }
        *seconds_by_speaker.entry(turn.speaker).or_insert(0.0) += turn.end_time - turn.start_time;
    }

    let total_speakers = seconds_by_speaker.len();
    if total_speakers == 0 {
        return None;
    }
    let dominant_seconds = seconds_by_speaker.values().copied().fold(0.0f64, f64::max);
    let significant_threshold_seconds = AUTO_SIGNIFICANT_SPEAKER_MIN_SECONDS
        .min(dominant_seconds * AUTO_SIGNIFICANT_SPEAKER_DOMINANT_SHARE);
    let significant_speakers = seconds_by_speaker
        .values()
        .filter(|seconds| **seconds >= significant_threshold_seconds)
        .count();

    Some(TurnSpeakerQuality {
        total_speakers,
        significant_speakers,
        micro_speakers: total_speakers.saturating_sub(significant_speakers),
        significant_threshold_seconds,
    })
}

fn explicit_speaker_mapping_is_collapsed(
    expected_speakers: usize,
    quality: &MappedDiarizationQuality,
) -> bool {
    expected_speakers > 1 && quality.speaker_count < expected_speakers
}

fn explicit_speaker_mapping_has_weak_requested_speaker(
    expected_speakers: usize,
    quality: &MappedDiarizationQuality,
) -> bool {
    expected_speakers > 1
        && quality.speaker_count >= expected_speakers
        && quality
            .min_speaker_seconds
            .map(|seconds| seconds < MIN_EXPLICIT_RETRY_SPEAKER_SECONDS)
            .unwrap_or(true)
}

fn explicit_speaker_mapping_needs_embedding_retry(
    expected_speakers: usize,
    quality: &MappedDiarizationQuality,
) -> bool {
    explicit_speaker_mapping_is_collapsed(expected_speakers, quality)
        || explicit_speaker_mapping_has_weak_requested_speaker(expected_speakers, quality)
}

fn explicit_mapping_failure_message(
    expected_speakers: usize,
    prepared_speaker_count: usize,
    quality: &MappedDiarizationQuality,
) -> String {
    let dominant = quality
        .dominant_speaker
        .as_deref()
        .map(|speaker| {
            format!(
                "{speaker} has {:.1}s ({:.1}%)",
                quality.dominant_seconds,
                quality.dominant_share * 100.0
            )
        })
        .unwrap_or_else(|| "no dominant speaker was mapped".to_string());

    format!(
        "Speaker diarization was asked for {expected_speakers} speakers, but the best mapping produced {} speaker label{} from {prepared_speaker_count} diarization lane{} ({dominant}). No labels were saved because this result looks collapsed. Try another speaker count or retranscribe with word timestamps before rerunning diarization.",
        quality.speaker_count,
        if quality.speaker_count == 1 { "" } else { "s" },
        if prepared_speaker_count == 1 { "" } else { "s" },
    )
}

fn auto_mapping_failure_message(
    reason: &str,
    prepared_speaker_count: usize,
    quality: &MappedDiarizationQuality,
) -> String {
    format!(
        "Auto speaker detection looked unreliable: {reason}. It produced {prepared_speaker_count} diarization lane{} and mapped {} transcript speaker label{}. No new labels were saved; choose an explicit speaker count or retranscribe before retrying.",
        if prepared_speaker_count == 1 { "" } else { "s" },
        quality.speaker_count,
        if quality.speaker_count == 1 { "" } else { "s" },
    )
}

fn diarization_mapping_has_zero_mapped_speakers(attempt: &DiarizationMappingAttempt) -> bool {
    attempt.quality.speaker_count == 0
}

fn zero_speaker_mapping_failure_message(prepared_speaker_count: usize) -> String {
    format!(
        "Speaker diarization detected {prepared_speaker_count} diarization lane{} but could not map any lane onto transcript text. No labels were saved. Retranscribe to add word timestamps, or choose an explicit speaker count and retry.",
        if prepared_speaker_count == 1 { "" } else { "s" }
    )
}

fn fallback_embedding_model_ids(
    current_id: Option<&str>,
    allow_zh_cn_fallback: bool,
) -> Vec<&'static str> {
    let mut ids = vec![
        "3dspeaker-campplus-en",
        "3dspeaker-eres2net-en",
        "nemo-titanet-small-en",
        DEFAULT_EMBEDDING_MODEL_ID,
    ];
    if allow_zh_cn_fallback {
        ids.push(ZH_CN_EMBEDDING_MODEL_ID);
    }
    if let Some(current_id) = current_id {
        ids.retain(|id| *id != current_id);
    }
    ids
}

fn optional_seconds_equal(left: Option<f64>, right: Option<f64>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => (left - right).abs() <= FLOAT_TIE_EPSILON,
        (None, None) => true,
        _ => false,
    }
}

fn profile_model_metadata(
    path: &Path,
    descriptor: Option<&DiarizationModelDescriptor>,
    kind: DiarizationModelKind,
    custom_path: bool,
) -> DiarizationProfileModel {
    match descriptor {
        Some(descriptor) => DiarizationProfileModel {
            kind: descriptor.kind.as_str().to_string(),
            id: Some(descriptor.id.to_string()),
            display_name: descriptor.display_name.to_string(),
            family: Some(descriptor.family.to_string()),
            language: descriptor.language.map(ToOwned::to_owned),
            path: path.display().to_string(),
            source_file: Some(descriptor.source_file.to_string()),
            download_url: Some(descriptor.download_url.to_string()),
            expected_sha256: descriptor.expected_sha256.map(ToOwned::to_owned),
            expected_bytes: descriptor.expected_bytes,
            is_default: descriptor.is_default,
            custom_path,
        },
        None => DiarizationProfileModel {
            kind: kind.as_str().to_string(),
            id: None,
            display_name: "Custom model path".to_string(),
            family: None,
            language: None,
            path: path.display().to_string(),
            source_file: None,
            download_url: None,
            expected_sha256: None,
            expected_bytes: None,
            is_default: false,
            custom_path,
        },
    }
}

impl DiarizationProfile {
    fn new(
        meeting_id: &str,
        sample_count: usize,
        explicit_num_speakers: Option<i32>,
        clustering_threshold: f32,
        model_paths: &DiarizationModelPaths,
    ) -> Self {
        let preferred_provider = preferred_diarization_provider().to_string();
        let profile = Self {
            meeting_id: meeting_id.to_string(),
            audio_seconds: sample_count as f64 / DIARIZATION_SAMPLE_RATE as f64,
            sample_count,
            num_threads: default_diarization_threads(),
            explicit_num_speakers,
            clustering_threshold,
            segmentation_model: profile_model_metadata(
                &model_paths.segmentation_model,
                model_paths.segmentation_descriptor,
                DiarizationModelKind::Segmentation,
                !model_paths.can_download_segmentation,
            ),
            embedding_model: profile_model_metadata(
                &model_paths.embedding_model,
                model_paths.embedding_descriptor,
                DiarizationModelKind::Embedding,
                !model_paths.can_download_embedding,
            ),
            directml_feature: cfg!(all(target_os = "windows", feature = "directml")),
            preferred_provider_before_probe: preferred_provider,
            selected_provider: None,
            decision: None,
            runtime_dlls: collect_sherpa_runtime_dlls(),
            mapping_diagnostics: None,
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

    fn set_mapping_diagnostics(&mut self, diagnostics: DiarizationMappingDiagnostics) {
        self.mapping_diagnostics = Some(diagnostics);
        log_profile_snapshot("mapping", self);
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

pub fn speaker_diarization_runtime_status() -> SpeakerDiarizationRuntimeStatus {
    SpeakerDiarizationRuntimeStatus {
        in_progress: DIARIZATION_IN_PROGRESS.load(Ordering::SeqCst),
        directml_compiled: cfg!(all(target_os = "windows", feature = "directml")),
        directml_fast: DIARIZATION_DIRECTML_FAST.load(Ordering::SeqCst),
        directml_slow: DIARIZATION_DIRECTML_SLOW.load(Ordering::SeqCst),
        directml_unavailable: DIARIZATION_DIRECTML_UNAVAILABLE.load(Ordering::SeqCst),
        preferred_provider: preferred_diarization_provider().to_string(),
        runtime_dlls: collect_sherpa_runtime_dlls(),
    }
}

fn diarization_log_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| anyhow!("Failed to resolve app data directory: {error}"))?
        .join("logs")
        .join("diarization"))
}

fn write_diarization_profile_result<R: Runtime>(
    app: &AppHandle<R>,
    profile: &DiarizationProfile,
) -> Result<PathBuf> {
    let profile_dir = diarization_log_dir(app)?;
    write_diarization_profile_to_dir(&profile_dir, profile)
}

fn write_diarization_profile_to_dir(
    profile_dir: &Path,
    profile: &DiarizationProfile,
) -> Result<PathBuf> {
    fs::create_dir_all(profile_dir)?;
    let safe_meeting_id = sanitize_profile_file_stem(&profile.meeting_id);
    let file_name = format!(
        "{}-{}.json",
        safe_meeting_id,
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    let path = profile_dir.join(file_name);
    fs::write(&path, serde_json::to_vec_pretty(profile)?)?;
    Ok(path)
}

fn write_diarization_profile<R: Runtime>(app: &AppHandle<R>, profile: &DiarizationProfile) {
    match write_diarization_profile_result(app, profile) {
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
    clustering_threshold: f32,
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

    if DIARIZATION_DIRECTML_FAST.load(Ordering::SeqCst) {
        profile.set_decision(
            "directml",
            "DirectML probe was already faster earlier in this app session",
        );
        return Ok("directml");
    }

    emit_progress(
        app,
        meeting_id,
        "checking_directml",
        40,
        "Checking whether DirectML accelerates speaker detection...",
    );

    let probe_samples = Arc::new(diarization_probe_samples(samples.as_ref().as_slice()));
    let directml_config = diarization_config_for_provider(
        model_paths,
        explicit_num_speakers,
        "directml",
        clustering_threshold,
    );
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
            DIARIZATION_DIRECTML_FAST.store(false, Ordering::SeqCst);
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

    let cpu_config = diarization_config_for_provider(
        model_paths,
        explicit_num_speakers,
        "cpu",
        clustering_threshold,
    );
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
            DIARIZATION_DIRECTML_FAST.store(true, Ordering::SeqCst);
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
        DIARIZATION_DIRECTML_FAST.store(true, Ordering::SeqCst);
        return Ok("directml");
    }

    DIARIZATION_DIRECTML_SLOW.store(true, Ordering::SeqCst);
    DIARIZATION_DIRECTML_FAST.store(false, Ordering::SeqCst);
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
    clustering_threshold: f32,
) -> SherpaDiarizationConfig {
    let mut config = SherpaDiarizationConfig::new(
        model_paths.segmentation_model.clone(),
        model_paths.embedding_model.clone(),
    );
    config.num_threads = default_diarization_threads();
    config.num_clusters = explicit_num_speakers;
    config.clustering_threshold = clustering_threshold;
    config.provider = provider.to_string();
    config.debug = sherpa_diarization_debug_enabled(provider);
    if explicit_num_speakers
        .filter(|speakers| *speakers > 1)
        .is_some()
    {
        config.min_duration_on = FIXED_COUNT_MIN_DURATION_ON_SECONDS;
        config.min_duration_off = FIXED_COUNT_MIN_DURATION_OFF_SECONDS;
    }
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
            DIARIZATION_DIRECTML_FAST.store(false, Ordering::SeqCst);
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

fn default_model_descriptor(
    catalog: &'static [DiarizationModelDescriptor],
    kind: DiarizationModelKind,
) -> &'static DiarizationModelDescriptor {
    catalog
        .iter()
        .find(|descriptor| descriptor.is_default)
        .unwrap_or_else(|| panic!("missing default {} diarization model", kind.as_str()))
}

fn model_descriptor_by_id(
    catalog: &'static [DiarizationModelDescriptor],
    id: &str,
) -> Option<&'static DiarizationModelDescriptor> {
    let id = id.trim();
    catalog.iter().find(|descriptor| {
        descriptor.id.eq_ignore_ascii_case(id)
            || descriptor.source_file.eq_ignore_ascii_case(id)
            || descriptor.display_name.eq_ignore_ascii_case(id)
    })
}

fn resolve_embedding_model_descriptor(
    embedding_model_id: Option<&str>,
) -> Result<&'static DiarizationModelDescriptor> {
    let Some(id) = embedding_model_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return Ok(default_model_descriptor(
            EMBEDDING_MODEL_CATALOG,
            DiarizationModelKind::Embedding,
        ));
    };

    model_descriptor_by_id(EMBEDDING_MODEL_CATALOG, id).ok_or_else(|| {
        anyhow!(
            "Unknown diarization embedding model id '{}'. Available ids: {}",
            id,
            EMBEDDING_MODEL_CATALOG
                .iter()
                .map(|descriptor| descriptor.id)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

fn default_clustering_threshold(model_paths: &DiarizationModelPaths) -> f32 {
    model_paths
        .embedding_descriptor
        .map(|descriptor| descriptor.default_clustering_threshold)
        .unwrap_or(DEFAULT_CLUSTERING_THRESHOLD)
}

fn resolve_embedding_model_id_for_meeting(
    folder_path: &Path,
    embedding_model_path: Option<&str>,
    embedding_model_id: Option<String>,
    stored_segments: &[StoredTranscriptSegment],
) -> Option<String> {
    let language_preference = crate::get_language_preference_internal();
    let transcription_source_language = match read_transcription_source_language_from_metadata(
        folder_path,
    ) {
        Ok(language) => language,
        Err(error) => {
            log::warn!("Failed to read transcription source language for diarization model selection: {error}");
            None
        }
    };
    let detected_summary_language = match read_detected_summary_language_from_metadata(folder_path)
    {
        Ok(language) => language,
        Err(error) => {
            log::warn!(
                "Failed to read cached summary language for diarization model selection: {error}"
            );
            None
        }
    };

    resolve_embedding_model_id_for_signals(
        embedding_model_path,
        embedding_model_id,
        transcription_source_language.as_deref(),
        language_preference.as_deref(),
        detected_summary_language.as_deref(),
        stored_segments,
    )
}

fn resolve_embedding_model_id_for_signals(
    embedding_model_path: Option<&str>,
    embedding_model_id: Option<String>,
    transcription_source_language: Option<&str>,
    language_preference: Option<&str>,
    detected_summary_language: Option<&str>,
    stored_segments: &[StoredTranscriptSegment],
) -> Option<String> {
    let explicit_model_id = embedding_model_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
    if explicit_model_id.is_some()
        || embedding_model_path
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .is_some()
    {
        return explicit_model_id;
    }

    if let Some(choice) = transcription_source_language.and_then(embedding_choice_for_language_code)
    {
        return choice.model_id();
    }

    if let Some(choice) = embedding_choice_for_language_preference(language_preference) {
        return choice.model_id();
    }

    if let Some(choice) = detected_summary_language.and_then(embedding_choice_for_language_code) {
        return choice.model_id();
    }

    infer_embedding_choice_from_transcript(stored_segments).and_then(|choice| choice.model_id())
}

fn infer_embedding_choice_from_transcript(
    stored_segments: &[StoredTranscriptSegment],
) -> Option<DiarizationEmbeddingChoice> {
    let transcript_texts = stored_segments
        .iter()
        .map(|segment| segment.transcript.clone())
        .collect::<Vec<_>>();
    let detection = detect_summary_language(&transcript_texts);

    detection
        .language
        .as_deref()
        .and_then(embedding_choice_for_language_code)
}

fn embedding_choice_for_language_preference(
    language_preference: Option<&str>,
) -> Option<DiarizationEmbeddingChoice> {
    let code = language_preference?
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-");
    if code.is_empty() || matches!(code.as_str(), "auto" | "auto-translate") {
        return None;
    }

    Some(embedding_choice_for_language_code(&code).unwrap_or(DiarizationEmbeddingChoice::Default))
}

fn embedding_choice_for_language_code(raw_code: &str) -> Option<DiarizationEmbeddingChoice> {
    let code = raw_code.trim().to_ascii_lowercase().replace('_', "-");
    if code.is_empty() {
        return None;
    }

    if language_code_is_chinese(&code) {
        Some(DiarizationEmbeddingChoice::Model(ZH_CN_EMBEDDING_MODEL_ID))
    } else {
        Some(DiarizationEmbeddingChoice::Default)
    }
}

fn language_code_is_chinese(code: &str) -> bool {
    matches!(
        code,
        "zh" | "zh-cn"
            | "zh-hans"
            | "zh-hant"
            | "zh-tw"
            | "cmn"
            | "cmn-cn"
            | "cmn-hans"
            | "cmn-hant"
            | "yue"
            | "yue-cn"
            | "yue-hk"
            | "cn"
    )
}

fn model_cache_path(models_dir: &Path, descriptor: &DiarizationModelDescriptor) -> PathBuf {
    models_dir
        .join(descriptor.cache_dir)
        .join(descriptor.cache_file)
}

fn catalog_model_candidate_paths(
    models_dir: &Path,
    descriptor: &DiarizationModelDescriptor,
) -> Vec<PathBuf> {
    let mut paths = vec![model_cache_path(models_dir, descriptor)];
    if let Some(legacy_flat_file) = descriptor.legacy_flat_file {
        paths.push(models_dir.join(legacy_flat_file));
    }
    paths
}

fn first_existing_catalog_path(
    models_dir: &Path,
    descriptor: &'static DiarizationModelDescriptor,
) -> Option<PathBuf> {
    first_existing_path(&catalog_model_candidate_paths(models_dir, descriptor))
}

fn resolve_segmentation_catalog_path(
    models_dir: &Path,
) -> (PathBuf, &'static DiarizationModelDescriptor) {
    for descriptor in SEGMENTATION_MODEL_CATALOG {
        if let Some(path) = first_existing_catalog_path(models_dir, descriptor) {
            return (path, descriptor);
        }
    }

    let descriptor = default_model_descriptor(
        SEGMENTATION_MODEL_CATALOG,
        DiarizationModelKind::Segmentation,
    );
    (model_cache_path(models_dir, descriptor), descriptor)
}

fn resolve_embedding_catalog_path(
    models_dir: &Path,
    descriptor: &'static DiarizationModelDescriptor,
) -> PathBuf {
    first_existing_catalog_path(models_dir, descriptor)
        .unwrap_or_else(|| model_cache_path(models_dir, descriptor))
}

fn resolve_model_paths_for_embedding<R: Runtime>(
    app: &AppHandle<R>,
    segmentation_model_path: Option<String>,
    embedding_model_path: Option<String>,
    embedding_model_id: Option<String>,
) -> Result<DiarizationModelPaths> {
    let models_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| anyhow!("Failed to resolve app data directory: {}", e))?
        .join("models")
        .join("diarization");

    resolve_model_paths_in_dir(
        &models_dir,
        segmentation_model_path.as_deref(),
        embedding_model_path.as_deref(),
        embedding_model_id.as_deref(),
    )
}

fn resolve_model_paths_in_dir(
    models_dir: &Path,
    segmentation_model_path: Option<&str>,
    embedding_model_path: Option<&str>,
    embedding_model_id: Option<&str>,
) -> Result<DiarizationModelPaths> {
    let segmentation_custom_path = segmentation_model_path
        .map(str::trim)
        .filter(|path| !path.is_empty());
    let (segmentation_model, segmentation_descriptor, can_download_segmentation): (
        PathBuf,
        Option<&'static DiarizationModelDescriptor>,
        bool,
    ) = match segmentation_custom_path {
        Some(path) => {
            let path = PathBuf::from(path);
            ensure_model_file(&path, "segmentation")?;
            (path, None, false)
        }
        None => {
            let (path, descriptor) = resolve_segmentation_catalog_path(models_dir);
            (path, Some(descriptor), true)
        }
    };

    let embedding_custom_path = embedding_model_path
        .map(str::trim)
        .filter(|path| !path.is_empty());
    let (embedding_model, embedding_descriptor, can_download_embedding): (
        PathBuf,
        Option<&'static DiarizationModelDescriptor>,
        bool,
    ) = match embedding_custom_path {
        Some(path) => {
            let path = PathBuf::from(path);
            ensure_model_file(&path, "embedding")?;
            (path, None, false)
        }
        None => {
            let descriptor = resolve_embedding_model_descriptor(embedding_model_id)?;
            (
                resolve_embedding_catalog_path(models_dir, descriptor),
                Some(descriptor),
                true,
            )
        }
    };

    Ok(DiarizationModelPaths {
        segmentation_model,
        embedding_model,
        segmentation_descriptor,
        embedding_descriptor,
        can_download_segmentation,
        can_download_embedding,
    })
}

async fn ensure_model_available<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    model_path: &Path,
    descriptor: Option<&DiarizationModelDescriptor>,
    model_name: &str,
    allow_download: bool,
) -> Result<()> {
    if model_path.is_file() {
        if !allow_download {
            return Ok(());
        }

        let descriptor = descriptor.ok_or_else(|| {
            anyhow!(
                "{} diarization model has no descriptor for validation: {}",
                model_name,
                model_path.display()
            )
        })?;
        let validation = validate_model_file(model_path, descriptor)?;
        if validation.is_valid() {
            return Ok(());
        }

        let validation_error = validation.error_message(descriptor, model_path);
        log::warn!("{validation_error}");
        match quarantine_invalid_default_model(model_path)? {
            Some(quarantine_path) => log::warn!(
                "Quarantined invalid {} diarization model at {}",
                model_name,
                quarantine_path.display()
            ),
            None => log::warn!(
                "Deleted invalid {} diarization model at {}",
                model_name,
                model_path.display()
            ),
        }
    }

    if !allow_download {
        ensure_model_file(model_path, model_name)?;
    }

    let descriptor = descriptor.ok_or_else(|| {
        anyhow!(
            "{} diarization model cannot be downloaded without a descriptor: {}",
            model_name,
            model_path.display()
        )
    })?;
    let download_url = descriptor.download_url;

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
    drop(file);

    if downloaded_bytes == 0 {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(anyhow!(
            "{} diarization model download was empty: {}",
            model_name,
            download_url
        ));
    }

    let validation = validate_model_file(&temp_path, descriptor)?;
    if !validation.is_valid() {
        let validation_error = validation.error_message(descriptor, &temp_path);
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(anyhow!(
            "Downloaded {} diarization model failed validation: {}",
            model_name,
            validation_error
        ));
    }

    tokio::fs::rename(&temp_path, model_path).await?;

    Ok(())
}

fn first_existing_path(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|path| path.is_file()).cloned()
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
            word_timestamps: None,
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

    fn transcript_with_word_timestamps(
        id: &str,
        words: &[(&str, f64, f64)],
        start: f64,
        end: f64,
    ) -> TranscriptSegment {
        let text = words
            .iter()
            .map(|(word, _, _)| *word)
            .collect::<Vec<_>>()
            .join(" ");
        let mut segment = transcript_with_text(id, &text, Some(start), Some(end));
        segment.word_timestamps = Some(
            words
                .iter()
                .map(|(word, start, end)| TranscriptWord {
                    text: (*word).to_string(),
                    start: *start,
                    end: *end,
                    confidence: None,
                    speaker: None,
                    timestamp_source: Some(TranscriptWordTimestampSource::Real),
                })
                .collect(),
        );
        segment
    }

    fn stored_transcript(text: &str) -> StoredTranscriptSegment {
        StoredTranscriptSegment {
            id: Uuid::new_v4().to_string(),
            transcript: text.to_string(),
            timestamp: "2026-06-22T12:00:00Z".to_string(),
            audio_start_time: Some(0.0),
            audio_end_time: Some(1.0),
            duration: Some(1.0),
            speaker: None,
            word_timestamps_json: None,
        }
    }

    fn turn(start: f64, end: f64, speaker: usize) -> DiarizationTurn {
        DiarizationTurn {
            start_time: start,
            end_time: end,
            speaker,
        }
    }

    fn test_model_paths() -> DiarizationModelPaths {
        DiarizationModelPaths {
            segmentation_model: PathBuf::from("segmentation.onnx"),
            embedding_model: PathBuf::from("embedding.onnx"),
            segmentation_descriptor: None,
            embedding_descriptor: None,
            can_download_segmentation: false,
            can_download_embedding: false,
        }
    }

    fn test_mapping_attempt(
        turns: Vec<DiarizationTurn>,
        mapped_segments: Vec<TranscriptSegment>,
        prepared_speaker_count: usize,
    ) -> DiarizationMappingAttempt {
        let model_paths = test_model_paths();
        let quality = mapped_diarization_quality(&mapped_segments);
        DiarizationMappingAttempt {
            profile: DiarizationProfile::new(
                "meeting",
                sample_count_for_minutes(8),
                None,
                DEFAULT_CLUSTERING_THRESHOLD,
                &model_paths,
            ),
            model_paths,
            provider: "cpu",
            turns,
            mapped_segments,
            prepared_speaker_count,
            quality,
        }
    }

    fn sample_count_for_minutes(minutes: usize) -> usize {
        sample_count_for_seconds(60 * minutes)
    }

    fn sample_count_for_seconds(seconds: usize) -> usize {
        DIARIZATION_SAMPLE_RATE as usize * seconds
    }

    fn test_model_descriptor(
        expected_sha256: Option<&'static str>,
        expected_bytes: Option<u64>,
    ) -> DiarizationModelDescriptor {
        DiarizationModelDescriptor {
            kind: DiarizationModelKind::Embedding,
            id: "test-model",
            display_name: "Test model",
            family: "test",
            language: Some("en"),
            cache_dir: "test-model",
            cache_file: "model.onnx",
            source_file: "test-model.onnx",
            download_url: "https://example.invalid/test-model.onnx",
            expected_sha256,
            expected_bytes,
            default_clustering_threshold: DEFAULT_CLUSTERING_THRESHOLD,
            is_default: false,
            legacy_flat_file: None,
        }
    }

    #[test]
    fn sha256_file_hashes_known_bytes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("model.onnx");
        std::fs::write(&path, b"clawscribe").unwrap();

        assert_eq!(
            sha256_file(&path).unwrap(),
            "9c497a187dfd743f242cfd7508a95f41ca8c943d08e8cd51a018822f18e89068"
        );
    }

    #[test]
    fn validates_model_sha256_and_expected_bytes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("model.onnx");
        std::fs::write(&path, b"clawscribe").unwrap();
        let descriptor = test_model_descriptor(
            Some("9c497a187dfd743f242cfd7508a95f41ca8c943d08e8cd51a018822f18e89068"),
            Some(10),
        );

        let validation = validate_model_file(&path, &descriptor).unwrap();
        assert!(validation.is_valid());

        let bad_sha = test_model_descriptor(
            Some("0000000000000000000000000000000000000000000000000000000000000000"),
            Some(10),
        );
        let validation = validate_model_file(&path, &bad_sha).unwrap();
        assert!(!validation.sha256_matches);
        assert!(validation.bytes_match);

        let bad_bytes = test_model_descriptor(
            Some("9c497a187dfd743f242cfd7508a95f41ca8c943d08e8cd51a018822f18e89068"),
            Some(11),
        );
        let validation = validate_model_file(&path, &bad_bytes).unwrap();
        assert!(validation.sha256_matches);
        assert!(!validation.bytes_match);
    }

    #[test]
    fn diarization_catalog_defaults_to_english_embedding() {
        let descriptor = resolve_embedding_model_descriptor(None).unwrap();

        assert_eq!(descriptor.id, DEFAULT_EMBEDDING_MODEL_ID);
        assert!(descriptor.is_default);
        assert_eq!(
            EMBEDDING_MODEL_CATALOG
                .iter()
                .filter(|descriptor| descriptor.is_default)
                .count(),
            1
        );
        assert_eq!(descriptor.language, Some("en"));
        assert_eq!(descriptor.source_file, "wespeaker_en_voxceleb_CAM++.onnx");
        assert_eq!(
            descriptor.expected_sha256,
            Some("c46fad10b5f81e1aa4a60c162714208577093655076c5450f8c469e522ec54ef")
        );
        assert_eq!(
            descriptor.default_clustering_threshold,
            DEFAULT_CLUSTERING_THRESHOLD
        );
    }

    #[test]
    fn embedding_catalog_includes_ab_candidates_with_pinned_checksums() {
        let expected = [
            (
                "3dspeaker-eres2net-base-zh-cn",
                "3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx",
                "1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b",
                39_593_761u64,
                0.90f32,
            ),
            (
                "3dspeaker-campplus-en",
                "3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx",
                "357a834f702b80161e5b981182c038e18553c1f2ca752ed6cec2052365d4129b",
                29_596_978u64,
                DEFAULT_CLUSTERING_THRESHOLD,
            ),
            (
                "3dspeaker-eres2net-en",
                "3dspeaker_speech_eres2net_sv_en_voxceleb_16k.onnx",
                "c59158379255ad66e161679cca6af8d52d51e389e3224ab7d7a7baae295c2db5",
                26_485_263u64,
                DEFAULT_CLUSTERING_THRESHOLD,
            ),
            (
                "wespeaker-camplusplus-en",
                "wespeaker_en_voxceleb_CAM++.onnx",
                "c46fad10b5f81e1aa4a60c162714208577093655076c5450f8c469e522ec54ef",
                29_292_684u64,
                DEFAULT_CLUSTERING_THRESHOLD,
            ),
            (
                "nemo-titanet-small-en",
                "nemo_en_titanet_small.onnx",
                "ad4a1802485d8b34c722d2a9d04249662f2ece5d28a7a039063ca22f515a789e",
                40_257_283u64,
                DEFAULT_CLUSTERING_THRESHOLD,
            ),
        ];

        for (id, source_file, sha256, bytes, threshold) in expected {
            let descriptor = resolve_embedding_model_descriptor(Some(id)).unwrap();
            assert_eq!(descriptor.source_file, source_file);
            assert_eq!(descriptor.expected_sha256, Some(sha256));
            assert_eq!(descriptor.expected_bytes, Some(bytes));
            assert_eq!(descriptor.default_clustering_threshold, threshold);
        }
    }

    #[test]
    fn default_catalog_resolution_keeps_existing_cache_layout() {
        let temp_dir = tempfile::tempdir().unwrap();
        let paths = resolve_model_paths_in_dir(temp_dir.path(), None, None, None).unwrap();

        assert_eq!(
            paths.segmentation_model,
            temp_dir
                .path()
                .join(SEGMENTATION_MODEL_DIR)
                .join("model.int8.onnx")
        );
        assert_eq!(
            paths.embedding_model,
            temp_dir
                .path()
                .join(DEFAULT_EMBEDDING_MODEL_DIR)
                .join("model.onnx")
        );
        assert_eq!(
            paths
                .segmentation_descriptor
                .map(|descriptor| descriptor.id),
            Some(DEFAULT_SEGMENTATION_MODEL_ID)
        );
        assert_eq!(
            paths.embedding_descriptor.map(|descriptor| descriptor.id),
            Some(DEFAULT_EMBEDDING_MODEL_ID)
        );
        assert_eq!(
            default_clustering_threshold(&paths),
            DEFAULT_CLUSTERING_THRESHOLD
        );
        assert!(paths.can_download_segmentation);
        assert!(paths.can_download_embedding);
    }

    #[test]
    fn speaker_embedding_model_uses_transcription_source_language_before_current_preference() {
        let segments = vec![stored_transcript(
            "我们今天讨论项目进展、后续行动和会议记录，需要区分不同发言人的贡献。",
        )];

        assert_eq!(
            resolve_embedding_model_id_for_signals(
                None,
                None,
                Some("zh"),
                Some("en"),
                Some("en"),
                &segments,
            )
            .as_deref(),
            Some(ZH_CN_EMBEDDING_MODEL_ID)
        );
        assert_eq!(
            resolve_embedding_model_id_for_signals(
                None,
                None,
                Some("en"),
                Some("zh"),
                Some("zh"),
                &segments,
            ),
            None
        );
    }

    #[test]
    fn speaker_embedding_model_uses_language_preference_before_text_detection() {
        let segments = vec![stored_transcript(
            "我们今天讨论项目进展、后续行动和会议记录，需要区分不同发言人的贡献。",
        )];

        assert_eq!(
            resolve_embedding_model_id_for_signals(
                None,
                None,
                None,
                Some("zh"),
                Some("en"),
                &segments,
            )
            .as_deref(),
            Some(ZH_CN_EMBEDDING_MODEL_ID)
        );
        assert_eq!(
            resolve_embedding_model_id_for_signals(
                None,
                None,
                None,
                Some("en"),
                Some("zh"),
                &segments,
            ),
            None
        );
    }

    #[test]
    fn speaker_embedding_model_uses_cached_summary_language_before_text_detection() {
        let segments = vec![stored_transcript(
            "Participants discussed campaign finance, defense contracts, follow-up actions, and open questions for the next meeting.",
        )];

        assert_eq!(
            resolve_embedding_model_id_for_signals(None, None, None, None, Some("zh"), &segments)
                .as_deref(),
            Some(ZH_CN_EMBEDDING_MODEL_ID)
        );
        assert_eq!(
            resolve_embedding_model_id_for_signals(None, None, None, None, Some("en"), &segments),
            None
        );
    }

    #[test]
    fn speaker_embedding_model_falls_back_to_shared_text_detection() {
        let chinese_segments = vec![stored_transcript(
            "我们今天讨论项目进展、后续行动和会议记录，需要区分不同发言人的贡献。",
        )];
        let english_segments = vec![stored_transcript(
            "Participants discussed campaign finance, defense contracts, follow-up actions, and open questions for the next meeting.",
        )];

        assert_eq!(
            infer_embedding_choice_from_transcript(&chinese_segments),
            Some(DiarizationEmbeddingChoice::Model(ZH_CN_EMBEDDING_MODEL_ID))
        );
        assert_eq!(
            resolve_embedding_model_id_for_signals(
                None,
                None,
                None,
                None,
                None,
                &chinese_segments,
            )
            .as_deref(),
            Some(ZH_CN_EMBEDDING_MODEL_ID)
        );
        assert_eq!(
            infer_embedding_choice_from_transcript(&english_segments),
            Some(DiarizationEmbeddingChoice::Default)
        );
        assert_eq!(
            resolve_embedding_model_id_for_signals(None, None, None, None, None, &english_segments,),
            None
        );
    }

    #[test]
    fn auto_language_preferences_do_not_force_embedding_model() {
        let segments = vec![stored_transcript(
            "我们今天讨论项目进展、后续行动和会议记录，需要区分不同发言人的贡献。",
        )];

        assert_eq!(
            resolve_embedding_model_id_for_signals(None, None, None, Some("auto"), None, &segments)
                .as_deref(),
            Some(ZH_CN_EMBEDDING_MODEL_ID)
        );
        assert_eq!(
            resolve_embedding_model_id_for_signals(
                None,
                None,
                None,
                Some("auto-translate"),
                None,
                &segments,
            )
            .as_deref(),
            Some(ZH_CN_EMBEDDING_MODEL_ID)
        );
    }

    #[test]
    fn explicit_embedding_model_overrides_language_signals() {
        let segments = vec![stored_transcript(
            "我们今天讨论项目进展、后续行动和会议记录，需要区分不同发言人的贡献。",
        )];

        assert_eq!(
            resolve_embedding_model_id_for_signals(
                None,
                Some("nemo-titanet-small-en".to_string()),
                Some("zh"),
                Some("zh"),
                Some("zh"),
                &segments,
            )
            .as_deref(),
            Some("nemo-titanet-small-en")
        );
        assert_eq!(
            resolve_embedding_model_id_for_signals(
                Some("C:/models/custom.onnx"),
                None,
                Some("zh"),
                Some("zh"),
                Some("zh"),
                &segments,
            ),
            None
        );
    }

    #[test]
    fn catalog_resolution_keeps_legacy_zh_cn_embedding_explicit() {
        let temp_dir = tempfile::tempdir().unwrap();
        let legacy_path = temp_dir
            .path()
            .join("3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx");
        std::fs::write(&legacy_path, b"legacy").unwrap();

        let paths =
            resolve_model_paths_in_dir(temp_dir.path(), None, None, Some(ZH_CN_EMBEDDING_MODEL_ID))
                .unwrap();

        assert_eq!(paths.embedding_model, legacy_path);
        assert_eq!(
            paths.embedding_descriptor.map(|descriptor| descriptor.id),
            Some(ZH_CN_EMBEDDING_MODEL_ID)
        );
        assert_eq!(default_clustering_threshold(&paths), 0.90);
    }

    #[test]
    fn explicit_custom_paths_are_existence_only() {
        let temp_dir = tempfile::tempdir().unwrap();
        let segmentation_path = temp_dir.path().join("custom-segmentation.onnx");
        let embedding_path = temp_dir.path().join("custom-embedding.onnx");
        std::fs::write(&segmentation_path, b"not a pinned segmentation model").unwrap();
        std::fs::write(&embedding_path, b"not a pinned embedding model").unwrap();

        let paths = resolve_model_paths_in_dir(
            temp_dir.path(),
            Some(segmentation_path.to_str().unwrap()),
            Some(embedding_path.to_str().unwrap()),
            Some("wespeaker-camplusplus-en"),
        )
        .unwrap();

        assert_eq!(paths.segmentation_model, segmentation_path);
        assert_eq!(paths.embedding_model, embedding_path);
        assert!(paths.segmentation_descriptor.is_none());
        assert!(paths.embedding_descriptor.is_none());
        assert!(!paths.can_download_segmentation);
        assert!(!paths.can_download_embedding);
    }

    #[test]
    fn invalid_default_cache_file_can_be_quarantined() {
        let temp_dir = tempfile::tempdir().unwrap();
        let model_dir = temp_dir.path().join(DEFAULT_EMBEDDING_MODEL_DIR);
        std::fs::create_dir_all(&model_dir).unwrap();
        let model_path = model_dir.join("model.onnx");
        std::fs::write(&model_path, b"bad model").unwrap();
        let descriptor = resolve_embedding_model_descriptor(None).unwrap();
        let validation = validate_model_file(&model_path, descriptor).unwrap();
        assert!(!validation.is_valid());

        let quarantine_path = quarantine_invalid_default_model(&model_path)
            .unwrap()
            .unwrap();

        assert!(!model_path.exists());
        assert!(quarantine_path.exists());
        assert!(quarantine_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(".invalid-"));
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
    fn mapping_preparation_preserves_auto_detected_speakers() {
        let turns = vec![
            turn(0.0, 13.0, 0),
            turn(13.0, 30.0, 1),
            turn(30.0, 35.0, 2),
            turn(35.0, 36.0, 1),
            turn(36.0, 41.0, 2),
            turn(41.0, 42.0, 3),
            turn(42.0, 48.0, 1),
            turn(48.0, 55.0, 4),
            turn(55.0, 57.0, 5),
            turn(57.0, 61.0, 6),
        ];

        let prepared =
            prepare_diarization_turns_for_mapping(sample_count_for_minutes(8), &turns, None);

        assert_eq!(speaker_count_from_turns(&prepared), 7);
        assert_eq!(
            prepared.iter().map(|turn| turn.speaker).collect::<Vec<_>>(),
            vec![0, 1, 2, 1, 2, 3, 1, 4, 5, 6]
        );
    }

    #[test]
    fn mapping_preparation_keeps_short_interjection_between_same_speaker() {
        let turns = vec![turn(0.0, 4.0, 0), turn(4.0, 4.5, 1), turn(4.5, 8.0, 0)];

        let prepared =
            prepare_diarization_turns_for_mapping(sample_count_for_minutes(2), &turns, Some(2));

        assert_eq!(prepared, turns);
    }

    #[test]
    fn mapping_preparation_resolves_overlapping_dominant_turns() {
        let turns = vec![turn(0.0, 30.0, 0), turn(10.0, 20.0, 1)];

        let prepared =
            prepare_diarization_turns_for_mapping(sample_count_for_minutes(1), &turns, Some(2));

        assert_eq!(
            prepared,
            vec![turn(0.0, 10.0, 0), turn(10.0, 20.0, 1), turn(20.0, 30.0, 0)]
        );

        let mapped = map_diarization_to_transcript_segments(
            &[transcript("interjection", Some(10.0), Some(20.0))],
            &prepared,
            DiarizationMappingOptions {
                existing_speaker_policy: ExistingSpeakerPolicy::Overwrite,
                ..Default::default()
            },
        );

        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 2"));
    }

    #[test]
    fn explicit_speaker_quality_rejects_missing_requested_speakers() {
        let mut segments = vec![
            transcript("a", Some(0.0), Some(450.0)),
            transcript("b", Some(450.0), Some(455.0)),
        ];
        segments[0].speaker = Some("Speaker 1".to_string());
        segments[1].speaker = Some("Speaker 2".to_string());

        let quality = mapped_diarization_quality(&segments);

        assert_eq!(quality.speaker_count, 2);
        assert!(quality.dominant_share > 0.98);
        assert!(explicit_speaker_mapping_is_collapsed(3, &quality));
        assert!(explicit_speaker_mapping_needs_embedding_retry(3, &quality));
    }

    #[test]
    fn explicit_speaker_quality_accepts_balanced_requested_speakers() {
        let mut segments = vec![
            transcript("a", Some(0.0), Some(180.0)),
            transcript("b", Some(180.0), Some(320.0)),
            transcript("c", Some(320.0), Some(455.0)),
        ];
        segments[0].speaker = Some("Speaker 1".to_string());
        segments[1].speaker = Some("Speaker 2".to_string());
        segments[2].speaker = Some("Speaker 3".to_string());

        let quality = mapped_diarization_quality(&segments);

        assert_eq!(quality.speaker_count, 3);
        assert!(!explicit_speaker_mapping_is_collapsed(3, &quality));
        assert!(!explicit_speaker_mapping_has_weak_requested_speaker(
            3, &quality
        ));
        assert!(!explicit_speaker_mapping_needs_embedding_retry(3, &quality));
    }

    #[test]
    fn explicit_speaker_quality_accepts_valid_dominant_requested_speakers() {
        let mut segments = vec![
            transcript("host", Some(0.0), Some(426.9)),
            transcript("guest", Some(426.9), Some(436.3)),
            transcript("jamie", Some(436.3), Some(445.6)),
        ];
        segments[0].speaker = Some("Speaker 1".to_string());
        segments[1].speaker = Some("Speaker 2".to_string());
        segments[2].speaker = Some("Speaker 3".to_string());

        let quality = mapped_diarization_quality(&segments);

        assert_eq!(quality.speaker_count, 3);
        assert!(quality.dominant_share > 0.95);
        assert!(!explicit_speaker_mapping_is_collapsed(3, &quality));
        assert!(!explicit_speaker_mapping_has_weak_requested_speaker(
            3, &quality
        ));
        assert!(!explicit_speaker_mapping_needs_embedding_retry(3, &quality));
    }

    #[test]
    fn explicit_speaker_quality_retries_near_empty_requested_speaker() {
        let mut segments = vec![
            transcript("host", Some(0.0), Some(437.0)),
            transcript("guest", Some(437.0), Some(459.1)),
            transcript("blip", Some(459.1), Some(459.5)),
        ];
        segments[0].speaker = Some("Speaker 1".to_string());
        segments[1].speaker = Some("Speaker 2".to_string());
        segments[2].speaker = Some("Speaker 3".to_string());

        let quality = mapped_diarization_quality(&segments);

        assert_eq!(quality.speaker_count, 3);
        assert!((quality.min_speaker_seconds.unwrap_or_default() - 0.4).abs() < FLOAT_TIE_EPSILON);
        assert!(!explicit_speaker_mapping_is_collapsed(3, &quality));
        assert!(explicit_speaker_mapping_has_weak_requested_speaker(
            3, &quality
        ));
        assert!(explicit_speaker_mapping_needs_embedding_retry(3, &quality));
    }

    #[test]
    fn explicit_mapping_attempt_prefers_non_weak_requested_speakers() {
        let weak = test_mapping_attempt(
            vec![
                turn(0.0, 437.0, 0),
                turn(437.0, 459.1, 1),
                turn(459.1, 459.5, 2),
            ],
            {
                let mut segments = vec![
                    transcript("host", Some(0.0), Some(437.0)),
                    transcript("guest", Some(437.0), Some(459.1)),
                    transcript("blip", Some(459.1), Some(459.5)),
                ];
                segments[0].speaker = Some("Speaker 1".to_string());
                segments[1].speaker = Some("Speaker 2".to_string());
                segments[2].speaker = Some("Speaker 3".to_string());
                segments
            },
            3,
        );
        let healthier = test_mapping_attempt(
            vec![
                turn(0.0, 426.9, 0),
                turn(426.9, 436.3, 1),
                turn(436.3, 445.6, 2),
            ],
            {
                let mut segments = vec![
                    transcript("host", Some(0.0), Some(426.9)),
                    transcript("guest", Some(426.9), Some(436.3)),
                    transcript("jamie", Some(436.3), Some(445.6)),
                ];
                segments[0].speaker = Some("Speaker 1".to_string());
                segments[1].speaker = Some("Speaker 2".to_string());
                segments[2].speaker = Some("Speaker 3".to_string());
                segments
            },
            3,
        );

        assert!(is_better_explicit_mapping_attempt(3, &healthier, &weak));
        assert!(!is_better_explicit_mapping_attempt(3, &weak, &healthier));
    }

    #[test]
    fn fallback_embedding_models_skip_current_default() {
        let ids = fallback_embedding_model_ids(Some(DEFAULT_EMBEDDING_MODEL_ID), false);

        assert!(!ids.contains(&DEFAULT_EMBEDDING_MODEL_ID));
        assert_eq!(ids.first().copied(), Some("3dspeaker-campplus-en"));
        assert!(ids.contains(&"nemo-titanet-small-en"));
        assert!(!ids.contains(&ZH_CN_EMBEDDING_MODEL_ID));
    }

    #[test]
    fn fallback_embedding_models_only_include_zh_cn_for_chinese_meetings() {
        let english_ids = fallback_embedding_model_ids(None, false);
        let chinese_ids = fallback_embedding_model_ids(None, true);

        assert!(!english_ids.contains(&ZH_CN_EMBEDDING_MODEL_ID));
        assert!(chinese_ids.contains(&ZH_CN_EMBEDDING_MODEL_ID));
    }

    #[test]
    fn fixed_speaker_count_uses_short_turn_duration_gates() {
        let model_paths = test_model_paths();

        let auto_config = diarization_config_for_provider(
            &model_paths,
            None,
            "cpu",
            DEFAULT_CLUSTERING_THRESHOLD,
        );
        assert_eq!(auto_config.min_duration_on, DEFAULT_MIN_DURATION_ON_SECONDS);
        assert_eq!(
            auto_config.min_duration_off,
            DEFAULT_MIN_DURATION_OFF_SECONDS
        );

        let fixed_config = diarization_config_for_provider(
            &model_paths,
            Some(3),
            "cpu",
            DEFAULT_CLUSTERING_THRESHOLD,
        );
        assert_eq!(
            fixed_config.min_duration_on,
            FIXED_COUNT_MIN_DURATION_ON_SECONDS
        );
        assert_eq!(
            fixed_config.min_duration_off,
            FIXED_COUNT_MIN_DURATION_OFF_SECONDS
        );
    }

    #[test]
    fn auto_diarization_quality_rejects_micro_cluster_overfragmentation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let model_paths = DiarizationModelPaths {
            segmentation_model: temp_dir.path().join("segmentation.onnx"),
            embedding_model: temp_dir.path().join("embedding.onnx"),
            segmentation_descriptor: None,
            embedding_descriptor: None,
            can_download_segmentation: false,
            can_download_embedding: false,
        };
        let mut mapped_segments = vec![
            transcript("a", Some(0.0), Some(90.0)),
            transcript("b", Some(90.0), Some(105.0)),
            transcript("c", Some(105.0), Some(120.0)),
            transcript("d", Some(120.0), Some(122.0)),
            transcript("e", Some(122.0), Some(124.0)),
            transcript("f", Some(124.0), Some(126.0)),
        ];
        for (index, segment) in mapped_segments.iter_mut().enumerate() {
            segment.speaker = Some(format!("Speaker {}", index + 1));
        }
        let quality = mapped_diarization_quality(&mapped_segments);
        let turns = vec![
            turn(0.0, 90.0, 0),
            turn(90.0, 105.0, 1),
            turn(105.0, 120.0, 2),
            turn(120.0, 121.0, 3),
            turn(121.0, 122.0, 4),
            turn(122.0, 123.0, 5),
            turn(123.0, 124.0, 6),
        ];
        let attempt = DiarizationMappingAttempt {
            profile: DiarizationProfile::new(
                "meeting",
                sample_count_for_minutes(8),
                None,
                DEFAULT_CLUSTERING_THRESHOLD,
                &model_paths,
            ),
            model_paths,
            provider: "cpu",
            turns,
            mapped_segments,
            prepared_speaker_count: 7,
            quality,
        };

        let reason = auto_diarization_low_confidence_reason(&attempt).unwrap();

        assert!(reason.contains("micro-lanes"));
        let recovery = auto_diarization_recovery_target(&attempt).unwrap();
        assert_eq!(recovery.speaker_count, 3);
        assert_eq!(recovery.reason, reason);
    }

    #[test]
    fn auto_diarization_quality_accepts_dominant_meaningful_speaker_split() {
        let mut mapped_segments = vec![
            transcript("dominant", Some(0.0), Some(100.0)),
            transcript("minor", Some(100.0), Some(107.0)),
        ];
        mapped_segments[0].speaker = Some("Speaker 1".to_string());
        mapped_segments[1].speaker = Some("Speaker 2".to_string());
        let attempt = test_mapping_attempt(
            vec![turn(0.0, 100.0, 0), turn(100.0, 107.0, 1)],
            mapped_segments,
            2,
        );

        assert!(attempt.quality.dominant_share > 0.92);
        assert_eq!(auto_diarization_low_confidence_reason(&attempt), None);
    }

    #[test]
    fn auto_diarization_recovery_does_not_snap_to_single_speaker() {
        let mut mapped_segments = (0..7)
            .map(|index| {
                transcript(
                    &format!("s{index}"),
                    Some(index as f64),
                    Some(index as f64 + 1.0),
                )
            })
            .collect::<Vec<_>>();
        for (index, segment) in mapped_segments.iter_mut().enumerate() {
            segment.speaker = Some(format!("Speaker {}", index + 1));
        }
        let attempt = test_mapping_attempt(
            vec![
                turn(0.0, 420.0, 0),
                turn(420.0, 421.0, 1),
                turn(421.0, 422.0, 2),
                turn(422.0, 423.0, 3),
                turn(423.0, 424.0, 4),
                turn(424.0, 425.0, 5),
                turn(425.0, 426.0, 6),
            ],
            mapped_segments,
            7,
        );

        assert!(auto_diarization_low_confidence_reason(&attempt).is_some());
        assert_eq!(auto_diarization_recovery_target(&attempt), None);
    }

    #[test]
    fn zero_speaker_mapping_is_failure_condition() {
        let attempt = test_mapping_attempt(
            vec![turn(0.0, 12.0, 0), turn(12.0, 24.0, 1)],
            vec![
                transcript("unmapped-a", Some(0.0), Some(12.0)),
                transcript("unmapped-b", Some(12.0), Some(24.0)),
            ],
            2,
        );

        assert_eq!(attempt.quality.speaker_count, 0);
        assert!(diarization_mapping_has_zero_mapped_speakers(&attempt));
        assert!(
            zero_speaker_mapping_failure_message(attempt.prepared_speaker_count)
                .contains("could not map any lane")
        );
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
    fn smooths_short_mid_phrase_speaker_island() {
        let mut segments = vec![
            transcript_with_text("a", "Citizens", Some(0.0), Some(2.0)),
            transcript_with_text("b", "united. This", Some(2.0), Some(2.5)),
            transcript_with_text("c", "is good, right?", Some(2.5), Some(5.0)),
        ];
        segments[0].speaker = Some("Speaker 1".to_string());
        segments[1].speaker = Some("Speaker 2".to_string());
        segments[2].speaker = Some("Speaker 1".to_string());

        let (smoothed, count) = smooth_short_transcript_speaker_islands(segments);
        let merged = merge_adjacent_transcript_segments_by_speaker(smoothed);

        assert_eq!(count, 1);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].speaker.as_deref(), Some("Speaker 1"));
        assert_eq!(merged[0].text, "Citizens united. This is good, right?");
    }

    #[test]
    fn preserves_short_standalone_backchannel_island() {
        let mut segments = vec![
            transcript_with_text("a", "The ruling changed elections.", Some(0.0), Some(2.0)),
            transcript_with_text("b", "Yeah.", Some(2.0), Some(2.5)),
            transcript_with_text(
                "c",
                "Fifteen years later it defined the campaign.",
                Some(2.5),
                Some(6.0),
            ),
        ];
        segments[0].speaker = Some("Speaker 1".to_string());
        segments[1].speaker = Some("Speaker 2".to_string());
        segments[2].speaker = Some("Speaker 1".to_string());

        let (smoothed, count) = smooth_short_transcript_speaker_islands(segments);

        assert_eq!(count, 0);
        assert_eq!(smoothed.len(), 3);
        assert_eq!(smoothed[1].speaker.as_deref(), Some("Speaker 2"));
    }

    #[test]
    fn expands_tiny_question_hint_to_full_utterance() {
        let transcripts = vec![transcript_with_word_timestamps(
            "a",
            &[
                ("You", 20.50, 20.66),
                ("can", 20.66, 20.82),
                ("look", 20.82, 21.06),
                ("into", 21.06, 21.30),
                ("who", 21.30, 21.62),
                ("who", 21.62, 21.78),
                ("made", 21.78, 22.02),
                ("money", 22.02, 22.18),
                ("on", 22.18, 22.34),
                ("that", 22.34, 22.50),
                ("one.", 22.50, 22.90),
                ("Who", 22.90, 23.14),
                ("made", 23.14, 23.30),
                ("money", 23.30, 23.54),
                ("on", 23.54, 23.62),
                ("that", 23.62, 23.78),
                ("one?", 23.78, 24.28),
            ],
            20.50,
            24.28,
        )];
        let turns = vec![turn(20.50, 23.00, 1), turn(23.00, 23.24, 0)];

        let mapped = map_diarization_to_transcript_segments(
            &transcripts,
            &turns,
            DiarizationMappingOptions {
                existing_speaker_policy: ExistingSpeakerPolicy::Overwrite,
                ..Default::default()
            },
        );

        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 2"));
        assert_eq!(
            mapped[0].text,
            "You can look into who who made money on that one."
        );
        assert_eq!(mapped[1].speaker.as_deref(), Some("Speaker 1"));
        assert_eq!(mapped[1].text, "Who made money on that one?");
    }

    #[test]
    fn expands_tiny_backchannel_hint_to_full_utterance() {
        let transcripts = vec![transcript_with_word_timestamps(
            "a",
            &[
                ("It", 40.41, 40.57),
                ("makes", 40.57, 40.73),
                ("a", 40.73, 40.81),
                ("lot", 40.81, 40.97),
                ("of", 40.97, 41.05),
                ("sense.", 41.05, 41.53),
                ("Of", 41.53, 41.61),
                ("course.", 42.27, 42.94),
                ("When", 43.13, 43.21),
                ("you", 43.21, 43.29),
                ("follow", 43.29, 43.53),
                ("APAC,", 43.53, 44.17),
            ],
            40.41,
            44.17,
        )];
        let turns = vec![
            turn(40.41, 41.75, 1),
            turn(41.75, 42.00, 0),
            turn(42.00, 44.17, 1),
        ];

        let mapped = map_diarization_to_transcript_segments(
            &transcripts,
            &turns,
            DiarizationMappingOptions {
                existing_speaker_policy: ExistingSpeakerPolicy::Overwrite,
                ..Default::default()
            },
        );

        assert_eq!(mapped.len(), 3);
        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 2"));
        assert_eq!(mapped[0].text, "It makes a lot of sense.");
        assert_eq!(mapped[1].speaker.as_deref(), Some("Speaker 1"));
        assert_eq!(mapped[1].text, "Of course.");
        assert_eq!(mapped[2].speaker.as_deref(), Some("Speaker 2"));
        assert_eq!(mapped[2].text, "When you follow APAC,");
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
    fn splits_transcript_row_inside_sentence_with_word_timestamps() {
        let transcripts = vec![transcript_with_word_timestamps(
            "a",
            &[
                ("Just", 0.0, 1.0),
                ("like", 1.0, 2.0),
                ("the", 2.0, 3.0),
                ("Patriot", 3.0, 4.0),
                ("Act", 4.0, 5.0),
                ("until", 5.0, 6.0),
                ("you", 6.0, 7.0),
                ("hear", 7.0, 8.0),
                ("scam", 8.0, 9.0),
                ("explained", 9.0, 10.0),
            ],
            0.0,
            10.0,
        )];
        let turns = vec![turn(0.0, 5.0, 0), turn(5.0, 7.0, 1), turn(7.0, 10.0, 0)];

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
        assert_eq!(mapped[0].text, "Just like the Patriot Act");
        assert_eq!(
            mapped[0]
                .word_timestamps
                .as_ref()
                .unwrap()
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Just", "like", "the", "Patriot", "Act"]
        );
        assert_eq!(mapped[1].speaker.as_deref(), Some("Speaker 2"));
        assert_eq!(mapped[1].text, "until you");
        assert!(mapped[1]
            .word_timestamps
            .as_ref()
            .unwrap()
            .iter()
            .all(|word| word.speaker.as_deref() == Some("Speaker 2")));
        assert_eq!(mapped[2].speaker.as_deref(), Some("Speaker 1"));
        assert_eq!(mapped[2].text, "hear scam explained");
    }

    #[test]
    fn splits_short_interjection_when_word_timestamps_are_available() {
        let transcripts = vec![transcript_with_word_timestamps(
            "a",
            &[
                ("What", 0.0, 0.4),
                ("about", 0.4, 0.8),
                ("drones?", 0.8, 4.0),
                ("Who", 4.0, 4.15),
                ("made", 4.15, 4.3),
                ("money", 4.3, 4.45),
                ("on", 4.45, 4.6),
                ("that", 4.6, 4.8),
                ("one?", 4.8, 5.0),
                ("Should", 5.0, 5.3),
                ("we", 5.3, 5.6),
                ("look", 5.6, 5.9),
                ("it", 5.9, 6.1),
                ("up?", 6.1, 8.0),
            ],
            0.0,
            8.0,
        )];
        let turns = vec![turn(0.0, 4.0, 1), turn(4.0, 5.0, 0), turn(5.0, 8.0, 1)];

        let mapped = map_diarization_to_transcript_segments(
            &transcripts,
            &turns,
            DiarizationMappingOptions {
                existing_speaker_policy: ExistingSpeakerPolicy::Overwrite,
                ..Default::default()
            },
        );

        assert_eq!(mapped.len(), 3);
        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 2"));
        assert_eq!(mapped[0].text, "What about drones?");
        assert_eq!(mapped[1].speaker.as_deref(), Some("Speaker 1"));
        assert_eq!(mapped[1].text, "Who made money on that one?");
        assert_eq!(mapped[1].audio_start_time, Some(4.0));
        assert_eq!(mapped[1].audio_end_time, Some(5.0));
        assert_eq!(mapped[2].speaker.as_deref(), Some("Speaker 2"));
        assert_eq!(mapped[2].text, "Should we look it up?");
    }

    #[test]
    fn splits_short_interjection_with_legacy_precise_word_timestamps() {
        let mut segment = transcript_with_word_timestamps(
            "a",
            &[
                ("What", 0.0, 0.4),
                ("about", 0.4, 0.8),
                ("drones?", 0.8, 4.0),
                ("Who", 4.0, 4.15),
                ("made", 4.15, 4.3),
                ("money", 4.3, 4.45),
                ("on", 4.45, 4.6),
                ("that", 4.6, 4.8),
                ("one?", 4.8, 5.0),
                ("Should", 5.0, 5.3),
                ("we", 5.3, 5.6),
                ("look", 5.6, 5.9),
                ("it", 5.9, 6.1),
                ("up?", 6.1, 8.0),
            ],
            0.0,
            8.0,
        );
        for word in segment.word_timestamps.as_mut().unwrap() {
            word.timestamp_source = None;
        }
        let turns = vec![turn(0.0, 4.0, 1), turn(4.0, 5.0, 0), turn(5.0, 8.0, 1)];

        let mapped = map_diarization_to_transcript_segments(
            &[segment],
            &turns,
            DiarizationMappingOptions {
                existing_speaker_policy: ExistingSpeakerPolicy::Overwrite,
                ..Default::default()
            },
        );

        assert_eq!(mapped.len(), 3);
        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 2"));
        assert_eq!(mapped[0].text, "What about drones?");
        assert_eq!(mapped[1].speaker.as_deref(), Some("Speaker 1"));
        assert_eq!(mapped[1].text, "Who made money on that one?");
        assert_eq!(mapped[2].speaker.as_deref(), Some("Speaker 2"));
        assert_eq!(mapped[2].text, "Should we look it up?");
    }

    #[test]
    fn does_not_bypass_short_part_gate_for_estimated_word_timestamps() {
        let mut segment = transcript_with_word_timestamps(
            "a",
            &[
                ("What", 0.0, 0.4),
                ("about", 0.4, 0.8),
                ("drones?", 0.8, 4.0),
                ("Who", 4.0, 4.15),
                ("made", 4.15, 4.3),
                ("money", 4.3, 4.45),
                ("on", 4.45, 4.6),
                ("that", 4.6, 4.8),
                ("one?", 4.8, 5.0),
                ("Should", 5.0, 5.3),
                ("we", 5.3, 5.6),
                ("look", 5.6, 5.9),
                ("it", 5.9, 6.1),
                ("up?", 6.1, 8.0),
            ],
            0.0,
            8.0,
        );
        for word in segment.word_timestamps.as_mut().unwrap() {
            word.timestamp_source = Some(TranscriptWordTimestampSource::Estimated);
        }
        let turns = vec![turn(0.0, 4.0, 1), turn(4.0, 5.0, 0), turn(5.0, 8.0, 1)];

        let mapped = map_diarization_to_transcript_segments(
            &[segment],
            &turns,
            DiarizationMappingOptions {
                existing_speaker_policy: ExistingSpeakerPolicy::Overwrite,
                ..Default::default()
            },
        );

        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 2"));
    }

    #[test]
    fn does_not_bypass_short_part_gate_for_legacy_estimated_word_timestamps() {
        let mut segment = transcript_with_text(
            "a",
            "What about drones? Who made money on that one? Should we look it up?",
            Some(0.0),
            Some(8.0),
        );
        segment.word_timestamps =
            crate::audio::common::estimate_word_timestamps(&segment.text, 0.0, 8.0, None, None);
        for word in segment.word_timestamps.as_mut().unwrap() {
            word.timestamp_source = None;
        }
        let turns = vec![turn(0.0, 4.0, 1), turn(4.0, 5.0, 0), turn(5.0, 8.0, 1)];

        let mapped = map_diarization_to_transcript_segments(
            &[segment],
            &turns,
            DiarizationMappingOptions {
                existing_speaker_policy: ExistingSpeakerPolicy::Overwrite,
                ..Default::default()
            },
        );

        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 2"));
    }

    #[test]
    fn mapping_diagnostics_report_split_eligibility() {
        let mut legacy_precise = transcript_with_word_timestamps(
            "a",
            &[
                ("What", 0.0, 0.4),
                ("about", 0.4, 0.8),
                ("drones?", 0.8, 4.0),
                ("Who", 4.0, 4.15),
                ("made", 4.15, 4.3),
                ("money", 4.3, 4.45),
                ("on", 4.45, 4.6),
                ("that", 4.6, 4.8),
                ("one?", 4.8, 5.0),
                ("Should", 5.0, 5.3),
                ("we", 5.3, 5.6),
                ("look", 5.6, 5.9),
                ("it", 5.9, 6.1),
                ("up?", 6.1, 8.0),
            ],
            0.0,
            8.0,
        );
        for word in legacy_precise.word_timestamps.as_mut().unwrap() {
            word.timestamp_source = None;
        }

        let mut legacy_estimated = transcript_with_text(
            "b",
            "What about drones? Who made money on that one? Should we look it up?",
            Some(10.0),
            Some(18.0),
        );
        legacy_estimated.word_timestamps = crate::audio::common::estimate_word_timestamps(
            &legacy_estimated.text,
            10.0,
            18.0,
            None,
            None,
        );
        for word in legacy_estimated.word_timestamps.as_mut().unwrap() {
            word.timestamp_source = None;
        }

        let turns = vec![
            turn(0.0, 4.0, 1),
            turn(4.0, 5.0, 0),
            turn(5.0, 8.0, 1),
            turn(10.0, 14.0, 1),
            turn(14.0, 15.0, 0),
            turn(15.0, 18.0, 1),
        ];

        let (mapped, diagnostics) = map_diarization_to_transcript_segments_with_diagnostics(
            &[legacy_precise, legacy_estimated],
            &turns,
            DiarizationMappingOptions {
                existing_speaker_policy: ExistingSpeakerPolicy::Overwrite,
                ..Default::default()
            },
        );

        assert_eq!(mapped.len(), 4);
        assert_eq!(diagnostics.transcript_rows, 2);
        assert_eq!(diagnostics.rows_with_word_timestamps, 2);
        assert_eq!(diagnostics.rows_with_legacy_precise_word_timestamps, 1);
        assert_eq!(diagnostics.rows_with_legacy_estimated_word_timestamps, 1);
        assert_eq!(diagnostics.rows_with_multiple_speaker_spans, 2);
        assert_eq!(diagnostics.rows_split_by_speaker_spans, 1);
        assert_eq!(diagnostics.rows_blocked_by_short_part_gate, 1);
        assert_eq!(diagnostics.short_speaker_spans, 2);
    }

    #[test]
    fn does_not_split_transcript_row_inside_sentence_without_word_timestamps() {
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
    fn does_not_split_for_tiny_speaker_span() {
        let transcripts = vec![transcript_with_text(
            "a",
            "The nonprofit mostly supports the nonprofit itself. It pays for overhead and staff.",
            Some(0.0),
            Some(8.0),
        )];
        let turns = vec![turn(0.0, 3.8, 0), turn(3.8, 4.3, 1), turn(4.3, 8.0, 0)];

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
            "The nonprofit mostly supports the nonprofit itself. It pays for overhead and staff."
        );
        assert_eq!(mapped[0].speaker.as_deref(), Some("Speaker 1"));
    }

    #[test]
    fn merges_adjacent_rows_with_same_speaker() {
        let transcripts = vec![
            transcript_with_text("a", "It is such an incentive to", Some(0.0), Some(13.0)),
            transcript_with_text(
                "b",
                "do things that people do not want.",
                Some(13.0),
                Some(15.0),
            ),
        ];
        let turns = vec![turn(0.0, 15.0, 0)];

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
            "It is such an incentive to do things that people do not want."
        );
        assert_eq!(mapped[0].audio_start_time, Some(0.0));
        assert_eq!(mapped[0].audio_end_time, Some(15.0));
        assert_eq!(mapped[0].duration, Some(15.0));
    }

    #[test]
    fn does_not_merge_adjacent_unlabeled_rows() {
        let transcripts = vec![
            transcript_with_text("a", "First unlabeled row.", Some(0.0), Some(1.0)),
            transcript_with_text("b", "Second unlabeled row.", Some(1.0), Some(2.0)),
        ];

        let mapped = map_diarization_to_transcript_segments(
            &transcripts,
            &[],
            DiarizationMappingOptions::default(),
        );

        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].speaker, None);
        assert_eq!(mapped[1].speaker, None);
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
    fn preserves_existing_labels_without_coverage_by_default() {
        let mut segment = transcript("stale", Some(5.0), Some(6.0));
        segment.speaker = Some("Alice".to_string());

        let mapped = map_diarization_to_transcript_segments(
            &[segment],
            &[turn(0.0, 2.0, 0)],
            DiarizationMappingOptions::default(),
        );

        assert_eq!(mapped[0].speaker.as_deref(), Some("Alice"));
    }

    #[test]
    fn overwrite_clears_stale_labels_without_coverage() {
        let mut segment = transcript_with_word_timestamps(
            "stale",
            &[("stale", 5.0, 5.4), ("label", 5.4, 6.0)],
            5.0,
            6.0,
        );
        segment.speaker = Some("Speaker 5".to_string());
        for word in segment.word_timestamps.as_mut().unwrap() {
            word.speaker = Some("Speaker 5".to_string());
        }

        let mapped = map_diarization_to_transcript_segments(
            &[segment],
            &[turn(0.0, 2.0, 0)],
            DiarizationMappingOptions {
                existing_speaker_policy: ExistingSpeakerPolicy::Overwrite,
                ..Default::default()
            },
        );

        assert_eq!(mapped[0].speaker, None);
        let words = mapped[0].word_timestamps.as_ref().unwrap();
        assert!(words.iter().all(|word| word.speaker.is_none()));
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
