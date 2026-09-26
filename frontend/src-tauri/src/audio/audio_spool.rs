//! Rebuild playable audio from durable capture chunks with bounded audio memory.
use super::incremental_saver::AudioRecoveryStatus;
use super::transcription::queue::read_chunk;
use std::fs::{self, File};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Bounded encode budget shared by final saving and long-recording recovery.
pub(super) fn encode_timeout(duration_seconds: f64) -> std::time::Duration {
    let seconds = if duration_seconds.is_finite() {
        duration_seconds.max(0.0)
    } else {
        0.0
    };
    std::time::Duration::from_secs_f64((seconds / 5.0 + 60.0).clamp(240.0, 3600.0))
}

/// Include the last unpublished chunk only at the next sequence position.
/// Unexpected readable temporary data is retained instead of silently discarded.
fn capture_paths(spool: &Path) -> std::io::Result<(Vec<PathBuf>, bool, bool)> {
    let entries = fs::read_dir(spool)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    let mut paths: Vec<_> = entries
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "chunk"))
        .cloned()
        .collect();
    paths.sort();
    let next = paths
        .iter()
        .filter_map(|path| path.file_stem()?.to_str()?.parse::<u64>().ok())
        .max()
        .map_or(Some(0), |index| index.checked_add(1));
    let mut gaps = false;
    let mut all_available = true;
    for path in entries
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "tmp"))
    {
        let index = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u64>().ok());
        if next.is_some() && index == next {
            paths.push(path.clone());
        } else {
            gaps = true;
            all_available &= read_chunk(path).is_err();
        }
    }
    Ok((paths, gaps, all_available))
}

/// Cheap recovery-dialog probe: inspect chunks only until readable audio is found.
pub(super) fn has_capture_audio(folder: &Path) -> std::io::Result<bool> {
    let spool = folder.join(".audio-spool");
    if !spool.is_dir() {
        return Ok(false);
    }
    let (paths, _, _) = capture_paths(&spool)?;
    Ok(paths.iter().any(|path| {
        read_chunk(path).is_ok_and(|chunk| chunk.sample_rate > 0 && !chunk.data.is_empty())
    }))
}

/// Returns (has gaps, all recoverable chunks encoded). Unreadable published
/// chunks are retained; a torn final temporary write only records a capture gap.
pub(super) fn encode_capture(
    folder: &Path,
    output: &std::path::PathBuf,
) -> anyhow::Result<(bool, bool)> {
    let spool = folder.join(".audio-spool");
    let (paths, temporary_gaps, mut all_encoded) = capture_paths(&spool)?;
    let first = paths
        .iter()
        .find_map(|path| {
            read_chunk(path)
                .ok()
                .filter(|chunk| chunk.sample_rate > 0 && !chunk.data.is_empty())
        })
        .ok_or_else(|| std::io::Error::other("No readable captured audio"))?;
    let sample_rate = first.sample_rate;
    drop(first);
    let samples: u64 = paths
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|metadata| metadata.len().saturating_sub(33) / 4)
        .sum();
    let timeout = encode_timeout(samples as f64 / sample_rate as f64);
    let mut gaps = temporary_gaps || spool.join(".incomplete").exists();
    super::encode::encode_pcm_stream(sample_rate, 1, output, timeout, |writer| {
        let mut written = false;
        for (index, path) in paths.iter().enumerate() {
            gaps |= path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.parse::<usize>().ok())
                != Some(index);
            let chunk = match read_chunk(path) {
                Ok(chunk) if chunk.sample_rate == sample_rate && !chunk.data.is_empty() => chunk,
                Err(_) if path.extension().is_some_and(|ext| ext == "tmp") => {
                    gaps = true;
                    continue;
                }
                _ => {
                    gaps = true;
                    all_encoded = false;
                    continue;
                }
            };
            writer.write_all(bytemuck::cast_slice(&chunk.data))?;
            written = true;
        }
        if !written {
            return Err(std::io::Error::other("No readable captured audio"));
        }
        Ok(())
    })?;
    Ok((gaps, all_encoded))
}

pub(super) fn recover(folder: &Path) -> Result<AudioRecoveryStatus, String> {
    recover_inner(folder).map_err(|_| {
        "Audio recovery could not finish. Check disk space and keep the meeting recovery files."
            .into()
    })
}

