//! A memory-bounded, disk-backed queue for live transcription audio.
//!
//! Speech recognition can be slower than real time on low-power notebooks.  A
//! normal unbounded channel retains every `Vec<f32>` in RAM in that situation.
//! This queue stages each VAD segment in a small per-session spool directory and
//! keeps only the segment currently being written/read in memory.

use crate::audio::recording_state::{AudioChunk, DeviceType};
use serde::Serialize;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

const QUEUE_MAGIC: &[u8; 4] = b"CSQ1";
const HEADER_BYTES: u64 = 4 + 4 + 8 + 8 + 1 + 8;
const MAX_SAMPLES_PER_CHUNK: usize = 16_000 * 120;
const STALE_SPOOL_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TranscriptionMetricsSnapshot {
    pub chunks_in_queue: usize,
    pub is_processing: bool,
    pub last_activity_ms: u64,
    pub queued_audio_seconds: f64,
    pub processing_realtime_factor: Option<f64>,
    pub estimated_seconds_remaining: Option<f64>,
    pub spool_bytes: u64,
    pub total_chunks_queued: u64,
    pub total_chunks_completed: u64,
    pub failed_chunks: u64,
}

#[derive(Debug, Default)]
pub struct TranscriptionMetrics {
    queued_chunks: AtomicU64,
    completed_chunks: AtomicU64,
    failed_chunks: AtomicU64,
    queued_audio_ms: AtomicU64,
    processed_audio_ms: AtomicU64,
    processing_wall_ms: AtomicU64,
    spool_bytes: AtomicU64,
    worker_active: AtomicBool,
    processing: AtomicBool,
    last_activity_epoch_ms: AtomicU64,
}

impl TranscriptionMetrics {
    pub fn mark_failed(&self) {
        self.failed_chunks.fetch_add(1, Ordering::AcqRel);
        self.touch();
    }
    fn touch(&self) {
        self.last_activity_epoch_ms
            .store(unix_epoch_ms(), Ordering::Release);
    }

    fn enqueued(&self, audio_ms: u64, spool_bytes: u64) {
        self.queued_chunks.fetch_add(1, Ordering::AcqRel);
        self.queued_audio_ms.fetch_add(audio_ms, Ordering::AcqRel);
        self.spool_bytes.fetch_add(spool_bytes, Ordering::AcqRel);
        self.touch();
    }

    pub fn set_worker_active(&self, active: bool) {
        self.worker_active.store(active, Ordering::Release);
        if !active {
            self.processing.store(false, Ordering::Release);
        }
        self.touch();
    }

    pub fn mark_processing_started(&self) {
        self.processing.store(true, Ordering::Release);
        self.touch();
    }

    pub fn mark_processing_stopped(&self) {
        self.processing.store(false, Ordering::Release);
        self.touch();
    }

    pub fn mark_completed(&self, audio_ms: u64, spool_bytes: u64, wall_time: Duration) {
        self.completed_chunks.fetch_add(1, Ordering::AcqRel);
        atomic_saturating_sub(&self.queued_audio_ms, audio_ms);
        atomic_saturating_sub(&self.spool_bytes, spool_bytes);
        self.processed_audio_ms
            .fetch_add(audio_ms, Ordering::AcqRel);
        self.processing_wall_ms.fetch_add(
            u64::try_from(wall_time.as_millis()).unwrap_or(u64::MAX),
            Ordering::AcqRel,
        );
        self.processing.store(false, Ordering::Release);
        self.touch();
    }

