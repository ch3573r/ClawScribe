use super::encode::encode_single_audio;
use super::recording_state::AudioChunk;
use anyhow::{anyhow, Result};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const CHECKPOINT_INTERVAL_SECONDS: usize = 10;
const CHECKPOINT_FILE_PREFIX: &str = "audio_chunk_";
const CHECKPOINT_FILE_SUFFIX: &str = ".mp4";
const FINAL_AUDIO_FILE: &str = "audio.mp4";

use super::ffmpeg::find_ffmpeg_path;

/// Audio data without device type (we only store mixed audio)
#[derive(Clone)]
struct AudioData {
    data: Vec<f32>,
    // sample_rate: u32,
}

/// Incremental audio saver that writes checkpoints every few seconds
/// to minimize memory usage and enable crash recovery
pub struct IncrementalAudioSaver {
    checkpoint_buffer: Vec<AudioData>,
    checkpoint_interval_samples: usize,
    checkpoint_count: u32,
    checkpoints_dir: PathBuf,
    meeting_folder: PathBuf,
    sample_rate: u32,
}

impl IncrementalAudioSaver {
    /// Create a new incremental saver
    ///
    /// # Arguments
    /// * `meeting_folder` - Path to the meeting folder (contains .checkpoints/)
    /// * `sample_rate` - Sample rate of audio (typically 48000)
    pub fn new(meeting_folder: PathBuf, sample_rate: u32) -> Result<Self> {
        let checkpoints_dir = meeting_folder.join(".checkpoints");

        // Verify checkpoints directory exists
        if !checkpoints_dir.exists() {
            return Err(anyhow!(
                "Checkpoints directory does not exist: {}",
                checkpoints_dir.display()
            ));
        }

        Ok(Self {
            checkpoint_buffer: Vec::new(),
            checkpoint_interval_samples: sample_rate as usize * CHECKPOINT_INTERVAL_SECONDS,
            checkpoint_count: 0,
            checkpoints_dir,
            meeting_folder,
            sample_rate,
        })
    }

    /// Add an audio chunk to the buffer
    /// Automatically saves a checkpoint when buffer reaches the checkpoint interval
    pub fn add_chunk(&mut self, chunk: AudioChunk) -> Result<()> {
        let audio_data = AudioData {
            data: chunk.data,
            // sample_rate: chunk.sample_rate,
        };

        self.checkpoint_buffer.push(audio_data);

        // Calculate total samples in buffer
        let total_samples: usize = self.checkpoint_buffer.iter().map(|c| c.data.len()).sum();

        // Save checkpoint when buffer reaches threshold
        if total_samples >= self.checkpoint_interval_samples {
            self.save_checkpoint()?;
            self.checkpoint_buffer.clear();
        }

        Ok(())
    }

    /// Save current buffer as a checkpoint file
    fn save_checkpoint(&mut self) -> Result<()> {
        // Concatenate all chunks in buffer
        let audio_data: Vec<f32> = self
            .checkpoint_buffer
            .iter()
            .flat_map(|c| &c.data)
            .cloned()
            .collect();

        if audio_data.is_empty() {
            warn!("Attempted to save empty checkpoint, skipping");
            return Ok(());
        }

        let checkpoint_path = checkpoint_path(&self.checkpoints_dir, self.checkpoint_count);
        let temp_path = checkpoint_temp_path(&self.checkpoints_dir, self.checkpoint_count);

        if temp_path.exists() {
            std::fs::remove_file(&temp_path)?;
        }

        // Encode into a temp file first, then atomically publish the complete chunk.
        encode_single_audio(
            bytemuck::cast_slice(&audio_data),
            self.sample_rate,
            1, // mono
            &temp_path,
        )?;

        validate_recoverable_audio_file(&temp_path)?;
        std::fs::File::open(&temp_path)?.sync_all()?;
        publish_checkpoint_file(&temp_path, &checkpoint_path)?;

        let duration_seconds = audio_data.len() as f32 / self.sample_rate as f32;
        self.checkpoint_count += 1;

        info!(
            "Saved checkpoint {}: {:.2}s of audio ({} samples)",
            self.checkpoint_count,
            duration_seconds,
            audio_data.len()
        );

        Ok(())
    }

    /// Finalize the recording: save final checkpoint, merge all checkpoints, cleanup
    ///
    /// Returns the path to the final merged audio.mp4 file
    pub async fn finalize(&mut self) -> Result<PathBuf> {
        info!("Finalizing incremental recording...");

        // Save final buffer if not empty
        if !self.checkpoint_buffer.is_empty() {
            info!(
                "Saving final checkpoint with remaining {} chunks",
                self.checkpoint_buffer.len()
            );
            self.save_checkpoint()?;
            self.checkpoint_buffer.clear();
        }

        if self.checkpoint_count == 0 {
            return Err(anyhow!(
                "No audio checkpoints to merge - recording may have failed"
            ));
        }

        // Merge all checkpoints using FFmpeg concat
        let final_audio_path = self.meeting_folder.join(FINAL_AUDIO_FILE);
        self.merge_checkpoints(&final_audio_path).await?;

        // Clean up checkpoints directory
        info!("Cleaning up {} checkpoint files", self.checkpoint_count);
        if let Err(e) = std::fs::remove_dir_all(&self.checkpoints_dir) {
            warn!("Failed to clean up checkpoints directory: {}", e);
            // Non-fatal - user can manually delete
        }

        info!("Finalized recording: {}", final_audio_path.display());

        Ok(final_audio_path)
    }

