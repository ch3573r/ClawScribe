use super::ffmpeg::find_ffmpeg_path; // Correct path to encode module
use super::AudioDevice;
use std::io::{Read, Write};
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
    wait_for_encoder_timeout(child, std::time::Duration::from_secs(30))
}

fn wait_for_encoder_timeout(
    child: &mut std::process::Child,
    timeout: std::time::Duration,
) -> anyhow::Result<()> {
    // Drain stderr to prevent pipe deadlock, retaining only bounded diagnostic
    // bytes. Surface categories, never raw encoder output containing paths.
    let diagnostics = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut tail = Vec::new();
            let mut block = [0; 4096];
            while let Ok(count) = stderr.read(&mut block) {
                if count == 0 {
                    break;
                }
                tail.extend_from_slice(&block[..count]);
                if tail.len() > 65536 {
                    tail.drain(..tail.len() - 65536);
                }
            }
            encoder_failure_category(&String::from_utf8_lossy(&tail))
        })
    });
    let deadline = std::time::Instant::now() + timeout;
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
    let category = diagnostics
        .and_then(|task| task.join().ok())
        .unwrap_or("unspecified encoder failure");
    result.map_err(|error| anyhow::anyhow!("{error}: {category}"))
}

fn encoder_failure_category(stderr: &str) -> &'static str {
    let text = stderr.to_ascii_lowercase();
    if text.contains("no space left") || text.contains("disk full") {
        "insufficient disk space"
    } else if text.contains("permission denied") || text.contains("access is denied") {
        "output access denied"
    } else if text.contains("invalid data") || text.contains("error while decoding") {
        "invalid or truncated audio"
    } else if text.contains("unknown encoder") || text.contains("encoder not found") {
        "required encoder unavailable"
    } else if text.contains("cannot allocate memory") {
        "insufficient memory"
    } else {
        "unspecified encoder failure"
    }
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
    encode_pcm_stream(
        sample_rate,
        channels,
        output_path,
        std::time::Duration::from_secs(30),
        |input| input.write_all(data),
    )
}

pub(super) fn encode_pcm_stream(
    sample_rate: u32,
    channels: u16,
    output_path: &PathBuf,
    timeout: std::time::Duration,
    write: impl FnOnce(&mut std::process::ChildStdin) -> std::io::Result<()> + Send,
) -> anyhow::Result<()> {
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
        .stderr(Stdio::piped());
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
        let writer = scope.spawn(move || write(&mut input));
        let result = wait_for_encoder_timeout(&mut child, timeout);
        let written = writer
            .join()
            .map_err(|_| anyhow::anyhow!("Audio encoder input task failed"))?;
        result?;
        written.map_err(|_| anyhow::anyhow!("Could not deliver audio to encoder"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_report_categories_without_echoing_paths() {
        assert_eq!(
            encoder_failure_category("private-recording.wav: Permission denied"),
            "output access denied"
        );
        assert_eq!(
            encoder_failure_category("private-recording.wav: No space left on device"),
            "insufficient disk space"
        );
        assert_eq!(
            encoder_failure_category("sensitive arbitrary output"),
            "unspecified encoder failure"
        );
    }
}
