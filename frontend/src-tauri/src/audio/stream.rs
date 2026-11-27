use anyhow::Result;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, Stream, SupportedStreamConfig};
use log::{error, info, warn};
use ringbuf::{
    traits::{Consumer, Observer, Producer, Split},
    HeapCons, HeapProd, HeapRb,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use super::capture::{get_current_backend, AudioCaptureBackend};
use super::devices::{get_device_and_config, AudioDevice};
use super::pipeline::AudioCapture;
use super::recording_state::{DeviceType, RecordingState};

#[cfg(target_os = "macos")]
use super::capture::CoreAudioCapture;

/// Stream backend implementation
pub enum StreamBackend {
    /// CPAL-based stream (ScreenCaptureKit or default)
    Cpal(Stream),
    /// Core Audio direct implementation (macOS only)
    #[cfg(target_os = "macos")]
    CoreAudio {
        task: Option<tokio::task::JoinHandle<()>>,
    },
}

// SAFETY: While Stream doesn't implement Send, we ensure it's only accessed
// from the same thread context by using spawn_blocking for operations that cross thread boundaries
unsafe impl Send for StreamBackend {}

/// Simplified audio stream wrapper with multi-backend support
pub struct AudioStream {
    device: Arc<AudioDevice>,
    backend: StreamBackend,
}

// SAFETY: AudioStream contains StreamBackend which we've marked as Send
unsafe impl Send for AudioStream {}

const PACKET_POOL_SIZE: usize = 32;

struct CapturePacket<T> {
    samples: Vec<T>,
    resume_revision: u64,
}

#[derive(Default)]
struct CaptureHandoffStatus {
    copying: AtomicBool,
    dropped_samples: AtomicUsize,
}

struct CaptureInput<T> {
    ready: HeapProd<CapturePacket<T>>,
    free: HeapCons<Vec<T>>,
    status: Arc<CaptureHandoffStatus>,
    packet_capacity: usize,
}

struct CaptureOutput<T> {
    ready: HeapCons<CapturePacket<T>>,
    free: HeapProd<Vec<T>>,
    status: Arc<CaptureHandoffStatus>,
}

fn capture_handoff<T>(
    packet_capacity: usize,
    packet_count: usize,
) -> (CaptureInput<T>, CaptureOutput<T>) {
    let (ready_tx, ready_rx) = HeapRb::new(packet_count).split();
    let (mut free_tx, free_rx) = HeapRb::new(packet_count).split();
    for _ in 0..packet_count {
        let _ = free_tx.try_push(Vec::with_capacity(packet_capacity));
    }
    let status = Arc::new(CaptureHandoffStatus::default());
    (
        CaptureInput {
            ready: ready_tx,
            free: free_rx,
            status: status.clone(),
            packet_capacity,
        },
        CaptureOutput {
            ready: ready_rx,
            free: free_tx,
            status,
        },
    )
}

impl<T: Copy> CaptureInput<T> {
    fn copy_samples(&mut self, samples: &[T], resume_revision: u64) {
        let mut remaining = samples;
        while !remaining.is_empty() {
            let Some(mut buffer) = self.free.try_pop() else {
                self.status
                    .dropped_samples
                    .fetch_add(remaining.len(), Ordering::Relaxed);
                return;
            };
            let (packet, rest) = remaining.split_at(remaining.len().min(self.packet_capacity));
            buffer.extend_from_slice(packet);
            // Every ready packet uses one of the fixed number of free buffers.
            let _ = self.ready.try_push(CapturePacket {
                samples: buffer,
                resume_revision,
            });
            remaining = rest;
        }
    }
}

impl<T: cpal::SizedSample + Copy + Send + 'static> CaptureOutput<T> {
    fn run(
        mut self,
        state: Arc<RecordingState>,
        capture: AudioCapture,
        sender: mpsc::Sender<super::recording_state::AudioChunk>,
        sample_rate: u32,
        channels: u16,
        to_f32: impl Fn(T) -> f32,
    ) {
        let mut converted = Vec::new();
        loop {
            let stopped = !state.is_recording() || !self.ready.write_is_held();
            // A Stop can race a callback already copying its final packet.
            let can_exit = stopped && !self.status.copying.load(Ordering::SeqCst);
            let dropped = self.status.dropped_samples.swap(0, Ordering::Relaxed);
            if dropped > 0 {
                state.record_sample_loss(
                    (dropped * 48_000).div_ceil(usize::from(channels) * sample_rate as usize),
                );
            }
            if let Some(mut packet) = self.ready.try_pop() {
                converted.clear();
                converted.extend(packet.samples.iter().copied().map(&to_f32));
                packet.samples.clear();
                let _ = self.free.try_push(packet.samples);
                capture.process_captured_data(&converted, packet.resume_revision, |chunk| {
                    state.send_captured_chunk(&sender, chunk)
                });
            } else if can_exit {
                return;
            } else {
                // The real-time thread only writes to the ring; it never wakes
                // the worker through a lock or performs an OS wait.
                std::thread::park_timeout(Duration::from_millis(2));
            }
        }
    }
}

