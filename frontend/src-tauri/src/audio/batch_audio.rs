//! Disk-backed batch preparation: decode once, maintain one continuous VAD state,
//! and load only the next bounded speech segment for inference.
use super::vad::{ContinuousVadProcessor, SpeechSegment};
use anyhow::{anyhow, Result};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// A transient engine error gets one fresh attempt. Cancellation never retries.
pub(crate) async fn retry_segment<T, F, Fut>(
    cancelled: &'static AtomicBool,
    mut transcribe: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    for attempt in 0..2 {
        let result = cancel_aware(transcribe(), cancelled).await;
        if result.is_ok() || cancelled.load(Ordering::Acquire) || attempt == 1 {
            return result;
        }
    }
    unreachable!()
}

pub(crate) async fn cancel_aware<T>(
    work: impl std::future::Future<Output = Result<T>>,
    cancelled: &'static AtomicBool,
) -> Result<T> {
    tokio::pin!(work);
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(anyhow!("Transcription cancelled"));
        }
        tokio::select! {
            result = &mut work => return result,
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
        }
    }
}

pub(crate) struct StoredSegment {
    path: PathBuf,
    pub sample_count: usize,
    pub start_timestamp_ms: f64,
    pub end_timestamp_ms: f64,
    confidence: f32,
}

impl StoredSegment {
    pub async fn load(&self) -> Result<SpeechSegment> {
        let path = self.path.clone();
        let count = self.sample_count;
        let samples = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
            let mut samples = vec![0.0f32; count];
            File::open(path)?.read_exact(bytemuck::cast_slice_mut(&mut samples))?;
            Ok(samples)
        })
        .await
        .map_err(|_| anyhow!("Speech segment reader failed"))??;
        Ok(SpeechSegment {
            samples,
            start_timestamp_ms: self.start_timestamp_ms,
            end_timestamp_ms: self.end_timestamp_ms,
            confidence: self.confidence,
        })
    }
}

pub(crate) struct PreparedAudio {
    // Retain temporary files until the last inference read has finished.
    _directory: tempfile::TempDir,
    pub segments: Vec<StoredSegment>,
    pub duration_seconds: f64,
}

impl PreparedAudio {
    pub fn timing_grid(&self) -> Vec<(f64, f64)> {
        self.segments
            .iter()
            .filter(|segment| segment.sample_count >= 1600)
            .map(|segment| (segment.start_timestamp_ms, segment.end_timestamp_ms))
            .collect()
    }
}

pub(crate) async fn prepare(
    path: &Path,
    redemption_ms: u32,
    max_samples: usize,
    cancelled: &'static AtomicBool,
) -> Result<PreparedAudio> {
    if max_samples < 1600 {
        return Err(anyhow!("Invalid speech segment size"));
    }
    let directory = tempfile::Builder::new()
        .prefix("clawscribe-batch-")
        .tempdir()?;
    let pcm = directory.path().join("audio.f32");
    decode_to_pcm(path, &pcm, cancelled).await?;
    tokio::task::spawn_blocking(move || -> Result<PreparedAudio> {
        let bytes = std::fs::metadata(&pcm)?.len();
        if bytes == 0 || bytes % 4 != 0 {
            return Err(anyhow!("Decoder produced no valid audio"));
        }
        let mut reader = BufReader::new(File::open(&pcm)?);
        // A continuous VAD state preserves boundaries across disk reads. Cap a
        // sustained utterance at 60 s, then use the existing silence split policy.
        let mut vad = ContinuousVadProcessor::new_with_max_segment_duration(
            16000,
            redemption_ms,
            Some(60_000),
        )?;
        let mut segments = Vec::new();
        let mut frame = vec![0.0f32; 16_000];
        let mut remaining = bytes / 4;
        let mut store_segments = |speech: Vec<SpeechSegment>| -> Result<()> {
            for segment in speech {
                let pieces = if segment.samples.len() > max_samples {
                    super::common::split_segment_at_silence(&segment, max_samples)
                } else {
                    vec![segment]
                };
                for segment in pieces {
                    let path = directory.path().join(format!("{:08}.f32", segments.len()));
                    File::create(&path)?.write_all(bytemuck::cast_slice(&segment.samples))?;
                    segments.push(StoredSegment {
                        path,
                        sample_count: segment.samples.len(),
                        start_timestamp_ms: segment.start_timestamp_ms,
                        end_timestamp_ms: segment.end_timestamp_ms,
                        confidence: segment.confidence,
                    });
                }
            }
            Ok(())
        };
        while remaining > 0 {
            if cancelled.load(Ordering::Acquire) {
                return Err(anyhow!("Audio preparation cancelled"));
            }
            let count = remaining.min(frame.len() as u64) as usize;
            reader.read_exact(bytemuck::cast_slice_mut(&mut frame[..count]))?;
            store_segments(vad.process_audio(&frame[..count])?)?;
            remaining -= count as u64;
        }
        store_segments(vad.flush()?)?;
        drop(reader);
        std::fs::remove_file(pcm)?;
        Ok(PreparedAudio {
            _directory: directory,
            segments,
            duration_seconds: bytes as f64 / 64_000.0,
        })
    })
    .await
    .map_err(|_| anyhow!("Speech preparation task failed"))?
}

