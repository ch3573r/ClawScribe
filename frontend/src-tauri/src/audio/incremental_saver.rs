use super::constants::AUDIO_EXTENSIONS;
use anyhow::{anyhow, Result};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// Legacy checkpoints were emitted every ten seconds. Used only for recovery estimates.
const CHECKPOINT_INTERVAL_SECONDS: usize = 10;
const CHECKPOINT_FILE_PREFIX: &str = "audio_chunk_";
const CHECKPOINT_FILE_SUFFIX: &str = ".mp4";
const FINAL_AUDIO_FILE: &str = "audio.mp4";
const AUDIO_FILE_CANDIDATES: &[&str] = &[
    "audio-recovered.mp4",
    "audio-recovered.wav",
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

use super::ffmpeg::find_ffmpeg_path;

/// Encodes the raw recording spool, with recovery support for legacy AAC checkpoints.
pub struct IncrementalAudioSaver {
    pub(super) capture_incomplete: bool,
    pub(super) raw_fully_encoded: bool,
    checkpoints_dir: PathBuf,
    meeting_folder: PathBuf,
}

impl IncrementalAudioSaver {
    pub fn new(meeting_folder: PathBuf) -> Self {
        Self {
            capture_incomplete: false,
            raw_fully_encoded: false,
            checkpoints_dir: meeting_folder.join(".checkpoints"),
            meeting_folder,
        }
    }

    /// Finalize raw audio, falling back to existing legacy checkpoints.
    ///
    /// Returns the path to the final merged audio.mp4 file
    pub async fn finalize(&mut self) -> Result<PathBuf> {
        info!("Finalizing incremental recording...");

        let final_audio_path = self.meeting_folder.join(FINAL_AUDIO_FILE);
        let staged = self.meeting_folder.join(".audio-finalizing.mp4");
        if self.meeting_folder.join(".audio-spool").is_dir() {
            match super::audio_spool::encode_capture(&self.meeting_folder, &staged) {
                Ok((gaps, all_encoded)) => {
                    self.capture_incomplete = gaps;
                    self.raw_fully_encoded = all_encoded;
                }
                Err(error) => {
                    // Old/interrupted sessions can still have usable checkpoints.
                    // New recordings use only the raw spool, so retain it on failure.
                    if list_checkpoint_files(&self.checkpoints_dir)?.is_empty() {
                        return Err(error);
                    }
                    warn!("Raw capture encoding failed; trying retained legacy checkpoints");
                    self.capture_incomplete = true;
                    self.merge_checkpoints(&staged).await?;
                }
            }
        } else {
            self.merge_checkpoints(&staged).await?;
        }
        if std::fs::metadata(&staged)?.len() == 0 {
            return Err(anyhow!("Final audio is empty"));
        }
        std::fs::OpenOptions::new()
            .write(true)
            .open(&staged)?
            .sync_all()?;
        std::fs::rename(&staged, &final_audio_path)?;

        // Clean up checkpoints directory
        if self.checkpoints_dir.is_dir()
            && (self.raw_fully_encoded || !self.meeting_folder.join(".audio-spool").is_dir())
        {
            if let Err(_e) = std::fs::remove_dir_all(&self.checkpoints_dir) {
                warn!("Failed to clean up checkpoints directory");
                // Non-fatal - user can manually delete
            }
        }
        info!("Finalized recording");

        Ok(final_audio_path)
    }

    /// Merge all checkpoint files into final audio.mp4 using FFmpeg concat
    /// Uses concat demuxer for fast merging without re-encoding
    async fn merge_checkpoints(&self, output: &PathBuf) -> Result<()> {
        info!("Merging legacy checkpoints into final audio file");

        let checkpoint_files = list_checkpoint_files(&self.checkpoints_dir)
            .map_err(|e| anyhow!("Failed to list checkpoints: {}", e))?;

        if checkpoint_files.is_empty() {
            return Err(anyhow!(
                "No complete audio checkpoints found in {}",
                self.checkpoints_dir.display()
            ));
        }

        // Create concat list file for FFmpeg
        let list_file = self.checkpoints_dir.join("concat_list.txt");
        let mut list_content = String::new();

        for checkpoint_path in &checkpoint_files {
            // Use absolute path for FFmpeg (required for safe mode)
            let abs_path = checkpoint_path.canonicalize()?;
            list_content.push_str(&concat_entry(&abs_path));
        }

        std::fs::write(&list_file, list_content)?;

        let ffmpeg_path = find_ffmpeg_path().ok_or_else(|| {
            anyhow!("FFmpeg not found. Please install FFmpeg to finalize recordings.")
        })?;
        info!("Using the audio encoder");

        // Run FFmpeg concat command
        // Using concat demuxer with copy codec for fast merging (no re-encoding)

        let mut command = std::process::Command::new(ffmpeg_path);

        command.args(&[
            "-f",
            "concat", // Use concat demuxer
            "-safe",
            "0", // Allow absolute paths
            "-i",
            list_file.to_str().unwrap(),
            "-c",
            "copy", // Copy codec - no re-encoding!
            "-y",   // Overwrite output file
            output.to_str().unwrap(),
        ]);

        // Hide console window on Windows to prevent CMD popup during finalization
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        tokio::task::spawn_blocking(move || {
            let mut child = command.spawn()?;
            super::encode::wait_for_encoder(&mut child)
        })
        .await
        .map_err(|_| anyhow!("Audio merge task failed"))??;

        // Verify output file was created
        if !output.exists() {
            return Err(anyhow!(
                "Merged audio file was not created: {}",
                output.display()
            ));
        }

        info!("Audio checkpoints merged");

        Ok(())
    }
}

/// Audio recovery status for transcript recovery feature
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioRecoveryStatus {
    pub status: String, // "success" | "partial" | "failed" | "none"
    pub chunk_count: u32,
    pub estimated_duration_seconds: f64,
    pub audio_file_path: Option<String>,
    pub message: String,
}

fn concat_entry(path: &Path) -> String {
    let path = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\\''");
    format!("file '{path}'\n")
}

fn checkpoint_index(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    if !name.starts_with(CHECKPOINT_FILE_PREFIX) || !name.ends_with(CHECKPOINT_FILE_SUFFIX) {
        return None;
    }

    let index = name
        .trim_start_matches(CHECKPOINT_FILE_PREFIX)
        .trim_end_matches(CHECKPOINT_FILE_SUFFIX);
    index.parse::<u32>().ok()
}

fn checkpoint_temp_index(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    let name = name.strip_prefix('.')?;
    let name = name.strip_suffix(".tmp")?;
    if !name.starts_with(CHECKPOINT_FILE_PREFIX) || !name.ends_with(CHECKPOINT_FILE_SUFFIX) {
        return None;
    }

    let index = name
        .trim_start_matches(CHECKPOINT_FILE_PREFIX)
        .trim_end_matches(CHECKPOINT_FILE_SUFFIX);
    index.parse::<u32>().ok()
}

fn validate_recoverable_audio_file(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| anyhow!("Failed to stat audio file {}: {}", path.display(), e))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(anyhow!("Audio file is empty: {}", path.display()));
    }
    Ok(())
}