impl AudioStream {
    /// Create a new audio stream for the given device
    pub async fn create(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        device_type: DeviceType,
        recording_sender: Option<mpsc::UnboundedSender<super::recording_state::AudioChunk>>,
    ) -> Result<Self> {
        // Get current backend from global config
        let backend_type = get_current_backend();
        Self::create_with_backend(device, state, device_type, recording_sender, backend_type).await
    }

    /// Create a new audio stream with explicit backend selection
    pub async fn create_with_backend(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        device_type: DeviceType,
        recording_sender: Option<mpsc::UnboundedSender<super::recording_state::AudioChunk>>,
        backend_type: AudioCaptureBackend,
    ) -> Result<Self> {
        info!(
            "🎵 Stream: Creating audio stream for device: {} with backend: {:?}, device_type: {:?}",
            device.name, backend_type, device_type
        );

        // For system audio devices, use the selected backend
        // For microphone devices, always use CPAL
        #[cfg(target_os = "macos")]
        let use_core_audio =
            device_type == DeviceType::System && backend_type == AudioCaptureBackend::CoreAudio;

        #[cfg(not(target_os = "macos"))]
        let use_core_audio = false;

        #[cfg(target_os = "macos")]
        info!(
            "🎵 Stream: use_core_audio = {}, device_type == System: {}, backend == CoreAudio: {}",
            use_core_audio,
            device_type == DeviceType::System,
            backend_type == AudioCaptureBackend::CoreAudio
        );

        #[cfg(not(target_os = "macos"))]
        info!(
            "🎵 Stream: use_core_audio = {}, device_type == System: {}",
            use_core_audio,
            device_type == DeviceType::System
        );

        #[cfg(target_os = "macos")]
        if use_core_audio {
            info!("🎵 Stream: Using Core Audio backend (cidre) for system audio");
            return Self::create_core_audio_stream(device, state, device_type, recording_sender)
                .await;
        }

        // Default path: use CPAL
        #[cfg(target_os = "macos")]
        let backend_name = if backend_type == AudioCaptureBackend::ScreenCaptureKit {
            "ScreenCaptureKit"
        } else {
            "CPAL (default)"
        };

        #[cfg(not(target_os = "macos"))]
        let backend_name = "CPAL";

        info!(
            "🎵 Stream: Using CPAL backend ({}) for device: {}",
            backend_name, device.name
        );
        Self::create_cpal_stream(device, state, device_type, recording_sender).await
    }

    /// Create a CPAL-based stream (ScreenCaptureKit on macOS)
    async fn create_cpal_stream(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        device_type: DeviceType,
        _recording_sender: Option<mpsc::UnboundedSender<super::recording_state::AudioChunk>>,
    ) -> Result<Self> {
        info!("Creating CPAL stream for device: {}", device.name);

        // Get the underlying cpal device and config
        let (cpal_device, config) = get_device_and_config(&device).await?;

        info!(
            "Audio config - Sample rate: {}, Channels: {}, Format: {:?}",
            config.sample_rate().0,
            config.channels(),
            config.sample_format()
        );

        // Create audio capture processor
        let capture = AudioCapture::new(
            device.clone(),
            state.clone(),
            config.sample_rate().0,
            config.channels(),
            device_type,
        );

        // Build the appropriate stream based on sample format
        let stream = Self::build_stream(&cpal_device, &config, state, capture)?;

        // Start the stream
        stream.play()?;
        info!("CPAL stream started for device: {}", device.name);

        Ok(Self {
            device,
            backend: StreamBackend::Cpal(stream),
        })
    }

