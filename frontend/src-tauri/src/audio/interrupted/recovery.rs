use super::{
    audio_evidence, folder_key, is_recording_marker, meeting_files, pending_audio_ids,
    recovered_provenance, registered_map, registered_rows, repair_saved_recording,
    snapshot_summary, verify_pending_audio_inputs, RecoveryCacheEntry, REGISTERED_BUDGET,
};
use crate::audio::{
    incremental_saver::{
        recover_interrupted_audio, release_registered_capture, AudioRecoveryStatus,
    },
    outcome::RecordingOutcome,
    recording_commands, recording_mode, recording_preferences,
};
use crate::database::repositories::transcript::TranscriptsRepository;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Runtime};

// Keep discard and recovery from mutating the same originals concurrently.
pub(super) static MUTATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const NO_CONTENT: &str = "No audio could be recovered yet. Keep the recording folder and retry recovery; a transcript is not required.";
const AUDIO_REQUIRED: &str = "No audio could be recovered yet. Reconnect the recordings folder if needed, then retry. Your draft and original files were kept.";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CachedTranscript {
    id: String,
    #[serde(default)]
    started_at: i64,
    #[serde(default)]
    last_updated: i64,
    #[serde(default)]
    had_audio: bool,
    transcripts: Vec<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveredMeeting {
    meeting_id: String,
    transcript_count: usize,
    audio_recovery_status: AudioRecoveryStatus,
}

/// Open one recovery operation. The task retains ownership through the
/// database commit and conditional cleanup if its webview goes away.
#[tauri::command]
pub(crate) async fn recover_interrupted_recording<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, crate::state::AppState>,
    folder: Option<String>,
    meeting_title: String,
    cached: Option<CachedTranscript>,
) -> Result<RecoveredMeeting, String> {
    let pool = state.db_manager.pool().clone();
    tauri::async_runtime::spawn(async move {
        let _install = crate::updates::admit_work()?;
        let _recovery = MUTATION.lock().await;
        let Some(folder) = folder else {
            return recover_detached(&pool, &meeting_title, cached.ok_or(NO_CONTENT)?).await;
        };
        let roots = recording_preferences::recording_roots(&app).await;
        let rows = registered_rows(&pool).await?;
        let pending = pending_audio_ids(&pool).await?;
        let active_folder = recording_commands::active_recording_folder();
        let source = folder.clone();
        let (folder, marker, existing) = tokio::task::spawn_blocking(move || {
            let authorised =
                meeting_files::authorize_new_meeting_folder(&source, &roots).or_else(|_| {
                    meeting_files::resolve_recording_folder(Path::new(&source), &roots).map(
                        |path| {
                            meeting_files::plain_path(&path)
                                .to_string_lossy()
                                .into_owned()
                        },
                    )
                })?;
            let path = Path::new(&authorised);
            if active_folder.protects(path) {
                return Err("A recording is running. Try again when it has stopped.".into());
            }
            let registered = registered_map(rows.clone(), &roots, REGISTERED_BUDGET);
            let marker = meeting_files::read_marker(path);
            let key = folder_key(path);
            let owner = registered.get(&key).or_else(|| {
                rows.iter()
                    .find(|(_, path)| folder_key(Path::new(path)) == key)
                    .map(|(id, _)| id)
            });
            let pending_repair = marker
                .as_ref()
                .filter(|marker| is_recording_marker(marker))
                .and_then(meeting_files::recording_id)
                .is_some_and(|id| pending.contains(id));
            Ok::<_, String>((authorised, marker, owner.is_some() || pending_repair))
        })
        .await
        .map_err(|_| "Could not verify the recording folder")??;

        if let (Some(cache), Some(marker)) = (&cached, &marker) {
            let entry = RecoveryCacheEntry {
                id: cache.id.clone(),
                folder: None,
                started_at: cache.started_at,
                last_updated: cache.last_updated,
            };
            if !entry.matches_marker(marker) {
                return Err(
                    "The recording folder changed. Check for interrupted recordings again.".into(),
                );
            }
        }
        if existing {
            let repaired = repair_saved_recording(&app, &pool, folder).await?;
            if repaired.wake_summary {
                recording_commands::wake_saved_recording_summary(&app, repaired.meeting_id.clone());
            }
            return Ok(RecoveredMeeting {
                meeting_id: repaired.meeting_id,
                transcript_count: 0,
                audio_recovery_status: AudioRecoveryStatus {
                    status: repaired.audio_status,
                    audio_file_path: repaired.audio_path,
                    ..audio_status("none", "")
                },
            });
        }
        let marker = marker
            .filter(|marker| {
                !meeting_files::has_invalid_marker_id(marker) && is_recording_marker(marker)
            })
            .ok_or("The recording folder is no longer available.")?;
        let (recovered, transcribes) =
            recover_new(&pool, folder, marker, meeting_title, cached).await?;
        if transcribes {
            recording_commands::wake_saved_recording_summary(&app, recovered.meeting_id.clone());
        }
        Ok(recovered)
    })
    .await
    .map_err(|_| {
        "Recording recovery could not finish. Keep the original files and retry.".to_owned()
    })?
}

