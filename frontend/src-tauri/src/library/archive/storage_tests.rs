use super::*;

#[test]
fn storage_breakdown_distinguishes_missing_and_inaccessible_recording_folders() {
    let row = |folder_path| StorageFolder {
        cursor: 1,
        title: "Saved notes".into(),
        folder_path,
    };
    let missing = measure_meeting_folder(&[], row(None));
    assert!(missing.no_folder);
    assert!(!missing.unavailable);
    let inaccessible = measure_meeting_folder(&[], row(Some("outside-roots".into())));
    assert!(!inaccessible.no_folder);
    assert!(inaccessible.unavailable);
}

#[cfg(windows)]
#[test]
fn storage_breakdown_does_not_require_pinning_ancestors_against_deletion() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("meeting");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("audio.wav"), b"audio").unwrap();
    let _can_rename_root = fs::OpenOptions::new()
        .access_mode(DELETE.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(root.path())
        .unwrap();
    let row = StorageFolder {
        cursor: 1,
        title: "Saved notes".into(),
        folder_path: Some(folder.to_str().unwrap().into()),
    };
    let measured = measure_meeting_folder(&[root.path().into()], row);
    assert!(!measured.unavailable);
    assert_eq!(measured.audio_bytes, 5);
}

#[tokio::test]
async fn per_meeting_storage_checks_roots_and_returns_bounded_pages() {
    let fixture = Fixture::new().await;
    fs::create_dir(fixture.folder.join(".checkpoints")).unwrap();
    fs::write(fixture.folder.join(".checkpoints/part"), "recovery").unwrap();
    let outside = fixture
        .folder
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("audio.mp4"), "outside").unwrap();
    add_meeting(&fixture.pool, "outside", &outside).await;
    for index in 0..STORAGE_PAGE_SIZE {
        add_meeting(&fixture.pool, &format!("new-{index}"), &fixture.folder).await;
    }
    let (first, next) = read_storage_folders(&fixture.pool, None).await.unwrap();
    assert_eq!(first.len(), STORAGE_PAGE_SIZE);
    let (last, next) = read_storage_folders(&fixture.pool, Some(next.unwrap().parse().unwrap()))
        .await
        .unwrap();
    assert_eq!(last.len(), 2);
    assert!(next.is_none());
    let mut last = last
        .into_iter()
        .map(|row| measure_meeting_folder(&fixture.roots, row));
    let inaccessible = last.next().unwrap();
    assert!(inaccessible.unavailable);
    assert_eq!(inaccessible.audio_bytes, 0);
    let measured = last.next().unwrap();
    assert_eq!(measured.audio_bytes, 11);
    assert_eq!(measured.recovery_bytes, 8);
    assert!(measured.other_bytes >= "Synthetic snapshotbackup copy".len() as u64);
    assert!(!measured.partial);
}

#[test]
fn storage_tree_stops_when_the_entry_budget_is_exhausted() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("first"), [0u8; 3]).unwrap();
    fs::write(temp.path().join("second"), [0u8; 3]).unwrap();
    let mut partial = false;
    assert_eq!(
        bounded_storage_bytes(temp.path(), 0, &mut 1, &mut partial),
        3
    );
    assert!(partial);
}

struct Fixture {
    _temp: tempfile::TempDir,
    roots: Vec<PathBuf>,
    folder: PathBuf,
    pool: SqlitePool,
}

impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::database::meeting_files::plain_path(&temp.path().canonicalize().unwrap())
            .join("recordings");
        let folder = root.join("one");
        fs::create_dir_all(&folder).unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        add_meeting(&pool, "first", &folder).await;
        sqlx::query("INSERT INTO meeting_notes (meeting_id, notes_markdown, created_at, updated_at) VALUES ('first', 'Synthetic notes', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp) VALUES ('t', 'first', 'Synthetic transcript', '00:00:01')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO summary_processes (meeting_id, status, result, created_at, updated_at) VALUES ('first', 'failed', 'Synthetic saved summary', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
            .execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO knowledge_sources (id, kind) VALUES ('document:reference', 'document')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO knowledge_documents (id, source_id, display_name, format, file_size, sha256, storage_name) VALUES ('reference', 'document:reference', 'Synthetic reference', 'txt', 0, ?, 'reference.txt')")
            .bind("1".repeat(64)).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO knowledge_document_attachments (meeting_id, document_id) VALUES ('first', 'reference')")
            .execute(&pool).await.unwrap();
        fs::write(folder.join("transcripts.json"), "Synthetic snapshot").unwrap();
        fs::write(folder.join("audio.mp4"), "saved audio").unwrap();
        fs::write(folder.join("audio.mp4.bak"), "backup copy").unwrap();
        marker(&folder, "first", "audio.mp4", false);
        Self {
            _temp: temp,
            roots: vec![root],
            folder,
            pool,
        }
    }

    async fn checks(&self) -> Vec<AudioRemovalCheck> {
        let rows = folders(&self.pool).await.unwrap();
        inspect_selection(&self.roots, &rows, &["first".into()])
    }

    async fn request(&self) -> AudioRemovalRequest {
        let check = self.checks().await.pop().unwrap();
        AudioRemovalRequest {
            id: check.id,
            revision: check.revision.unwrap(),
        }
    }
}

