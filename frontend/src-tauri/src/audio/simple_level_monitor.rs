use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, SampleRate, Stream, StreamConfig};
use log::{error, info, warn};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Runtime};

use super::audio_processing::audio_to_mono;

#[derive(Debug, Serialize, Clone)]
pub struct AudioLevelData {
    pub device_name: String,
    pub device_type: String, // "input" or "output"
    pub rms_level: f32,      // RMS level (0.0 to 1.0)
    pub peak_level: f32,     // Peak level (0.0 to 1.0)
    pub is_active: bool,     // Whether audio is being detected
}

#[derive(Debug, Serialize, Clone)]
pub struct AudioLevelUpdate {
    pub timestamp: u64,
    pub levels: Vec<AudioLevelData>,
}

#[derive(Clone)]
struct LevelSnapshot {
    rms_level: f32,
    peak_level: f32,
    is_active: bool,
    updated_at: Instant,
}

struct MonitorState {
    keep_running: Arc<AtomicBool>,
    worker: std::thread::JoinHandle<()>,
}

static IS_MONITORING: AtomicBool = AtomicBool::new(false);
static MONITOR_STATE: LazyLock<Mutex<Option<MonitorState>>> = LazyLock::new(|| Mutex::new(None));

/// Start real microphone level monitoring for the specified input devices.
pub async fn start_monitoring<R: Runtime>(
    app_handle: AppHandle<R>,
    device_names: Vec<String>,
) -> Result<()> {
    info!(
        "Starting audio level monitoring for devices: {:?}",
        device_names
    );

    stop_monitoring().await?;

    let normalized_device_names = normalize_device_names(device_names);
    if normalized_device_names.is_empty() {
        return Err(anyhow!("No microphone devices were provided to monitor"));
    }

    let levels = Arc::new(Mutex::new(HashMap::<String, LevelSnapshot>::new()));
    let keep_running = Arc::new(AtomicBool::new(true));
    let (setup_tx, setup_rx) = mpsc::channel::<Result<(), String>>();
    let worker_keep_running = keep_running.clone();
    let worker_levels = levels.clone();
    let worker_device_names = normalized_device_names.clone();
    let worker_app_handle = app_handle.clone();

    super::com_anchor::ensure_device_enumerator();
    let worker = std::thread::Builder::new()
        .name("clawscribe-mic-level-monitor".to_string())
        .spawn(move || {
            run_monitor_thread(
                worker_app_handle,
                worker_device_names,
                worker_levels,
                worker_keep_running,
                setup_tx,
            );
        })
        .map_err(|err| anyhow!("Failed to start microphone level monitor thread: {}", err))?;

    let setup_result =
        tokio::task::spawn_blocking(move || setup_rx.recv_timeout(Duration::from_secs(10)))
            .await
            .map_err(|err| anyhow!("Microphone level monitor setup task failed: {}", err))?
            .map_err(|err| anyhow!("Microphone level monitor setup timed out: {}", err))?;

    if let Err(err) = setup_result {
        keep_running.store(false, Ordering::SeqCst);
        if let Err(join_err) = worker.join() {
            warn!(
                "Microphone level monitor thread failed after setup error: {:?}",
                join_err
            );
        }
        return Err(anyhow!(err));
    }

    let mut state = MONITOR_STATE
        .lock()
        .map_err(|_| anyhow!("Audio level monitor state lock is poisoned"))?;

    IS_MONITORING.store(true, Ordering::SeqCst);
    *state = Some(MonitorState {
        keep_running,
        worker,
    });

    Ok(())
}

/// Stop audio level monitoring and release microphone streams.
pub async fn stop_monitoring() -> Result<()> {
    info!("Stopping audio level monitoring");
    IS_MONITORING.store(false, Ordering::SeqCst);

    let mut state = MONITOR_STATE
        .lock()
        .map_err(|_| anyhow!("Audio level monitor state lock is poisoned"))?;

    if let Some(state) = state.take() {
        state.keep_running.store(false, Ordering::SeqCst);
        if let Err(err) = state.worker.join() {
            warn!("Microphone level monitor thread failed to stop: {:?}", err);
        }
    }

    Ok(())
}

/// Check if currently monitoring.
pub fn is_monitoring() -> bool {
    IS_MONITORING.load(Ordering::SeqCst)
}

fn normalize_device_names(device_names: Vec<String>) -> Vec<String> {
    device_names
        .into_iter()
        .map(|name| strip_device_type_suffix(&name).to_string())
        .filter(|name| !name.trim().is_empty())
        .fold(Vec::new(), |mut unique, name| {
            if !unique.iter().any(|existing| existing == &name) {
                unique.push(name);
            }
            unique
        })
}

fn strip_device_type_suffix(device_name: &str) -> &str {
    let trimmed = device_name.trim();
    trimmed
        .strip_suffix("(input)")
        .or_else(|| trimmed.strip_suffix("(Input)"))
        .or_else(|| trimmed.strip_suffix("(INPUT)"))
        .unwrap_or(trimmed)
        .trim()
}

fn find_input_device(host: &cpal::Host, device_name: &str) -> Result<cpal::Device> {
    for device in host.input_devices()? {
        if let Ok(name) = device.name() {
            if name == device_name {
                return Ok(device);
            }
        }
    }

    Err(anyhow!("Input device '{}' was not found", device_name))
}