// Older browser entries need a durable identity of their own when they cannot
// attach their original folder. Include the start time for reused legacy IDs.
pub(super) fn detached_meeting_id(id: &str, started_at: i64) -> String {
    let hash = Sha256::new()
        .chain_update(b"ClawScribe detached transcript recovery")
        .chain_update(started_at.to_be_bytes())
        .chain_update(id.as_bytes())
        .finalize();
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash[..16]);
    format!(
        "meeting-{}",
        uuid::Builder::from_custom_bytes(bytes).into_uuid()
    )
}

async fn recover_detached(
    pool: &SqlitePool,
    title: &str,
    cached: CachedTranscript,
) -> Result<RecoveredMeeting, String> {
    let (_, active_id) = recording_commands::active_recording_recovery();
    if active_id.as_deref() == Some(cached.id.as_str()) {
        return Err("A recording is running. Try again when it has stopped.".into());
    }
    let superseded: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?
         UNION ALL SELECT 1 FROM recording_library_deletions WHERE meeting_id = ?)",
    )
    .bind(&cached.id)
    .bind(&cached.id)
    .fetch_one(pool)
    .await
    .map_err(|_| "Could not check the library")?;
    if superseded {
        return Err(
            "This recording was already saved or deleted. Check for interrupted recordings again."
                .into(),
        );
    }
    let transcripts = crate::api::parse_recording_transcripts(cached.transcripts)?;
    let audio = audio_status("none", "No folder path available");
    let outcome = recovery_outcome(&audio, transcripts.len(), false, false)?;
    let meeting_id = TranscriptsRepository::save_detached_recovery(
        pool,
        detached_meeting_id(&cached.id, cached.started_at),
        title,
        &transcripts,
        &outcome,
    )
    .await
    .map_err(|_| "Could not save the recovered transcript")?;
    Ok(RecoveredMeeting {
        meeting_id,
        transcript_count: transcripts.len(),
        audio_recovery_status: audio,
    })
}