pub(super) fn validate_recoverable_temp_audio_file(path: &Path) -> Result<()> {
    validate_recoverable_audio_file(path)?;
    // A container signature survives a torn write. Require a complete decode
    // before admitting an unpublished checkpoint to the recovery concatenation.
    let ffmpeg = find_ffmpeg_path().ok_or_else(|| anyhow!("Audio verifier unavailable"))?;
    let mut command = std::process::Command::new(ffmpeg);
    command
        .args(["-nostdin", "-v", "error", "-xerror", "-i"])
        .arg(path)
        .args(["-vn", "-f", "null", "-"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|_| anyhow!("Audio verifier could not start"))?;
    super::encode::wait_for_encoder(&mut child)
}

fn list_checkpoint_files(checkpoints_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    if !checkpoints_dir.exists() {
        return Ok(Vec::new());
    }

    let mut complete_files: BTreeMap<u32, PathBuf> = BTreeMap::new();
    let mut temp_files: BTreeMap<u32, PathBuf> = BTreeMap::new();

    for entry in std::fs::read_dir(checkpoints_dir)?.filter_map(|entry| entry.ok()) {
        let path = entry.path();

        if let Some(index) = checkpoint_index(&path) {
            if validate_recoverable_audio_file(&path).is_ok() {
                complete_files.insert(index, path);
            }
            continue;
        }

        if let Some(index) = checkpoint_temp_index(&path) {
            if validate_recoverable_temp_audio_file(&path).is_ok() {
                temp_files.entry(index).or_insert(path);
            }
        }
    }

    for (index, path) in temp_files {
        complete_files.entry(index).or_insert(path);
    }

    Ok(complete_files.into_values().collect())
}

/// Find an existing audio file in a meeting folder.
/// Tries ClawScribe's known recording names first, then scans by audio extension.
pub fn find_existing_audio_file(folder: &Path) -> Result<PathBuf> {
    for name in AUDIO_FILE_CANDIDATES {
        let path = folder.join(name);
        if path.is_file() {
            return Ok(path);
        }
    }

    for entry in std::fs::read_dir(folder)
        .map_err(|e| anyhow!("Failed to scan meeting folder {}: {}", folder.display(), e))?
    {
        let path = entry?.path();
        if !path.is_file()
            || path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
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

/// Find an audio file, recovering it from checkpoints first when the final file is missing.
pub async fn find_or_recover_audio_file(folder: &Path) -> Result<PathBuf> {
    // Recovery originals are deliberately retained. Reuse the completed result;
    // an explicit Recover action can rebuild it if originals are later changed.
    for name in ["audio-recovered.mp4", "audio-recovered.wav"] {
        let path = folder.join(name);
        if validate_recoverable_audio_file(&path).is_ok() {
            return Ok(path);
        }
    }
    if super::recording_commands::is_recording().await {
        return find_existing_audio_file(folder);
    }
    if folder.join(".audio-spool").is_dir() {
        let status = recover_audio_from_checkpoints(folder.to_string_lossy().to_string())
            .await
            .map_err(anyhow::Error::msg)?;
        if let Some(path) = status.audio_file_path {
            return Ok(PathBuf::from(path));
        }
    }
    match find_existing_audio_file(folder) {
        Ok(path) => Ok(path),
        Err(initial_error) => {
            let recovery = recover_audio_from_checkpoints(folder.to_string_lossy().to_string())
                .await
                .map_err(|recovery_error| {
                    anyhow!(
                        "{}; audio recovery failed: {}",
                        initial_error,
                        recovery_error
                    )
                })?;

            if let Some(audio_file_path) = recovery.audio_file_path {
                let path = PathBuf::from(audio_file_path);
                if path.is_file() {
                    return Ok(path);
                }
            }

            find_existing_audio_file(folder).map_err(|after_recovery| {
                anyhow!(
                    "{}; audio recovery result: {}",
                    after_recovery,
                    recovery.message
                )
            })
        }
    }
}

/// Resolve a meeting audio file for UI gating. Missing audio/checkpoints are not an error here.
pub async fn resolve_audio_file_or_recover(folder: &Path) -> Result<Option<PathBuf>, String> {
    if folder.join(".audio-spool").is_dir() {
        return find_or_recover_audio_file(folder)
            .await
            .map(Some)
            .map_err(|error| error.to_string());
    }
    match find_existing_audio_file(folder) {
        Ok(path) => Ok(Some(path)),
        Err(_) => {
            let recovery =
                recover_audio_from_checkpoints(folder.to_string_lossy().to_string()).await?;

            if let Some(audio_file_path) = recovery.audio_file_path {
                let path = PathBuf::from(audio_file_path);
                if path.is_file() {
                    return Ok(Some(path));
                }
            }

            match find_existing_audio_file(folder) {
                Ok(path) => Ok(Some(path)),
                Err(_) if recovery.status == "none" => Ok(None),
                Err(error) => Err(format!(
                    "{}; audio recovery result: {}",
                    error, recovery.message
                )),
            }
        }
    }
}

/// Recover audio from checkpoint files
/// This is called by the transcript recovery system to merge audio chunks after a crash
#[tauri::command]
pub async fn recover_audio_from_checkpoints(
    meeting_folder: String,
) -> Result<AudioRecoveryStatus, String> {
    if super::recording_commands::is_recording().await {
        return Err("Stop recording before recovering audio.".into());
    }
    let spool_folder = PathBuf::from(&meeting_folder);
    if spool_folder.join(".audio-spool").is_dir() {
        return tokio::task::spawn_blocking(move || super::audio_spool::recover(&spool_folder))
            .await
            .map_err(|_| "Audio recovery task failed".to_string())?;
    }
    tokio::task::spawn_blocking(move || recover_legacy_checkpoints(&meeting_folder))
        .await
        .map_err(|_| "Audio recovery task failed".to_string())?
}

fn recover_legacy_checkpoints(meeting_folder: &str) -> Result<AudioRecoveryStatus, String> {
    info!("Starting checkpoint audio recovery");

    let folder_path = PathBuf::from(&meeting_folder);
    let checkpoints_dir = folder_path.join(".checkpoints");
    let output_path = folder_path.join(FINAL_AUDIO_FILE);

    if validate_recoverable_audio_file(&output_path).is_ok() {
        let output_path_str = output_path.to_string_lossy().to_string();
        info!("Meeting already has playable final audio");
        return Ok(AudioRecoveryStatus {
            status: "success".to_string(),
            chunk_count: 0,
            estimated_duration_seconds: 0.0,
            audio_file_path: Some(output_path_str),
            message: "Final audio file already exists".to_string(),
        });
    }

    // Check if checkpoints directory exists
    if !checkpoints_dir.exists() {
        info!("No checkpoint directory found");
        return Ok(AudioRecoveryStatus {
            status: "none".to_string(),
            chunk_count: 0,
            estimated_duration_seconds: 0.0,
            audio_file_path: None,
            message: "No audio checkpoints found".to_string(),
        });
    }

    // Scan for complete checkpoint files only. Incomplete temp files are ignored.
    let checkpoint_files = list_checkpoint_files(&checkpoints_dir)
        .map_err(|e| format!("Failed to read checkpoints directory: {}", e))?
        .into_iter()
        .collect::<Vec<_>>();

    if checkpoint_files.is_empty() {
        info!("No checkpoint files found");
        return Ok(AudioRecoveryStatus {
            status: "none".to_string(),
            chunk_count: 0,
            estimated_duration_seconds: 0.0,
            audio_file_path: None,
            message: "No audio checkpoint files found".to_string(),
        });
    }

    let chunk_count = checkpoint_files.len() as u32;
    let estimated_duration = (chunk_count as f64) * CHECKPOINT_INTERVAL_SECONDS as f64;

    info!(
        "Found {} checkpoint files, estimated duration: {:.2}s",
        chunk_count, estimated_duration
    );

    // Create FFmpeg concat file
    let concat_file_path = checkpoints_dir.join("concat_list.txt");
    let mut concat_content = String::new();

    for checkpoint_path in &checkpoint_files {
        let path = checkpoint_path
            .canonicalize()
            .map_err(|e| format!("Failed to canonicalize path: {}", e))?;
        concat_content.push_str(&concat_entry(&path));
    }

    std::fs::write(&concat_file_path, concat_content)
        .map_err(|e| format!("Failed to write concat file: {}", e))?;

    // Run FFmpeg to merge chunks
    let staged_path = folder_path.join(format!(".recovery-{}.mp4", uuid::Uuid::new_v4()));
    let output_path_str = output_path.to_string_lossy().to_string();

    let ffmpeg_path = find_ffmpeg_path()
        .ok_or_else(|| "FFmpeg not found. Please install FFmpeg to recover audio.".to_string())?;
    info!("Using the audio recovery encoder");

    let mut command = std::process::Command::new(ffmpeg_path);

    command
        .args(&["-f", "concat", "-safe", "0", "-i"])
        .arg(&concat_file_path)
        .args(["-c", "copy", "-y"])
        .arg(&staged_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());

    // Hide console window on Windows
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let ffmpeg_result = (|| -> anyhow::Result<()> {
        let mut child = command.spawn()?;
        super::encode::wait_for_encoder(&mut child)?;
        validate_recoverable_audio_file(&staged_path)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&staged_path)?
            .sync_all()?;
        std::fs::rename(&staged_path, &output_path)?;
        Ok(())
    })();

    match ffmpeg_result {
        Ok(()) => {
            // Clean up concat file
            let _ = std::fs::remove_file(concat_file_path);
            if let Err(e) = validate_recoverable_audio_file(&output_path) {
                error!("Recovered audio validation failed");
                return Ok(AudioRecoveryStatus {
                    status: "failed".to_string(),
                    chunk_count,
                    estimated_duration_seconds: estimated_duration,
                    audio_file_path: None,
                    message: format!("Recovered audio validation failed: {}", e),
                });
            }

            info!("Audio recovery completed");

            Ok(AudioRecoveryStatus {
                status: "success".to_string(),
                chunk_count,
                estimated_duration_seconds: estimated_duration,
                audio_file_path: Some(output_path_str),
                message: format!("Successfully recovered {} audio chunks", chunk_count),
            })
        }
        Err(_) => {
            let _ = std::fs::remove_file(&staged_path);
            error!("Checkpoint recovery failed; original checkpoints retained");
            Ok(AudioRecoveryStatus {
                status: "failed".to_string(),
                chunk_count,
                estimated_duration_seconds: estimated_duration,
                audio_file_path: None,
                message: "Recovery failed or timed out. Original checkpoints were retained; check disk space and retry.".into(),
            })
        }
    }
}

/// Clean up checkpoint files after successful recording or recovery
/// This command is called by the frontend after successful save to clean up checkpoint files
#[tauri::command]
pub async fn cleanup_checkpoints(meeting_folder: String) -> Result<(), String> {
    info!("Cleaning up saved checkpoints");

    let folder_path = PathBuf::from(&meeting_folder);
    let checkpoints_dir = folder_path.join(".checkpoints");

    if checkpoints_dir.exists() {
        std::fs::remove_dir_all(&checkpoints_dir)
            .map_err(|e| format!("Failed to remove checkpoints directory: {}", e))?;
        info!("Successfully cleaned up checkpoints directory");
    } else {
        info!("No checkpoints directory to clean up");
    }

    Ok(())
}

/// Check for saved/recovered audio or readable raw/legacy recovery data.
/// The command name is retained for existing recovery-dialog callers.
#[tauri::command]
pub async fn has_audio_checkpoints(meeting_folder: String) -> Result<bool, String> {
    let folder_path = PathBuf::from(&meeting_folder);
    let checkpoints_dir = folder_path.join(".checkpoints");

    for name in [
        FINAL_AUDIO_FILE,
        "audio-recovered.mp4",
        "audio-recovered.wav",
    ] {
        if validate_recoverable_audio_file(&folder_path.join(name)).is_ok() {
            return Ok(true);
        }
    }
    if super::audio_spool::has_capture_audio(&folder_path)
        .map_err(|_| "Failed to inspect captured audio".to_string())?
    {
        return Ok(true);
    }

    // Check if checkpoints directory exists
    if !checkpoints_dir.exists() {
        return Ok(false);
    }

    let has_mp4_files = list_checkpoint_files(&checkpoints_dir)
        .map_err(|e| format!("Failed to read checkpoints directory: {}", e))?
        .into_iter()
        .next()
        .is_some();

    Ok(has_mp4_files)
}

#[cfg(test)]
mod tests {
    use super::super::encode::encode_single_audio;
    use super::super::recording_state::{AudioChunk, DeviceType};
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn recovery_dialog_recognizes_spool_and_temporary_tail() {
        for temporary in [false, true] {
            let root = tempdir().unwrap();
            assert!(!has_audio_checkpoints(root.path().to_string_lossy().into())
                .await
                .unwrap());
            let (sender, _, _) =
                super::super::transcription::queue::recording_audio_queue(root.path()).unwrap();
            assert!(!has_audio_checkpoints(root.path().to_string_lossy().into())
                .await
                .unwrap());
            sender
                .send(AudioChunk {
                    data: vec![0.05; 160],
                    sample_rate: 16000,
                    timestamp: 0.0,
                    chunk_id: 0,
                    device_type: DeviceType::System,
                })
                .await
                .unwrap();
            drop(sender);
            let chunk = root.path().join(".audio-spool/00000000000000000000.chunk");
            let path = if temporary {
                let temp = chunk.with_extension("tmp");
                std::fs::rename(chunk, &temp).unwrap();
                temp
            } else {
                chunk
            };
            assert!(has_audio_checkpoints(root.path().to_string_lossy().into())
                .await
                .unwrap());
            std::fs::write(path, b"torn capture").unwrap();
            assert!(!has_audio_checkpoints(root.path().to_string_lossy().into())
                .await
                .unwrap());
        }
    }

    #[tokio::test]
    async fn recovery_dialog_recognizes_recovered_and_legacy_audio() {
        for name in [
            "audio-recovered.wav",
            "audio-recovered.mp4",
            "audio.mp4",
            ".checkpoints/audio_chunk_000.mp4",
        ] {
            let root = tempdir().unwrap();
            let file = root.path().join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            // The existing saved-file validator checks that it is a nonempty file.
            std::fs::write(&file, b"synthetic saved audio").unwrap();
            assert!(has_audio_checkpoints(root.path().to_string_lossy().into())
                .await
                .unwrap());
            std::fs::write(file, b"").unwrap();
            assert!(!has_audio_checkpoints(root.path().to_string_lossy().into())
                .await
                .unwrap());
        }
    }

    #[tokio::test]
    async fn failed_raw_encode_can_use_retained_legacy_checkpoints() {
        let root = tempdir().unwrap();
        std::fs::create_dir(root.path().join(".checkpoints")).unwrap();
        std::fs::create_dir(root.path().join(".audio-spool")).unwrap();
        std::fs::write(
            root.path().join(".audio-spool/00000000000000000000.chunk"),
            b"damaged",
        )
        .unwrap();
        let mut saver = IncrementalAudioSaver::new(root.path().to_path_buf());
        encode_single_audio(
            bytemuck::cast_slice(&vec![0.05f32; 4800]),
            48000,
            1,
            &root.path().join(".checkpoints/audio_chunk_000.mp4"),
        )
        .unwrap();
        let output = saver.finalize().await.unwrap();
        validate_recoverable_temp_audio_file(&output).unwrap();
        assert!(saver.capture_incomplete && !saver.raw_fully_encoded);
        assert!(root.path().join(".audio-spool").exists());
        assert!(root.path().join(".checkpoints").exists());
    }

    #[tokio::test]
    async fn final_encode_has_only_one_aac_padding_boundary() {
        let root = tempdir().unwrap();
        let (writer, _reader, _) =
            super::super::transcription::queue::recording_audio_queue(root.path()).unwrap();
        let mut saver = IncrementalAudioSaver::new(root.path().to_path_buf());
        for id in 0..8 {
            let chunk = AudioChunk {
                data: vec![0.05; 24000],
                sample_rate: 48000,
                timestamp: id as f64 / 2.0,
                chunk_id: id,
                device_type: DeviceType::System,
            };
            writer.send(chunk).await.unwrap();
        }
        drop(writer);
        let output = saver.finalize().await.unwrap();
        let mut command = std::process::Command::new(find_ffmpeg_path().unwrap());
        command
            .args(["-v", "error", "-i"])
            .arg(output)
            .args(["-f", "f32le", "-ac", "1", "pipe:1"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let decoded = command.output().unwrap();
        assert!(decoded.status.success());
        let samples = decoded.stdout.len() / 4;
        assert!(
            samples >= 192000 && samples - 192000 < 1024,
            "Only final AAC frame padding is permitted; got {samples} samples"
        );
    }

    #[tokio::test]
    async fn opening_recovered_audio_does_not_rebuild_it() {
        let root = tempdir().unwrap();
        std::fs::create_dir(root.path().join(".audio-spool")).unwrap();
        let recovered = root.path().join("audio-recovered.wav");
        std::fs::write(&recovered, b"previous recovery result").unwrap();
        let before = std::fs::metadata(&recovered).unwrap().modified().unwrap();
        assert_eq!(
            find_or_recover_audio_file(root.path()).await.unwrap(),
            recovered
        );
        assert_eq!(
            resolve_audio_file_or_recover(root.path()).await.unwrap(),
            Some(recovered.clone())
        );
        assert_eq!(
            std::fs::metadata(recovered).unwrap().modified().unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn legacy_checkpoint_recording_still_finalizes_and_recovers() {
        let temp_dir = tempdir().unwrap();
        let meeting_folder = temp_dir.path().join("Synthetic Meeting's Session");
        std::fs::create_dir_all(meeting_folder.join(".checkpoints")).unwrap();
        for index in 0..2 {
            encode_single_audio(
                bytemuck::cast_slice(&vec![0.05f32; 4800]),
                48000,
                1,
                &meeting_folder.join(format!(".checkpoints/audio_chunk_{index:03}.mp4")),
            )
            .unwrap();
        }
        let recovery = recover_legacy_checkpoints(meeting_folder.to_str().unwrap()).unwrap();
        assert_eq!(recovery.status, "success");
        assert_eq!(recovery.chunk_count, 2);
        validate_recoverable_temp_audio_file(&meeting_folder.join(FINAL_AUDIO_FILE)).unwrap();
        std::fs::remove_file(meeting_folder.join(FINAL_AUDIO_FILE)).unwrap();
        let mut saver = IncrementalAudioSaver::new(meeting_folder.clone());
        let final_path = saver.finalize().await.unwrap();
        validate_recoverable_temp_audio_file(&final_path).unwrap();
        assert!(!meeting_folder.join(".checkpoints").exists());
    }

    #[tokio::test]
    async fn test_empty_recording() {
        let temp_dir = tempdir().unwrap();
        let meeting_folder = temp_dir.path().join("Empty_Test");
        std::fs::create_dir_all(&meeting_folder).unwrap();

        let mut saver = IncrementalAudioSaver::new(meeting_folder.clone());

        // Try to finalize without adding any chunks
        let result = saver.finalize().await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No complete audio checkpoints"));
        assert!(!meeting_folder.join(FINAL_AUDIO_FILE).exists());
    }

    #[test]
    fn test_checkpoint_scanner_ignores_temp_and_unrelated_files() {
        let temp_dir = tempdir().unwrap();
        let checkpoints_dir = temp_dir.path().join(".checkpoints");
        std::fs::create_dir_all(&checkpoints_dir).unwrap();
        std::fs::write(checkpoints_dir.join("audio_chunk_002.mp4"), b"ok").unwrap();
        std::fs::write(checkpoints_dir.join("audio_chunk_000.mp4"), b"ok").unwrap();
        std::fs::write(checkpoints_dir.join(".audio_chunk_001.mp4.tmp"), b"partial").unwrap();
        std::fs::write(checkpoints_dir.join("concat_list.txt"), b"ignored").unwrap();
        std::fs::write(checkpoints_dir.join("random.mp4"), b"ignored").unwrap();

        let files = list_checkpoint_files(&checkpoints_dir).unwrap();
        let names = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["audio_chunk_000.mp4", "audio_chunk_002.mp4"]);
    }

    #[test]
    fn test_checkpoint_scanner_recovers_valid_temp_checkpoint() {
        let temp_dir = tempdir().unwrap();
        let checkpoints_dir = temp_dir.path().join(".checkpoints");
        std::fs::create_dir_all(&checkpoints_dir).unwrap();
        let valid = checkpoints_dir.join(".audio_chunk_000.mp4.tmp");
        encode_single_audio(bytemuck::cast_slice(&vec![0.1f32; 4800]), 48000, 1, &valid).unwrap();
        let bytes = std::fs::read(&valid).unwrap();
        std::fs::write(
            checkpoints_dir.join(".audio_chunk_002.mp4.tmp"),
            &bytes[..bytes.len() / 2],
        )
        .unwrap();
        std::fs::write(checkpoints_dir.join("audio_chunk_001.mp4"), b"ok").unwrap();

        let files = list_checkpoint_files(&checkpoints_dir).unwrap();
        let names = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![".audio_chunk_000.mp4.tmp", "audio_chunk_001.mp4"]
        );
    }
}
