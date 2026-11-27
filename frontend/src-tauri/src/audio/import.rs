// Audio file import module - allows importing external audio files as new meetings

use crate::api::TranscriptSegment;
use crate::config::{DEFAULT_PARAKEET_MODEL, DEFAULT_WHISPER_MODEL};
use crate::database::repositories::setting::SettingsRepository;
use crate::parakeet_engine::ParakeetEngine;
use crate::state::AppState;
use crate::whisper_engine::WhisperEngine;
use anyhow::{anyhow, Result};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

use super::audio_processing::create_meeting_folder;
use super::common::{
    create_readable_transcript_segments_with_words, split_transcripts_to_timing_grid,
    transcript_words_from_token_timestamps, TranscribedSegment,
};
use super::constants::AUDIO_EXTENSIONS;
use super::recording_preferences::{get_default_recordings_folder, load_recording_preferences};

pub mod queue;

/// Global flag to track if import is in progress
static IMPORT_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Global flag to signal cancellation
static IMPORT_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Where the import is, as far as a cancel from a starting recording is
/// concerned. The lock makes "cancel accepted" and "save begins" mutually
/// exclusive.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Before the meeting is saved: a cancel takes effect.
    Open,
    /// The meeting is being created and the import runs to completion.
    Saving,
    /// The import is over, successfully or not.
    Finished,
}

static IMPORT_PHASE: std::sync::Mutex<Phase> = std::sync::Mutex::new(Phase::Finished);

fn set_import_phase(phase: Phase) {
    *IMPORT_PHASE
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = phase;
}

/// Opens the cancel window for one import and closes it when dropped. Declared
/// after the job registration, so it is dropped (and cancel starts reporting
/// false) before the registration is cleared.
struct PhaseGuard;

impl PhaseGuard {
    fn open() -> Self {
        set_import_phase(Phase::Open);
        PhaseGuard
    }
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        set_import_phase(Phase::Finished);
    }
}

/// Cancels for a starting recording. Returns false when the import is already
/// saving, or over, and a cancel cannot take effect.
fn cancel_import_if_possible() -> bool {
    let phase = IMPORT_PHASE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if *phase != Phase::Open {
        return false;
    }
    IMPORT_CANCELLED.store(true, Ordering::SeqCst);
    true
}

/// The last cancel check: after this the meeting is created.
fn begin_import_save() -> Result<()> {
    let mut phase = IMPORT_PHASE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if IMPORT_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Import cancelled"));
    }
    *phase = Phase::Saving;
    Ok(())
}

/// RAII guard for IMPORT_IN_PROGRESS flag
/// Ensures flag is cleared even if import panics or returns early
struct ImportGuard;

impl ImportGuard {
    /// Create guard and set flag atomically
    fn acquire() -> Result<Self, String> {
        if IMPORT_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("Import already in progress".to_string());
        }
        Ok(ImportGuard)
    }
}

impl Drop for ImportGuard {
    fn drop(&mut self) {
        IMPORT_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

/// Owns only the newly created import directory, until its database commit.
struct ImportFolderGuard {
    path: PathBuf,
    committed: bool,
    owner: String,
    audio_name: String,
    audio_copied: bool,
}

impl ImportFolderGuard {
    fn owned(path: PathBuf, meeting_id: &str, audio_name: &str) -> Result<Self> {
        if crate::database::meeting_files::directory_chain(&path).is_err() {
            return Err(anyhow!("Could not verify the new import folder"));
        }
        let marker = path.join(IMPORT_OWNER_FILE);
        let mut file = std::fs::File::create_new(&marker)?;
        file.write_all(meeting_id.as_bytes())?;
        file.sync_all()?;
        Ok(Self {
            path,
            committed: false,
            owner: meeting_id.into(),
            audio_name: audio_name.into(),
            audio_copied: false,
        })
    }
}

impl Drop for ImportFolderGuard {
    fn drop(&mut self) {
        if !self.committed && owns_import_folder(&self.path, &self.owner) {
            if self.audio_copied {
                let _ = std::fs::remove_file(self.path.join(&self.audio_name));
            }
            let _ = std::fs::remove_file(self.path.join(IMPORT_OWNER_FILE));
            let _ = std::fs::remove_dir(&self.path);
        }
    }
}

pub(crate) const IMPORT_OWNER_FILE: &str = ".clawscribe-import-owner";
const COPY_BUFFER_BYTES: usize = 256 * 1024;

pub(crate) fn owns_import_folder(folder: &Path, meeting_id: &str) -> bool {
    if meeting_id.is_empty() || crate::database::meeting_files::directory_chain(folder).is_err() {
        return false;
    }
    let marker = folder.join(IMPORT_OWNER_FILE);
    std::fs::symlink_metadata(&marker).is_ok_and(|meta| {
        meta.is_file()
            && !crate::database::meeting_files::is_link(&marker, &meta)
            && meta.len() == meeting_id.len() as u64
    }) && std::fs::read_to_string(&marker).is_ok_and(|id| id == meeting_id)
}

fn copy_audio_chunks(
    reader: &mut impl Read,
    writer: &mut impl Write,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<u64> {
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
    let mut total = 0;
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(anyhow!("Import cancelled"));
        }
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(total);
        }
        total += count as u64;
        if total > MAX_FILE_SIZE_BYTES {
            return Err(anyhow!("The source audio file exceeds 20 GB"));
        }
        if cancelled.load(Ordering::Acquire) {
            return Err(anyhow!("Import cancelled"));
        }
        writer.write_all(&buffer[..count])?;
        progress(total);
    }
}

fn copy_import_audio<R: Runtime>(
    app: &AppHandle<R>,
    source: &Path,
    destination: &Path,
    owner: &str,
) -> Result<()> {
    if !std::fs::metadata(source)?.is_file() {
        return Err(anyhow!("Choose a regular audio or video file"));
    }
    let mut input = std::fs::File::open(source)?;
    let metadata = input.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_FILE_SIZE_BYTES {
        return Err(anyhow!(
            "Choose a non-empty audio or video file up to 20 GB"
        ));
    }
    let size = metadata.len();
    let folder = destination
        .parent()
        .ok_or_else(|| anyhow!("Missing import folder"))?;
    if !owns_import_folder(folder, owner) {
        return Err(anyhow!("Could not verify the import folder"));
    }
    let mut output = tempfile::NamedTempFile::new_in(folder)?;
    let mut last = 5;
    let copied = copy_audio_chunks(&mut input, &mut output, &IMPORT_CANCELLED, |bytes| {
        let percent = 5 + (bytes.min(size) * 9 / size) as u32;
        if percent != last {
            last = percent;
            emit_progress(app, "copying", percent, "Copying audio file...");
        }
    })?;
    if copied != size || input.metadata()?.modified().ok() != metadata.modified().ok() {
        return Err(anyhow!(
            "The source changed during the copy. Retry when the file is stable"
        ));
    }
    if IMPORT_CANCELLED.load(Ordering::Acquire) {
        return Err(anyhow!("Import cancelled"));
    }
    output.as_file().sync_all()?;
    if IMPORT_CANCELLED.load(Ordering::Acquire) {
        return Err(anyhow!("Import cancelled"));
    }
    if !owns_import_folder(folder, owner) {
        return Err(anyhow!("The import folder changed during the copy"));
    }
    output.persist_noclobber(destination)?;
    Ok(())
}

fn import_error(error: anyhow::Error, cancelled: bool) -> anyhow::Error {
    if cancelled {
        anyhow!("Import cancelled")
    } else {
        error
    }
}

/// VAD redemption time in milliseconds. Keep this close to the live pipeline so
/// import rows do not bridge normal speaker handoffs into long multi-speaker
/// transcript rows that diarization then has to split after the fact.
const VAD_REDEMPTION_TIME_MS: u32 = 500;
const MAX_TRANSCRIPTION_SEGMENT_SAMPLES: usize = 25 * 16000;

/// Maximum file size: 20GB (prevents OOM and excessive processing time)
const MAX_FILE_SIZE_BYTES: u64 = 20 * 1024 * 1024 * 1024; // 20GB

/// Information about a selected audio file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFileInfo {
    pub path: String,
    pub filename: String,
    pub duration_seconds: f64,
    pub size_bytes: u64,
    pub format: String,
    pub modified_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Progress update emitted during import
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportProgress {
    pub stage: String, // "copying", "decoding", "vad", "transcribing", "saving"
    pub progress_percentage: u32,
    pub message: String,
}

/// Result of import
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub meeting_id: String,
    pub title: String,
    pub segments_count: usize,
    pub duration_seconds: f64,
    #[serde(default)]
    pub files_pending: bool,
    #[serde(default)]
    pub transcription_provider: Option<String>,
    #[serde(default)]
    pub transcription_incomplete: bool,
}

/// Warning emitted during import (non-fatal)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportWarning {
    pub warning: String,
    pub details: Option<String>,
}

/// Check if import is currently in progress
#[cfg(test)]
pub fn is_import_in_progress() -> bool {
    IMPORT_IN_PROGRESS.load(Ordering::SeqCst)
}

/// Cancel ongoing import
#[cfg(test)]
pub fn cancel_import() {
    IMPORT_CANCELLED.store(true, Ordering::SeqCst);
}