async fn recover_new(
    pool: &SqlitePool,
    folder: String,
    marker: Value,
    title: String,
    cached: Option<CachedTranscript>,
) -> Result<(RecoveredMeeting, bool), String> {
    let recording_id = meeting_files::recording_id(&marker)
        .or_else(|| meeting_files::stamped_meeting_id(&marker))
        .map(str::to_owned);
    if let Some(id) = recording_id.as_deref() {
        let unavailable: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?
             UNION ALL SELECT 1 FROM recording_library_deletions WHERE meeting_id = ?)",
        )
        .bind(id)
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(|_| "Could not check the library")?;
        if unavailable {
            return Err("This recording belongs to another library entry or was deleted.".into());
        }
    }
    let live = recording_mode::marker_transcribes(&marker);
    let handoff_complete = marker.get("status").and_then(Value::as_str) == Some("completed")
        && meeting_files::recording_id(&marker).is_some();
    let from_cache = cached.is_some();
    let had_audio = cached.as_ref().is_some_and(|cache| cache.had_audio);
    let folder_path = PathBuf::from(&folder);
    let inspect = folder_path.clone();
    let (transcripts, transcript_count, disk_has_audio, audio_inputs_safe, mut outcome) =
        tokio::task::spawn_blocking(move || {
            let audio_inputs_safe = verify_pending_audio_inputs(&inspect).is_ok();
            let (segments, count, fallback) = if let Some(cache) = cached {
                let count = cache.transcripts.len();
                (
                    crate::api::parse_recording_transcripts(cache.transcripts)?,
                    count,
                    false,
                )
            } else {
                let (count, _) = snapshot_summary(&inspect);
                if count == 0 {
                    (Vec::new(), 0, false)
                } else {
                    let (rows, fallback) = crate::api::select_recording_transcripts(
                        crate::api::read_recording_snapshot(Some(&inspect)),
                        Vec::new(),
                    )?;
                    (rows, count as usize, fallback)
                }
            };
            let (final_audio, (spool_chunks, _), (checkpoints, _)) = audio_evidence(&inspect);
            let disk_has_audio = final_audio || spool_chunks > 0 || checkpoints > 0;
            let mut outcome = match RecordingOutcome::read(&inspect) {
                Ok(saved) => saved.unwrap_or_default(),
                Err(_) => RecordingOutcome {
                    recording_files_incomplete: true,
                    ..Default::default()
                },
            };
            outcome.transcription_incomplete |= fallback;
            Ok::<_, String>((segments, count, disk_has_audio, audio_inputs_safe, outcome))
        })
        .await
        .map_err(|_| "Could not read the interrupted recording")??;

    let audio = if !audio_inputs_safe {
        audio_status(
            "failed",
            "Could not verify the audio files. Keep the recording folder and retry.",
        )
    } else if from_cache || disk_has_audio {
        recover_interrupted_audio(folder.clone())
            .await
            .unwrap_or_else(|error| audio_status("failed", &error))
    } else {
        audio_status("none", "No audio on disk")
    };
    let needs_audio = from_cache && (had_audio || disk_has_audio || audio.status == "failed");
    let recovered_outcome = recovery_outcome(
        &audio,
        transcript_count,
        needs_audio,
        !from_cache && handoff_complete,
    )?;
    outcome.audio_save_failed |= recovered_outcome.audio_save_failed;
    outcome.transcription_incomplete |= recovered_outcome.transcription_incomplete;
    outcome.capture_incomplete |= recovered_outcome.capture_incomplete;
    let provenance =
        tokio::task::spawn_blocking(move || recovered_provenance(&folder_path, &marker, None))
            .await
            .map_err(|_| "Could not read recovered recording details")?;

    let calendar_edits = crate::library::recording_calendar::EDIT.lock().await;
    let meeting_id = match recording_id {
        Some(id) => {
            TranscriptsRepository::save_recording_handoff(
                pool,
                &id,
                &title,
                &transcripts,
                folder.clone(),
                &outcome,
                provenance,
            )
            .await
        }
        None => {
            TranscriptsRepository::save_recording(
                pool,
                &title,
                &transcripts,
                Some(folder.clone()),
                Some(&outcome),
                None,
                provenance,
            )
            .await
        }
    }
    .map_err(|_| "Could not save the recovered recording. The original files were kept.")?;
    drop(calendar_edits);
    finish_committed_recovery(pool, &folder, &meeting_id, &audio).await;
    Ok((
        RecoveredMeeting {
            meeting_id,
            transcript_count,
            audio_recovery_status: audio,
        },
        live,
    ))
}

