//! Public synthetic reference archives. No parser processes or providers run here.
use super::*;
use crate::knowledge::{conversations, documents, evidence, types::*};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

const PDF_ID: &str = "00000000-0000-4000-8000-000000000061";
const DOCX_ID: &str = "00000000-0000-4000-8000-000000000062";
const TXT_ID: &str = "00000000-0000-4000-8000-000000000063";
const MD_ID: &str = "00000000-0000-4000-8000-000000000064";
const DOCUMENT_TABLES: &[&str] = &[
    "knowledge_documents",
    "knowledge_document_blocks",
    "knowledge_document_attachments",
];

async fn rows(pool: &SqlitePool, table: &str) -> Vec<Map<String, Value>> {
    let columns = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await
        .unwrap();
    let fields = columns
        .iter()
        .map(|column| {
            let name: String = column.get("name");
            format!("'{name}',\"{name}\"")
        })
        .collect::<Vec<_>>()
        .join(",");
    sqlx::query_scalar::<_, String>(&format!(
        "SELECT json_object({fields}) FROM {table} ORDER BY rowid"
    ))
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| serde_json::from_str(&row).unwrap())
    .collect()
}

async fn seed_document(
    pool: &SqlitePool,
    root: &Path,
    id: &str,
    format: documents::DocumentFormat,
    name: &str,
    bytes: &[u8],
) -> documents::ExtractedDocument {
    let extracted = documents::extract::extract(format, bytes).unwrap();
    let result = documents::store::publish(
        pool,
        "archive-source",
        id,
        name,
        bytes.len() as u64,
        &format!("{:x}", Sha256::digest(bytes)),
        &extracted,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(result.original_kept);
    std::fs::write(root.join(format!("{id}.{}", format.extension())), bytes).unwrap();
    extracted
}

async fn document_fixture() -> (SqlitePool, AskRequest, tempfile::TempDir) {
    let (pool, mut request) = super::tests::conversation_fixture().await;
    let root = tempfile::tempdir().unwrap();
    let pdf = seed_document(
        &pool,
        root.path(),
        PDF_ID,
        documents::DocumentFormat::Pdf,
        "Aster pages.pdf",
        &documents::fixtures::pdf(24, true, false),
    )
    .await;
    assert!(pdf.blocks.iter().any(|block| block.page == Some(24)));
    let docx = seed_document(
        &pool,
        root.path(),
        DOCX_ID,
        documents::DocumentFormat::Docx,
        "Aster tables.docx",
        &documents::fixtures::docx(&documents::fixtures::table_document(), &[]),
    )
    .await;
    assert!(docx.blocks.len() > 64);
    seed_document(
        &pool,
        root.path(),
        TXT_ID,
        documents::DocumentFormat::Text,
        "Aster notes.txt",
        b"Public reference notes.\n\nSecond paragraph: budget unchanged.",
    )
    .await;
    seed_document(
        &pool,
        root.path(),
        MD_ID,
        documents::DocumentFormat::Markdown,
        "Aster decisions.md",
        b"# Public decisions\n\nApproved milestone: September.",
    )
    .await;
    sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('archive-added','Second public meeting','2026-09-02','2026-09-02')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO knowledge_document_attachments(meeting_id,document_id) VALUES ('archive-added',?)")
        .bind(PDF_ID).execute(&pool).await.unwrap();

    // Start with a real persisted transcript conversation, then add canonical
    // document snapshots directly so this archive contract is independent of
    // the retrieval/UI implementation being developed alongside it.
    request.search.document_ids = vec![PDF_ID.into(), DOCX_ID.into(), TXT_ID.into(), MD_ID.into()];
    // Request identities are immutable. Build the synthetic persisted snapshot
    // through fresh insertion rather than weakening that production trigger.
    let mut request_row = rows(&pool, "knowledge_requests").await.remove(0);
    let original_messages = rows(&pool, "knowledge_messages").await;
    let original_evidence = rows(&pool, "knowledge_request_evidence").await;
    let original_sources = rows(&pool, "knowledge_request_sources").await;
    request_row.insert(
        "input_json".into(),
        serde_json::to_string(&request).unwrap().into(),
    );
    request_row.insert(
        "input_fingerprint".into(),
        conversations::input_fingerprint(&request, "builtin-ai", "qwen3.5:4b")
            .unwrap()
            .into(),
    );
    sqlx::query("DELETE FROM knowledge_requests WHERE id=?")
        .bind(&request.request_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut connection = pool.acquire().await.unwrap();
    insert_archive_row(&mut connection, "knowledge_requests", &request_row)
        .await
        .unwrap();
    for (table, original_rows) in [
        ("knowledge_messages", original_messages),
        ("knowledge_request_evidence", original_evidence),
        ("knowledge_request_sources", original_sources),
    ] {
        for row in original_rows {
            insert_archive_row(&mut connection, table, &row)
                .await
                .unwrap();
        }
    }
    drop(connection);
    sqlx::query("CREATE TABLE IF NOT EXISTS knowledge_document_permissions(owner_id TEXT PRIMARY KEY REFERENCES knowledge_owners(id) ON DELETE CASCADE,enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)))")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO knowledge_document_permissions(owner_id,enabled) VALUES (?,1)")
        .bind(conversations::owner_key(&request.owner).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let block_rows = rows(&pool, "knowledge_document_blocks").await;
    let chosen = [
        block_rows
            .iter()
            .find(|row| row["document_id"] == PDF_ID && row["page"] == 24)
            .unwrap(),
        block_rows
            .iter()
            .rev()
            .find(|row| row["document_id"] == DOCX_ID)
            .unwrap(),
        block_rows
            .iter()
            .find(|row| row["document_id"] == TXT_ID)
            .unwrap(),
        block_rows
            .iter()
            .find(|row| row["document_id"] == MD_ID)
            .unwrap(),
    ];
    let mut revisions = BTreeMap::new();
    for id in [PDF_ID, DOCX_ID, TXT_ID, MD_ID] {
        // Intentionally stale saved counters must not be rewritten as current.
        let revision: i64 =
            sqlx::query_scalar("SELECT revision+11 FROM knowledge_sources WHERE id=?")
                .bind(format!("document:{id}"))
                .fetch_one(&pool)
                .await
                .unwrap();
        revisions.insert(id, revision);
        sqlx::query(
            "INSERT INTO knowledge_request_sources(request_id,source_id,revision) VALUES (?,?,?)",
        )
        .bind(&request.request_id)
        .bind(format!("document:{id}"))
        .bind(revision)
        .execute(&pool)
        .await
        .unwrap();
    }
    for ordinal in 2..=700 {
        let block = chosen[(ordinal - 2) % chosen.len()];
        let id = block["document_id"].as_str().unwrap();
        let text = block["text"].as_str().unwrap();
        let reference = serde_json::json!({
            "historical":false,"source_id":format!("document:{id}"),
            "source_revision":revisions[id],"chunk_id":format!("public-document-{ordinal}"),
            "fingerprint":format!("{:x}",Sha256::digest(text.as_bytes())),
            "locator":{"kind":"document","document_id":id,"page":block["page"],
                "paragraph":block["paragraph"],"spans":[{"transcript_id":block["id"],
                    "start_byte":0,"end_byte":text.len()}]}
        });
        let display = serde_json::json!({"title":"Public reference material","date":"2026-09-01",
            "speaker":null,"metadata_truncated":false});
        sqlx::query("INSERT INTO knowledge_request_evidence(request_id,ordinal,reference_json,display_json) VALUES (?,?,?,?)")
            .bind(&request.request_id).bind(ordinal as i64).bind(reference.to_string()).bind(display.to_string())
            .execute(&pool).await.unwrap();
    }
    sqlx::query("UPDATE knowledge_messages SET content='Reviewed transcript and references [K1][K700].' WHERE request_id=? AND role='assistant'")
        .bind(&request.request_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO knowledge_chunks(id,source_id,revision,generation,ordinal,transcript_id,start_byte,end_byte,fingerprint,text) SELECT 'public-document-cache',id,revision,generation,1,?,0,6,'public-fixture','Public' FROM knowledge_sources WHERE id=?")
        .bind(format!("{PDF_ID}:1")).bind(format!("document:{PDF_ID}")).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO knowledge_vectors(chunk_id,space,dimensions,vector) VALUES ('public-document-cache','public-fixture',384,zeroblob(1536))")
        .execute(&pool).await.unwrap();
    (pool, request, root)
}

// Hand-authored V2 input bypasses backup generation for hostile unpack tests.
async fn raw_v2(pool: &SqlitePool) -> Manifest {
    let (mut manifest, _) = snapshot(pool).await.unwrap();
    manifest.version = 2;
    for table in DOCUMENT_TABLES {
        manifest
            .tables
            .insert((*table).into(), rows(pool, table).await);
    }
    for row in &manifest.tables["knowledge_documents"] {
        manifest.document_files.insert(
            row["id"].as_str().unwrap().into(),
            format!(
                "reference-documents/{}",
                row["storage_name"].as_str().unwrap()
            ),
        );
    }
    manifest
}

fn raw_archive(path: &Path, manifest: &Manifest, originals: &Path, tamper: bool) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    zip.start_file("manifest.json", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&serde_json::to_vec(manifest).unwrap())
        .unwrap();
    for (id, relative) in &manifest.document_files {
        let document = manifest.tables["knowledge_documents"]
            .iter()
            .find(|row| row["id"] == *id)
            .unwrap();
        // Inputs are from our trusted fixture root, never from hostile metadata.
        let extension = if id == PDF_ID {
            "pdf"
        } else if id == DOCX_ID {
            "docx"
        } else if id == TXT_ID {
            "txt"
        } else {
            "md"
        };
        let mut bytes = std::fs::read(originals.join(format!("{id}.{extension}"))).unwrap();
        if tamper && id == PDF_ID {
            bytes[0] ^= 1; // Same size: only content-integrity verification catches it.
        }
        assert!(document.contains_key("sha256"));
        zip.start_file(relative, SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.finish().unwrap();
}

fn unpack_error(path: &Path, destination: &Path) -> String {
    match unpack(path, destination, &HashSet::new()) {
        Ok(_) => panic!("Hostile document archive was accepted"),
        Err(error) => error,
    }
}

#[tokio::test]
async fn backup_v2_document_roundtrip() {
    let (source, request, originals) = document_fixture().await;
    let expected_documents = rows(&source, "knowledge_documents").await;
    let expected_blocks = rows(&source, "knowledge_document_blocks").await;
    let expected_evidence = rows(&source, "knowledge_request_evidence").await;
    let (manifest, folders) = snapshot_with_documents(&source).await.unwrap();
    assert_eq!(
        manifest.version, 2,
        "Reference originals need the portable V2 archive contract"
    );
    for table in DOCUMENT_TABLES {
        assert!(
            manifest.tables.contains_key(*table),
            "Missing canonical table: {table}"
        );
    }
    for forbidden in [
        "knowledge_sources",
        "knowledge_chunks",
        "knowledge_vectors",
        "knowledge_fts",
        "knowledge_document_fts",
        "knowledge_index_jobs",
        "knowledge_settings",
        "knowledge_document_permissions",
        "settings",
    ] {
        assert!(
            !manifest.tables.contains_key(forbidden),
            "Private or derived table: {forbidden}"
        );
    }
    std::fs::write(
        originals.path().join("tokens.json"),
        b"excluded synthetic credentials",
    )
    .unwrap();
    std::fs::write(
        originals.path().join("model.onnx"),
        b"excluded synthetic model",
    )
    .unwrap();
    let archive_root = tempfile::tempdir().unwrap();
    let archive = archive_root.path().join("references.zip");
    let report =
        write_archive_with_documents(&archive, manifest, folders, originals.path()).unwrap();
    assert_eq!(report.files, 4);
    let mut zip = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
    assert_eq!(zip.len(), 5);
    assert!(zip
        .by_name(&format!("reference-documents/{PDF_ID}.pdf"))
        .is_ok());
    assert!(zip.by_name("reference-documents/tokens.json").is_err());
    assert!(zip.by_name("reference-documents/model.onnx").is_err());
    drop(zip);
    let destination = super::tests::empty_conversation_destination().await;
    sqlx::query("CREATE TABLE IF NOT EXISTS knowledge_document_permissions(owner_id TEXT PRIMARY KEY REFERENCES knowledge_owners(id) ON DELETE CASCADE,enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)))")
        .execute(&destination).await.unwrap();
    let target = tempfile::tempdir().unwrap();
    let restored_originals = target.path().join("reference-documents");
    let (manifest, stage) =
        unpack(&archive, &target.path().join("recordings"), &HashSet::new()).unwrap();
    let restored =
        import_manifest_with_documents(&destination, manifest, stage, &restored_originals)
            .await
            .unwrap();
    assert_eq!(restored.meetings, 2);
    assert_eq!(
        rows(&destination, "knowledge_documents").await,
        expected_documents
    );
    assert_eq!(
        rows(&destination, "knowledge_document_blocks").await,
        expected_blocks
    );
    assert_eq!(
        rows(&destination, "knowledge_document_attachments")
            .await
            .len(),
        5
    );
    for (id, extension) in [
        (PDF_ID, "pdf"),
        (DOCX_ID, "docx"),
        (TXT_ID, "txt"),
        (MD_ID, "md"),
    ] {
        assert_eq!(
            std::fs::read(restored_originals.join(format!("{id}.{extension}"))).unwrap(),
            std::fs::read(originals.path().join(format!("{id}.{extension}"))).unwrap()
        );
    }
    let history = conversations::history(&destination, &request.owner)
        .await
        .unwrap();
    let sharing: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT enabled FROM knowledge_document_permissions WHERE owner_id=?),0)",
    )
    .bind(conversations::owner_key(&request.owner).unwrap())
    .fetch_one(&destination)
    .await
    .unwrap();
    assert_eq!(
        sharing, 0,
        "Restoring an archive must never grant external document sharing"
    );
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[1].content,
        "Reviewed transcript and references [K1][K700]."
    );
    let reply = history[1].reply.as_ref().unwrap();
    assert_eq!(reply.evidence.len(), 700);
    assert_eq!(reply.evidence_metadata.len(), 700);
    assert_eq!(reply.cited_tags, vec![1, 700]);
    let restored_evidence = rows(&destination, "knowledge_request_evidence").await;
    for (before, after) in expected_evidence.iter().zip(&restored_evidence) {
        let mut expected: Value =
            serde_json::from_str(before["reference_json"].as_str().unwrap()).unwrap();
        expected["historical"] = Value::Bool(true);
        let actual: Value =
            serde_json::from_str(after["reference_json"].as_str().unwrap()).unwrap();
        assert_eq!(
            actual, expected,
            "Restore must preserve canonical anchors/spans, IDs, hashes, and stale counters"
        );
        assert_eq!(after["display_json"], before["display_json"]);
    }
    assert_eq!(
        reply.evidence[1].locator,
        serde_json::from_value::<EvidenceRef>(
            serde_json::from_str::<Value>(expected_evidence[1]["reference_json"].as_str().unwrap())
                .unwrap()
        )
        .unwrap()
        .locator
    );
    assert_eq!(
        evidence::resolve(&destination, &reply.evidence[1])
            .await
            .unwrap()
            .status,
        evidence::EvidenceStatus::Stale
    );
    assert!(rows(&destination, "knowledge_chunks").await.is_empty());
    assert!(rows(&destination, "knowledge_vectors").await.is_empty());
    let jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM knowledge_index_jobs WHERE source_id LIKE 'document:%'",
    )
    .fetch_one(&destination)
    .await
    .unwrap();
    assert_eq!(
        jobs, 4,
        "Every restored document must requeue derived indexing"
    );
    let fts: i64 = sqlx::query_scalar("SELECT count(*) FROM knowledge_document_fts")
        .fetch_one(&destination)
        .await
        .unwrap();
    assert_eq!(
        fts,
        expected_blocks.len() as i64,
        "Canonical inserts rebuild local keyword rows"
    );
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&destination)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn restore_v1_without_documents() {
    let (source, request) = super::tests::conversation_fixture().await;
    let (mut manifest, folders) = snapshot(&source).await.unwrap();
    manifest.version = 1;
    for table in DOCUMENT_TABLES {
        manifest.tables.remove(*table);
    }
    manifest.document_files.clear();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("legacy.zip");
    write_archive(&path, manifest, folders).unwrap();
    let destination = super::tests::empty_conversation_destination().await;
    let (manifest, stage) = unpack(&path, &root.path().join("restored"), &HashSet::new()).unwrap();
    import_manifest_with_documents(
        &destination,
        manifest,
        stage,
        &root.path().join("references"),
    )
    .await
    .unwrap();
    assert!(rows(&destination, "knowledge_documents").await.is_empty());
    assert_eq!(
        conversations::history(&destination, &request.owner)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(!root.path().join("references").exists());
}

#[tokio::test]
async fn tampered_document_hash_rejected_before_publication() {
    let (source, _, originals) = document_fixture().await;
    let manifest = raw_v2(&source).await;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("tampered.zip");
    raw_archive(&path, &manifest, originals.path(), true);
    let target = root.path().join("unpublished");
    let error = unpack_error(&path, &target);
    assert!(
        error.to_lowercase().contains("hash"),
        "Expected document-integrity rejection, got: {error}"
    );
    assert!(
        !target.exists(),
        "Validate all original bytes before creating the restore stage"
    );
}

#[tokio::test]
async fn traversal_and_unsafe_document_names_rejected_before_extraction() {
    let (source, _, originals) = document_fixture().await;
    for unsafe_name in [
        "../escape.pdf",
        "C:/escape.pdf",
        "/escape.pdf",
        "reference-documents/../escape.pdf",
        "reference-documents/CON.pdf",
        "reference-documents/tokens.json",
        "reference-documents/model.onnx",
    ] {
        let mut manifest = raw_v2(&source).await;
        manifest
            .document_files
            .insert(PDF_ID.into(), unsafe_name.into());
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("unsafe.zip");
        raw_archive(&path, &manifest, originals.path(), false);
        let target = root.path().join("unpublished");
        let error = unpack_error(&path, &target);
        assert!(
            error.to_lowercase().contains("filename"),
            "Expected filename validation for {unsafe_name}, got: {error}"
        );
        assert!(!target.exists());
    }
}

#[tokio::test]
async fn malicious_document_metadata_rejected_before_extraction() {
    let (source, _, originals) = document_fixture().await;
    for (key, value) in [
        ("id", Value::from("invalid-uuid")),
        ("source_id", Value::from("meeting:archive-source")),
        ("sha256", Value::from("not-a-hash")),
        ("file_size", Value::from(26_214_401)),
        ("file_size", Value::from(-1)),
        ("storage_name", Value::from("../escape.pdf")),
        ("format", Value::from("exe")),
        ("unexpected_column", Value::from("invalid")),
    ] {
        let mut manifest = raw_v2(&source).await;
        // Keep the file-map lookup valid when testing a noncanonical identity.
        if key == "id" {
            manifest.tables.get_mut("knowledge_documents").unwrap()[0].insert("id".into(), value);
            manifest.document_files.clear();
        } else {
            manifest.tables.get_mut("knowledge_documents").unwrap()[0].insert(key.into(), value);
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("invalid.zip");
        raw_archive(&path, &manifest, originals.path(), false);
        let target = root.path().join("unpublished");
        let error = unpack_error(&path, &target);
        assert!(
            error.to_lowercase().contains("document"),
            "Expected document metadata rejection for {key}, got: {error}"
        );
        assert!(!target.exists());
    }
}

#[tokio::test]
async fn skipped_meeting_keeps_shared_documents_for_new_owners() {
    let (source, request, originals) = document_fixture().await;
    let manifest = raw_v2(&source).await;
    let destination = super::tests::empty_conversation_destination().await;
    // Restore skips this meeting, but its same canonical source remains usable
    // by the newly restored library owner and archive-added attachment relation.
    let meeting = &manifest.tables["meetings"][0];
    let mut connection = destination.acquire().await.unwrap();
    insert_archive_row(&mut connection, "meetings", meeting)
        .await
        .unwrap();
    for row in &manifest.tables["transcripts"] {
        insert_archive_row(&mut connection, "transcripts", row)
            .await
            .unwrap();
    }
    drop(connection);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("shared.zip");
    raw_archive(&path, &manifest, originals.path(), false);
    let (manifest, stage) = unpack(
        &path,
        &root.path().join("stage"),
        &HashSet::from(["archive-source".into()]),
    )
    .unwrap();
    let restored_originals = root.path().join("references");
    let result = import_manifest_with_documents(&destination, manifest, stage, &restored_originals)
        .await
        .unwrap();
    assert_eq!((result.meetings, result.skipped), (1, 1));
    let shared: i64 = sqlx::query_scalar("SELECT count(*) FROM knowledge_document_attachments WHERE meeting_id='archive-added' AND document_id=?")
        .bind(PDF_ID).fetch_one(&destination).await.unwrap();
    assert_eq!(shared, 1);
    assert!(restored_originals.join(format!("{PDF_ID}.pdf")).exists());
    let history = conversations::history(&destination, &request.owner)
        .await
        .unwrap();
    assert_eq!(history[1].status, "completed");
    assert_eq!(history[1].reply.as_ref().unwrap().evidence.len(), 700);
}

#[tokio::test]
async fn document_id_collision_remaps_originals_blocks_and_historical_references() {
    let (source, request, originals) = document_fixture().await;
    let manifest = raw_v2(&source).await;
    let destination = super::tests::empty_conversation_destination().await;
    sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('archive-source','Existing local meeting','2026-09-01','2026-09-01')")
        .execute(&destination).await.unwrap();
    let target = tempfile::tempdir().unwrap();
    let restored_originals = target.path().join("references");
    std::fs::create_dir(&restored_originals).unwrap();
    let existing_bytes = documents::fixtures::pdf(1, true, false);
    seed_document(
        &destination,
        &restored_originals,
        PDF_ID,
        documents::DocumentFormat::Pdf,
        "Existing local reference.pdf",
        &existing_bytes,
    )
    .await;
    // Leave the skipped meeting source identical so this test isolates the
    // independent document-ID collision from meeting collision invalidation.
    sqlx::query("UPDATE meetings SET title='Public archive fixture' WHERE id='archive-source'")
        .execute(&destination)
        .await
        .unwrap();
    let mut connection = destination.acquire().await.unwrap();
    for row in &manifest.tables["transcripts"] {
        insert_archive_row(&mut connection, "transcripts", row)
            .await
            .unwrap();
    }
    drop(connection);
    let archive = target.path().join("collision.zip");
    raw_archive(&archive, &manifest, originals.path(), false);
    let (manifest, stage) = unpack(
        &archive,
        &target.path().join("stage"),
        &HashSet::from(["archive-source".into()]),
    )
    .unwrap();
    import_manifest_with_documents(&destination, manifest, stage, &restored_originals)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(restored_originals.join(format!("{PDF_ID}.pdf"))).unwrap(),
        existing_bytes
    );
    let remapped: String = sqlx::query_scalar(
        "SELECT id FROM knowledge_documents WHERE display_name='Aster pages.pdf'",
    )
    .fetch_one(&destination)
    .await
    .unwrap();
    assert_ne!(remapped, PDF_ID);
    assert!(uuid::Uuid::parse_str(&remapped).is_ok());
    assert_eq!(
        std::fs::read(restored_originals.join(format!("{remapped}.pdf"))).unwrap(),
        std::fs::read(originals.path().join(format!("{PDF_ID}.pdf"))).unwrap()
    );
    let history = conversations::history(&destination, &request.owner)
        .await
        .unwrap();
    assert_eq!(
        history[1].content,
        "Reviewed transcript and references [K1][K700]."
    );
    let reference = &history[1].reply.as_ref().unwrap().evidence[1];
    assert!(reference.historical);
    assert_eq!(reference.source_id, format!("document:{remapped}"));
    let locator = serde_json::to_value(&reference.locator).unwrap();
    assert_eq!(locator["document_id"], remapped);
    assert_eq!(locator["page"], 24);
    assert!(locator["spans"][0]["transcript_id"]
        .as_str()
        .unwrap()
        .starts_with(&format!("{remapped}:")));
    let dependency: String = sqlx::query_scalar(
        "SELECT source_id FROM knowledge_request_sources WHERE request_id=? AND source_id=?",
    )
    .bind(&request.request_id)
    .bind(format!("document:{remapped}"))
    .fetch_one(&destination)
    .await
    .unwrap();
    assert_eq!(dependency, reference.source_id);
    let original_dependency: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM knowledge_request_sources WHERE request_id=? AND source_id=?",
    )
    .bind(&request.request_id)
    .bind(format!("document:{PDF_ID}"))
    .fetch_one(&destination)
    .await
    .unwrap();
    assert_eq!(original_dependency, 0);
}

