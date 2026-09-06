//! Rebuild playable audio from durable capture chunks without an encoder or a full-meeting buffer.
use super::incremental_saver::AudioRecoveryStatus;
use super::transcription::queue::read_chunk;
use std::fs::{self, File};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

pub(super) fn recover(folder: &Path) -> Result<AudioRecoveryStatus, String> {
    recover_inner(folder).map_err(|_| {
        "Audio recovery could not finish. Check disk space and keep the meeting recovery files."
            .into()
    })
}

fn recover_inner(folder: &Path) -> std::io::Result<AudioRecoveryStatus> {
    let spool = folder.join(".audio-spool");
    let mut paths = fs::read_dir(&spool)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| path.extension().is_some_and(|ext| ext == "chunk"));
    paths.sort();
    let staged = folder.join(format!(".audio-recovered-{}.tmp", uuid::Uuid::new_v4()));
    let output = folder.join("audio-recovered.wav");
    let result = (|| -> std::io::Result<AudioRecoveryStatus> {
        let mut writer = BufWriter::new(File::create(&staged)?);
        writer.write_all(&[0; 44])?;
        let mut sample_rate = 0;
        let mut samples = 0u64;
        let mut count = 0u32;
        let mut partial = spool.join(".incomplete").exists();
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