/// Validate an audio file and return its info using metadata-only approach
/// Missing duration is shown as unknown and measured during bounded batch preparation.
pub fn validate_audio_file(path: &Path) -> Result<AudioFileInfo> {
    // Check file exists
    if !path.exists() {
        return Err(anyhow!("File does not exist: {}", path.display()));
    }

    // Check extension
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if !AUDIO_EXTENSIONS.contains(&extension.as_str()) {
        return Err(anyhow!(
            "Unsupported format: .{}. Supported: {}",
            extension,
            AUDIO_EXTENSIONS.join(", ")
        ));
    }

    // Get file size
    let metadata = std::fs::metadata(path).map_err(|e| anyhow!("Cannot read file: {}", e))?;
    let size_bytes = metadata.len();
    let modified_at = metadata
        .modified()
        .ok()
        .map(chrono::DateTime::<chrono::Utc>::from);

    // Check file size limit
    if size_bytes > MAX_FILE_SIZE_BYTES {
        return Err(anyhow!(
            "File too large: {:.2}GB. Maximum supported size is {}GB",
            size_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            MAX_FILE_SIZE_BYTES / (1024 * 1024 * 1024)
        ));
    }

    // Get filename without extension for title
    let filename = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported Audio")
        .to_string();

    // Try fast metadata-only validation first
    let duration_seconds = match extract_duration_from_metadata(path) {
        Ok(duration) => {
            debug!("Got duration from metadata: {:.2}s (fast path)", duration);
            duration
        }
        Err(_) => {
            debug!("Duration unavailable in file metadata; it will be measured during import");
            0.0
        }
    };

    Ok(AudioFileInfo {
        path: path.to_string_lossy().to_string(),
        filename,
        duration_seconds,
        size_bytes,
        format: extension.to_uppercase(),
        modified_at,
    })
}

/// Compute duration from track codec parameters.
///
/// MP4 and Matroska/WebM containers store `n_frames` as timestamp ticks in
/// `time_base` units, not as PCM sample counts. When `time_base` is available
/// it is converted via `TimeBase::calc_time`, which uses integer arithmetic to
/// avoid f64 precision loss at high tick counts. `sample_rate` is the fallback
/// for containers such as WAV that express `n_frames` as PCM sample counts.
fn duration_from_params(
    time_base: Option<symphonia::core::units::TimeBase>,
    sample_rate: Option<u32>,
    n_frames: u64,
) -> Option<f64> {
    if let Some(tb) = time_base {
        if tb.denom > 0 {
            let t = tb.calc_time(n_frames);
            let secs = t.seconds as f64 + t.frac;
            if secs.is_finite() && secs > 0.0 {
                return Some(secs);
            }
        }
    }
    if let Some(rate) = sample_rate {
        if rate > 0 {
            let secs = n_frames as f64 / rate as f64;
            if secs.is_finite() && secs > 0.0 {
                return Some(secs);
            }
        }
    }
    None
}

/// Extract duration from audio file metadata without full decode
/// Returns an error when the container does not expose duration metadata.
pub(super) fn extract_duration_from_metadata(path: &Path) -> Result<f64> {
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    // Open the file
    let file =
        std::fs::File::open(path).map_err(|e| anyhow!("Failed to open audio file: {}", e))?;

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    // Set up format hint based on file extension
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    // Probe the file format (lightweight operation)
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| anyhow!("Failed to probe audio format: {}", e))?;

    let format = probed.format;

    // Find the first audio track
    use symphonia::core::codecs::CODEC_TYPE_NULL;
    let duration_seconds = format
        .tracks()
        .iter()
        .filter(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .find_map(|track| {
            let n_frames = track.codec_params.n_frames?;
            duration_from_params(
                track.codec_params.time_base,
                track.codec_params.sample_rate,
                n_frames,
            )
        })
        .ok_or_else(|| anyhow!("Audio duration not available in metadata"))?;

    debug!("Extracted metadata duration: {:.2}s", duration_seconds);

    super::provenance::measured_duration(Some(duration_seconds))
        .ok_or_else(|| anyhow!("Invalid audio duration"))
}

#[derive(Clone, Copy)]
pub(crate) struct ImportDate {
    meeting_at: Option<chrono::DateTime<chrono::Utc>>,
    imported_at: chrono::DateTime<chrono::Utc>,
    from_file: bool,
}

impl ImportDate {
    fn meeting_date(self) -> chrono::DateTime<chrono::Utc> {
        self.meeting_at.unwrap_or(self.imported_at)
    }

    fn provenance(self) -> &'static str {
        if self.from_file {
            "file_modified_time"
        } else if self.meeting_at.is_some() {
            "user_selected"
        } else {
            "import_time"
        }
    }
}

impl Default for ImportDate {
    fn default() -> Self {
        Self {
            meeting_at: None,
            imported_at: chrono::Utc::now(),
            from_file: false,
        }
    }
}

struct ImportOptions {
    meeting_id: String,
    date: ImportDate,
    summary_request: Option<Result<crate::summary::automatic::AuthorizedRequest, String>>,
    summary_language: Option<String>,
    cloud_destination: Option<String>,
}

/// Start import of an audio file
async fn start_import<R: Runtime>(
    app: AppHandle<R>,
    source_path: String,
    title: String,
    language: Option<String>,
    model: Option<String>,
    mut provider: Option<String>,
    options: ImportOptions,
    guard: ImportGuard,
    job: super::inference::BatchPermit,
    phase: PhaseGuard,
) -> Result<ImportResult> {
    // Registered so a starting recording can cancel this job. A cancel before
    // the database commit removes the half-built meeting folder (see
    // `ImportFolderGuard`); the source file is never touched.
    let _ = crate::summary::summary_engine::force_shutdown_sidecar().await;

    let result = run_import(
        app.clone(),
        source_path,
        title,
        language,
        model,
        &mut provider,
        &options,
    )
    .await
    .map_err(|error| import_error(error, IMPORT_CANCELLED.load(Ordering::SeqCst)));

    unload_local_batch_engine(provider.as_deref()).await;

    // Release the transcription slot before starting a local summary or
    // telling the UI that another import can begin.
    drop(phase);
    drop(job);
    if let Ok(res) = &result {
        crate::webhook::recording_saved(&app, &res.meeting_id);
        if let Some(request) = options.summary_request {
            if let Some(state) = app.try_state::<AppState>() {
                if crate::summary::automatic::start_import(
                    app.clone(),
                    state.db_manager.pool().clone(),
                    res.meeting_id.clone(),
                    request,
                )
                .await
                .is_err()
                {
                    warn!("Could not start notes for imported meeting");
                    let _ = app.emit(
                        "import-warning",
                        ImportWarning {
                            warning: "Could not start notes for this import.".into(),
                            details: Some("Open the saved meeting to generate a summary.".into()),
                        },
                    );
                }
            }
        }
        drop(guard);
        let _ = app.emit("import-complete", res);
    }

    result
}