    pub fn snapshot(&self) -> TranscriptionMetricsSnapshot {
        let queued = self.queued_chunks.load(Ordering::Acquire);
        let completed = self.completed_chunks.load(Ordering::Acquire);
        let queued_audio_ms = self.queued_audio_ms.load(Ordering::Acquire);
        let processed_audio_ms = self.processed_audio_ms.load(Ordering::Acquire);
        let processing_wall_ms = self.processing_wall_ms.load(Ordering::Acquire);
        let processing_realtime_factor =
            (processed_audio_ms > 0).then(|| processing_wall_ms as f64 / processed_audio_ms as f64);
        let estimated_seconds_remaining =
            processing_realtime_factor.map(|rtf| queued_audio_ms as f64 / 1000.0 * rtf);
        let last_activity_epoch_ms = self.last_activity_epoch_ms.load(Ordering::Acquire);

        TranscriptionMetricsSnapshot {
            chunks_in_queue: usize::try_from(queued.saturating_sub(completed))
                .unwrap_or(usize::MAX),
            is_processing: self.worker_active.load(Ordering::Acquire)
                && (self.processing.load(Ordering::Acquire) || queued > completed),
            last_activity_ms: unix_epoch_ms().saturating_sub(last_activity_epoch_ms),
            queued_audio_seconds: queued_audio_ms as f64 / 1000.0,
            processing_realtime_factor,
            estimated_seconds_remaining,
            spool_bytes: self.spool_bytes.load(Ordering::Acquire),
            total_chunks_queued: queued,
            total_chunks_completed: completed,
            failed_chunks: self.failed_chunks.load(Ordering::Acquire),
        }
    }
}

fn atomic_saturating_sub(value: &AtomicU64, amount: u64) {
    let _ = value.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        Some(current.saturating_sub(amount))
    });
}

fn unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

struct QueueShared {
    spool_dir: PathBuf,
    retain_files: bool,
    next_write: AtomicU64,
    closed: AtomicBool,
    notify: Notify,
    metrics: Arc<TranscriptionMetrics>,
}

impl Drop for QueueShared {
    fn drop(&mut self) {
        if self.retain_files {
            return; // Recording recovery data belongs to the meeting, not this task.
        }
        if let Err(error) = fs::remove_dir_all(&self.spool_dir) {
            if error.kind() != io::ErrorKind::NotFound {
                log::warn!("Failed to remove transcription spool");
            }
        }
    }
}

pub struct TranscriptionQueueSender {
    shared: Arc<QueueShared>,
}

pub struct TranscriptionQueueReceiver {
    shared: Arc<QueueShared>,
    next_read: u64,
}

#[derive(Debug)]
pub struct QueuedAudioChunk {
    pub chunk: AudioChunk,
    audio_ms: u64,
    spool_bytes: u64,
}

impl QueuedAudioChunk {
    pub fn audio_ms(&self) -> u64 {
        self.audio_ms
    }

    pub fn spool_bytes(&self) -> u64 {
        self.spool_bytes
    }
}

pub fn transcription_queue() -> io::Result<(
    TranscriptionQueueSender,
    TranscriptionQueueReceiver,
    Arc<TranscriptionMetrics>,
)> {
    let spool_root = std::env::temp_dir()
        .join("clawscribe")
        .join("transcription-queue");
    cleanup_stale_spools(&spool_root);
    let spool_dir = spool_root.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&spool_dir)?;

    create_queue(spool_dir, false)
}

/// Preserve mixed capture on disk until final audio has been durably saved.
pub fn recording_audio_queue(
    folder: &Path,
) -> io::Result<(
    TranscriptionQueueSender,
    TranscriptionQueueReceiver,
    Arc<TranscriptionMetrics>,
)> {
    let spool_dir = folder.join(".audio-spool");
    fs::create_dir(&spool_dir)?;
    create_queue(spool_dir, true)
}

fn create_queue(
    spool_dir: PathBuf,
    retain_files: bool,
) -> io::Result<(
    TranscriptionQueueSender,
    TranscriptionQueueReceiver,
    Arc<TranscriptionMetrics>,
)> {
    let metrics = Arc::new(TranscriptionMetrics::default());
    let shared = Arc::new(QueueShared {
        spool_dir,
        retain_files,
        next_write: AtomicU64::new(0),
        closed: AtomicBool::new(false),
        notify: Notify::new(),
        metrics: metrics.clone(),
    });

    Ok((
        TranscriptionQueueSender {
            shared: shared.clone(),
        },
        TranscriptionQueueReceiver {
            shared,
            next_read: 0,
        },
        metrics,
    ))
}

fn cleanup_stale_spools(spool_root: &Path) {
    let Ok(entries) = fs::read_dir(spool_root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .and_then(|modified| modified.elapsed().map_err(io::Error::other))
            .map(|age| age >= STALE_SPOOL_AGE)
            .unwrap_or(false);
        if is_stale {
            if let Err(_error) = fs::remove_dir_all(&path) {
                log::warn!("Failed to remove stale transcription spool");
            }
        }
    }
}