async fn finish_committed_recovery(
    pool: &SqlitePool,
    folder: &str,
    meeting_id: &str,
    audio: &AudioRecoveryStatus,
) {
    let owns_folder: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM active_meetings WHERE id = ? AND folder_path = ?)",
    )
    .bind(meeting_id)
    .bind(folder)
    .fetch_one(pool)
    .await
    .unwrap_or(false);
    if !owns_folder {
        return;
    }
    let path = PathBuf::from(folder);
    let id = meeting_id.to_owned();
    if !matches!(
        tokio::task::spawn_blocking(move || meeting_files::stamp_meeting_id(&path, &id)).await,
        Ok(Ok(()))
    ) {
        log::warn!("Could not stamp the recovered meeting folder");
        return;
    }
    if has_recovered_audio(audio) {
        if release_registered_capture(pool, folder).await.is_err() {
            log::warn!("Could not release recovered capture files");
        }
        if audio.status == "success" {
            super::remove_legacy_checkpoints(folder, audio.audio_file_path.as_deref()).await;
        }
    }
}

fn has_recovered_audio(audio: &AudioRecoveryStatus) -> bool {
    audio.audio_file_path.is_some() && matches!(audio.status.as_str(), "success" | "partial")
}

fn recovery_outcome(
    audio: &AudioRecoveryStatus,
    transcript_count: usize,
    needs_audio: bool,
    handoff_complete: bool,
) -> Result<RecordingOutcome, String> {
    if !has_recovered_audio(audio) {
        if needs_audio {
            return Err(AUDIO_REQUIRED.into());
        }
        if transcript_count == 0 {
            return Err(NO_CONTENT.into());
        }
    }
    Ok(RecordingOutcome {
        audio_save_failed: audio.status == "failed",
        transcription_incomplete: transcript_count > 0 && !handoff_complete,
        capture_incomplete: audio.status == "partial",
        ..Default::default()
    })
}