/// Internal function to run import
async fn run_import<R: Runtime>(
    app: AppHandle<R>,
    source_path: String,
    title: String,
    language: Option<String>,
    mut model: Option<String>,
    provider: &mut Option<String>,
    options: &ImportOptions,
) -> Result<ImportResult> {
    let source = PathBuf::from(&source_path);

    // Validate source file
    if !source.exists() {
        return Err(anyhow!(
            "The source file is unavailable. Choose the original file again."
        ));
    }

    info!(
        "Starting audio import (language={:?}, model={:?}, provider={:?})",
        language, model, provider
    );

    emit_progress(&app, "copying", 5, "Creating meeting folder...");

    // Check for cancellation
    if IMPORT_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Import cancelled"));
    }

    // Create meeting folder in the configured recording location.
    let base_folder = match load_recording_preferences(&app).await {
        Ok(prefs) => prefs.save_folder,
        Err(_e) => {
            warn!("Failed to load recording preferences for import destination, using default");
            get_default_recordings_folder()
        }
    };
    // Copy audio file to meeting folder
    emit_progress(&app, "copying", 5, "Copying audio file...");

    let dest_filename = format!(
        "audio.{}",
        source
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mp4")
            .to_ascii_lowercase()
    );
    let meeting_folder = create_meeting_folder(&base_folder, &title)?;
    let mut folder_guard =
        ImportFolderGuard::owned(meeting_folder.clone(), &options.meeting_id, &dest_filename)?;
    let dest_path = meeting_folder.join(&dest_filename);

    let src = source.clone();
    let dst = dest_path.clone();
    let copy_app = app.clone();
    let owner = options.meeting_id.clone();
    tokio::task::spawn_blocking(move || copy_import_audio(&copy_app, &src, &dst, &owner))
        .await
        .map_err(|e| anyhow!("Copy task join error: {}", e))?
        .map_err(|e| anyhow!("Failed to copy audio file: {}", e))?;
    folder_guard.audio_copied = true;

    info!("Imported audio copy saved");

    // Check for cancellation
    if IMPORT_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Import cancelled"));
    }

    let mut cloud_fallback_error = None;
    if super::transcription::cloud::is_cloud_provider(provider.as_deref()) {
        if super::recording_commands::cloud_transcription_enabled()
            && options.cloud_destination.is_some()
        {
            let provider_id = provider.as_deref().unwrap_or_default().to_string();
            emit_progress(
                &app,
                "transcribing",
                15,
                "Uploading audio for cloud transcription...",
            );
            let outcome = super::transcription::cloud::transcribe_whole_file(
                &app,
                &provider_id,
                model.as_deref(),
                &dest_path,
                language.as_deref(),
                &IMPORT_CANCELLED,
                options.cloud_destination.as_deref(),
                |progress, message| {
                    emit_progress(&app, "transcribing", 15 + 60 * progress / 100, message)
                },
            )
            .await;
            if IMPORT_CANCELLED.load(Ordering::Acquire) {
                return Err(anyhow!("Import cancelled"));
            }
            match outcome {
                Ok(outcome) => {
                    let (duration_seconds, cloud_segments) = if outcome.requires_local_timing_grid {
                        let (decoded_duration, timed_segments) =
                            prepare_cloud_import_timing_grid(&app, &dest_path, &outcome.segments)
                                .await?;
                        (Some(decoded_duration), timed_segments)
                    } else {
                        let duration = tokio::task::spawn_blocking({
                            let path = dest_path.clone();
                            move || extract_duration_from_metadata(&path).ok()
                        })
                        .await
                        .ok()
                        .flatten();
                        (duration, outcome.segments)
                    };
                    let source_language =
                        super::common::transcription_source_language_hint(language.as_deref());
                    return save_import_transcripts(
                        &app,
                        &mut folder_guard,
                        &title,
                        &dest_filename,
                        duration_seconds,
                        &cloud_segments,
                        &outcome.provider,
                        &outcome.model,
                        source_language.as_deref(),
                        0,
                        options,
                    )
                    .await;
                }
                Err(error) => {
                    super::transcription::cloud::emit_fallback_event(
                        &app,
                        None,
                        &provider_id,
                        &error,
                    );
                    cloud_fallback_error = Some(error);
                }
            }
        }
        let local = resolve_local_fallback(&app, cloud_fallback_error.as_ref()).await?;
        *provider = Some(local.0.to_owned());
        model = Some(local.1);
    }

    // Determine which local provider to use after any cloud fallback.
    let use_parakeet = provider.as_deref() == Some("parakeet");
    let use_nemotron = provider.as_deref() == Some("nemotron");
    let language = local_batch_language(provider.as_deref(), language);

    emit_progress(&app, "decoding", 15, "Decoding audio file...");

    let prepared_audio = super::batch_audio::prepare(
        &dest_path,
        VAD_REDEMPTION_TIME_MS,
        MAX_TRANSCRIPTION_SEGMENT_SAMPLES,
        &IMPORT_CANCELLED,
    )
    .await?;
    let duration_seconds = prepared_audio.duration_seconds;
    let speech_segments = &prepared_audio.segments;

    let total_segments = speech_segments.len();
    info!(
        "VAD detected {} speech segments (redemption_time={}ms)",
        total_segments, VAD_REDEMPTION_TIME_MS
    );

    // Diagnostic: log segment duration distribution
    if !speech_segments.is_empty() {
        let durations_ms: Vec<f64> = speech_segments
            .iter()
            .map(|s| s.end_timestamp_ms - s.start_timestamp_ms)
            .collect();
        let total_speech_ms: f64 = durations_ms.iter().sum();
        let avg_duration = total_speech_ms / durations_ms.len() as f64;
        let min_duration = durations_ms.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_duration = durations_ms
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        info!(
            "VAD segment stats: avg={:.0}ms, min={:.0}ms, max={:.0}ms, total_speech={:.1}s/{:.1}s ({:.0}%)",
            avg_duration, min_duration, max_duration,
            total_speech_ms / 1000.0, duration_seconds,
            (total_speech_ms / 1000.0 / duration_seconds) * 100.0
        );
        // Log first 10 segments for detailed inspection
        for (i, seg) in speech_segments.iter().take(10).enumerate() {
            let dur = seg.end_timestamp_ms - seg.start_timestamp_ms;
            debug!(
                "  Segment {}: {:.0}ms-{:.0}ms ({:.0}ms, {} samples)",
                i, seg.start_timestamp_ms, seg.end_timestamp_ms, dur, seg.sample_count
            );
        }
        if total_segments > 10 {
            debug!("  ... and {} more segments", total_segments - 10);
        }
    }

    if total_segments == 0 {
        warn!("No speech detected in audio");

        // Emit warning to frontend
        let _ = app.emit(
            "import-warning",
            ImportWarning {
                warning: "No speech detected in audio file".to_string(),
                details: Some(
                    "The file was imported successfully, but VAD did not detect any speech. \
                     The meeting was created but contains no transcripts."
                        .to_string(),
                ),
            },
        );
        // Still create the meeting, just with no transcripts
    }

    // Check for cancellation
    if IMPORT_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Import cancelled"));
    }

    emit_progress(&app, "transcribing", 30, "Loading transcription engine...");

    // Initialize the appropriate engine
    let whisper_engine = if !use_parakeet && !use_nemotron && total_segments > 0 {
        match get_or_init_whisper(&app, model.as_deref()).await {
            Ok(engine) => Some(engine),
            Err(error) => {
                return Err(match &cloud_fallback_error {
                    Some(cloud_error) => super::transcription::cloud::local_fallback_error_context(
                        cloud_error,
                        error,
                    ),
                    None => error,
                });
            }
        }
    } else {
        None
    };
    let parakeet_engine = if use_parakeet && total_segments > 0 {
        match get_or_init_parakeet(&app, model.as_deref()).await {
            Ok(engine) => Some(engine),
            Err(error) => {
                return Err(match &cloud_fallback_error {
                    Some(cloud_error) => super::transcription::cloud::local_fallback_error_context(
                        cloud_error,
                        error,
                    ),
                    None => error,
                });
            }
        }
    } else {
        None
    };
    let nemotron_engine = if use_nemotron && total_segments > 0 {
        match get_or_init_nemotron(&app, model.as_deref()).await {
            Ok(engine) => Some(engine),
            Err(error) => {
                return Err(match &cloud_fallback_error {
                    Some(cloud_error) => super::transcription::cloud::local_fallback_error_context(
                        cloud_error,
                        error,
                    ),
                    None => error,
                });
            }
        }
    } else {
        None
    };

    // Split very long segments at silence boundaries for better transcription quality.
    // Hard cuts at arbitrary sample positions lose words at boundaries. Instead, scan
    // for the lowest-energy window near the target split point and cut there.
    // Consume the VAD segments by value: cloning them would double the resident
    // memory of the meeting's entire speech audio.
    let processable_segments = &prepared_audio.segments;

    let processable_count = processable_segments.len();
    info!(
        "Processing {} segments (after splitting)",
        processable_count
    );

    // Process each speech segment
    let mut all_transcripts: Vec<TranscribedSegment> = Vec::new();
    let mut failed_segments = 0usize;
    let mut consecutive_failures = 0usize;
    let mut total_confidence = 0.0f32;
    let mut confidence_count = 0usize;

    for (i, segment) in processable_segments.iter().enumerate() {
        let segment = segment.load().await?;
        if IMPORT_CANCELLED.load(Ordering::SeqCst) {
            return Err(anyhow!("Import cancelled"));
        }

        let progress = 30 + ((i as f32 / processable_count.max(1) as f32) * 50.0) as u32;
        let segment_duration_sec = (segment.end_timestamp_ms - segment.start_timestamp_ms) / 1000.0;
        emit_progress(
            &app,
            "transcribing",
            progress,
            &format!(
                "Transcribing segment {} of {} ({:.1}s)...",
                i + 1,
                processable_count,
                segment_duration_sec
            ),
        );

        // Skip very short segments
        if segment.samples.len() < 1600 {
            debug!(
                "Skipping short segment {} with {} samples",
                i,
                segment.samples.len()
            );
            continue;
        }

        // Transcribe
        let segment_result = super::batch_audio::retry_segment(&IMPORT_CANCELLED, || async {
            let value = if use_nemotron {
                let engine = nemotron_engine.as_ref().unwrap();
                let text = engine
                    .transcribe_audio(segment.samples.clone(), language.clone())
                    .await
                    .map_err(|e| {
                        anyhow!("Nemotron transcription failed on segment {}: {}", i, e)
                    })?;
                (text, None, None)
            } else if use_parakeet {
                let engine = parakeet_engine.as_ref().unwrap();
                let result = engine
                    .transcribe_audio_timestamped(segment.samples.clone())
                    .await
                    .map_err(|e| {
                        anyhow!("Parakeet transcription failed on segment {}: {}", i, e)
                    })?;
                let text = result.text;
                let word_timestamps = transcript_words_from_token_timestamps(
                    &text,
                    &result.tokens,
                    &result.timestamps,
                    segment.start_timestamp_ms / 1000.0,
                    segment.end_timestamp_ms / 1000.0,
                    None,
                    None,
                );
                (text, None, word_timestamps)
            } else {
                let engine = whisper_engine.as_ref().unwrap();
                let (text, conf, partial) = engine
                    .transcribe_audio_with_confidence(segment.samples.clone(), language.clone())
                    .await
                    .map_err(|e| anyhow!("Whisper transcription failed on segment {}: {}", i, e))?;
                if partial {
                    return Err(anyhow!("The speech engine returned incomplete text"));
                }
                (text, conf, None)
            };
            Ok(value)
        })
        .await;
        if IMPORT_CANCELLED.load(Ordering::SeqCst) {
            return Err(anyhow!("Import cancelled"));
        }
        let Some((text, conf, word_timestamps)) = accept_import_segment(
            segment_result,
            i + 1,
            &mut consecutive_failures,
            &mut failed_segments,
        )?
        else {
            continue;
        };

        let trimmed = text.trim();
        if !trimmed.is_empty() {
            debug!(
                "Segment {}/{}: audio={:.1}s, confidence={:?}, chars={}",
                i + 1,
                processable_count,
                segment_duration_sec,
                conf,
                trimmed.chars().count()
            );
            all_transcripts.push(TranscribedSegment {
                text,
                start_ms: segment.start_timestamp_ms,
                end_ms: segment.end_timestamp_ms,
                word_timestamps,
            });
            if let Some(confidence) = conf {
                total_confidence += confidence;
                confidence_count += 1;
            }
        } else {
            debug!(
                "Segment {}/{}: {:.1}s — empty transcription",
                i + 1,
                processable_count,
                segment_duration_sec
            );
        }
    }

    let transcribed_count = all_transcripts.len();
    let avg_confidence = (confidence_count > 0).then(|| total_confidence / confidence_count as f32);

    info!(
        "Transcription complete: {} segments transcribed out of {}, avg reported confidence: {:?}",
        transcribed_count, processable_count, avg_confidence
    );

    // Check for cancellation
    if IMPORT_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Import cancelled"));
    }

    // Record the engine + model that actually ran (read back from the live
    // engine so the metadata reflects reality, not the requested config).
    let used_provider = if use_nemotron {
        "nemotron"
    } else if use_parakeet {
        "parakeet"
    } else {
        "localWhisper"
    };
    let used_model = if let Some(e) = &nemotron_engine {
        e.get_current_model().await
    } else if let Some(e) = &parakeet_engine {
        e.get_current_model().await
    } else if let Some(e) = &whisper_engine {
        e.get_current_model().await
    } else {
        None
    }
    .or_else(|| model.clone())
    .unwrap_or_default();
    let source_language = super::common::transcription_source_language_hint(language.as_deref());

    save_import_transcripts(
        &app,
        &mut folder_guard,
        &title,
        &dest_filename,
        Some(duration_seconds),
        &all_transcripts,
        used_provider,
        &used_model,
        source_language.as_deref(),
        failed_segments,
        options,
    )
    .await
}

