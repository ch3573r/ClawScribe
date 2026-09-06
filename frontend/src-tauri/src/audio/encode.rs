use super::ffmpeg::find_ffmpeg_path; // Correct path to encode module
use super::AudioDevice;
use std::io::Write;
use std::sync::Arc;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

pub struct AudioInput {
    pub data: Arc<Vec<f32>>,
    pub sample_rate: u32,
    pub channels: u16,
    pub device: Arc<AudioDevice>,
}

/// Wait with a deadline and always reap the encoder, including on errors.
pub(super) fn wait_for_encoder(child: &mut std::process::Child) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(status)) => {
                break Err(anyhow::anyhow!(
                    "Audio encoder exited unsuccessfully ({status})"
                ))
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            Ok(None) => {
                break Err(anyhow::anyhow!(
                    "Audio encoder timed out; recovery files preserved"
                ))
            }
            Err(_) => {
                break Err(anyhow::anyhow!(
                    "Audio encoder status unavailable; recovery files preserved"
                ))
            }
        }
    };
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

pub fn encode_single_audio(
    data: &[u8],
    sample_rate: u32,
    channels: u16,
    output_path: &PathBuf,
) -> anyhow::Result<()> {
    if data.is_empty() || sample_rate == 0 || channels == 0 {
        return Err(anyhow::anyhow!("No valid audio data provided for encoding"));
    }
    let ffmpeg_path = find_ffmpeg_path().ok_or_else(|| {
        anyhow::anyhow!("FFmpeg not found. Please install FFmpeg to save recordings.")
    })?;
    let mut command = Command::new(ffmpeg_path);
    command
        .args([
            "-nostats",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "f32le",
            "-ar",
            &sample_rate.to_string(),
            "-ac",
            &channels.to_string(),
            "-i",
            "pipe:0",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-profile:a",
            "aac_low",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ])
        .arg(output_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|_| anyhow::anyhow!("Could not start audio encoder"))?;
    let Some(mut input) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(anyhow::anyhow!("Audio encoder input unavailable"));
    };
    // Feed stdin independently so a hung encoder cannot block the deadline.
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || input.write_all(data));
        let result = wait_for_encoder(&mut child);
        let written = writer
            .join()
            .map_err(|_| anyhow::anyhow!("Audio encoder input task failed"))?;
        result?;
        written.map_err(|_| anyhow::anyhow!("Could not deliver audio to encoder"))
    })
}