fn audio_status(status: &str, message: &str) -> AudioRecoveryStatus {
    AudioRecoveryStatus {
        status: status.into(),
        chunk_count: 0,
        estimated_duration_seconds: 0.0,
        audio_file_path: None,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::incremental_saver::recover_audio_in_folder;
    use crate::audio::inference;
    use crate::audio::recording_state::{AudioChunk, DeviceType};
    use serde_json::json;
    use tauri::Manager;

    #[test]
    fn outcome_preserves_audio_only_partial_and_complete_handoff_cases() {
        for status in ["success", "partial"] {
            let audio = AudioRecoveryStatus {
                audio_file_path: Some("synthetic-audio.wav".into()),
                ..audio_status(status, "")
            };
            let audio_only = recovery_outcome(&audio, 0, true, false).unwrap();
            assert_eq!(audio_only.capture_incomplete, status == "partial");
            assert!(!audio_only.transcription_incomplete);
            assert!(!audio_only.audio_save_failed);
            assert!(
                recovery_outcome(&audio, 1, true, false)
                    .unwrap()
                    .transcription_incomplete
            );
            assert!(
                !recovery_outcome(&audio, 1, true, true)
                    .unwrap()
                    .transcription_incomplete
            );
        }
        for status in ["none", "failed"] {
            let missing = audio_status(status, "");
            assert_eq!(
                recovery_outcome(&missing, 0, false, false).unwrap_err(),
                NO_CONTENT
            );
            assert_eq!(
                recovery_outcome(&missing, 1, true, false).unwrap_err(),
                AUDIO_REQUIRED
            );
            let transcript_only = recovery_outcome(&missing, 1, false, false).unwrap();
            assert_eq!(transcript_only.audio_save_failed, status == "failed");
            assert!(transcript_only.transcription_incomplete);
        }
    }

    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    fn cache(started_at: i64) -> CachedTranscript {
        CachedTranscript {
            id: "legacy-cache".into(),
            started_at,
            last_updated: started_at + 1_000,
            had_audio: false,
            transcripts: vec![
                json!({"id":"synthetic-row", "text":"Synthetic words", "timestamp":"00:00"}),
            ],
        }
    }

    async fn test_app(root: &Path) -> tauri::App<tauri::test::MockRuntime> {
        let db_manager = crate::database::manager::DatabaseManager::new_isolated(
            root.join("library.sqlite").to_str().unwrap(),
            root.join("absent.sqlite").to_str().unwrap(),
        )
        .await
        .unwrap();
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().identifier =
            format!("net.example.clawscribe.test.{}", uuid::Uuid::new_v4());
        tauri::test::mock_builder()
            .plugin(tauri_plugin_store::Builder::default().build())
            .manage(crate::state::AppState { db_manager })
            .build(context)
            .unwrap()
    }

    fn draft_folder(root: &Path, mode: &str) -> (String, PathBuf) {
        let id = format!("meeting-{}", uuid::Uuid::new_v4());
        let folder = root.join(&id);
        std::fs::create_dir(&folder).unwrap();
        meeting_files::register_core_folder(&folder);
        let marker = json!({
            "recording_id": id, "source": "recording", "status": "completed",
            "created_at": "2026-01-01T12:00:00Z", "recording_mode": mode,
            "version": "1.0", "transcript_file": "transcripts.json",
        });
        std::fs::write(folder.join("metadata.json"), marker.to_string()).unwrap();
        std::fs::write(
            folder.join("transcripts.json"),
            json!({"segments": [{"id": "native-row", "text": "Synthetic words", "display_time": "00:01"}]}).to_string(),
        ).unwrap();
        (id, folder)
    }

    async fn recording_fixture(root: &Path, mode: &str, partial: bool) -> (String, PathBuf) {
        let (id, folder) = draft_folder(root, mode);
        let (sender, _, _) =
            crate::audio::transcription::queue::recording_audio_queue(&folder).unwrap();
        for index in 0..3 {
            sender
                .send(AudioChunk {
                    data: vec![0.05; 4800],
                    sample_rate: 48000,
                    timestamp: index as f64 / 10.0,
                    chunk_id: index,
                    device_type: DeviceType::System,
                })
                .await
                .unwrap();
        }
        drop(sender);
        if partial {
            std::fs::write(
                folder.join(".audio-spool/00000000000000000001.chunk"),
                b"torn capture",
            )
            .unwrap();
        }
        let checkpoints = folder.join(".checkpoints");
        std::fs::create_dir(&checkpoints).unwrap();
        crate::audio::encode::encode_single_audio(
            bytemuck::cast_slice(&vec![0.05f32; 4800]),
            48000,
            1,
            &checkpoints.join("audio_chunk_0.mp4"),
        )
        .unwrap();
        (id, folder)
    }

    async fn run_recovery(
        app: &tauri::App<tauri::test::MockRuntime>,
        folder: &Path,
        cached: Option<CachedTranscript>,
    ) -> Result<RecoveredMeeting, String> {
        recover_interrupted_recording(
            app.handle().clone(),
            app.state(),
            Some(folder.to_string_lossy().into_owned()),
            "Recovered".into(),
            cached,
        )
        .await
    }

    #[tokio::test]
    async fn new_recovery_succeeds_while_an_inference_job_owns_the_slot() {
        let _serial = inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let root = tempfile::tempdir().unwrap();
        let app = test_app(root.path()).await;
        let (id, folder) = recording_fixture(root.path(), "audio_only", false).await;
        let _job = inference::claim_job().unwrap();
        let saved = run_recovery(&app, &folder, None).await.unwrap();
        assert_eq!(saved.meeting_id, id);
        assert_eq!(saved.audio_recovery_status.status, "success");
        assert!(!inference::job_slot_idle());
    }

    #[tokio::test]
    async fn another_recovery_succeeds_when_local_notes_take_the_slot_after_the_first_save() {
        let _serial = inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let root = tempfile::tempdir().unwrap();
        let app = test_app(root.path()).await;
        let (first_id, first) = recording_fixture(root.path(), "live", false).await;
        let (second_id, second) = recording_fixture(root.path(), "audio_only", false).await;
        assert_eq!(
            run_recovery(&app, &first, None).await.unwrap().meeting_id,
            first_id
        );
        // Take the same permit as the local automatic notes worker at its start,
        // without depending on model files or inference timing.
        let _notes = inference::claim_local_summary(None).await.unwrap();
        assert_eq!(
            run_recovery(&app, &second, None).await.unwrap().meeting_id,
            second_id
        );
        assert!(!inference::job_slot_idle());
    }

    #[tokio::test]
    async fn full_recovery_releases_originals_only_after_the_database_commits() {
        let _serial = inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let root = tempfile::tempdir().unwrap();
        let app = test_app(root.path()).await;
        let pool = app
            .state::<crate::state::AppState>()
            .db_manager
            .pool()
            .clone();
        let (id, folder) = recording_fixture(root.path(), "audio_only", false).await;
        sqlx::query("CREATE TRIGGER fail_recovery BEFORE INSERT ON meetings BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END")
            .execute(&pool).await.unwrap();
        assert!(run_recovery(&app, &folder, None).await.is_err());
        assert!(folder.join("audio-recovered.mp4").is_file());
        assert!(folder.join(".audio-spool").is_dir());
        assert!(folder.join(".checkpoints").is_dir());
        sqlx::query("DROP TRIGGER fail_recovery")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            run_recovery(&app, &folder, None).await.unwrap().meeting_id,
            id
        );
        assert!(!folder.join(".audio-spool").exists());
        assert!(!folder.join(".checkpoints").exists());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn partial_recovery_keeps_checkpoints_and_unreadable_originals() {
        let _serial = inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let root = tempfile::tempdir().unwrap();
        let app = test_app(root.path()).await;
        let (_, folder) = recording_fixture(root.path(), "audio_only", true).await;
        let saved = run_recovery(&app, &folder, None).await.unwrap();
        assert_eq!(saved.audio_recovery_status.status, "partial");
        assert!(folder
            .join(".audio-spool/00000000000000000001.chunk")
            .is_file());
        assert!(folder.join(".checkpoints/audio_chunk_0.mp4").is_file());
    }

    #[tokio::test]
    async fn mismatched_cached_marker_rejects_the_command_before_recovery() {
        let _serial = inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let root = tempfile::tempdir().unwrap();
        let app = test_app(root.path()).await;
        let (_, folder) = recording_fixture(root.path(), "audio_only", false).await;
        let error = run_recovery(&app, &folder, Some(cache(1)))
            .await
            .err()
            .unwrap();
        assert!(error.contains("folder changed"));
        assert!(!folder.join("audio-recovered.mp4").exists());
        assert!(folder.join(".audio-spool").is_dir());
        let pool = app
            .state::<crate::state::AppState>()
            .db_manager
            .pool()
            .clone();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn cached_draft_with_missing_expected_audio_stays_unsaved() {
        let _serial = inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let root = tempfile::tempdir().unwrap();
        let app = test_app(root.path()).await;
        let (id, folder) = draft_folder(root.path(), "live");
        let draft = CachedTranscript {
            id,
            had_audio: true,
            ..cache(1)
        };
        let error = run_recovery(&app, &folder, Some(draft))
            .await
            .err()
            .unwrap();
        assert_eq!(error, AUDIO_REQUIRED);
        let pool = app
            .state::<crate::state::AppState>()
            .db_manager
            .pool()
            .clone();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn retry_after_commit_defers_busy_repair_and_reuses_the_saved_meeting() {
        let _serial = inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let root = tempfile::tempdir().unwrap();
        let app = test_app(root.path()).await;
        let (id, folder) = recording_fixture(root.path(), "audio_only", false).await;
        let pool = app
            .state::<crate::state::AppState>()
            .db_manager
            .pool()
            .clone();
        let path = folder.to_string_lossy().into_owned();
        let audio = recover_audio_in_folder(path.clone()).await.unwrap();
        assert_eq!(audio.status, "success");
        // Simulate the process stopping after the database commit, before the
        // marker stamp, original release, or command reply.
        let rows = crate::api::read_recording_snapshot(Some(&folder)).unwrap();
        TranscriptsRepository::save_recording_handoff(
            &pool,
            &id,
            "Recovered",
            &rows,
            path,
            &RecordingOutcome::default(),
            Default::default(),
        )
        .await
        .unwrap();
        assert!(folder.join(".audio-spool").is_dir());
        let job = inference::claim_job().unwrap();
        let deferred = run_recovery(&app, &folder, None).await.unwrap();
        assert_eq!(deferred.meeting_id, id);
        assert_eq!(deferred.audio_recovery_status.status, "deferred");
        assert!(folder.join(".audio-spool").is_dir());
        drop(job);
        let saved = run_recovery(&app, &folder, None).await.unwrap();
        assert_eq!(saved.meeting_id, id);
        assert!(!folder.join(".audio-spool").exists());
        assert!(!folder.join(".checkpoints").exists());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn detached_cache_retries_return_the_same_meeting_and_resolve_the_scan() {
        let pool = pool().await;
        let saved = recover_detached(&pool, "Recovered", cache(1))
            .await
            .unwrap();
        let retry = recover_detached(&pool, "Recovered", cache(1))
            .await
            .unwrap();
        let other_generation = recover_detached(&pool, "Recovered later", cache(2))
            .await
            .unwrap();
        assert_eq!(saved.meeting_id, retry.meeting_id);
        assert_ne!(saved.meeting_id, other_generation.meeting_id);
        let ids = super::super::recovery_owners(&pool).await.unwrap();
        let reconciled = super::super::reconcile_cache_entries(
            vec![RecoveryCacheEntry {
                id: "legacy-cache".into(),
                folder: None,
                started_at: 1,
                last_updated: 1_001,
            }],
            &ids,
            &Default::default(),
            &[],
            &super::super::ActiveFolder::None,
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        assert_eq!(reconciled.resolved, ["legacy-cache"]);
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcripts WHERE meeting_id = ?")
            .bind(&saved.meeting_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn failed_handoff_keeps_originals_and_retry_commits_the_native_snapshot() {
        let pool = pool().await;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path();
        let id = format!("meeting-{}", uuid::Uuid::new_v4());
        let marker = json!({
            "recording_id": id, "source": "recording", "status": "completed",
            "created_at": "2026-01-01T12:00:00Z", "recording_mode": "live",
            "version": "1.0", "transcript_file": "transcripts.json",
        });
        std::fs::write(path.join("metadata.json"), marker.to_string()).unwrap();
        std::fs::write(path.join("transcripts.json"), json!({
            "segments": [{"id":"native-row", "text":"Synthetic native words", "display_time":"00:01"}],
        }).to_string()).unwrap();
        let checkpoints = path.join(".checkpoints");
        std::fs::create_dir(&checkpoints).unwrap();
        // A nonempty saved audio file lets the legacy resolver skip encoding.
        // Its invalid contents must prevent the subsequent cleanup.
        std::fs::write(path.join("audio.mp4"), b"invalid audio").unwrap();
        std::fs::write(
            checkpoints.join("audio_chunk_0.mp4"),
            b"original checkpoint",
        )
        .unwrap();
        sqlx::query("CREATE TRIGGER fail_recovery BEFORE INSERT ON meetings BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END")
            .execute(&pool).await.unwrap();
        let folder = path.to_string_lossy().into_owned();
        let result = recover_new(
            &pool,
            folder.clone(),
            marker.clone(),
            "Recovered".into(),
            None,
        )
        .await;
        assert!(result.is_err());
        assert!(checkpoints.join("audio_chunk_0.mp4").is_file());
        assert!(
            meeting_files::stamped_meeting_id(&meeting_files::read_marker(path).unwrap()).is_none()
        );

        sqlx::query("DROP TRIGGER fail_recovery")
            .execute(&pool)
            .await
            .unwrap();
        let (saved, live) = recover_new(&pool, folder, marker, "Recovered".into(), None)
            .await
            .unwrap();
        assert!(live);
        assert_eq!(saved.meeting_id, id);
        assert_eq!(saved.transcript_count, 1);
        assert_eq!(
            meeting_files::stamped_meeting_id(&meeting_files::read_marker(path).unwrap()),
            Some(id.as_str())
        );
        let incomplete: bool = sqlx::query_scalar(
            "SELECT transcription_incomplete FROM recording_outcomes WHERE meeting_id = ?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!incomplete);
        assert!(checkpoints.join("audio_chunk_0.mp4").is_file());
    }
}