async fn prepare_cloud_import_timing_grid<R: Runtime>(
    app: &AppHandle<R>,
    audio_path: &Path,
    cloud_segments: &[TranscribedSegment],
) -> Result<(f64, Vec<TranscribedSegment>)> {
    emit_progress(app, "decoding", 75, "Preparing cloud transcript timing...");

    if IMPORT_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Import cancelled"));
    }

    let prepared_audio = super::batch_audio::prepare(
        audio_path,
        VAD_REDEMPTION_TIME_MS,
        MAX_TRANSCRIPTION_SEGMENT_SAMPLES,
        &IMPORT_CANCELLED,
    )
    .await?;
    let duration_seconds = prepared_audio.duration_seconds;
    let timing_grid = prepared_audio.timing_grid();
    if timing_grid.is_empty() {
        warn!("Cloud provider returned collapsed transcript output, but local VAD found no timing grid; preserving cloud transcript as returned");
        return Ok((duration_seconds, cloud_segments.to_vec()));
    }

    let timed_segments = split_transcripts_to_timing_grid(cloud_segments, &timing_grid);
    if timed_segments.is_empty() {
        warn!("Cloud provider returned collapsed transcript output, but text could not be split onto local timing grid; preserving cloud transcript as returned");
        return Ok((duration_seconds, cloud_segments.to_vec()));
    }

    info!("Applied local VAD timing to imported cloud transcript");

    Ok((duration_seconds, timed_segments))
}

async fn save_import_transcripts<R: Runtime>(
    app: &AppHandle<R>,
    folder_guard: &mut ImportFolderGuard,
    title: &str,
    dest_filename: &str,
    duration_seconds: Option<f64>,
    all_transcripts: &[TranscribedSegment],
    used_provider: &str,
    used_model: &str,
    transcription_source_language: Option<&str>,
    failed_segments: usize,
    options: &ImportOptions,
) -> Result<ImportResult> {
    begin_import_save()?;
    let meeting_folder = folder_guard.path.as_path();
    if !owns_import_folder(meeting_folder, &options.meeting_id) {
        return Err(anyhow!("Could not verify the import folder"));
    }
    emit_progress(app, "saving", 85, "Creating meeting...");

    // Create transcript segments, then stitch obvious VAD fragments so imported
    // transcripts do not save one-word rows for natural sentence continuations.
    let mut segments = create_readable_transcript_segments_with_words(all_transcripts);
    let vocabulary = super::vocabulary_settings::load_vocabulary(app);
    super::vocabulary_settings::correct_segments(&vocabulary, &mut segments);

    // Save to database
    let app_state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("App state not available"))?;

    let meeting_id = options.meeting_id.clone();
    let metadata = import_metadata(
        &meeting_id,
        title,
        duration_seconds,
        dest_filename,
        "import",
        used_provider,
        used_model,
        transcription_source_language,
        options.date,
        options.summary_language.as_deref(),
    );
    insert_import(
        app_state.db_manager.pool(),
        &meeting_id,
        title,
        &segments,
        meeting_folder.to_string_lossy().to_string(),
        failed_segments > 0,
        options.date,
        duration_seconds,
        Some(&metadata),
    )
    .await?;
    folder_guard.committed = true;
    if failed_segments > 0 {
        let _ = app.emit("import-warning", ImportWarning {
            warning: format!("Imported with incomplete transcription: {failed_segments} segments failed after retry."),
            details: Some("The audio was saved. You can retry transcription from the meeting.".into()),
        });
    }

    // Write transcripts.json and metadata.json to the meeting folder
    emit_progress(app, "saving", 90, "Writing transcript files...");
    let roots = super::recording_preferences::recording_roots(app).await;
    let files_pending = crate::database::transcript_edits::sync_file_with_profile(
        app_state.db_manager.pool(),
        &meeting_id,
        &roots,
        |_| Ok(None),
    )
    .await
    .is_err();
    if files_pending {
        let _ = app.emit(
            "import-warning",
            ImportWarning {
                warning: "The meeting was saved. Its recording-folder files need updating.".into(),
                details: Some("Open the saved meeting to retry the folder copy.".into()),
            },
        );
    }

    emit_progress(app, "complete", 100, "Meeting saved");

    Ok(ImportResult {
        meeting_id,
        title: title.to_string(),
        segments_count: segments.len(),
        duration_seconds: duration_seconds.unwrap_or_default(),
        files_pending,
        transcription_provider: Some(used_provider.to_string()),
        transcription_incomplete: failed_segments > 0,
    })
}

/// Emit progress event
fn emit_progress<R: Runtime>(app: &AppHandle<R>, stage: &str, progress: u32, message: &str) {
    let progress = ImportProgress {
        stage: stage.to_string(),
        progress_percentage: progress,
        message: message.to_string(),
    };
    queue::set_progress(progress.clone());
    let _ = app.emit("import-progress", progress);
}

/// Create a new meeting with transcripts in the database
pub(crate) async fn create_meeting_with_transcripts(
    pool: &sqlx::SqlitePool,
    title: &str,
    segments: &[TranscriptSegment],
    folder_path: String,
    transcription_incomplete: bool,
    date: ImportDate,
    duration_seconds: Option<f64>,
) -> Result<String> {
    let meeting_id = new_meeting_id();
    insert_import(
        pool,
        &meeting_id,
        title,
        segments,
        folder_path,
        transcription_incomplete,
        date,
        duration_seconds,
        None,
    )
    .await?;
    Ok(meeting_id)
}

fn new_meeting_id() -> String {
    format!("meeting-{}", Uuid::new_v4())
}

#[allow(clippy::too_many_arguments)]
async fn insert_import(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
    title: &str,
    segments: &[TranscriptSegment],
    folder_path: String,
    transcription_incomplete: bool,
    date: ImportDate,
    duration_seconds: Option<f64>,
    metadata: Option<&serde_json::Value>,
) -> Result<()> {
    let provenance = super::provenance::RecordingProvenance::import(duration_seconds);

    // Start transaction
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| anyhow!("DB error: {}", e))?;
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(|e| anyhow!("Failed to start transaction: {}", e))?;

    // Insert meeting
    sqlx::query(
        "INSERT INTO meetings (id, title, created_at, updated_at, folder_path,
                               recording_source, recording_duration_s)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(meeting_id)
    .bind(title)
    .bind(date.meeting_date())
    .bind(date.imported_at)
    .bind(&folder_path)
    .bind(provenance.source)
    .bind(provenance.duration_seconds)
    .execute(&mut *tx)
    .await
    .map_err(|e| anyhow!("Failed to create meeting: {}", e))?;

    // Insert transcripts
    for segment in segments {
        let word_timestamps_json = segment
            .word_timestamps
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| anyhow!("Invalid word timestamps: {}", e))?;
        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker, word_timestamps_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&segment.id)
        .bind(meeting_id)
        .bind(&segment.text)
        .bind(&segment.timestamp)
        .bind(segment.audio_start_time)
        .bind(segment.audio_end_time)
        .bind(segment.duration)
        .bind(&segment.speaker)
        .bind(word_timestamps_json)
        .execute(&mut *tx)
        .await
        .map_err(|e| anyhow!("Failed to insert transcript: {}", e))?;
    }

    if transcription_incomplete || metadata.is_some() {
        sqlx::query("INSERT INTO recording_outcomes (meeting_id, transcription_incomplete, recording_files_incomplete) VALUES (?, ?, ?)")
            .bind(meeting_id).bind(transcription_incomplete).bind(metadata.is_some()).execute(&mut *tx).await?;
    }
    if let Some(metadata) = metadata {
        let payload = crate::database::transcript_edits::import_file_payload(metadata)
            .map_err(anyhow::Error::msg)?;
        sqlx::query(
            "INSERT INTO transcript_file_sync (meeting_id, retranscription_json) VALUES (?, ?)",
        )
        .bind(meeting_id)
        .bind(payload)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit()
        .await
        .map_err(|e| anyhow!("Failed to commit transaction: {}", e))?;

    info!(
        "Created meeting '{}' with {} transcripts",
        meeting_id,
        segments.len()
    );

    Ok(())
}