fn run_monitor_thread<R: Runtime>(
    app_handle: AppHandle<R>,
    monitored_names: Vec<String>,
    levels: Arc<Mutex<HashMap<String, LevelSnapshot>>>,
    keep_running: Arc<AtomicBool>,
    setup_tx: mpsc::Sender<Result<(), String>>,
) {
    let host = cpal::default_host();
    let mut streams = Vec::new();
    let mut failures = Vec::new();

    for device_name in &monitored_names {
        match find_input_device(&host, device_name) {
            Ok(device) => match create_input_level_stream(&device, device_name, levels.clone()) {
                Ok(stream) => streams.push(stream),
                Err(err) => {
                    warn!(
                        "Failed to create microphone level stream for '{}': {}",
                        device_name, err
                    );
                    failures.push(format!("{}: {}", device_name, err));
                }
            },
            Err(err) => {
                warn!(
                    "Microphone device not found for level test: {}",
                    device_name
                );
                failures.push(format!("{}: {}", device_name, err));
            }
        }
    }

    if streams.is_empty() {
        let _ = setup_tx.send(Err(format!(
            "No microphone level streams could be started{}",
            format_failure_suffix(&failures)
        )));
        return;
    }

    if !failures.is_empty() {
        warn!(
            "Audio level monitoring started with some device failures: {}",
            failures.join("; ")
        );
    }

    let _ = setup_tx.send(Ok(()));

    while keep_running.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));
        let level_update = build_level_update(&monitored_names, &levels);

        if let Err(err) = app_handle.emit("audio-levels", &level_update) {
            error!("Failed to emit microphone audio levels: {}", err);
            break;
        }
    }

    drop(streams);
    info!("Audio level monitoring thread ended");
}

fn create_input_level_stream(
    device: &cpal::Device,
    device_name: &str,
    levels: Arc<Mutex<HashMap<String, LevelSnapshot>>>,
) -> Result<Stream> {
    let config = device
        .default_input_config()
        .map_err(|err| anyhow!("Failed to get default input config: {}", err))?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels();
    let sample_format = config.sample_format();
    let stream_config = StreamConfig {
        channels,
        sample_rate: SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    info!(
        "Opening microphone level stream for '{}': {} Hz, {} channels, {:?}",
        device_name, sample_rate, channels, sample_format
    );

    let device_name = device_name.to_string();
    let stream = match sample_format {
        SampleFormat::F32 => {
            let levels = levels.clone();
            let device_name = device_name.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    update_levels(data, channels, &device_name, levels.clone());
                },
                |err| error!("Microphone level stream error: {}", err),
                None,
            )?
        }
        SampleFormat::I16 => {
            let levels = levels.clone();
            let device_name = device_name.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let data: Vec<f32> = data.iter().map(|&sample| sample.to_sample()).collect();
                    update_levels(&data, channels, &device_name, levels.clone());
                },
                |err| error!("Microphone level stream error: {}", err),
                None,
            )?
        }
        SampleFormat::U16 => {
            let levels = levels.clone();
            let device_name = device_name.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    let data: Vec<f32> = data.iter().map(|&sample| sample.to_sample()).collect();
                    update_levels(&data, channels, &device_name, levels.clone());
                },
                |err| error!("Microphone level stream error: {}", err),
                None,
            )?
        }
        other => {
            return Err(anyhow!("Unsupported microphone sample format: {:?}", other));
        }
    };

    stream.play()?;
    Ok(stream)
}

fn update_levels(
    data: &[f32],
    channels: u16,
    device_name: &str,
    levels: Arc<Mutex<HashMap<String, LevelSnapshot>>>,
) {
    if data.is_empty() {
        return;
    }

    let mono_data = if channels > 1 {
        audio_to_mono(data, channels)
    } else {
        data.to_vec()
    };

    if mono_data.is_empty() {
        return;
    }

    let rms = (mono_data.iter().map(|sample| sample * sample).sum::<f32>()
        / mono_data.len() as f32)
        .sqrt()
        .min(1.0);
    let peak = mono_data
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0, f32::max)
        .min(1.0);

    let snapshot = LevelSnapshot {
        rms_level: rms,
        peak_level: peak,
        is_active: rms > 0.001,
        updated_at: Instant::now(),
    };

    if let Ok(mut levels) = levels.try_lock() {
        levels.insert(device_name.to_string(), snapshot);
    }
}

fn build_level_update(
    monitored_names: &[String],
    levels: &Arc<Mutex<HashMap<String, LevelSnapshot>>>,
) -> AudioLevelUpdate {
    let now = Instant::now();
    let snapshots = levels
        .lock()
        .map(|levels| levels.clone())
        .unwrap_or_default();
    let emitted_levels = monitored_names
        .iter()
        .map(|device_name| {
            let snapshot = snapshots.get(device_name);
            let is_fresh = snapshot
                .map(|snapshot| {
                    now.duration_since(snapshot.updated_at) < Duration::from_millis(500)
                })
                .unwrap_or(false);

            AudioLevelData {
                device_name: device_name.clone(),
                device_type: "input".to_string(),
                rms_level: snapshot
                    .filter(|_| is_fresh)
                    .map(|snapshot| snapshot.rms_level)
                    .unwrap_or(0.0),
                peak_level: snapshot
                    .filter(|_| is_fresh)
                    .map(|snapshot| snapshot.peak_level)
                    .unwrap_or(0.0),
                is_active: snapshot
                    .filter(|_| is_fresh)
                    .map(|snapshot| snapshot.is_active)
                    .unwrap_or(false),
            }
        })
        .collect();

    AudioLevelUpdate {
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        levels: emitted_levels,
    }
}

fn format_failure_suffix(failures: &[String]) -> String {
    if failures.is_empty() {
        String::new()
    } else {
        format!(": {}", failures.join("; "))
    }
}
