//! Local throughput check using the same model loaders and native decode calls as live speech.
use crate::nemotron_engine::{
    commands as nemotron, model::SessionBackend, nemotron_engine::NemotronEngine,
};
use crate::parakeet_engine::{self, ParakeetEngine};
use crate::whisper_engine::{self, WhisperCompiledBackend, WhisperEngine};
use serde::Serialize;
use std::sync::{atomic::Ordering, Arc};
use std::time::{Duration, Instant};

const SAMPLE_RATE: usize = 16_000;
const SAMPLES: [(&str, &[u8]); 2] = [
    ("de", include_bytes!("pc_test_samples/de.s16le")),
    ("en", include_bytes!("pc_test_samples/en.s16le")),
];
const TEST_TIMEOUT: Duration = Duration::from_secs(240);
const MODEL_UNAVAILABLE: &str =
    "The selected model could not be loaded. Check that it is installed and ready in Transcription settings.";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleMeasurement {
    language: &'static str,
    audio_seconds: f64,
    elapsed_seconds: f64,
    recognized: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PcTestResult {
    engine: &'static str,
    model: String,
    backend: String,
    fallback_reason: Option<&'static str>,
    audio_seconds: f64,
    elapsed_seconds: f64,
    realtime_factor: f64,
    samples: Vec<SampleMeasurement>,
}

enum LocalEngine {
    Whisper(Arc<WhisperEngine>),
    Parakeet(Arc<ParakeetEngine>),
    Nemotron(Arc<NemotronEngine>),
}

impl LocalEngine {
    async fn get(provider: &str) -> Result<Self, String> {
        match provider {
            "localWhisper" => {
                whisper_engine::whisper_init()
                    .await
                    .map_err(|_| MODEL_UNAVAILABLE)?;
                whisper_engine::WHISPER_ENGINE
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                    .map(Self::Whisper)
                    .ok_or_else(|| MODEL_UNAVAILABLE.to_string())
            }
            "parakeet" => {
                parakeet_engine::parakeet_init()
                    .await
                    .map_err(|_| MODEL_UNAVAILABLE)?;
                parakeet_engine::PARAKEET_ENGINE
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                    .map(Self::Parakeet)
                    .ok_or_else(|| MODEL_UNAVAILABLE.to_string())
            }
            "nemotron" => nemotron::NEMOTRON_ENGINE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .map(Self::Nemotron)
                .ok_or_else(|| MODEL_UNAVAILABLE.to_string()),
            _ => Err("Choose an installed local transcription model to test this PC.".into()),
        }
    }

    async fn current_model(&self) -> Option<String> {
        match self {
            Self::Whisper(engine) => engine.get_current_model().await,
            Self::Parakeet(engine) => engine.get_current_model().await,
            Self::Nemotron(engine) => engine.get_current_model().await,
        }
    }

    async fn load(&self, model: &str) -> Result<(), String> {
        // Discovery populates each engine's pinned local catalog. The model
        // argument is an identifier looked up in that catalog, never a path.
        match self {
            Self::Whisper(engine) => {
                engine
                    .discover_models()
                    .await
                    .map_err(|_| MODEL_UNAVAILABLE)?;
                engine.load_model(model).await
            }
            Self::Parakeet(engine) => {
                engine
                    .discover_models()
                    .await
                    .map_err(|_| MODEL_UNAVAILABLE)?;
                engine.load_model(model).await
            }
            Self::Nemotron(engine) => {
                engine
                    .discover_models()
                    .await
                    .map_err(|_| MODEL_UNAVAILABLE)?;
                engine.load_model(model).await
            }
        }
        .map_err(|_| MODEL_UNAVAILABLE.to_string())
    }

    async fn unload(&self) {
        match self {
            Self::Whisper(engine) => engine.unload_model().await,
            Self::Parakeet(engine) => engine.unload_model().await,
            Self::Nemotron(engine) => engine.unload_model().await,
        };
    }

    async fn restore(&self, previous: Option<String>) -> Result<(), String> {
        if self.current_model().await == previous {
            return Ok(());
        }
        super::inference::wait_for_native_idle().await;
        self.unload().await;
        if let Some(model) = previous {
            self.load(&model).await.map_err(|_| {
                log::warn!("The previous local speech model could not be restored after the PC test");
                "The previous model could not be reloaded. Check that it is still installed in Transcription settings.".to_string()
            })?;
        }
        Ok(())
    }
}

trait TimedEngine {
    fn name(&self) -> &'static str;
    async fn worker_generation(&self) -> u64;
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: &str,
    ) -> Result<(bool, &'static str, Option<&'static str>), String>;
}