impl TranscriptionQueueSender {
    pub(crate) fn mark_failed(&self) {
        self.shared.metrics.mark_failed();
    }

    pub async fn send(&self, chunk: AudioChunk) -> io::Result<()> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "transcription queue is closed",
            ));
        }

        // There is intentionally one producer (the audio pipeline), so a file is
        // fully committed before `next_write` advances to the next sequence.
        let sequence = self.shared.next_write.load(Ordering::Acquire);
        let final_path = queue_path(&self.shared.spool_dir, sequence);
        let temp_path = final_path.with_extension("tmp");
        let audio_ms = chunk_audio_ms(&chunk);
        let spool_bytes = HEADER_BYTES.saturating_add(
            u64::try_from(chunk.data.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(std::mem::size_of::<f32>() as u64),
        );

        let written =
            tokio::task::spawn_blocking(move || write_chunk(&temp_path, &final_path, &chunk))
                .await
                .map_err(|error| io::Error::other(format!("queue writer task failed: {error}")))
                .and_then(|result| result);
        if let Err(error) = written {
            self.shared.metrics.mark_failed();
            if self.shared.retain_files {
                let _ = fs::write(
                    self.shared.spool_dir.join(".incomplete"),
                    b"capture incomplete",
                );
            }
            return Err(error);
        }

        self.shared
            .next_write
            .store(sequence + 1, Ordering::Release);
        self.shared.metrics.enqueued(audio_ms, spool_bytes);
        self.shared.notify.notify_one();
        Ok(())
    }
}

impl Drop for TranscriptionQueueSender {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        self.shared.notify.notify_waiters();
    }
}

impl TranscriptionQueueReceiver {
    pub fn metrics(&self) -> Arc<TranscriptionMetrics> {
        self.shared.metrics.clone()
    }

    pub async fn recv(&mut self) -> io::Result<Option<QueuedAudioChunk>> {
        loop {
            if self.next_read < self.shared.next_write.load(Ordering::Acquire) {
                return self.read_next().await.map(Some);
            }
            if self.shared.closed.load(Ordering::Acquire) {
                return Ok(None);
            }

            // Hold a separate Arc while registering the waiter so the borrow
            // does not overlap `read_next(&mut self)` on the re-check.
            let shared = self.shared.clone();
            let notified = shared.notify.notified();
            if self.next_read < self.shared.next_write.load(Ordering::Acquire) {
                drop(notified);
                return self.read_next().await.map(Some);
            }
            if self.shared.closed.load(Ordering::Acquire) {
                return Ok(None);
            }
            notified.await;
        }
    }

    async fn read_next(&mut self) -> io::Result<QueuedAudioChunk> {
        let path = queue_path(&self.shared.spool_dir, self.next_read);
        let spool_bytes = fs::metadata(&path).map(|metadata| metadata.len())?;
        let read_path = path.clone();
        let chunk = tokio::task::spawn_blocking(move || read_chunk(&read_path))
            .await
            .map_err(|error| io::Error::other(format!("queue reader task failed: {error}")))??;
        if !self.shared.retain_files {
            fs::remove_file(path)?;
        }
        self.next_read += 1;
        let audio_ms = chunk_audio_ms(&chunk);
        Ok(QueuedAudioChunk {
            chunk,
            audio_ms,
            spool_bytes,
        })
    }
}

fn queue_path(spool_dir: &Path, sequence: u64) -> PathBuf {
    spool_dir.join(format!("{sequence:020}.chunk"))
}

fn chunk_audio_ms(chunk: &AudioChunk) -> u64 {
    if chunk.sample_rate == 0 {
        return 0;
    }
    ((chunk.data.len() as f64 / chunk.sample_rate as f64) * 1000.0)
        .round()
        .max(0.0) as u64
}