    /// Create a Core Audio stream (macOS only)
    #[cfg(target_os = "macos")]
    async fn create_core_audio_stream(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        device_type: DeviceType,
        _recording_sender: Option<mpsc::UnboundedSender<super::recording_state::AudioChunk>>,
    ) -> Result<Self> {
        info!(
            "🔊 Stream: Creating Core Audio stream for device: {}",
            device.name
        );

        // Create Core Audio capture
        info!("🔊 Stream: Calling CoreAudioCapture::new()...");
        let capture_impl = CoreAudioCapture::new().map_err(|e| {
            error!("❌ Stream: CoreAudioCapture::new() failed: {}", e);
            anyhow::anyhow!("Failed to create Core Audio capture: {}", e)
        })?;

        info!("✅ Stream: CoreAudioCapture created, calling stream()...");
        let core_stream = capture_impl.stream().map_err(|e| {
            error!("❌ Stream: capture_impl.stream() failed: {}", e);
            anyhow::anyhow!("Failed to create Core Audio stream: {}", e)
        })?;

        let sample_rate = core_stream.sample_rate();
        info!(
            "✅ Stream: Core Audio stream created with sample rate: {} Hz",
            sample_rate
        );

        // Create audio capture processor for pipeline integration
        // CRITICAL: Core Audio tap is MONO (with_mono_global_tap_excluding_processes)
        let capture = AudioCapture::new(
            device.clone(),
            state.clone(),
            sample_rate,
            1, // Core Audio tap is MONO (not stereo!)
            device_type,
        );

        // Spawn task to process Core Audio stream samples
        // The stream needs to be polled continuously to produce samples
        let device_name = device.name.clone();
        info!("🔊 Stream: Spawning tokio task to poll Core Audio stream...");
        let task = tokio::spawn({
            let capture = capture.clone();
            let mut stream = core_stream;

            async move {
                use futures_util::StreamExt;

                let mut buffer = Vec::new();
                let mut frame_count = 0;
                let frames_per_chunk = 1024; // Process in chunks of 1024 samples

                info!(
                    "✅ Stream: Core Audio processing task started for {}",
                    device_name
                );

                let mut _sample_count = 0u64;
                while let Some(sample) = stream.next().await {
                    _sample_count += 1;
                    // if _sample_count % 48000 == 0 {
                    //     info!("📊 Stream: Received {} samples from Core Audio stream", _sample_count);
                    // }

                    buffer.push(sample);
                    frame_count += 1;

                    // Process when we have enough samples
                    if frame_count >= frames_per_chunk {
                        capture.process_audio_data(&buffer);
                        buffer.clear();
                        frame_count = 0;
                    }
                }

                // Process any remaining samples
                if !buffer.is_empty() {
                    capture.process_audio_data(&buffer);
                }

                info!(
                    "⚠️ Stream: Core Audio processing task ended for {}",
                    device_name
                );
            }
        });

        info!(
            "✅ Stream: Core Audio stream fully initialized for device: {}",
            device.name
        );

        Ok(Self {
            device: device.clone(),
            backend: StreamBackend::CoreAudio { task: Some(task) },
        })
    }

    /// Build stream based on sample format
    fn build_stream(
        device: &Device,
        config: &SupportedStreamConfig,
        state: Arc<RecordingState>,
        capture: AudioCapture,
    ) -> Result<Stream> {
        match config.sample_format() {
            cpal::SampleFormat::F32 => {
                Self::build_typed_stream(device, config, state, capture, |sample: f32| sample)
            }
            cpal::SampleFormat::I16 => {
                Self::build_typed_stream(device, config, state, capture, |sample: i16| {
                    sample as f32 / i16::MAX as f32
                })
            }
            cpal::SampleFormat::I32 => {
                Self::build_typed_stream(device, config, state, capture, |sample: i32| {
                    sample as f32 / i32::MAX as f32
                })
            }
            cpal::SampleFormat::I8 => {
                Self::build_typed_stream(device, config, state, capture, |sample: i8| {
                    sample as f32 / i8::MAX as f32
                })
            }
            format => Err(anyhow::anyhow!("Unsupported sample format: {format:?}")),
        }
    }

