use super::*;
use crate::audio::diarization::{DiarizationRuntimeDll, SpeakerDiarizationRuntimeStatus};
use crate::diagnostics::{
    AppDiagnosticInfo, AudioDiagnostics, IntegrationDiagnostics, ModelEngineDiagnostics,
    TranscriptionDiagnostics,
};
use crate::whisper_engine::commands::WhisperAccelerationStatus;

fn snapshot() -> DiagnosticSnapshot {
    let private_path = "private-folder/meeting.wav";
    let private_endpoint = "https://private-support.example.invalid/meeting";
    let private_note = "private meeting transcript";
    DiagnosticSnapshot {
        app: AppDiagnosticInfo {
            version: private_note.into(),
            os: private_note.into(),
            arch: private_note.into(),
            build_profile: private_note.into(),
            app_data_dir: Some(private_path.into()),
            logs_dir: Some(private_path.into()),
        },
        system_resources: None,
        acceleration: Some(WhisperAccelerationStatus {
            compiled_backend: "Vulkan".into(),
            runtime_detected_gpu: private_note.into(),
            use_gpu: true,
            flash_attn: false,
            label: private_note.into(),
            gpu_available_but_unused: false,
            cpu_cores: 4,
            memory_gb: 8,
            performance_tier: private_note.into(),
        }),
        transcription: TranscriptionDiagnostics {
            selected_provider: private_endpoint.into(),
            selected_model: private_note.into(),
            api_key_configured: true,
            parakeet_directml_enabled: Some(false),
            engines: vec![ModelEngineDiagnostics {
                provider: "parakeet".into(),
                loaded: Some(true),
                current_model: Some(private_note.into()),
                selected_model_present: Some(false),
                downloaded_models: 1,
                total_models: 3,
                models_directory: Some(private_path.into()),
                error: Some("private credential fixture".into()),
            }],
        },
        audio: AudioDiagnostics {
            input_devices: 1,
            output_devices: 2,
            total_devices: 3,
            device_names: vec!["private named microphone".into()],
            error: Some(private_path.into()),
        },
        recording: json!({ "session_id": "private session ID", "transcript": private_note }),
        diarization: SpeakerDiarizationRuntimeStatus {
            in_progress: false,
            directml_compiled: true,
            directml_fast: false,
            directml_slow: true,
            directml_unavailable: false,
            preferred_provider: private_note.into(),
            runtime_dlls: vec![DiarizationRuntimeDll {
                name: private_path.into(),
                present: true,
                bytes: Some(100),
            }],
        },
        integrations: IntegrationDiagnostics {
            teams: None,
            codex: None,
        },
        errors: vec![
            format!("audio_devices: {private_path}"),
            format!("audio_devices: {private_note}"),
            format!("unknown-check: {private_endpoint}"),
        ],
    }
}

#[test]
fn exports_allowlisted_metadata_and_failure_categories_only() {
    let report = build_report(&snapshot());
    let serialized = report.to_string();
    assert!(!serialized.contains("private"));
    for key in [
        "recording",
        "integrations",
        "selectedModel",
        "apiKeyConfigured",
    ] {
        assert!(!serialized.contains(&format!("\"{key}\"")));
    }
    assert_eq!(report["schemaVersion"], 1);
    assert_eq!(report["app"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["audio"]["inputDevices"], 1);
    assert_eq!(report["whisperAcceleration"]["compiledBackend"], "Vulkan");
    assert!(report["whisperAcceleration"]["runtimeDetectedGpu"].is_null());
    assert!(report["transcription"]["selectedProvider"].is_null());
    assert!(report["speakerSeparation"]["preferredProvider"].is_null());
    assert_eq!(
        report["transcription"]["engines"],
        json!([{
            "provider": "parakeet",
            "loaded": true,
            "selectedModelPresent": false,
            "downloadedModels": 1,
            "totalModels": 3,
            "checkFailed": true
        }])
    );
    assert_eq!(report["collectionFailures"], json!(["audio_devices"]));
}

#[test]
fn report_size_stays_bounded_and_repeated_checks_are_deduplicated() {
    let mut snapshot = snapshot();
    snapshot.transcription.selected_provider = "parakeet".into();
    snapshot
        .transcription
        .engines
        .resize(500, snapshot.transcription.engines[0].clone());
    snapshot.errors = vec!["recording_folder: ".to_string() + &"x".repeat(8192); 40];
    let report = build_report(&snapshot);
    assert!(serde_json::to_vec_pretty(&report).unwrap().len() < MAX_REPORT_BYTES);
    assert_eq!(report["transcription"]["selectedProvider"], "parakeet");
    assert_eq!(
        report["transcription"]["engines"].as_array().unwrap().len(),
        1
    );
    assert_eq!(report["collectionFailures"], json!(["recording_folder"]));
}

#[test]
fn save_writes_only_the_chosen_json_file_and_cancel_writes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("report.json");
    let report = build_report(&snapshot());

    assert!(!write_report(None, &report).unwrap());
    assert_eq!(directory.path().read_dir().unwrap().count(), 0);

    assert!(write_report(Some(destination.clone()), &report).unwrap());
    let written: Value = serde_json::from_slice(&std::fs::read(destination).unwrap()).unwrap();
    assert_eq!(written, report);
    assert_eq!(directory.path().read_dir().unwrap().count(), 1);
}

#[test]
fn failed_save_leaves_existing_files_intact_and_returns_generic_errors() {
    let directory = tempfile::tempdir().unwrap();
    let wrong_extension = directory.path().join("private existing name.txt");
    std::fs::write(&wrong_extension, "existing file").unwrap();
    let report = build_report(&snapshot());

    assert!(write_report(Some(wrong_extension.clone()), &report).is_err());
    assert_eq!(
        std::fs::read_to_string(wrong_extension).unwrap(),
        "existing file"
    );

    let destination = directory.path().join("private directory.json");
    std::fs::create_dir(&destination).unwrap();
    let error = write_report(Some(destination.clone()), &report).unwrap_err();
    assert!(!error.contains("private"));
    assert!(destination.is_dir());
    assert_eq!(directory.path().read_dir().unwrap().count(), 2);
}