    /// Merge all checkpoint files into final audio.mp4 using FFmpeg concat
    /// Uses concat demuxer for fast merging without re-encoding
    async fn merge_checkpoints(&self, output: &PathBuf) -> Result<()> {
        info!(
            "Merging {} checkpoints into final audio file...",
            self.checkpoint_count
        );

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
            list_content.push_str(&format!("file '{}'\n", abs_path.display()));
        }

        std::fs::write(&list_file, list_content)?;

        let ffmpeg_path = find_ffmpeg_path().ok_or_else(|| {
            anyhow!("FFmpeg not found. Please install FFmpeg to finalize recordings.")
        })?;
        info!("Using FFmpeg at: {:?}", ffmpeg_path);

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

        let ffmpeg_output = command.output()?;

        if !ffmpeg_output.status.success() {
            let stderr = String::from_utf8_lossy(&ffmpeg_output.stderr);
            error!("FFmpeg merge failed: {}", stderr);
            return Err(anyhow!("FFmpeg concat failed: {}", stderr));
        }

        // Verify output file was created
        if !output.exists() {
            return Err(anyhow!(
                "Merged audio file was not created: {}",
                output.display()
            ));
        }

        info!(
            "Successfully merged {} checkpoints → {}",
            checkpoint_files.len(),
            output.display()
        );

