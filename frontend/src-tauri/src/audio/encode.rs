use super::ffmpeg::find_ffmpeg_path;
use super::AudioDevice;
use std::{path::PathBuf, process::Command, sync::Arc, time::Duration};

pub struct AudioInput {
    pub data: Arc<Vec<f32>>,
    pub sample_rate: u32,
    pub channels: u16,
    pub device: Arc<AudioDevice>,
}

/// Runs on the dedicated recording writer (or another blocking worker).
/// The sample slice is borrowed by a scoped writer; it is not copied again.
pub fn encode_single_audio(data: &[u8], sample_rate: u32, channels: u16, output_path: &PathBuf) -> anyhow::Result<()> {
    if data.is_empty() || sample_rate == 0 || channels == 0 {
        return Err(anyhow::anyhow!("Audio data, sample rate and channel count must be nonzero"));
    }
    let ffmpeg = find_ffmpeg_path().ok_or_else(|| anyhow::anyhow!("FFmpeg not found. Repair ClawScribe to save recordings."))?;
    let mut command = Command::new(ffmpeg);
    command.args(["-hide_banner", "-loglevel", "error", "-nostats", "-nostdin", "-f", "f32le", "-ar"])
        .arg(sample_rate.to_string()).arg("-ac").arg(channels.to_string())
        .args(["-i", "pipe:0", "-c:a", "aac", "-b:a", "192k", "-profile:a", "aac_low", "-movflags", "+faststart", "-f", "mp4"])
        .arg(output_path);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let result = super::process_runner::run(&mut command, Some(data), Duration::from_secs(120))?;
    if !result.status.success() {
        return Err(anyhow::anyhow!("Audio encoder failed ({}): {}", result.status, String::from_utf8_lossy(&result.stderr)));
    }
    Ok(())
}