impl TimedEngine for LocalEngine {
    fn name(&self) -> &'static str {
        match self {
            Self::Whisper(_) => "Whisper",
            Self::Parakeet(_) => "Parakeet",
            Self::Nemotron(_) => "Nemotron",
        }
    }

    async fn worker_generation(&self) -> u64 {
        if let Self::Parakeet(engine) = self {
            return engine
                .session_status()
                .await
                .map_or(0, |(_, generation)| generation);
        }
        0
    }

    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: &str,
    ) -> Result<(bool, &'static str, Option<&'static str>), String> {
        let error =
            "The speech engine could not transcribe the test sample. Try another installed model.";
        match self {
            Self::Whisper(engine) => {
                let ((text, _, _), backend) = engine
                    .transcribe_with_backend(audio, Some(language.into()), true)
                    .await
                    .map_err(|_| error)?;
                let (backend, reason) = match backend {
                    Some(WhisperCompiledBackend::Cpu)
                        if WhisperCompiledBackend::current() != WhisperCompiledBackend::Cpu =>
                    {
                        (
                            "CPU",
                            Some("Whisper could not initialize the GPU backend and used CPU."),
                        )
                    }
                    Some(WhisperCompiledBackend::Cpu) => ("CPU", None),
                    Some(backend) => (backend.as_str(), None),
                    None => (
                        "Unconfirmed",
                        Some("Whisper did not report which backend handled this sample."),
                    ),
                };
                Ok((!text.trim().is_empty(), backend, reason))
            }
            Self::Parakeet(engine) => {
                let result = engine
                    .transcribe_audio_timestamped(audio)
                    .await
                    .map_err(|_| error)?;
                let (directml, _) = engine.session_status().await.ok_or(error)?;
                let requested =
                    parakeet_engine::parakeet_engine::USE_PARAKEET_DIRECTML.load(Ordering::Relaxed);
                let (backend, reason) = if directml {
                    ("DirectML + CPU", None)
                } else if requested && parakeet_engine::worker::directml_session_disabled() {
                    ("CPU", Some("Parakeet switched to CPU after its DirectML worker stopped responding."))
                } else if requested {
                    (
                        "CPU",
                        Some("DirectML was unavailable for this model session. Parakeet used CPU."),
                    )
                } else {
                    ("CPU", None)
                };
                Ok((!result.text.trim().is_empty(), backend, reason))
            }
            Self::Nemotron(engine) => {
                let text = engine
                    .transcribe_audio(audio, Some(language.into()))
                    .await
                    .map_err(|_| error)?;
                let (encoder, decoder) = engine.session_backends().await.ok_or(error)?;
                let backend = if [encoder, decoder].contains(&SessionBackend::DirectMl) {
                    "DirectML + CPU"
                } else {
                    "CPU"
                };
                let reason = if [encoder, decoder].contains(&SessionBackend::CpuFallback) {
                    Some("Nemotron used CPU for stages where DirectML was unavailable or did not pass its accuracy and speed checks.")
                } else {
                    None
                };
                Ok((!text.trim().is_empty(), backend, reason))
            }
        }
    }
}

fn pcm_samples(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|s| f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.0)
        .collect()
}

async fn measure(engine: &impl TimedEngine, model: String) -> Result<PcTestResult, String> {
    // A worker may lazily restart or fall back to CPU during the first decode.
    // Discard that entire mixed/load-bearing pass and measure the settled worker once.
    for _ in 0..2 {
        let generation = engine.worker_generation().await;
        let mut samples = Vec::with_capacity(SAMPLES.len());
        let mut backends = Vec::new();
        let mut fallback_reason = None;
        for (language, bytes) in SAMPLES {
            let audio = pcm_samples(bytes);
            let audio_seconds = audio.len() as f64 / SAMPLE_RATE as f64;
            let start = Instant::now();
            let (recognized, backend, reason) = engine.transcribe(audio, language).await?;
            let elapsed_seconds = start.elapsed().as_secs_f64();
            if !backends.contains(&backend) {
                backends.push(backend);
            }
            fallback_reason = reason.or(fallback_reason);
            samples.push(SampleMeasurement {
                language,
                audio_seconds,
                elapsed_seconds,
                recognized,
            });
        }
        if generation == engine.worker_generation().await {
            let audio_seconds = samples.iter().map(|s| s.audio_seconds).sum::<f64>();
            let elapsed_seconds = samples.iter().map(|s| s.elapsed_seconds).sum::<f64>();
            return Ok(PcTestResult {
                engine: engine.name(),
                model,
                backend: backends.join(" / "),
                fallback_reason,
                audio_seconds,
                elapsed_seconds,
                realtime_factor: elapsed_seconds / audio_seconds,
                samples,
            });
        }
    }
    Err("The transcription worker restarted repeatedly. Try another installed model or turn off DirectML.".into())
}