async fn add_meeting(pool: &SqlitePool, id: &str, folder: &Path) {
    sqlx::query("INSERT INTO meetings (id, title, folder_path, created_at, updated_at) VALUES (?, 'Synthetic meeting', ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
        .bind(id).bind(folder.to_string_lossy().as_ref()).execute(pool).await.unwrap();
}

fn marker(folder: &Path, id: &str, audio: &str, retranscribed: bool) {
    let mut value = serde_json::json!({
        "version": "1.0", "meeting_id": id, "audio_file": audio,
        "transcript_file": "transcripts.json", "status": "completed", "source": "import"
    });
    if retranscribed {
        value["retranscribed_at"] = serde_json::json!("2026-11-20");
    }
    fs::write(folder.join("metadata.json"), value.to_string()).unwrap();
}

#[tokio::test]
async fn removes_only_the_reviewed_app_copy_and_keeps_database_and_folder() {
    let _guard = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let request = fixture.request().await;
    let results = remove_selected(fixture.pool.clone(), fixture.roots.clone(), vec![request])
        .await
        .unwrap();
    assert_eq!(results[0].removed_bytes, Some(11));
    assert!(!fixture.folder.join("audio.mp4").exists());
    for name in ["transcripts.json", "metadata.json", "audio.mp4.bak"] {
        assert!(fixture.folder.join(name).exists());
    }
    let rows = super::super::archive_rows(&fixture.pool).await.unwrap();
    assert!(rows[0].has_transcript);
    assert!(rows[0].has_summary);
    assert_eq!(rows[0].summary_status.as_deref(), Some("failed"));
    let attachments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_document_attachments WHERE meeting_id = 'first'",
    )
    .fetch_one(&fixture.pool)
    .await
    .unwrap();
    assert_eq!(attachments, 1);
    let notes: String =
        sqlx::query_scalar("SELECT notes_markdown FROM meeting_notes WHERE meeting_id = 'first'")
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    assert_eq!(notes, "Synthetic notes");
    assert!(fixture.checks().await[0].revision.is_none());
}

#[tokio::test]
async fn removes_active_audio_with_an_unrelated_recording_in_trash() {
    use crate::database::repositories::meeting::MeetingsRepository;
    let _guard = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let other_folder = fixture.roots[0].join("other");
    fs::create_dir(&other_folder).unwrap();
    let retained = other_folder.join("audio.mp4");
    fs::write(&retained, "retained audio").unwrap();
    add_meeting(&fixture.pool, "trashed", &other_folder).await;
    assert!(
        MeetingsRepository::move_to_trash(&fixture.pool, "trashed", true)
            .await
            .unwrap()
    );

    let request = fixture.request().await;
    let results = remove_selected(fixture.pool.clone(), fixture.roots.clone(), vec![request])
        .await
        .unwrap();
    assert_eq!(
        results[0].removed_bytes,
        Some(11),
        "{:?}",
        results[0].reason
    );
    assert!(retained.exists());
}

#[tokio::test]
async fn protects_a_folder_also_claimed_by_a_trashed_meeting() {
    use crate::database::repositories::meeting::MeetingsRepository;
    let fixture = Fixture::new().await;
    add_meeting(&fixture.pool, "trashed", &fixture.folder).await;
    assert!(
        MeetingsRepository::move_to_trash(&fixture.pool, "trashed", true)
            .await
            .unwrap()
    );

    let checks = fixture.checks().await;
    assert_eq!(
        checks[0].reason,
        Some("Another meeting uses this recording folder.")
    );
    assert!(checks[0].revision.is_none());
}

