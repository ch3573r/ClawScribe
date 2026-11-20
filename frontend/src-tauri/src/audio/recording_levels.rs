//! Live input levels of a running recording, for the UI's level meters.
//!
//! The pipeline task (not the capture callback) already looks at every chunk of
//! each captured source. `LevelMeter` folds those chunks into one snapshot per
//! window of about 100 ms, the cadence of the setup screen's `audio-levels`
//! (`simple_level_monitor`), and reports it as `recording-levels`. It keeps a
//! running sum, a count and a peak per source, so it never allocates.
use super::recording_state::DeviceType;
use serde::Serialize;
use std::time::{Duration, Instant};

/// One snapshot per window, so about ten per second.
pub(crate) const LEVEL_WINDOW: Duration = Duration::from_millis(100);

/// Linear level of one source over a window, 0..1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SourceLevel {
    pub rms: f32,
    pub peak: f32,
}

/// Payload of `recording-levels`. A source that is not captured is `null`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct RecordingLevels {
    pub mic: Option<SourceLevel>,
    pub system: Option<SourceLevel>,
}

#[derive(Default)]
struct Window {
    sum_squares: f64,
    samples: u64,
    peak: f32,
}

impl Window {
    fn add(&mut self, samples: &[f32]) {
        let mut sum = 0.0f64;
        let mut peak = self.peak;
        for &sample in samples {
            sum += f64::from(sample) * f64::from(sample);
            peak = peak.max(sample.abs());
        }
        self.sum_squares += sum;
        self.samples += samples.len() as u64;
        self.peak = peak;
    }

    /// A captured source with no chunk in the window reads as silence.
    fn level(&self) -> SourceLevel {
        if self.samples == 0 {
            return SourceLevel {
                rms: 0.0,
                peak: 0.0,
            };
        }
        SourceLevel {
            rms: ((self.sum_squares / self.samples as f64).sqrt() as f32).min(1.0),
            peak: self.peak.min(1.0),
        }
    }
}

pub(crate) struct LevelMeter {
    window_start: Instant,
    mic: Window,
    system: Window,
}