    fn build_typed_stream<T: cpal::SizedSample + Copy + Send + 'static>(
        device: &Device,
        config: &SupportedStreamConfig,
        state: Arc<RecordingState>,
        capture: AudioCapture,
        to_f32: impl Fn(T) -> f32 + Send + 'static,
    ) -> Result<Stream> {
        let sender = state
            .capture_sender()
            .ok_or_else(|| anyhow::anyhow!("Audio pipeline not ready"))?;
        let rate = config.sample_rate().0;
        let channels = config.channels();
        let packet_capacity = (rate as usize).div_ceil(10) * usize::from(channels);
        let (mut input, output) = capture_handoff(packet_capacity, PACKET_POOL_SIZE);
        let callback_state = state.clone();
        let error_handler = capture.clone();
        let stream = device.build_input_stream(
            &config.clone().into(),
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                input.status.copying.store(true, Ordering::SeqCst);
                let revision = callback_state.resume_revision();
                if callback_state.is_active() && callback_state.resume_revision() == revision {
                    input.copy_samples(data, revision);
                }
                input.status.copying.store(false, Ordering::SeqCst);
            },
            move |err| error_handler.handle_stream_error(err),
            None,
        )?;
        std::thread::Builder::new()
            .name("audio-capture-dsp".into())
            .spawn(move || output.run(state, capture, sender, rate, channels, to_f32))?;
        Ok(stream)
    }

    /// Get device info
    pub fn device(&self) -> &AudioDevice {
        &self.device
    }

    /// Stop the stream
    pub fn stop(self) -> Result<()> {
        info!("Stopping audio stream for device: {}", self.device.name);

        match self.backend {
            StreamBackend::Cpal(stream) => {
                // CRITICAL: Pause the stream first to stop callbacks immediately
                // This ensures closures stop executing before we drop the stream,
                // allowing Arc references captured in callbacks to be released
                if let Err(e) = stream.pause() {
                    warn!("Failed to pause stream before drop: {}", e);
                }
                info!("Stream paused, now dropping to release callbacks");
                drop(stream);
            }
            #[cfg(target_os = "macos")]
            StreamBackend::CoreAudio { task } => {
                // Abort the processing task and wait briefly for cleanup
                if let Some(task_handle) = task {
                    info!("Aborting Core Audio task...");
                    task_handle.abort();
                    // Give the runtime a moment to clean up the aborted task
                    // This helps ensure Arc references in the closure are dropped
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    info!("Core Audio task aborted");
                }
            }
        }

        // Explicitly drop self.device Arc reference
        drop(self.device);
        info!("Audio stream stopped and device reference dropped");
        Ok(())
    }
}

/// Audio stream manager for handling multiple streams
pub struct AudioStreamManager {
    microphone_stream: Option<AudioStream>,
    system_stream: Option<AudioStream>,
    state: Arc<RecordingState>,
}

// SAFETY: AudioStreamManager contains AudioStream which we've marked as Send
unsafe impl Send for AudioStreamManager {}

impl AudioStreamManager {
    pub fn new(state: Arc<RecordingState>) -> Self {
        Self {
            microphone_stream: None,
            system_stream: None,
            state,
        }
    }

    /// Start audio streams for the given devices
    pub async fn start_streams(
        &mut self,
        microphone_device: Option<Arc<AudioDevice>>,
        system_device: Option<Arc<AudioDevice>>,
        recording_sender: Option<mpsc::UnboundedSender<super::recording_state::AudioChunk>>,
        mut first_source_started: impl FnMut(),
    ) -> Result<()> {
        let microphone_first = microphone_device.is_some();
        if let Some(device) = microphone_device {
            self.start_source(device, DeviceType::Microphone, recording_sender.clone())
                .await?;
            first_source_started();
        }

        if let Some(device) = system_device {
            if self
                .start_source(device.clone(), DeviceType::System, recording_sender)
                .await
                .is_err()
            {
                self.state.mark_source_missing(device, DeviceType::System);
                self.state.report_warning(
                    "Could not capture system audio from the selected output. Retry the connection or choose a different output for your next recording.",
                );
            } else if !microphone_first {
                first_source_started();
            }
        }

        if !self.has_active_streams() {
            return Err(anyhow::anyhow!("No audio streams could be created"));
        }
        Ok(())
    }