#[tokio::test]
async fn refuses_audio_removal_if_the_reviewed_meeting_enters_trash() {
    use crate::database::repositories::meeting::MeetingsRepository;
    let _guard = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let request = fixture.request().await;
    let rows = folders(&fixture.pool).await.unwrap();
    let canonical = canonical_folders(&fixture.roots, &rows);
    let candidate = inspect_candidate(&fixture.roots, &rows, &canonical, "first", true).unwrap();
    assert!(
        MeetingsRepository::move_to_trash(&fixture.pool, "first", true)
            .await
            .unwrap()
    );
    let job = Arc::new(crate::audio::inference::claim_job().unwrap());
    let result = remove_prepared(
        &fixture.pool,
        &fixture.roots,
        &request,
        &rows,
        candidate,
        job,
    )
    .await;
    assert_eq!(result, Err(FOLDER_CHANGED));
    assert!(fixture.folder.join("audio.mp4").exists());
}

#[tokio::test]
async fn refuses_unknown_provenance_outside_paths_and_shared_folders() {
    let fixture = Fixture::new().await;
    for (id, name, retranscribed) in [
        ("first", "../audio.mp4", false),
        ("first", "audio.mp4", true),
        ("different", "audio.mp4", false),
    ] {
        marker(&fixture.folder, id, name, retranscribed);
        assert!(fixture.checks().await[0].revision.is_none());
    }
    marker(&fixture.folder, "first", "audio.mp4", false);
    assert!(inspect_selection(
        &[],
        &folders(&fixture.pool).await.unwrap(),
        &["first".into()]
    )[0]
    .revision
    .is_none());
    add_meeting(&fixture.pool, "second", &fixture.folder).await;
    assert!(fixture.checks().await[0].revision.is_none());
    assert!(fixture.folder.join("audio.mp4").exists());
}

#[tokio::test]
async fn rejects_changed_files_and_concurrent_audio_jobs() {
    let _guard = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let request = fixture.request().await;
    let job = crate::audio::inference::claim_job().unwrap();
    assert!(remove_selected(
        fixture.pool.clone(),
        fixture.roots.clone(),
        vec![request.clone()]
    )
    .await
    .is_err());
    drop(job);
    fs::write(
        fixture.folder.join("audio.mp4"),
        "newer copy with a different size",
    )
    .unwrap();
    let results = remove_selected(fixture.pool.clone(), fixture.roots.clone(), vec![request])
        .await
        .unwrap();
    assert!(results[0].removed_bytes.is_none());
    assert!(fixture.folder.join("audio.mp4").exists());
}

#[tokio::test]
async fn pending_transcript_copy_blocks_confirmed_audio_removal() {
    let _guard = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let request = fixture.request().await;
    let rows = folders(&fixture.pool).await.unwrap();
    let canonical = canonical_folders(&fixture.roots, &rows);
    let candidate = inspect_candidate(&fixture.roots, &rows, &canonical, "first", true).unwrap();
    sqlx::query("INSERT INTO transcript_file_sync (meeting_id) VALUES ('first')")
        .execute(&fixture.pool)
        .await
        .unwrap();
    assert!(super::super::archive_rows(&fixture.pool).await.unwrap()[0].pending_file_sync);
    assert_eq!(fixture.checks().await[0].reason, Some(RECOVERY_PENDING));
    let job = Arc::new(crate::audio::inference::claim_job().unwrap());
    let result = remove_prepared(
        &fixture.pool,
        &fixture.roots,
        &request,
        &rows,
        candidate,
        job,
    )
    .await;
    assert_eq!(result, Err(RECOVERY_PENDING));
    assert!(fixture.folder.join("audio.mp4").exists());
}