#[tokio::test]
async fn restore_failure_rolls_back_documents_and_keeps_existing_originals_and_audio() {
    let (source, _, originals) = document_fixture().await;
    let manifest = raw_v2(&source).await;
    let destination = super::tests::empty_conversation_destination().await;
    sqlx::query("CREATE TRIGGER public_fixture_restore_failure BEFORE INSERT ON knowledge_document_attachments BEGIN SELECT RAISE(ABORT,'synthetic restore failure'); END")
        .execute(&destination).await.unwrap();
    let target = tempfile::tempdir().unwrap();
    let restored_originals = target.path().join("references");
    std::fs::create_dir(&restored_originals).unwrap();
    let sentinel = restored_originals.join("existing-original.txt");
    std::fs::write(&sentinel, b"existing original").unwrap();
    let audio = target.path().join("existing-audio.wav");
    std::fs::write(&audio, b"existing playable fixture").unwrap();
    let archive = target.path().join("failure.zip");
    raw_archive(&archive, &manifest, originals.path(), false);
    let (manifest, stage) =
        unpack(&archive, &target.path().join("stage"), &HashSet::new()).unwrap();
    let stage_path = stage.path().to_path_buf();
    assert!(
        import_manifest_with_documents(&destination, manifest, stage, &restored_originals)
            .await
            .is_err()
    );
    assert!(!stage_path.exists());
    assert!(rows(&destination, "knowledge_documents").await.is_empty());
    assert!(rows(&destination, "knowledge_document_blocks")
        .await
        .is_empty());
    assert!(rows(&destination, "meetings").await.is_empty());
    assert_eq!(std::fs::read_dir(&restored_originals).unwrap().count(), 1);
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"existing original");
    assert_eq!(std::fs::read(&audio).unwrap(), b"existing playable fixture");
}