impl LevelMeter {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            mic: Window::default(),
            system: Window::default(),
        }
    }

    /// Adds one chunk. Returns a snapshot when the window has elapsed and
    /// `active` (recording and not paused); `captured` tells which sources are
    /// being captured and is only asked for when a snapshot is built.
    pub(crate) fn observe(
        &mut self,
        source: &DeviceType,
        samples: &[f32],
        now: Instant,
        active: bool,
        captured: impl FnOnce() -> (bool, bool),
    ) -> Option<RecordingLevels> {
        match source {
            DeviceType::Microphone => self.mic.add(samples),
            DeviceType::System => self.system.add(samples),
        }
        if now.duration_since(self.window_start) < LEVEL_WINDOW {
            return None;
        }
        self.window_start = now;
        let mic = std::mem::take(&mut self.mic);
        let system = std::mem::take(&mut self.system);
        if !active {
            return None;
        }
        let (mic_captured, system_captured) = captured();
        Some(RecordingLevels {
            mic: mic_captured.then(|| mic.level()),
            system: system_captured.then(|| system.level()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both() -> (bool, bool) {
        (true, true)
    }

    #[test]
    fn nothing_is_reported_inside_a_window() {
        let start = Instant::now();
        let mut meter = LevelMeter::new(start);
        let chunk = [0.5f32; 480];
        assert!(meter
            .observe(&DeviceType::Microphone, &chunk, start + Duration::from_millis(10), true, both)
            .is_none());
        assert!(meter
            .observe(&DeviceType::System, &chunk, start + Duration::from_millis(99), true, both)
            .is_none());
    }

    #[test]
    fn about_ten_snapshots_a_second_aggregate_the_whole_window() {
        let start = Instant::now();
        let mut meter = LevelMeter::new(start);
        let mut snapshots = 0;
        let mut last = None;
        // 480-sample chunks every 10 ms for one second.
        for tick in 1..=100u64 {
            let now = start + Duration::from_millis(tick * 10);
            let quiet = [0.1f32; 480];
            let mut loud = [0.1f32; 480];
            loud[7] = -0.8;
            if let Some(levels) = meter.observe(&DeviceType::Microphone, &loud, now, true, both) {
                snapshots += 1;
                last = Some(levels);
            }
            meter.observe(&DeviceType::System, &quiet, now, true, both);
        }
        assert_eq!(snapshots, 10);
        let mic = last.unwrap().mic.unwrap();
        // The peak keeps the loudest sample of the window, not the last chunk's.
        assert!((mic.peak - 0.8).abs() < 1e-6);
        assert!(mic.rms > 0.1 && mic.rms < 0.2);
    }

    #[test]
    fn rms_and_peak_are_linear_and_correct_for_a_known_signal() {
        let start = Instant::now();
        let mut meter = LevelMeter::new(start);
        // Alternating +-0.5: rms 0.5, peak 0.5.
        let wave: Vec<f32> = (0..480).map(|i| if i % 2 == 0 { 0.5 } else { -0.5 }).collect();
        let levels = meter
            .observe(&DeviceType::Microphone, &wave, start + LEVEL_WINDOW, true, both)
            .unwrap();
        let mic = levels.mic.unwrap();
        assert!((mic.rms - 0.5).abs() < 1e-6);
        assert!((mic.peak - 0.5).abs() < 1e-6);
    }

    #[test]
    fn values_are_clamped_to_one() {
        let start = Instant::now();
        let mut meter = LevelMeter::new(start);
        let levels = meter
            .observe(&DeviceType::System, &[2.0, -3.0], start + LEVEL_WINDOW, true, both)
            .unwrap();
        let system = levels.system.unwrap();
        assert_eq!(system.peak, 1.0);
        assert_eq!(system.rms, 1.0);
    }

    #[test]
    fn a_source_that_is_not_captured_is_null_and_a_silent_captured_one_is_zero() {
        let start = Instant::now();
        let mut meter = LevelMeter::new(start);
        // Only microphone chunks arrive; the system source is not captured.
        let levels = meter
            .observe(&DeviceType::Microphone, &[0.25; 100], start + LEVEL_WINDOW, true, || (true, false))
            .unwrap();
        assert_eq!(levels.system, None);
        assert!(levels.mic.is_some());
        // Captured but no chunk in the window reads as silence, not as missing.
        let levels = meter
            .observe(&DeviceType::Microphone, &[], start + LEVEL_WINDOW * 2, true, || (true, true))
            .unwrap();
        assert_eq!(levels.system, Some(SourceLevel { rms: 0.0, peak: 0.0 }));
        assert_eq!(levels.mic, Some(SourceLevel { rms: 0.0, peak: 0.0 }));
    }

    #[test]
    fn nothing_is_reported_while_paused_or_stopped_and_the_window_restarts() {
        let start = Instant::now();
        let mut meter = LevelMeter::new(start);
        assert!(meter
            .observe(&DeviceType::Microphone, &[0.9; 100], start + LEVEL_WINDOW, false, both)
            .is_none());
        // Audio from before is not carried into the first window after resuming.
        let levels = meter
            .observe(&DeviceType::Microphone, &[0.1; 100], start + LEVEL_WINDOW * 2, true, both)
            .unwrap();
        assert!((levels.mic.unwrap().peak - 0.1).abs() < 1e-6);
    }

    #[test]
    fn the_capture_check_is_only_made_when_a_snapshot_is_built() {
        let start = Instant::now();
        let mut meter = LevelMeter::new(start);
        let mut asked = 0;
        for tick in 1..=9u64 {
            meter.observe(
                &DeviceType::Microphone,
                &[0.1; 10],
                start + Duration::from_millis(tick * 10),
                true,
                || {
                    asked += 1;
                    both()
                },
            );
        }
        assert_eq!(asked, 0);
    }

    #[test]
    fn the_payload_has_the_documented_shape() {
        let json = serde_json::to_value(RecordingLevels {
            mic: Some(SourceLevel { rms: 0.25, peak: 0.5 }),
            system: None,
        })
        .unwrap();
        assert_eq!(json, serde_json::json!({ "mic": { "rms": 0.25, "peak": 0.5 }, "system": null }));
    }
}