fn recover_inner(folder: &Path) -> std::io::Result<AudioRecoveryStatus> {
    let spool = folder.join(".audio-spool");
    let (paths, temporary_gaps, _) = capture_paths(&spool)?;
    let staged = folder.join(format!(".audio-recovered-{}.tmp", uuid::Uuid::new_v4()));
    // Keep encoder-free WAV recovery for normal recordings. Above RIFF's
    // 32-bit limit, stream into one AAC encode instead of rejecting the meeting.
    let bytes = paths.iter().try_fold(0u64, |total, path| {
        fs::metadata(path).map(|metadata| total.saturating_add(metadata.len()))
    })?;
    if bytes > u32::MAX as u64 - 36 {
        let first = read_chunk(
            paths
                .first()
                .ok_or_else(|| std::io::Error::other("No captured audio"))?,
        )?;
        let samples = paths.iter().try_fold(0u64, |total, path| {
            fs::metadata(path)
                .map(|metadata| total.saturating_add(metadata.len().saturating_sub(33) / 4))
        })?;
        let mut encoded_gaps = false;
        let encoded = encode_capture(folder, &staged)
            .map(|(gaps, _)| {
                encoded_gaps = gaps;
            })
            .map_err(|_| std::io::Error::other("Long recording recovery failed"));
        if let Err(error) = encoded {
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
        let output = folder.join("audio-recovered.mp4");
        let published = std::fs::OpenOptions::new()
            .write(true)
            .open(&staged)
            .and_then(|file| file.sync_all())
            .and_then(|_| fs::rename(&staged, &output));
        if let Err(error) = published {
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
        let partial = encoded_gaps
            || spool.join(".incomplete").exists()
            || paths.iter().enumerate().any(|(index, path)| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .and_then(|stem| stem.parse::<usize>().ok())
                    != Some(index)
            });
        return Ok(AudioRecoveryStatus {
            status: if partial { "partial" } else { "success" }.into(),
            chunk_count: paths.len() as u32,
            estimated_duration_seconds: samples as f64 / first.sample_rate as f64,
            audio_file_path: Some(output.to_string_lossy().into_owned()),
            message: "Recovered captured audio. Recovery originals have been retained.".into(),
        });
    }
    let output = folder.join("audio-recovered.wav");
    let result = (|| -> std::io::Result<AudioRecoveryStatus> {
        let mut writer = BufWriter::new(File::create(&staged)?);
        writer.write_all(&[0; 44])?;
        let mut sample_rate = 0;
        let mut samples = 0u64;
        let mut count = 0u32;
        let mut partial = temporary_gaps || spool.join(".incomplete").exists();
        for (index, path) in paths.iter().enumerate() {
            if path
                .file_stem()
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse::<usize>().ok())
                != Some(index)
            {
                partial = true;
            }
            let chunk = match read_chunk(path) {
                Ok(chunk) if chunk.sample_rate > 0 && !chunk.data.is_empty() => chunk,
                _ => {
                    partial = true;
                    continue;
                }
            };
            if sample_rate == 0 {
                sample_rate = chunk.sample_rate;
            }
            if chunk.sample_rate != sample_rate {
                return Err(std::io::Error::other("Inconsistent capture format"));
            }
            samples += chunk.data.len() as u64;
            // Standard WAV has a 32-bit length. Fail without touching recovery originals.
            if samples * 4 > u32::MAX as u64 - 36 {
                return Err(std::io::Error::other("Recording exceeds WAV size limit"));
            }
            writer.write_all(bytemuck::cast_slice(&chunk.data))?;
            count += 1;
        }
        if samples == 0 {
            return Err(std::io::Error::other("No recoverable samples"));
        }
        let data_bytes = (samples * 4) as u32;
        writer.seek(SeekFrom::Start(0))?;
        writer.write_all(b"RIFF")?;
        writer.write_all(&(data_bytes + 36).to_le_bytes())?;
        writer.write_all(b"WAVEfmt ")?;
        writer.write_all(&16u32.to_le_bytes())?;
        writer.write_all(&3u16.to_le_bytes())?; // IEEE float32
        writer.write_all(&1u16.to_le_bytes())?; // mono
        writer.write_all(&sample_rate.to_le_bytes())?;
        writer.write_all(
            &sample_rate
                .checked_mul(4)
                .ok_or_else(|| std::io::Error::other("Invalid sample rate"))?
                .to_le_bytes(),
        )?;
        writer.write_all(&4u16.to_le_bytes())?;
        writer.write_all(&32u16.to_le_bytes())?;
        writer.write_all(b"data")?;
        writer.write_all(&data_bytes.to_le_bytes())?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&staged, &output)?;
        Ok(AudioRecoveryStatus {
            status: if partial { "partial" } else { "success" }.into(),
            chunk_count: count,
            estimated_duration_seconds: samples as f64 / sample_rate as f64,
            audio_file_path: Some(output.to_string_lossy().into_owned()),
            message: if partial { "Recovered available audio; some capture chunks are missing. Review the transcript before using notes." }
                else { "Recovered captured audio. Recovery originals have been retained." }.into(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::recording_state::{AudioChunk, DeviceType};

    #[test]
    fn encode_deadline_scales_for_long_meetings_and_stays_bounded() {
        assert_eq!(encode_timeout(60.0).as_secs(), 240);
        assert_eq!(encode_timeout(3.0 * 3600.0).as_secs(), 2220);
        assert_eq!(encode_timeout(24.0 * 3600.0).as_secs(), 3600);
        assert_eq!(encode_timeout(f64::NAN).as_secs(), 240);
    }

    #[tokio::test]
    async fn encode_keeps_good_chunks_when_one_original_is_unreadable() {
        let root = tempfile::tempdir().unwrap();
        let (sender, _, _) =
            super::super::transcription::queue::recording_audio_queue(root.path()).unwrap();
        for id in 0..3 {
            sender
                .send(AudioChunk {
                    data: vec![0.05; 4800],
                    sample_rate: 48000,
                    timestamp: id as f64 / 10.0,
                    chunk_id: id,
                    device_type: DeviceType::System,
                })
                .await
                .unwrap();
        }
        drop(sender);
        let damaged = root.path().join(".audio-spool/00000000000000000001.chunk");
        fs::write(&damaged, b"interrupted chunk").unwrap();
        let output = root.path().join("audio.mp4");
        let (gaps, all_encoded) = encode_capture(root.path(), &output).unwrap();
        assert!(gaps && !all_encoded);
        assert!(damaged.exists());
        super::super::incremental_saver::validate_recoverable_temp_audio_file(&output).unwrap();
    }
    #[tokio::test]
    async fn recovery_includes_readable_temporary_tail_and_reports_torn_writes() {
        for (published, torn) in [(0, false), (2, false), (2, true)] {
            let root = tempfile::tempdir().unwrap();
            let (sender, _, _) =
                super::super::transcription::queue::recording_audio_queue(root.path()).unwrap();
            for id in 0..=published {
                sender
                    .send(AudioChunk {
                        data: vec![0.05; 160],
                        sample_rate: 16000,
                        timestamp: id as f64 / 100.0,
                        chunk_id: id,
                        device_type: DeviceType::System,
                    })
                    .await
                    .unwrap();
            }
            drop(sender);
            let chunk = root
                .path()
                .join(format!(".audio-spool/{published:020}.chunk"));
            let temp = chunk.with_extension("tmp");
            fs::rename(chunk, &temp).unwrap();
            if torn {
                fs::write(temp, b"torn write").unwrap();
            }
            let status = recover(root.path()).unwrap();
            let expected = if torn { published } else { published + 1 };
            assert_eq!(status.status, if torn { "partial" } else { "success" });
            assert_eq!(status.chunk_count, expected as u32);
            assert_eq!(
                fs::metadata(status.audio_file_path.unwrap()).unwrap().len(),
                44 + expected * 160 * 4
            );
            assert!(root.path().join(".audio-spool").exists());
        }
    }

    #[tokio::test]
    async fn recovery_includes_unconsumed_tail_and_retains_originals() {
        let folder = tempfile::tempdir().unwrap();
        let (sender, mut receiver, _) =
            crate::audio::transcription::queue::recording_audio_queue(folder.path()).unwrap();
        for index in 0..3 {
            sender
                .send(AudioChunk {
                    data: vec![index as f32; 160],
                    sample_rate: 16000,
                    timestamp: index as f64 / 100.0,
                    chunk_id: index,
                    device_type: DeviceType::System,
                })
                .await
                .unwrap();
        }
        receiver.recv().await.unwrap().unwrap();
        drop(receiver);
        drop(sender);
        let status = recover(folder.path()).unwrap();
        assert_eq!(status.status, "success");
        assert_eq!(status.chunk_count, 3);
        let bytes = fs::read(status.audio_file_path.unwrap()).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(bytes.len(), 44 + 3 * 160 * 4);
        assert_eq!(
            fs::read_dir(folder.path().join(".audio-spool"))
                .unwrap()
                .count(),
            3
        );
    }
}