    async fn start_source(
        &mut self,
        device: Arc<AudioDevice>,
        device_type: DeviceType,
        sender: Option<mpsc::UnboundedSender<super::recording_state::AudioChunk>>,
    ) -> Result<()> {
        let stream = AudioStream::create(
            device.clone(),
            self.state.clone(),
            device_type.clone(),
            sender,
        )
        .await?;
        match device_type {
            DeviceType::Microphone => {
                self.state.set_microphone_device(device);
                self.microphone_stream = Some(stream);
            }
            DeviceType::System => {
                self.state.set_system_device(device);
                self.system_stream = Some(stream);
            }
        }
        Ok(())
    }

    /// Replace only the requested source. A failed open leaves the other
    /// source's stream running and the requested source's slot empty.
    pub async fn reconnect_source(
        &mut self,
        device: Arc<AudioDevice>,
        device_type: DeviceType,
    ) -> Result<()> {
        let stream = match device_type {
            DeviceType::Microphone => self.microphone_stream.take(),
            DeviceType::System => self.system_stream.take(),
        };
        self.state.clear_active_device(&device_type);
        if let Some(stream) = stream {
            let _ = stream.stop();
        }
        self.start_source(device, device_type, None).await
    }

    pub fn source_active(&self, device_type: &DeviceType) -> bool {
        match device_type {
            DeviceType::Microphone => self.microphone_stream.is_some(),
            DeviceType::System => self.system_stream.is_some(),
        }
    }

    /// Stop all audio streams
    pub fn stop_streams(&mut self) -> Result<()> {
        info!("Stopping all audio streams");

        let mut errors = Vec::new();

        // Stop microphone stream
        if let Some(mic_stream) = self.microphone_stream.take() {
            if let Err(e) = mic_stream.stop() {
                error!("Failed to stop microphone stream: {}", e);
                errors.push(e);
            }
        }

        // Stop system stream
        if let Some(sys_stream) = self.system_stream.take() {
            if let Err(e) = sys_stream.stop() {
                error!("Failed to stop system stream: {}", e);
                errors.push(e);
            }
        }

        if !errors.is_empty() {
            Err(anyhow::anyhow!("Failed to stop some streams: {:?}", errors))
        } else {
            info!("All audio streams stopped successfully");
            Ok(())
        }
    }

    /// Get stream count
    pub fn active_stream_count(&self) -> usize {
        let mut count = 0;
        if self.microphone_stream.is_some() {
            count += 1;
        }
        if self.system_stream.is_some() {
            count += 1;
        }
        count
    }

    /// Check if any streams are active
    pub fn has_active_streams(&self) -> bool {
        self.microphone_stream.is_some() || self.system_stream.is_some()
    }
}

impl Drop for AudioStreamManager {
    fn drop(&mut self) {
        if let Err(e) = self.stop_streams() {
            error!("Error stopping streams during drop: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_handoff_is_bounded_and_drains_accepted_packets_after_stop() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        let (sender, mut receiver) = mpsc::channel(2);
        state.set_audio_sender(sender.clone());
        let capture = AudioCapture::new(
            Arc::new(AudioDevice::new(
                "Fixture output".into(),
                super::super::devices::DeviceType::Output,
            )),
            state.clone(),
            48_000,
            1,
            DeviceType::System,
        );
        let (mut input, output) = capture_handoff(2, 2);
        input.copy_samples(&[1_i16, 2, 3, 4, 5, 6], 0);

        // The driver can still own its callback when Stop freezes capture.
        state.stop_recording();
        output.run(state.clone(), capture, sender, 48_000, 1, f32::from);

        assert_eq!(receiver.try_recv().unwrap().data, [1.0, 2.0]);
        assert_eq!(receiver.try_recv().unwrap().data, [3.0, 4.0]);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        ));
        assert!(state.capture_incomplete());
        assert_eq!(state.health.lock().unwrap().details.dropped_samples, 2);
    }
}