type LocalModel = (&'static str, String);

fn choose_local_fallback<'a>(
    configured: Option<(&str, &str)>,
    installed: &'a [LocalModel],
) -> Option<&'a LocalModel> {
    configured
        .and_then(|(provider, model)| {
            let provider = if provider == "whisper" {
                "localWhisper"
            } else {
                provider
            };
            installed
                .iter()
                .find(|(id, name)| *id == provider && name == model)
        })
        .or_else(|| {
            installed.iter().find(|(provider, model)| {
                *provider == "parakeet" && model == DEFAULT_PARAKEET_MODEL
            })
        })
        .or_else(|| installed.first())
}

pub(super) async fn resolve_local_fallback<R: Runtime>(
    app: &AppHandle<R>,
    cloud_error: Option<&super::transcription::cloud::CloudTranscriptionError>,
) -> Result<LocalModel> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("Meeting storage is unavailable"))?;
    let configured = SettingsRepository::get_transcript_config(state.db_manager.pool())
        .await
        .unwrap_or_else(|_| {
            warn!("Could not read the configured local transcription model");
            None
        });
    let preferred = configured
        .as_ref()
        .map(|config| (config.provider.as_str(), config.model.as_str()));
    let (parakeet, whisper, nemotron) = tokio::join!(
        crate::parakeet_engine::commands::parakeet_get_available_models(),
        crate::whisper_engine::commands::whisper_get_available_models(),
        crate::nemotron_engine::commands::nemotron_get_available_models(app.clone()),
    );
    let mut installed = Vec::new();
    installed.extend(
        parakeet
            .unwrap_or_default()
            .into_iter()
            .filter(|model| matches!(model.status, crate::parakeet_engine::ModelStatus::Available))
            .map(|model| ("parakeet", model.name)),
    );
    installed.extend(
        whisper
            .unwrap_or_default()
            .into_iter()
            .filter(|model| matches!(model.status, crate::whisper_engine::ModelStatus::Available))
            .map(|model| ("localWhisper", model.name)),
    );
    installed.extend(
        nemotron
            .unwrap_or_default()
            .into_iter()
            .filter(|model| matches!(model.status, crate::parakeet_engine::ModelStatus::Available))
            .map(|model| ("nemotron", model.name)),
    );
    installed.sort_unstable();
    let choice = choose_local_fallback(preferred, &installed).cloned().ok_or_else(|| {
        let error = anyhow!("No installed local transcription model is available. Download one in Transcription settings.");
        match cloud_error {
            Some(cloud_error) => super::transcription::cloud::local_fallback_error_context(cloud_error, error),
            None => error,
        }
    })?;
    info!("Using local transcription fallback ({})", choice.0);
    Ok(choice)
}

pub(super) async fn unload_local_batch_engine(provider: Option<&str>) {
    if !super::transcription::cloud::is_cloud_provider(provider) {
        super::common::unload_engine_after_batch_for(
            provider == Some("parakeet"),
            provider == Some("nemotron"),
        )
        .await;
    }
}

pub(super) fn local_batch_language(
    provider: Option<&str>,
    language: Option<String>,
) -> Option<String> {
    if provider == Some("parakeet")
        || (provider == Some("nemotron") && language.as_deref() == Some("auto-translate"))
    {
        return None;
    }
    language
}

/// Get or initialize the Whisper engine
async fn get_or_init_whisper<R: Runtime>(
    app: &AppHandle<R>,
    requested_model: Option<&str>,
) -> Result<Arc<WhisperEngine>> {
    use crate::whisper_engine::commands::WHISPER_ENGINE;

    let engine = {
        let guard = WHISPER_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().cloned()
    };

    match engine {
        Some(e) => {
            let target_model = match requested_model {
                Some(model) => model.to_string(),
                None => get_configured_model(app, "whisper").await?,
            };

            let current_model = e.get_current_model().await;
            let needs_load = match &current_model {
                Some(loaded) => loaded != &target_model,
                None => true,
            };

            if needs_load {
                info!(
                    "Loading Whisper model '{}' (current: {:?})",
                    target_model, current_model
                );

                if let Err(_e) = e.discover_models().await {
                    warn!("Model discovery error (continuing)");
                }

                e.load_model(&target_model)
                    .await
                    .map_err(|e| anyhow!("Failed to load model '{}': {}", target_model, e))?;
            }

            Ok(e)
        }
        None => Err(anyhow!("Whisper engine not initialized")),
    }
}

/// Get or initialize the Parakeet engine
async fn get_or_init_parakeet<R: Runtime>(
    app: &AppHandle<R>,
    requested_model: Option<&str>,
) -> Result<Arc<ParakeetEngine>> {
    use crate::parakeet_engine::commands::PARAKEET_ENGINE;

    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().cloned()
    };

    match engine {
        Some(e) => {
            let target_model = match requested_model {
                Some(model) => model.to_string(),
                None => get_configured_model(app, "parakeet").await?,
            };

            let current_model = e.get_current_model().await;
            let needs_load = match &current_model {
                Some(loaded) => loaded != &target_model,
                None => true,
            };

            if needs_load {
                info!(
                    "Loading Parakeet model '{}' (current: {:?})",
                    target_model, current_model
                );

                if let Err(_e) = e.discover_models().await {
                    warn!("Model discovery error (continuing)");
                }

                e.load_model(&target_model)
                    .await
                    .map_err(|e| anyhow!("Failed to load model '{}': {}", target_model, e))?;
            }

            Ok(e)
        }
        None => Err(anyhow!("Parakeet engine not initialized")),
    }
}

/// Get or initialize the Nemotron engine for a batch import.
async fn get_or_init_nemotron<R: Runtime>(
    app: &AppHandle<R>,
    requested_model: Option<&str>,
) -> Result<Arc<crate::nemotron_engine::nemotron_engine::NemotronEngine>> {
    use crate::nemotron_engine::commands::{nemotron_init, NEMOTRON_ENGINE};
    use crate::nemotron_engine::nemotron_engine::NEMOTRON_MODEL;

    nemotron_init(app.clone())
        .await
        .map_err(|e| anyhow!("Failed to initialize Nemotron engine: {}", e))?;

    let engine = {
        let guard = NEMOTRON_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().cloned()
    };

    match engine {
        Some(e) => {
            let target_model = requested_model.unwrap_or(NEMOTRON_MODEL).to_string();
            let current_model = e.get_current_model().await;
            let needs_load = current_model.as_deref() != Some(target_model.as_str());
            if needs_load {
                if let Err(_err) = e.discover_models().await {
                    warn!("Nemotron model discovery error (continuing)");
                }
                e.load_model(&target_model).await.map_err(|err| {
                    anyhow!("Failed to load Nemotron model '{}': {}", target_model, err)
                })?;
            }
            Ok(e)
        }
        None => Err(anyhow!("Nemotron engine not initialized")),
    }
}

/// Get the configured model from database
async fn get_configured_model<R: Runtime>(
    app: &AppHandle<R>,
    provider_type: &str,
) -> Result<String> {
    let app_state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("App state not available"))?;

    let result: Option<(String, String)> =
        sqlx::query_as("SELECT provider, model FROM transcript_settings WHERE id = '1'")
            .fetch_optional(app_state.db_manager.pool())
            .await
            .map_err(|e| anyhow!("Failed to query config: {}", e))?;

    match result {
        Some((provider, model)) => {
            if (provider_type == "whisper" && (provider == "localWhisper" || provider == "whisper"))
                || (provider_type == "parakeet" && provider == "parakeet")
            {
                Ok(model)
            } else {
                // Return default model for the requested type
                Ok(if provider_type == "parakeet" {
                    DEFAULT_PARAKEET_MODEL.to_string()
                } else {
                    DEFAULT_WHISPER_MODEL.to_string()
                })
            }
        }
        None => Ok(if provider_type == "parakeet" {
            DEFAULT_PARAKEET_MODEL.to_string()
        } else {
            DEFAULT_WHISPER_MODEL.to_string()
        }),
    }
}