        Ok(())
    }

    /// Get the meeting folder path
    pub fn get_meeting_folder(&self) -> &PathBuf {
        &self.meeting_folder
    }

    /// Get current checkpoint count
    pub fn get_checkpoint_count(&self) -> u32 {
        self.checkpoint_count
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

fn checkpoint_path(checkpoints_dir: &Path, checkpoint_count: u32) -> PathBuf {
    checkpoints_dir.join(format!(
        "{}{:03}{}",
        CHECKPOINT_FILE_PREFIX, checkpoint_count, CHECKPOINT_FILE_SUFFIX
    ))
}

fn checkpoint_temp_path(checkpoints_dir: &Path, checkpoint_count: u32) -> PathBuf {
    checkpoints_dir.join(format!(
        ".{}{:03}{}.tmp",
        CHECKPOINT_FILE_PREFIX, checkpoint_count, CHECKPOINT_FILE_SUFFIX
    ))
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

fn validate_recoverable_temp_audio_file(path: &Path) -> Result<()> {
    validate_recoverable_audio_file(path)?;

    let bytes = std::fs::read(path)
        .map_err(|e| anyhow!("Failed to read temp audio file {}: {}", path.display(), e))?;
    if !bytes.windows(4).any(|window| window == b"ftyp") {
        return Err(anyhow!(
            "Temp audio file does not look like a complete MP4 chunk: {}",
            path.display()
        ));
    }

    Ok(())
}

fn publish_checkpoint_file(temp_path: &Path, checkpoint_path: &Path) -> Result<()> {
    let mut last_error = None;

    for attempt in 0..5 {
        match std::fs::rename(temp_path, checkpoint_path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if attempt < 4 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
    }

    Err(anyhow!(
        "Failed to publish checkpoint {} from {}: {}",
        checkpoint_path.display(),
        temp_path.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown rename error".to_string())
    ))
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

/// Recover audio from checkpoint files
/// This is called by the transcript recovery system to merge audio chunks after a crash
#[tauri::command]
pub async fn recover_audio_from_checkpoints(
    meeting_folder: String,
    _sample_rate: u32,
) -> Result<AudioRecoveryStatus, String> {
    info!("Starting audio recovery for folder: {}", meeting_folder);

    let folder_path = PathBuf::from(&meeting_folder);
    let checkpoints_dir = folder_path.join(".checkpoints");
    let output_path = folder_path.join(FINAL_AUDIO_FILE);

    if validate_recoverable_audio_file(&output_path).is_ok() {
        let output_path_str = output_path.to_string_lossy().to_string();
        info!(
            "Recovered meeting already has final audio: {}",
            output_path_str
        );
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
        info!(
            "No checkpoints directory found at: {}",
            checkpoints_dir.display()
        );
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
        info!(
            "No checkpoint files found in: {}",
            checkpoints_dir.display()
        );
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
        concat_content.push_str(&format!("file '{}'\n", path.display()));
    }

    std::fs::write(&concat_file_path, concat_content)
        .map_err(|e| format!("Failed to write concat file: {}", e))?;

    // Run FFmpeg to merge chunks
    let output_path_str = output_path
        .to_str()
        .ok_or("Invalid output path")?
        .to_string();

    let ffmpeg_path = find_ffmpeg_path()
        .ok_or_else(|| "FFmpeg not found. Please install FFmpeg to recover audio.".to_string())?;
    info!("Using FFmpeg at: {:?}", ffmpeg_path);

    let mut command = std::process::Command::new(ffmpeg_path);

    command.args(&[
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        concat_file_path.to_str().unwrap(),
        "-c",
        "copy",
        "-y", // Overwrite if exists
        &output_path_str,
    ]);

    // Hide console window on Windows
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let ffmpeg_result = command.output();

    match ffmpeg_result {
        Ok(output) if output.status.success() => {
            // Clean up concat file
            let _ = std::fs::remove_file(concat_file_path);
            if let Err(e) = validate_recoverable_audio_file(&output_path) {
                error!("Recovered audio validation failed: {}", e);
                return Ok(AudioRecoveryStatus {
                    status: "failed".to_string(),
                    chunk_count,
                    estimated_duration_seconds: estimated_duration,
                    audio_file_path: None,
                    message: format!("Recovered audio validation failed: {}", e),
                });
            }

            info!("Successfully recovered audio: {}", output_path_str);

            Ok(AudioRecoveryStatus {
                status: "success".to_string(),
                chunk_count,
                estimated_duration_seconds: estimated_duration,
                audio_file_path: Some(output_path_str),
                message: format!("Successfully recovered {} audio chunks", chunk_count),
            })
        }
        Ok(output) => {
            let error = String::from_utf8_lossy(&output.stderr);
            error!("FFmpeg recovery failed: {}", error);
            Ok(AudioRecoveryStatus {
                status: "failed".to_string(),
                chunk_count,
                estimated_duration_seconds: estimated_duration,
                audio_file_path: None,
                message: format!("FFmpeg failed: {}", error),
            })
        }
        Err(e) => {
            error!("Failed to run FFmpeg: {}", e);
            Ok(AudioRecoveryStatus {
                status: "failed".to_string(),
                chunk_count,
                estimated_duration_seconds: estimated_duration,
                audio_file_path: None,
                message: format!("Failed to run FFmpeg: {}", e),
            })
        }
    }
}

/// Clean up checkpoint files after successful recording or recovery
/// This command is called by the frontend after successful save to clean up checkpoint files
#[tauri::command]
pub async fn cleanup_checkpoints(meeting_folder: String) -> Result<(), String> {
    info!("Cleaning up checkpoints for folder: {}", meeting_folder);

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

/// Check if a meeting folder has audio checkpoint files
/// Returns true if .checkpoints/ directory exists and contains .mp4 files
#[tauri::command]
pub async fn has_audio_checkpoints(meeting_folder: String) -> Result<bool, String> {
    let folder_path = PathBuf::from(&meeting_folder);
    let checkpoints_dir = folder_path.join(".checkpoints");

    if validate_recoverable_audio_file(&folder_path.join(FINAL_AUDIO_FILE)).is_ok() {
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
    use super::super::recording_state::DeviceType;
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_checkpoint_creation() {
        // Create temp meeting folder
        let temp_dir = tempdir().unwrap();
        let meeting_folder = temp_dir.path().join("Test_Meeting");
        std::fs::create_dir_all(&meeting_folder).unwrap();
        std::fs::create_dir_all(meeting_folder.join(".checkpoints")).unwrap();

        let mut saver = IncrementalAudioSaver::new(meeting_folder.clone(), 48000).unwrap();

        // Add 60 seconds worth of audio (should create 2 checkpoints)
        for i in 0..120 {
            // 120 chunks of 0.5s each
            let chunk = AudioChunk {
                data: vec![0.5f32; 24000], // 0.5s at 48kHz
                sample_rate: 48000,
                timestamp: i as f64 * 0.5, // timestamp in seconds
                chunk_id: i as u64,
                device_type: DeviceType::Microphone,
            };
            saver.add_chunk(chunk).unwrap();
        }

        // Verify checkpoints were created at the crash-recovery interval
        assert_eq!(
            saver.checkpoint_count,
            (60 / CHECKPOINT_INTERVAL_SECONDS) as u32
        );

        // Finalize and verify merge
        let final_path = saver.finalize().await.unwrap();
        assert!(final_path.exists());

        // Verify checkpoints directory deleted
        assert!(!meeting_folder.join(".checkpoints").exists());
    }

    #[tokio::test]
    async fn test_empty_recording() {
        let temp_dir = tempdir().unwrap();
        let meeting_folder = temp_dir.path().join("Empty_Test");
        std::fs::create_dir_all(&meeting_folder).unwrap();
        std::fs::create_dir_all(meeting_folder.join(".checkpoints")).unwrap();

        let mut saver = IncrementalAudioSaver::new(meeting_folder.clone(), 48000).unwrap();

        // Try to finalize without adding any chunks
        let result = saver.finalize().await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No audio checkpoints"));
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
        std::fs::write(
            checkpoints_dir.join(".audio_chunk_000.mp4.tmp"),
            b"\0\0\0 ftypisom\0\0\0\0",
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