fn write_chunk(temp_path: &Path, final_path: &Path, chunk: &AudioChunk) -> io::Result<()> {
    let file = File::create(temp_path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(QUEUE_MAGIC)?;
    writer.write_all(&chunk.sample_rate.to_le_bytes())?;
    writer.write_all(&chunk.timestamp.to_le_bytes())?;
    writer.write_all(&chunk.chunk_id.to_le_bytes())?;
    writer.write_all(&[match chunk.device_type {
        DeviceType::Microphone => 0,
        DeviceType::System => 1,
    }])?;
    writer.write_all(
        &u64::try_from(chunk.data.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    )?;
    writer.write_all(bytemuck::cast_slice(&chunk.data))?;
    writer.flush()?;
    writer.get_ref().sync_data()?;
    drop(writer);
    fs::rename(temp_path, final_path)?;
    Ok(())
}

pub(crate) fn read_chunk(path: &Path) -> io::Result<AudioChunk> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if &magic != QUEUE_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid transcription queue file",
        ));
    }

    let sample_rate = read_u32(&mut reader)?;
    let timestamp = read_f64(&mut reader)?;
    let chunk_id = read_u64(&mut reader)?;
    let mut device = [0u8; 1];
    reader.read_exact(&mut device)?;
    let device_type = match device[0] {
        0 => DeviceType::Microphone,
        1 => DeviceType::System,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid transcription queue device type",
            ))
        }
    };
    let sample_count = usize::try_from(read_u64(&mut reader)?)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "sample count overflow"))?;
    if sample_count > MAX_SAMPLES_PER_CHUNK {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transcription queue chunk is too large",
        ));
    }
    let mut data = vec![0.0f32; sample_count];
    reader.read_exact(bytemuck::cast_slice_mut(&mut data))?;

    Ok(AudioChunk {
        data,
        sample_rate,
        timestamp,
        chunk_id,
        device_type,
    })
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> io::Result<u64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_f64(reader: &mut impl Read) -> io::Result<f64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(f64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: u64) -> AudioChunk {
        AudioChunk {
            data: vec![0.25, -0.5, 0.75],
            sample_rate: 16_000,
            timestamp: 1.25,
            chunk_id: id,
            device_type: DeviceType::System,
        }
    }

    #[tokio::test]
    async fn queue_round_trips_audio_and_reports_real_status() {
        let (sender, mut receiver, metrics) = transcription_queue().unwrap();
        sender.send(chunk(7)).await.unwrap();

        let queued = metrics.snapshot();
        assert_eq!(queued.chunks_in_queue, 1);
        assert!(queued.spool_bytes > HEADER_BYTES);

        let item = receiver.recv().await.unwrap().unwrap();
        assert_eq!(item.chunk.chunk_id, 7);
        assert_eq!(item.chunk.device_type, DeviceType::System);
        assert_eq!(item.chunk.data, vec![0.25, -0.5, 0.75]);

        metrics.mark_processing_started();
        metrics.mark_completed(
            item.audio_ms(),
            item.spool_bytes(),
            Duration::from_millis(2),
        );
        let completed = metrics.snapshot();
        assert_eq!(completed.chunks_in_queue, 0);
        assert_eq!(completed.total_chunks_completed, 1);
        assert_eq!(completed.spool_bytes, 0);
    }

    #[tokio::test]
    async fn dropping_sender_closes_receiver_after_spooled_items() {
        let (sender, mut receiver, _) = transcription_queue().unwrap();
        sender.send(chunk(1)).await.unwrap();
        drop(sender);

        assert!(receiver.recv().await.unwrap().is_some());
        assert!(receiver.recv().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn two_hour_live_segment_backlog_is_spooled_and_ordered() {
        // 1,200 six-second VAD segments represent two hours of continuous
        // speech. Tiny payloads keep the test fast; the queue's storage path is
        // identical for production-sized buffers.
        const SEGMENTS: u64 = 1_200;
        let (sender, mut receiver, metrics) = transcription_queue().unwrap();
        for id in 0..SEGMENTS {
            sender.send(chunk(id)).await.unwrap();
        }
        drop(sender);

        assert_eq!(metrics.snapshot().chunks_in_queue, SEGMENTS as usize);
        for expected_id in 0..SEGMENTS {
            let item = receiver.recv().await.unwrap().unwrap();
            assert_eq!(item.chunk.chunk_id, expected_id);
            metrics.mark_completed(
                item.audio_ms(),
                item.spool_bytes(),
                Duration::from_millis(1),
            );
        }
        assert!(receiver.recv().await.unwrap().is_none());
        assert_eq!(metrics.snapshot().chunks_in_queue, 0);
    }
}