/// Write metadata.json to a meeting folder (atomic write with temp file)
fn import_metadata(
    meeting_id: &str,
    title: &str,
    duration_seconds: Option<f64>,
    audio_filename: &str,
    source: &str,
    transcription_provider: &str,
    transcription_model: &str,
    transcription_source_language: Option<&str>,
    date: ImportDate,
    summary_language: Option<&str>,
) -> serde_json::Value {
    let mut json = serde_json::json!({
        "version": "1.0",
        "meeting_id": meeting_id,
        "meeting_name": title,
        "created_at": date.meeting_date().to_rfc3339(),
        "completed_at": chrono::Utc::now().to_rfc3339(),
        "imported_at": date.imported_at.to_rfc3339(),
        "meeting_date_source": date.provenance(),
        "duration_seconds": duration_seconds,
        "duration_measured": duration_seconds.is_some(),
        "audio_file": audio_filename,
        "transcript_file": "transcripts.json",
        "status": "completed",
        "source": source,
        // Which transcription engine + model actually ran (no silent fallback).
        "transcription_provider": transcription_provider,
        "transcription_model": transcription_model
    });
    if let Some(language) = transcription_source_language {
        json["transcription_source_language"] = serde_json::json!(language);
    }

    if let Some(language) = summary_language {
        json["summary_language"] = serde_json::json!(language);
    }
    json
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// Select an audio file and validate it
#[tauri::command]
pub async fn select_and_validate_audio_command<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Option<AudioFileInfo>, String> {
    info!("Opening file dialog for audio import");

    // Use spawn_blocking to avoid blocking async runtime
    let app_clone = app.clone();
    let file_path = tokio::task::spawn_blocking(move || {
        app_clone
            .dialog()
            .file()
            .add_filter(
                "Audio Files",
                &AUDIO_EXTENSIONS.iter().map(|s| *s).collect::<Vec<_>>(),
            )
            .blocking_pick_file()
    })
    .await
    .map_err(|e| format!("File dialog task failed: {}", e))?;

    match file_path {
        Some(path) => {
            let path_str = path.to_string();
            info!("Audio file selected");
            // The user chose it in a dialog the core opened: the import accepts
            // this file once, and no other path.
            crate::file_grants::grant(
                crate::file_grants::Purpose::AudioImport,
                Path::new(&path_str),
            )?;

            match validate_audio_file(Path::new(&path_str)) {
                Ok(info) => Ok(Some(info)),
                Err(e) => {
                    error!("Validation failed");
                    Err(e.to_string())
                }
            }
        }
        None => {
            info!("User cancelled file selection");
            Ok(None)
        }
    }
}

/// Choose and validate a small set in the core. Each selected file retains
/// its own one-use grant and is reviewed and imported separately.
#[tauri::command]
pub async fn select_audio_files_command<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<AudioFileInfo>, String> {
    tokio::task::spawn_blocking(move || {
        const MAX_SELECTION: usize = 12;
        let paths = app
            .dialog()
            .file()
            .add_filter("Audio Files", AUDIO_EXTENSIONS)
            .blocking_pick_files()
            .unwrap_or_default();
        if paths.len() > MAX_SELECTION {
            return Err(format!(
                "Choose up to {MAX_SELECTION} audio files at a time."
            ));
        }
        let files: Vec<AudioFileInfo> = paths
            .iter()
            .map(|path| {
                validate_audio_file(Path::new(&path.to_string())).map_err(|error| error.to_string())
            })
            .collect::<Result<_, String>>()?;
        for file in &files {
            crate::file_grants::grant(
                crate::file_grants::Purpose::AudioImport,
                Path::new(&file.path),
            )?;
        }
        Ok(files)
    })
    .await
    .map_err(|_| "Could not select audio files".to_string())?
}

/// Validate an audio file from a given path (for drag-drop)
#[tauri::command]
pub async fn validate_audio_file_command(path: String) -> Result<AudioFileInfo, String> {
    info!("Validating selected audio file");
    // Only a file the user chose (or dropped on the window) and the core saw.
    let granted = crate::file_grants::peek(crate::file_grants::Purpose::AudioImport, &path)?;
    validate_audio_file(&granted).map_err(|e| e.to_string())
}

fn accept_import_segment<T>(
    result: Result<T>,
    segment: usize,
    consecutive: &mut usize,
    failed: &mut usize,
) -> Result<Option<T>> {
    match result {
        Ok(value) => {
            *consecutive = 0;
            Ok(Some(value))
        }
        Err(error) => {
            *consecutive += 1;
            *failed += 1;
            // Engine errors contain failure diagnostics, never successful text.
            warn!(
                "Import segment {} failed after retry: {}",
                segment,
                crate::summary::codex_provider::redact_secrets(&error.to_string())
            );
            if *consecutive >= 3 {
                Err(error)
            } else {
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::common::{create_transcript_segments, write_transcripts_json};
    use super::*;
    use crate::audio::common::split_segment_at_silence;

    #[test]
    fn local_fallback_prefers_available_config_then_default_parakeet() {
        let installed = [
            ("localWhisper", "small".to_owned()),
            ("parakeet", DEFAULT_PARAKEET_MODEL.to_owned()),
            ("nemotron", "nemotron-test".to_owned()),
        ];
        assert_eq!(
            choose_local_fallback(Some(("localWhisper", "small")), &installed),
            Some(&installed[0]),
        );
        assert_eq!(
            choose_local_fallback(Some(("nemotron", "nemotron-test")), &installed),
            Some(&installed[2]),
        );
        assert_eq!(
            choose_local_fallback(Some(("nemotron", "missing")), &installed),
            Some(&installed[1]),
        );
        assert_eq!(
            choose_local_fallback(None, &installed[..1]),
            Some(&installed[0])
        );
        assert_eq!(choose_local_fallback(None, &[]), None);
    }

    #[test]
    fn copying_checks_cancel_between_bounded_writes() {
        let mut source = std::io::Cursor::new(vec![7; 3 * COPY_BUFFER_BYTES]);
        let mut destination = Vec::new();
        let cancelled = AtomicBool::new(false);
        let result = copy_audio_chunks(&mut source, &mut destination, &cancelled, |_| {
            cancelled.store(true, Ordering::Release);
        });
        assert!(result.is_err());
        assert_eq!(destination.len(), COPY_BUFFER_BYTES);
    }

    #[tokio::test]
    async fn repair_resumes_after_metadata_and_refuses_a_foreign_folder() {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let folder = create_meeting_folder(&root.path().to_path_buf(), "Import").unwrap();
        let id = new_meeting_id();
        let _owner = ImportFolderGuard::owned(folder.clone(), &id, "audio.wav").unwrap();
        std::fs::write(folder.join("audio.wav"), b"audio").unwrap();
        std::fs::create_dir(folder.join("transcripts.json")).unwrap();
        let date = ImportDate::default();
        let metadata = import_metadata(
            &id,
            "Import",
            None,
            "audio.wav",
            "import",
            "localWhisper",
            "test",
            None,
            date,
            None,
        );
        insert_import(
            &pool,
            &id,
            "Import",
            &[],
            folder.to_string_lossy().into(),
            false,
            date,
            None,
            Some(&metadata),
        )
        .await
        .unwrap();
        let roots = [root.path().to_path_buf()];
        use crate::database::transcript_edits::{import_file_stage, sync_file};
        assert!(sync_file(&pool, &id, &roots).await.is_err());
        assert_eq!(
            import_file_stage(&pool, &id).await.unwrap(),
            Some("transcript")
        );
        assert!(folder.join("metadata.json").is_file());

        std::fs::remove_dir(folder.join("transcripts.json")).unwrap();
        std::fs::write(folder.join(IMPORT_OWNER_FILE), b"foreign meeting").unwrap();
        assert!(sync_file(&pool, &id, &roots).await.is_err());
        assert!(!folder.join("transcripts.json").exists());
        std::fs::write(folder.join(IMPORT_OWNER_FILE), &id).unwrap();
        sync_file(&pool, &id, &roots).await.unwrap();
        assert!(folder.join("transcripts.json").is_file());
        assert_eq!(import_file_stage(&pool, &id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn three_consecutive_engine_failures_abort_and_remove_the_import() {
        static CANCELLED: AtomicBool = AtomicBool::new(false);
        let root = tempfile::tempdir().unwrap();
        let folder = create_meeting_folder(&root.path().to_path_buf(), "Import").unwrap();
        let mut calls = 0;
        let result: Result<()> = async {
            let mut guard =
                ImportFolderGuard::owned(folder.clone(), &new_meeting_id(), "audio.wav")?;
            std::fs::write(folder.join("audio.wav"), b"synthetic audio")?;
            guard.audio_copied = true;
            let mut consecutive = 0;
            let mut failed = 0;
            for index in 1..=10 {
                let result: Result<()> =
                    super::super::batch_audio::retry_segment(&CANCELLED, || {
                        calls += 1;
                        async move { Err(anyhow!("Engine unavailable on segment {index}")) }
                    })
                    .await;
                accept_import_segment(result, index, &mut consecutive, &mut failed)?;
            }
            Ok(())
        }
        .await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "Engine unavailable on segment 3"
        );
        assert_eq!(calls, 6);
        assert!(!folder.exists());
    }

    #[test]
    fn failed_or_cancelled_import_removes_only_its_owned_copy() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.wav");
        std::fs::write(&source, b"source audio").unwrap();
        for cancelled in [false, true] {
            let folder = create_meeting_folder(&root.path().to_path_buf(), "Import").unwrap();
            let result: Result<()> = (|| {
                let mut guard =
                    ImportFolderGuard::owned(folder.clone(), &new_meeting_id(), "audio.wav")?;
                std::fs::copy(&source, folder.join("audio.wav"))?;
                guard.audio_copied = true;
                // A segment failure propagates through the same guard as decode,
                // cloud, engine initialization and database failures.
                Err(anyhow!("Engine failed on segment 2"))
            })();
            let error = import_error(result.unwrap_err(), cancelled);
            assert_eq!(
                error.to_string(),
                if cancelled {
                    "Import cancelled"
                } else {
                    "Engine failed on segment 2"
                }
            );
            assert!(!folder.exists());
            assert!(source.exists());
        }
    }

    #[test]
    fn committed_import_keeps_audio() {
        let root = tempfile::tempdir().unwrap();
        let folder = create_meeting_folder(&root.path().to_path_buf(), "Import").unwrap();
        {
            let mut guard =
                ImportFolderGuard::owned(folder.clone(), &new_meeting_id(), "audio.wav").unwrap();
            std::fs::write(folder.join("audio.wav"), b"saved audio").unwrap();
            guard.audio_copied = true;
            guard.committed = true;
        }
        assert!(folder.join("audio.wav").exists());
    }

    #[tokio::test]
    async fn persistent_segment_failure_saves_an_incomplete_import() {
        static CANCELLED: AtomicBool = AtomicBool::new(false);
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let mut kept = Vec::new();
        let mut failed = 0;
        let mut consecutive = 0;
        for index in 0..3 {
            let result = super::super::batch_audio::retry_segment(&CANCELLED, || async {
                if index == 1 {
                    Err(anyhow!("engine failure"))
                } else {
                    Ok((
                        "kept".to_string(),
                        index as f64 * 1000.0,
                        (index + 1) as f64 * 1000.0,
                    ))
                }
            })
            .await;
            if let Some(segment) =
                accept_import_segment(result, index + 1, &mut consecutive, &mut failed).unwrap()
            {
                kept.push(segment);
            }
        }
        let folder = tempfile::tempdir().unwrap();
        let id = create_meeting_with_transcripts(
            &pool,
            "Import",
            &create_transcript_segments(&kept),
            folder.path().to_string_lossy().into(),
            failed > 0,
            ImportDate::default(),
            Some(120.5),
        )
        .await
        .unwrap();
        let incomplete: bool = sqlx::query_scalar(
            "SELECT transcription_incomplete FROM recording_outcomes WHERE meeting_id = ?",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM transcripts WHERE meeting_id = ?")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(incomplete);
        let facts: (String, f64) = sqlx::query_as(
            "SELECT recording_source, recording_duration_s FROM meetings WHERE id = ?",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(facts, ("import".into(), 120.5));
        assert_eq!(count, 2);
        assert_eq!(failed, 1);
    }

    fn write_test_wav(path: &Path) {
        let sample_rate = 16_000u32;
        let sample_count = sample_rate / 10;
        let data_len = sample_count * 2;
        let mut bytes = Vec::with_capacity(44 + data_len as usize);

        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());

        for _ in 0..sample_count {
            bytes.extend_from_slice(&0i16.to_le_bytes());
        }

        std::fs::write(path, bytes).expect("write test wav");
    }

    #[test]
    fn recovery_measures_the_selected_audio_and_ignores_legacy_transcript_duration() {
        let folder = tempfile::tempdir().unwrap();
        let marker = serde_json::json!({
            "recording_mode": "audio_only", "duration_seconds": 12.0,
        });
        let measure = |marker: &serde_json::Value| {
            super::super::interrupted::recovered_provenance(folder.path(), marker, None)
        };
        assert_eq!(measure(&marker).source, Some("recording"));
        assert_eq!(measure(&marker).duration_seconds, None);
        write_test_wav(&folder.path().join("unrelated.wav"));
        assert_eq!(measure(&marker).duration_seconds, None);
        write_test_wav(&folder.path().join("audio-recovered.wav"));
        assert_eq!(measure(&marker).duration_seconds, Some(0.1));
    }

    #[test]
    fn test_audio_extensions() {
        assert!(AUDIO_EXTENSIONS.contains(&"mp4"));
        assert!(AUDIO_EXTENSIONS.contains(&"wav"));
        assert!(AUDIO_EXTENSIONS.contains(&"mp3"));
        assert!(!AUDIO_EXTENSIONS.contains(&"txt"));
    }

    #[test]
    fn test_create_transcript_segments_empty() {
        let transcripts: Vec<(String, f64, f64)> = vec![];
        let segments = create_transcript_segments(&transcripts);
        assert!(segments.is_empty());
    }

    #[test]
    fn test_create_transcript_segments_single() {
        let transcripts = vec![("Hello world".to_string(), 0.0, 1500.0)];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello world");
        assert_eq!(segments[0].audio_start_time, Some(0.0));
        assert_eq!(segments[0].audio_end_time, Some(1.5));
    }

    #[test]
    fn test_cancellation_flag() {
        IMPORT_CANCELLED.store(false, Ordering::SeqCst);
        IMPORT_IN_PROGRESS.store(false, Ordering::SeqCst);

        assert!(!is_import_in_progress());

        cancel_import();
        assert!(IMPORT_CANCELLED.load(Ordering::SeqCst));

        // Reset
        IMPORT_CANCELLED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn test_extract_duration_from_metadata_wav() {
        let test_path = std::env::temp_dir().join("clawscribe-test-metadata.wav");
        write_test_wav(&test_path);

        let result = extract_duration_from_metadata(&test_path);
        let _ = std::fs::remove_file(&test_path);

        assert!(result.is_ok());
        let duration = result.unwrap();
        assert!(
            duration > 0.0 && duration < 1.0,
            "Duration {} seems unreasonable",
            duration
        );
    }

    // End-to-end demux probe of a real M4A container. This exercises the
    // time_base branch of duration_from_params: Symphonia's ISOBMFF reader sets
    // time_base from the mdhd time-scale box, so n_frames are ticks — not PCM
    // sample counts.  Using sample_rate alone would give the wrong answer for
    // HE-AAC whose SBR doubling makes the codec rate differ from the container
    // time-scale.
    #[test]
    fn extract_duration_from_m4a_fixture_uses_time_base_path() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/he_aac_48k_5s.m4a");
        let duration = extract_duration_from_metadata(&path)
            .expect("should probe m4a duration without full decode");
        assert!(
            (duration - 5.16).abs() < 0.5,
            "M4A duration {:.2}s outside expected ~5.16s",
            duration
        );
    }

    #[test]
    fn test_validate_audio_file_with_metadata() {
        let test_path = std::env::temp_dir().join("clawscribe-test-validation.wav");
        write_test_wav(&test_path);

        let result = validate_audio_file(&test_path);
        let _ = std::fs::remove_file(&test_path);

        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.format, "WAV");
        assert!(info.duration_seconds > 0.0);
        assert!(info.size_bytes > 0);
    }

    #[test]
    fn test_validate_audio_file_nonexistent() {
        let missing = std::env::temp_dir()
            .join("clawscribe-missing-folder")
            .join("file.mp4");
        let result = validate_audio_file(&missing);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
        // A network or relative path is refused before the filesystem is asked.
        // Only a granted path is accepted by the commands.
        assert!(crate::file_grants::peek(
            crate::file_grants::Purpose::AudioImport,
            "//host/share/never-granted.mp4"
        )
        .is_err());
        assert!(crate::file_grants::take(
            crate::file_grants::Purpose::AudioImport,
            "relative/file.mp4"
        )
        .is_err());
    }

    #[test]
    fn test_validate_audio_file_wrong_extension() {
        // Create a temporary file with wrong extension
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join("test_audio.txt");
        let _ = std::fs::write(&temp_file, b"dummy content");

        let result = validate_audio_file(&temp_file);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unsupported format"));

        // Cleanup
        let _ = std::fs::remove_file(temp_file);
    }

    #[test]
    fn test_split_segment_at_silence_short_segment() {
        // Segment shorter than max — returned as-is
        let segment = crate::audio::vad::SpeechSegment {
            samples: vec![0.1; 16000], // 1 second
            start_timestamp_ms: 0.0,
            end_timestamp_ms: 1000.0,
            confidence: 0.9,
        };
        let result = split_segment_at_silence(&segment, 25 * 16000);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].samples.len(), 16000);
    }

    #[test]
    fn test_split_segment_at_silence_splits_long_segment() {
        // 60-second segment of low-level noise with a silent gap at ~25s
        let mut samples = vec![0.01f32; 60 * 16000];
        // Insert silence at 25 seconds (sample 400000)
        for i in (25 * 16000)..(25 * 16000 + 3200) {
            samples[i] = 0.0;
        }
        let segment = crate::audio::vad::SpeechSegment {
            samples,
            start_timestamp_ms: 0.0,
            end_timestamp_ms: 60_000.0,
            confidence: 0.9,
        };

        let result = split_segment_at_silence(&segment, 25 * 16000);
        assert!(
            result.len() >= 2,
            "Should split into at least 2 segments, got {}",
            result.len()
        );

        // All sub-segments should have samples
        for (i, seg) in result.iter().enumerate() {
            assert!(!seg.samples.is_empty(), "Segment {} is empty", i);
            assert!(
                seg.start_timestamp_ms < seg.end_timestamp_ms,
                "Segment {} has invalid timestamps: {} >= {}",
                i,
                seg.start_timestamp_ms,
                seg.end_timestamp_ms
            );
        }
    }

    #[test]
    fn test_split_segment_at_silence_no_silence_uses_overlap() {
        // Continuous speech (constant energy) — should still split with overlap
        let segment = crate::audio::vad::SpeechSegment {
            samples: vec![0.5f32; 60 * 16000], // 60 seconds of "speech"
            start_timestamp_ms: 0.0,
            end_timestamp_ms: 60_000.0,
            confidence: 0.9,
        };

        let result = split_segment_at_silence(&segment, 25 * 16000);
        assert!(result.len() >= 2);

        // Total samples should exceed input due to overlap
        let total_samples: usize = result.iter().map(|s| s.samples.len()).sum();
        assert!(
            total_samples >= 60 * 16000,
            "Overlap should not lose samples"
        );
    }

    #[test]
    fn test_write_transcripts_json() {
        let dir = tempfile::tempdir().unwrap();
        let segments = vec![
            TranscriptSegment {
                id: "t-1".to_string(),
                text: "Hello world".to_string(),
                timestamp: "2024-01-01T00:00:00Z".to_string(),
                audio_start_time: Some(0.0),
                audio_end_time: Some(1.5),
                duration: Some(1.5),
                speaker: Some("Participants".to_string()),
                word_timestamps: None,
            },
            TranscriptSegment {
                id: "t-2".to_string(),
                text: "Second segment".to_string(),
                timestamp: "2024-01-01T00:00:01Z".to_string(),
                audio_start_time: Some(2.0),
                audio_end_time: Some(3.5),
                duration: Some(1.5),
                speaker: None,
                word_timestamps: None,
            },
        ];

        let result = write_transcripts_json(dir.path(), &segments);
        assert!(
            result.is_ok(),
            "write_transcripts_json failed: {:?}",
            result
        );

        // Verify file exists and is valid JSON
        let path = dir.path().join("transcripts.json");
        assert!(path.exists());

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["total_segments"], 2);
        assert_eq!(parsed["version"], "1.0");
        assert_eq!(parsed["segments"][0]["text"], "Hello world");
        assert_eq!(parsed["segments"][1]["text"], "Second segment");
        assert_eq!(parsed["segments"][0]["speaker"], "Participants");
        assert!(parsed["segments"][1]["speaker"].is_null());
        assert_eq!(parsed["segments"][0]["sequence_id"], 0);
        assert_eq!(parsed["segments"][1]["sequence_id"], 1);

        // Verify temp file was cleaned up
        assert!(!dir.path().join(".transcripts.json.tmp").exists());
    }

    #[tokio::test]
    async fn test_create_meeting_with_transcripts_persists_speaker() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let segments = vec![TranscriptSegment {
            id: "t-speaker".to_string(),
            text: "Hello from system audio".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            audio_start_time: Some(0.0),
            audio_end_time: Some(1.5),
            duration: Some(1.5),
            speaker: Some("Participants".to_string()),
            word_timestamps: None,
        }];

        let meeting_id = create_meeting_with_transcripts(
            &pool,
            "Speaker Persistence",
            &segments,
            "C:\\recordings\\speaker-persistence".to_string(),
            false,
            ImportDate::default(),
            Some(1.5),
        )
        .await
        .unwrap();

        let speaker: Option<String> =
            sqlx::query_scalar("SELECT speaker FROM transcripts WHERE meeting_id = ? AND id = ?")
                .bind(&meeting_id)
                .bind("t-speaker")
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(speaker.as_deref(), Some("Participants"));
        let provenance: (String, f64) = sqlx::query_as(
            "SELECT recording_source, recording_duration_s FROM meetings WHERE id = ?",
        )
        .bind(&meeting_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(provenance, ("import".into(), 1.5));
    }

    #[test]
    fn test_import_metadata() {
        let parsed = import_metadata(
            "meeting-123",
            "Test Meeting",
            Some(1800.0),
            "audio.mp4",
            "import",
            "parakeet",
            "parakeet-tdt-0.6b-v3-int8",
            Some("en"),
            ImportDate {
                meeting_at: Some("2025-09-15T13:05:00Z".parse().unwrap()),
                imported_at: "2025-11-22T10:00:00Z".parse().unwrap(),
                from_file: true,
            },
            None,
        );

        assert_eq!(parsed["version"], "1.0");
        assert_eq!(parsed["meeting_id"], "meeting-123");
        assert_eq!(parsed["meeting_name"], "Test Meeting");
        assert_eq!(parsed["duration_seconds"], 1800.0);
        assert_eq!(parsed["audio_file"], "audio.mp4");
        assert_eq!(parsed["status"], "completed");
        assert_eq!(parsed["source"], "import");
        assert_eq!(parsed["created_at"], "2025-09-15T13:05:00+00:00");
        assert_eq!(parsed["imported_at"], "2025-11-22T10:00:00+00:00");
        assert_eq!(parsed["meeting_date_source"], "file_modified_time");
        assert_eq!(parsed["transcription_provider"], "parakeet");
        assert_eq!(parsed["transcription_model"], "parakeet-tdt-0.6b-v3-int8");
        assert_eq!(parsed["transcription_source_language"], "en");
    }

    /// Integration test that decodes a real audio file and runs VAD.
    /// Run with: TEST_AUDIO_PATH=/path/to/audio.mp4 cargo test -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_import_pipeline_decode_vad() {
        let audio_path = std::env::var("TEST_AUDIO_PATH")
            .expect("Set TEST_AUDIO_PATH to run this integration test");

        let path = Path::new(&audio_path);
        assert!(path.exists(), "Audio file not found: {}", audio_path);

        // Step 1: Decode
        println!("Decoding {}...", audio_path);
        let decoded =
            crate::audio::decoder::decode_audio_file(path).expect("Failed to decode audio file");
        println!(
            "Decoded: {:.2}s, {}Hz, {} channels, {} samples",
            decoded.duration_seconds,
            decoded.sample_rate,
            decoded.channels,
            decoded.samples.len()
        );

        // Step 2: Resample to 16kHz mono
        println!("Resampling to 16kHz mono...");
        let duration_seconds = decoded.duration_seconds;
        let samples = decoded.to_whisper_format();
        println!(
            "Resampled: {} samples ({:.2}s at 16kHz)",
            samples.len(),
            samples.len() as f64 / 16000.0
        );

        // Step 3: Run VAD with common redemption times and compare
        for redemption_ms in [400u32, 500, 2000] {
            println!("\n--- VAD with redemption_time={}ms ---", redemption_ms);
            let segments = crate::audio::vad::get_speech_chunks_with_progress(
                &samples,
                redemption_ms,
                |progress, count| {
                    if progress % 20 == 0 {
                        println!("  VAD progress: {}% ({} segments)", progress, count);
                    }
                    true
                },
            )
            .expect("VAD failed");

            let total_segments = segments.len();
            println!("Found {} segments", total_segments);

            if !segments.is_empty() {
                let durations: Vec<f64> = segments
                    .iter()
                    .map(|s| s.end_timestamp_ms - s.start_timestamp_ms)
                    .collect();
                let total_speech: f64 = durations.iter().sum();
                let avg = total_speech / durations.len() as f64;
                let min = durations.iter().cloned().fold(f64::INFINITY, f64::min);
                let max = durations.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

                println!(
                    "Stats: avg={:.0}ms, min={:.0}ms, max={:.0}ms, total_speech={:.1}s/{:.1}s ({:.0}%)",
                    avg, min, max,
                    total_speech / 1000.0,
                    duration_seconds,
                    (total_speech / 1000.0 / duration_seconds) * 100.0
                );

                // Segments over 25s that would be split
                let oversized = durations.iter().filter(|d| **d > 25_000.0).count();
                println!("Segments >25s (would be split): {}", oversized);

                // Basic sanity checks
                assert!(total_speech > 0.0, "No speech detected");
                for (i, seg) in segments.iter().enumerate() {
                    assert!(!seg.samples.is_empty(), "Segment {} has no samples", i);
                    assert!(
                        seg.end_timestamp_ms > seg.start_timestamp_ms,
                        "Segment {} has invalid timestamps",
                        i
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod phase_tests {
    use super::*;

    /// A cancel from a starting recording is accepted only while the import
    /// can still stop, and reports false once it is saving or over, so no
    /// "being stopped" notice is sent for a job that has already succeeded.
    #[test]
    fn cancel_is_accepted_only_before_the_save_and_never_after_the_job_ends() {
        set_import_phase(Phase::Finished);
        assert!(!cancel_import_if_possible(), "no job is running");
        {
            let _phase = PhaseGuard::open();
            assert!(
                cancel_import_if_possible(),
                "before the save a cancel takes effect"
            );
            assert!(cancel_import_if_possible(), "repeats stay accepted");
            set_import_phase(Phase::Saving);
            assert!(
                !cancel_import_if_possible(),
                "a save in progress cannot be cancelled"
            );
        }
        // Dropping the guard (declared after the job registration) ends the
        // window before the registration is cleared.
        assert!(
            !cancel_import_if_possible(),
            "the finished job refuses a cancel"
        );
    }

    // ── duration_from_params via TimeBase::calc_time ───────────────────────

    fn tb(numer: u32, denom: u32) -> symphonia::core::units::TimeBase {
        symphonia::core::units::TimeBase { numer, denom }
    }

    // Matroska/WebM: n_frames are nanosecond ticks (time_base = 1/1_000_000_000).
    // sample_rate = 48_000 would give 5_000_000_000 / 48_000 ≈ 104_167 s — wrong.
    // calc_time must yield exactly 5 s.
    #[test]
    fn duration_matroska_nanosecond_time_base_ignores_sample_rate() {
        let secs = duration_from_params(Some(tb(1, 1_000_000_000)), Some(48_000), 5_000_000_000);
        assert!((secs.unwrap() - 5.0).abs() < 1e-6, "got {:?}", secs);
    }

    // Matroska/WebM millisecond time_base (common in WebM containers).
    // sample_rate = 44_100 would give 5_000 / 44_100 ≈ 0.113 s — wrong.
    #[test]
    fn duration_matroska_millisecond_time_base() {
        let secs = duration_from_params(Some(tb(1, 1_000)), Some(44_100), 5_000);
        assert!((secs.unwrap() - 5.0).abs() < 1e-6, "got {:?}", secs);
    }

    // MP4: 90 kHz tick clock — common for ISOBMFF video and some audio tracks.
    // sample_rate = 44_100 would give 450_000 / 44_100 ≈ 10.2 s — wrong.
    #[test]
    fn duration_mp4_90khz_time_base_ignores_sample_rate() {
        let secs = duration_from_params(Some(tb(1, 90_000)), Some(44_100), 450_000);
        assert!((secs.unwrap() - 5.0).abs() < 1e-6, "got {:?}", secs);
    }

    // WAV has no time_base; sample_rate is the correct fallback.
    #[test]
    fn duration_wav_no_time_base_uses_sample_rate() {
        let secs = duration_from_params(None, Some(44_100), 220_500);
        assert!((secs.unwrap() - 5.0).abs() < 1e-6, "got {:?}", secs);
    }

    // No time_base and no sample_rate → None.
    #[test]
    fn duration_no_params_returns_none() {
        assert!(duration_from_params(None, None, 44_100).is_none());
    }

    // Zero n_frames → None (not a valid duration).
    #[test]
    fn duration_zero_frames_returns_none() {
        assert!(duration_from_params(Some(tb(1, 1_000)), Some(44_100), 0).is_none());
    }
}