async fn decode_to_pcm(path: &Path, pcm: &Path, cancelled: &AtomicBool) -> Result<()> {
    let ffmpeg = super::ffmpeg::find_ffmpeg_path().ok_or_else(|| {
        anyhow!("FFmpeg is required to prepare meeting audio. Repair the ClawScribe installation.")
    })?;
    let mut command = tokio::process::Command::new(ffmpeg);
    command
        .args(["-nostdin", "-nostats", "-loglevel", "error", "-y", "-i"])
        .arg(path)
        .args([
            "-map", "0:a:0", "-vn", "-ac", "1", "-ar", "16000", "-f", "f32le",
        ])
        .arg(&pcm)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(target_os = "windows")]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .map_err(|_| anyhow!("Could not start audio decoder"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1800);
    loop {
        if cancelled.load(Ordering::Acquire) || std::time::Instant::now() >= deadline {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(anyhow!(
                "Audio preparation cancelled or timed out; original audio retained"
            ));
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(anyhow!(
                    "Audio decoding failed. Check the file and available disk space."
                ));
            }
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Ok(())
}

pub(crate) async fn prepare_diarization_samples(
    path: &Path,
    cancelled: std::sync::Arc<AtomicBool>,
) -> Result<Vec<f32>> {
    let directory = tempfile::Builder::new()
        .prefix("clawscribe-diarization-")
        .tempdir()?;
    let pcm = directory.path().join("audio.f32");
    decode_to_pcm(path, &pcm, &cancelled).await?;
    tokio::task::spawn_blocking(move || {
        let _directory = directory;
        read_diarization_samples(&pcm, &cancelled, |_| {})
    })
    .await
    .map_err(|_| anyhow!("Speaker audio preparation task failed"))?
}

fn read_diarization_samples(
    path: &Path,
    cancelled: &AtomicBool,
    mut observe_chunk: impl FnMut(usize),
) -> Result<Vec<f32>> {
    let bytes = std::fs::metadata(path)?.len();
    if bytes == 0 || bytes % 4 != 0 {
        return Err(anyhow!("Decoder produced no valid audio"));
    }
    let count = usize::try_from(bytes / 4)?;
    let mut samples = Vec::with_capacity(count);
    let mut reader = BufReader::new(File::open(path)?);
    let mut frame = vec![0.0f32; 16_000];
    while samples.len() < count {
        if cancelled.load(Ordering::Acquire) {
            return Err(anyhow!("Speaker detection cancelled"));
        }
        let size = frame.len().min(count - samples.len());
        reader.read_exact(bytemuck::cast_slice_mut(&mut frame[..size]))?;
        observe_chunk(size);
        samples.extend(frame[..size].iter().map(|sample| sample.clamp(-1.0, 1.0)));
    }
    Ok(samples)
}

#[cfg(test)]
mod tests {
    #[test]
    fn diarization_pcm_reader_uses_bounded_chunks() {
        let directory = tempfile::tempdir().unwrap();
        let pcm = directory.path().join("synthetic.f32");
        let expected: Vec<f32> = (0..64_123).map(|i| (i % 100) as f32 / 100.0).collect();
        std::fs::write(&pcm, bytemuck::cast_slice(&expected)).unwrap();
        let mut largest = 0;
        let mut reads = 0;
        let actual = read_diarization_samples(&pcm, &AtomicBool::new(false), |size| {
            largest = largest.max(size);
            reads += 1;
        })
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(largest, 16_000);
        assert_eq!(reads, 5);
    }

    #[tokio::test]
    async fn streaming_diarization_matches_existing_conversion_for_short_mono_audio() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic.wav");
        let mut wav = Vec::new();
        let data_bytes = 32_123_u32 * 2;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(data_bytes + 36).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&16_000_u32.to_le_bytes());
        wav.extend_from_slice(&32_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        for i in 0..32_123 {
            wav.extend_from_slice(&((i % 2000) as i16 - 1000).to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();
        let previous = crate::audio::decoder::decode_audio_file(&path)
            .unwrap()
            .to_whisper_format();
        let streamed =
            prepare_diarization_samples(&path, std::sync::Arc::new(AtomicBool::new(false)))
                .await
                .unwrap();
        assert_eq!(streamed, previous);
    }

    #[tokio::test]
    async fn segment_retry_recovers_transient_failure_and_stops_after_two_attempts() {
        static CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let calls = std::cell::Cell::new(0);
        let value = super::retry_segment(&CANCELLED, || {
            calls.set(calls.get() + 1);
            std::future::ready(if calls.get() == 1 {
                Err(anyhow::anyhow!("temporary engine failure"))
            } else {
                Ok("recovered")
            })
        })
        .await
        .unwrap();
        assert_eq!(value, "recovered");
        assert_eq!(calls.get(), 2);
        calls.set(0);
        let result: anyhow::Result<()> = super::retry_segment(&CANCELLED, || {
            calls.set(calls.get() + 1);
            std::future::ready(Err(anyhow::anyhow!("persistent engine failure")))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.get(), 2);
    }
    use super::*;

    #[tokio::test]
    async fn batch_preparation_normalizes_stereo_without_retaining_decoded_audio() {
        static CANCELLED: AtomicBool = AtomicBool::new(false);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic.wav");
        let mut writer = std::io::BufWriter::new(File::create(&path).unwrap());
        let data_bytes = 48000_u32 * 2 * 3 * 2;
        writer.write_all(b"RIFF").unwrap();
        writer.write_all(&(data_bytes + 36).to_le_bytes()).unwrap();
        writer.write_all(b"WAVEfmt ").unwrap();
        writer.write_all(&16_u32.to_le_bytes()).unwrap();
        writer.write_all(&1_u16.to_le_bytes()).unwrap();
        writer.write_all(&2_u16.to_le_bytes()).unwrap();
        writer.write_all(&48000_u32.to_le_bytes()).unwrap();
        writer.write_all(&192000_u32.to_le_bytes()).unwrap();
        writer.write_all(&4_u16.to_le_bytes()).unwrap();
        writer.write_all(&16_u16.to_le_bytes()).unwrap();
        writer.write_all(b"data").unwrap();
        writer.write_all(&data_bytes.to_le_bytes()).unwrap();
        for _ in 0..48000 * 2 * 3 {
            writer.write_all(&0_i16.to_le_bytes()).unwrap();
        }
        writer.flush().unwrap();
        drop(writer);
        let prepared = prepare(&path, 500, 25 * 16000, &CANCELLED).await.unwrap();
        assert!((prepared.duration_seconds - 3.0).abs() < 0.01);
        assert!(
            prepared.segments.is_empty(),
            "silence should not invent speech"
        );
        let spool = prepared._directory.path().to_path_buf();
        assert!(!spool.join("audio.f32").exists());
        drop(prepared);
        assert!(!spool.exists());
        assert!(path.exists(), "original audio is never removed");
    }

    #[tokio::test]
    async fn cancellation_does_not_wait_for_pending_inference() {
        static CANCELLED: AtomicBool = AtomicBool::new(true);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            cancel_aware(std::future::pending::<Result<()>>(), &CANCELLED),
        )
        .await
        .unwrap();
        assert!(result.unwrap_err().to_string().contains("cancelled"));
    }
    #[tokio::test]
    async fn cancelling_in_progress_work_drops_the_upload_future() {
        static CANCELLED_UPLOAD: AtomicBool = AtomicBool::new(false);
        struct Dropped(std::sync::Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let dropped = std::sync::Arc::new(AtomicBool::new(false));
        let drop_signal = Dropped(dropped.clone());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let upload = tokio::spawn(cancel_aware(
            async move {
                let _drop_signal = drop_signal;
                let _ = started_tx.send(());
                std::future::pending::<Result<()>>().await
            },
            &CANCELLED_UPLOAD,
        ));
        started_rx.await.unwrap();
        CANCELLED_UPLOAD.store(true, Ordering::Release);
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), upload)
            .await
            .unwrap()
            .unwrap();
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(
            dropped.load(Ordering::Acquire),
            "cancel must release request data"
        );
    }
}