#[tokio::test]
async fn keeps_recovery_originals_and_classifies_storage_without_reading_payloads() {
    let _guard = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let request = fixture.request().await;
    let recovery = fixture.folder.join(".audio-spool");
    fs::create_dir(&recovery).unwrap();
    fs::write(recovery.join("0001.chunk"), "synthetic capture").unwrap();
    fs::write(fixture.folder.join("audio-recovered.wav"), "saved recovery").unwrap();
    let results = remove_selected(fixture.pool.clone(), fixture.roots.clone(), vec![request])
        .await
        .unwrap();
    assert_eq!(results[0].reason, Some(RECOVERY_PENDING));
    assert!(recovery.join("0001.chunk").exists());
    assert!(fixture.folder.join("audio-recovered.wav").exists());

    add_meeting(&fixture.pool, "second", &fixture.folder).await;
    let data = fixture._temp.path().join("app");
    fs::create_dir_all(data.join("reference-documents")).unwrap();
    fs::create_dir_all(data.join("backups")).unwrap();
    fs::write(data.join("meeting_minutes.sqlite"), "database").unwrap();
    fs::write(data.join("reference-documents/attachment"), "reference").unwrap();
    fs::write(data.join("backups/snapshot.sqlite"), "snapshot").unwrap();
    let rows = folders(&fixture.pool).await.unwrap();
    let measure = measure_storage(&fixture.roots, &rows, &data);
    assert_eq!(measure.saved_audio_bytes, 11);
    assert_eq!(measure.recovery_bytes, 31);
    assert_eq!(measure.database_bytes, 8);
    assert_eq!(measure.reference_bytes, 9);
    assert_eq!(measure.database_backup_bytes, 8);
}

#[cfg(unix)]
#[tokio::test]
async fn refuses_audio_symlinks() {
    let fixture = Fixture::new().await;
    let audio = fixture.folder.join("audio.mp4");
    let original = fixture._temp.path().join("source.mp4");
    fs::rename(&audio, &original).unwrap();
    std::os::unix::fs::symlink(&original, &audio).unwrap();
    assert!(fixture.checks().await[0].revision.is_none());
    assert!(original.exists());
}

#[tokio::test]
async fn refuses_audio_hardlinks() {
    let fixture = Fixture::new().await;
    let audio = fixture.folder.join("audio.mp4");
    let original = fixture._temp.path().join("source.mp4");
    fs::rename(&audio, &original).unwrap();
    fs::hard_link(&original, &audio).unwrap();
    assert!(fixture.checks().await[0].revision.is_none());
    assert!(original.exists());
}

#[tokio::test]
async fn refuses_when_folder_claimed_by_new_meeting_at_phase_b() {
    let _guard = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let request = fixture.request().await;
    let rows = folders(&fixture.pool).await.unwrap();
    let canonical = canonical_folders(&fixture.roots, &rows);
    let candidate = inspect_candidate(&fixture.roots, &rows, &canonical, "first", true).unwrap();
    add_meeting(&fixture.pool, "second", &fixture.folder.join(".")).await;
    let job = Arc::new(crate::audio::inference::claim_job().unwrap());
    let result = remove_prepared(
        &fixture.pool,
        &fixture.roots,
        &request,
        &rows,
        candidate,
        job,
    )
    .await;
    assert_eq!(result, Err(FOLDER_CLAIMS_CHANGED));
    assert!(fixture.folder.join("audio.mp4").exists());
}

#[tokio::test]
async fn unrelated_write_between_phases_does_not_block_removal() {
    let _guard = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let request = fixture.request().await;
    let other_folder = fixture.roots[0].join("other");
    fs::create_dir_all(&other_folder).unwrap();
    add_meeting(&fixture.pool, "second", &other_folder).await;
    let rows = folders(&fixture.pool).await.unwrap();
    let canonical = canonical_folders(&fixture.roots, &rows);
    let candidate = inspect_candidate(&fixture.roots, &rows, &canonical, "first", true).unwrap();
    sqlx::query("UPDATE meetings SET title = 'Renamed' WHERE id = 'second'")
        .execute(&fixture.pool)
        .await
        .unwrap();
    let job = Arc::new(crate::audio::inference::claim_job().unwrap());
    let result = remove_prepared(
        &fixture.pool,
        &fixture.roots,
        &request,
        &rows,
        candidate,
        job,
    )
    .await;
    assert_eq!(result, Ok(11));
    assert!(!fixture.folder.join("audio.mp4").exists());
}

#[tokio::test]
async fn refuses_recovery_directory_created_after_candidate_inspection() {
    let _guard = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let request = fixture.request().await;
    let rows = folders(&fixture.pool).await.unwrap();
    let canonical = canonical_folders(&fixture.roots, &rows);
    let candidate = inspect_candidate(&fixture.roots, &rows, &canonical, "first", true).unwrap();
    fs::create_dir(fixture.folder.join(".audio-spool")).unwrap();
    let job = Arc::new(crate::audio::inference::claim_job().unwrap());
    let result = remove_prepared(
        &fixture.pool,
        &fixture.roots,
        &request,
        &rows,
        candidate,
        job,
    )
    .await;
    assert_eq!(result, Err(RECOVERY_PENDING));
    assert!(fixture.folder.join("audio.mp4").exists());
}