#[tauri::command]
pub async fn run_transcription_pc_test(
    app: tauri::AppHandle,
    provider: String,
    model: String,
) -> Result<PcTestResult, String> {
    let job = super::inference::claim_job()?;
    if provider == "nemotron" {
        nemotron::nemotron_init(app)
            .await
            .map_err(|_| MODEL_UNAVAILABLE)?;
    }
    // Closing a settings view cannot strand the tested model in the shared slot.
    tokio::spawn(run_test(job, provider, model))
        .await
        .map_err(|_| "The PC test could not finish.".to_string())?
}

async fn run_test(
    job: tokio::sync::OwnedSemaphorePermit,
    provider: String,
    model: String,
) -> Result<PcTestResult, String> {
    let lifecycle = super::common::acquire_engine_lifecycle_lock().await;
    let engine = LocalEngine::get(&provider).await?;
    let previous = engine.current_model().await;
    let result = tokio::time::timeout(TEST_TIMEOUT, async {
        if previous.as_deref() != Some(model.as_str()) {
            engine.load(&model).await?;
        }
        let actual_model = engine.current_model().await.ok_or(MODEL_UNAVAILABLE)?;
        measure(&engine, actual_model).await
    })
    .await;
    // A timed-out in-process call can still own its native model lock. Cleanup
    // retains both gates until FFI is finished, even if the UI has returned.
    let cleanup = tokio::spawn(async move {
        let (_job, _lifecycle) = (job, lifecycle);
        engine.restore(previous).await
    });
    let measured = result.map_err(|_| {
        "The PC test timed out. Wait for the speech engine to finish and try a smaller model."
            .to_string()
    })?;
    tokio::time::timeout(TEST_TIMEOUT, cleanup)
        .await
        .map_err(|_| {
            "The previous speech model is still being restored. Wait before starting another job."
                .to_string()
        })?
        .map_err(|_| "The speech engine could not finish the PC test.".to_string())??;
    measured
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct RestartingEngine {
        calls: Cell<u64>,
        repeated_restarts: bool,
    }

    impl TimedEngine for RestartingEngine {
        fn name(&self) -> &'static str {
            "Test engine"
        }
        async fn worker_generation(&self) -> u64 {
            let passes = self.calls.get() / SAMPLES.len() as u64;
            if self.repeated_restarts {
                passes
            } else {
                passes.min(1)
            }
        }

        async fn transcribe(
            &self,
            _: Vec<f32>,
            language: &str,
        ) -> Result<(bool, &'static str, Option<&'static str>), String> {
            let calls = self.calls.get();
            self.calls.set(calls + 1);
            if calls < 2 {
                return Ok((true, "GPU", None));
            }
            Ok((language == "en", "CPU", Some("Worker fell back to CPU")))
        }
    }

    #[tokio::test]
    async fn measures_only_a_stable_worker_and_preserves_empty_results() {
        let engine = RestartingEngine {
            calls: Cell::new(0),
            repeated_restarts: false,
        };
        let result = measure(&engine, "sample-model".into()).await.unwrap();
        assert_eq!(engine.calls.get(), 4);
        assert_eq!(result.engine, "Test engine");
        assert_eq!(result.model, "sample-model");
        assert_eq!(result.backend, "CPU");
        assert_eq!(result.fallback_reason, Some("Worker fell back to CPU"));
        assert_eq!(result.samples.len(), 2);
        assert!(!result.samples[0].recognized);
        assert!(result.samples[1].recognized);
        assert_eq!(
            result.audio_seconds,
            result.samples.iter().map(|s| s.audio_seconds).sum::<f64>()
        );
        assert_eq!(
            result.elapsed_seconds,
            result
                .samples
                .iter()
                .map(|s| s.elapsed_seconds)
                .sum::<f64>()
        );
        assert_eq!(
            result.realtime_factor,
            result.elapsed_seconds / result.audio_seconds
        );
    }

    #[tokio::test]
    async fn repeated_worker_restarts_do_not_create_unbounded_retries() {
        let engine = RestartingEngine {
            calls: Cell::new(0),
            repeated_restarts: true,
        };
        assert!(measure(&engine, "sample-model".into()).await.is_err());
        assert_eq!(engine.calls.get(), 4);
    }

    #[test]
    fn built_in_fixtures_have_bounded_duration_and_speech_energy() {
        assert_eq!(SAMPLES.map(|(language, _)| language), ["de", "en"]);
        for (_, bytes) in SAMPLES {
            assert_eq!(bytes.len() % 2, 0);
            let audio = pcm_samples(bytes);
            assert!((14 * SAMPLE_RATE..16 * SAMPLE_RATE).contains(&audio.len()));
            assert!(audio.iter().all(|sample| (-1.0..=1.0).contains(sample)));
            assert!(audio.iter().filter(|sample| sample.abs() > 0.01).count() > SAMPLE_RATE);
        }
    }

    #[tokio::test]
    async fn cloud_and_unknown_providers_cannot_start_a_test() {
        for provider in ["cloud-whisper", "mai-transcribe", "unknown"] {
            assert!(LocalEngine::get(provider).await.is_err());
        }
    }
}
