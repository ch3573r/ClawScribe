//! Canonical document archive validation and rollback-safe original publication.
use super::*;
use crate::knowledge::documents::{INPUT_BYTES, PAGE_LIMIT, TEXT_BYTES};
use sha2::{Digest, Sha256};

pub(super) const TABLES: &[&str] = &[
    "knowledge_documents",
    "knowledge_document_blocks",
    "knowledge_document_attachments",
];

pub(super) struct Document {
    pub format: String,
    pub storage_name: String,
    pub size: u64,
    pub hash: String,
}
impl Document {
    pub fn relative(&self) -> String {
        format!("reference-documents/{}", self.storage_name)
    }
}

fn rows<'a>(manifest: &'a Manifest, table: &str) -> &'a [Map<String, Value>] {
    manifest.tables.get(table).map(Vec::as_slice).unwrap_or(&[])
}
fn text<'a>(row: &'a Map<String, Value>, name: &str) -> Result<&'a str, String> {
    row.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| "Invalid document metadata.".into())
}
fn canonical_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .map(|id| id.to_string())
        .ok()
        .as_deref()
        == Some(value)
}
fn bounded_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn fields(row: &Map<String, Value>, names: &[&str]) -> Result<(), String> {
    if row.len() != names.len() || row.keys().any(|name| !names.contains(&name.as_str())) {
        return Err("Unsupported document metadata columns.".into());
    }
    Ok(())
}

pub(super) fn set_file_references(manifest: &mut Manifest) -> Result<(), String> {
    if manifest.version != 2 {
        return Ok(());
    }
    manifest.document_files.clear();
    for row in rows(manifest, "knowledge_documents").to_vec() {
        manifest.document_files.insert(
            text(&row, "id")?.into(),
            format!("reference-documents/{}", text(&row, "storage_name")?),
        );
    }
    Ok(())
}

pub(super) fn validate(manifest: &Manifest) -> Result<BTreeMap<String, Document>, String> {
    if manifest.version == 1 {
        if !manifest.document_files.is_empty()
            || TABLES
                .iter()
                .any(|table| manifest.tables.contains_key(*table))
        {
            return Err("Document originals require archive version 2.".into());
        }
        return Ok(BTreeMap::new());
    }
    if manifest.version != 2
        || TABLES
            .iter()
            .any(|table| !manifest.tables.contains_key(*table))
    {
        return Err("Unsupported document archive metadata.".into());
    }
    for (id, relative) in &manifest.document_files {
        let name = relative
            .strip_prefix("reference-documents/")
            .ok_or("Unsafe document filename.")?;
        let (stem, extension) = name.rsplit_once('.').ok_or("Unsafe document filename.")?;
        if !canonical_uuid(id) || stem != id || !matches!(extension, "pdf" | "docx" | "txt" | "md")
        {
            return Err("Unsafe document filename.".into());
        }
    }
    let mut documents = BTreeMap::new();
    let mut hashes = HashSet::new();
    for row in rows(manifest, "knowledge_documents") {
        fields(
            row,
            &[
                "id",
                "source_id",
                "display_name",
                "format",
                "file_size",
                "sha256",
                "storage_name",
                "created_at",
            ],
        )?;
        let id = text(row, "id")?;
        let format = text(row, "format")?;
        let name = text(row, "display_name")?;
        let hash = text(row, "sha256")?;
        let storage = text(row, "storage_name")?;
        let size = row
            .get("file_size")
            .and_then(Value::as_u64)
            .filter(|n| *n <= INPUT_BYTES as u64)
            .ok_or("Invalid document file size.")?;
        if !canonical_uuid(id)
            || text(row, "source_id")? != format!("document:{id}")
            || !matches!(format, "pdf" | "docx" | "txt" | "md")
            || storage != format!("{id}.{format}")
            || !bounded_text(name, 1024)
            || name.contains(['/', '\\', ':'])
            || !bounded_text(text(row, "created_at")?, 1024)
            || hash.len() != 64
            || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !hashes.insert((hash.to_ascii_lowercase(), format.to_string()))
        {
            return Err("Invalid or duplicate document metadata.".into());
        }
        let document = Document {
            format: format.into(),
            storage_name: storage.into(),
            size,
            hash: hash.into(),
        };
        if manifest.document_files.get(id) != Some(&document.relative()) {
            return Err("Missing or mismatched document filename.".into());
        }
        if documents.insert(id.to_string(), document).is_some() {
            return Err("Duplicate document identity.".into());
        }
    }
    if documents.len() != manifest.document_files.len() {
        return Err("Unknown document filename reference.".into());
    }
    let mut blocks = BTreeMap::<&str, BTreeMap<u64, usize>>::new();
    for row in rows(manifest, "knowledge_document_blocks") {
        fields(
            row,
            &["id", "document_id", "ordinal", "page", "paragraph", "text"],
        )?;
        let id = text(row, "document_id")?;
        let document = documents.get(id).ok_or("Unknown document block parent.")?;
        let ordinal = row
            .get("ordinal")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
            .ok_or("Invalid document block ordinal.")?;
        row.get("paragraph")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0 && *n <= u32::MAX as u64)
            .ok_or("Invalid document paragraph.")?;
        let page = if row["page"].is_null() {
            None
        } else {
            Some(
                row["page"]
                    .as_u64()
                    .filter(|n| *n > 0 && *n <= PAGE_LIMIT as u64)
                    .ok_or("Invalid document page.")?,
            )
        };
        let body = text(row, "text")?;
        if text(row, "id")? != format!("{id}:{ordinal}")
            || body.trim().is_empty()
            || body.contains('\0')
            || (document.format == "pdf") != page.is_some()
            || blocks
                .entry(id)
                .or_default()
                .insert(ordinal, body.len())
                .is_some()
        {
            return Err("Invalid or duplicate canonical document block.".into());
        }
    }
    for id in documents.keys() {
        let blocks = blocks
            .get(id.as_str())
            .ok_or("Document has no extracted blocks.")?;
        let mut bytes = 0usize;
        for (index, (ordinal, size)) in blocks.iter().enumerate() {
            bytes = bytes
                .checked_add(*size)
                .filter(|n| *n <= TEXT_BYTES)
                .ok_or("Document text exceeds 2 MiB.")?;
            if *ordinal != index as u64 + 1 {
                return Err("Non-contiguous document blocks.".into());
            }
        }
    }
    let meetings: HashSet<_> = rows(manifest, "meetings")
        .iter()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .collect();
    let mut attachments = HashSet::new();
    for row in rows(manifest, "knowledge_document_attachments") {
        fields(row, &["meeting_id", "document_id", "created_at"])?;
        let meeting = text(row, "meeting_id")?;
        let document = text(row, "document_id")?;
        if !meetings.contains(meeting)
            || !documents.contains_key(document)
            || !bounded_text(text(row, "created_at")?, 1024)
            || !attachments.insert((meeting, document))
        {
            return Err("Invalid or duplicate document attachment.".into());
        }
    }
    Ok(documents)
}

fn checked_copy(
    input: &mut impl Read,
    output: &mut impl Write,
    document: &Document,
) -> Result<(), String> {
    let mut hasher = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer).map_err(failure)?;
        if read == 0 {
            break;
        }
        count = count
            .checked_add(read as u64)
            .filter(|n| *n <= document.size && *n <= INPUT_BYTES as u64)
            .ok_or("Document original size does not match its metadata.")?;
        hasher.update(&buffer[..read]);
        output.write_all(&buffer[..read]).map_err(failure)?;
    }
    if count != document.size {
        return Err("Document original size does not match its metadata.".into());
    }
    if format!("{:x}", hasher.finalize()) != document.hash.to_ascii_lowercase() {
        return Err("Document original hash does not match its metadata.".into());
    }
    Ok(())
}

fn regular_file(path: &Path) -> Result<File, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(failure)?;
    if !metadata.is_file() || linked(&metadata) {
        return Err("Document original links are unsupported.".into());
    }
    File::open(path).map_err(failure)
}
fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

fn check_root(root: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() && !linked(&metadata) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(failure(error)),
        Ok(_) => Err("Document original directory links are unsupported.".into()),
    }
}

pub(super) fn write_originals<W: Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    documents: &BTreeMap<String, Document>,
    root: Option<&Path>,
    options: SimpleFileOptions,
    files: &mut usize,
    total: &mut u64,
) -> Result<(), String> {
    for document in documents.values() {
        let root = root.ok_or("Reference document originals are unavailable.")?;
        check_root(root)?;
        let mut input = regular_file(&root.join(&document.storage_name))?;
        *files += 1;
        *total = total
            .checked_add(document.size)
            .ok_or("Archive is too large.")?;
        if *files > MAX_FILES || *total > MAX_BYTES {
            return Err("Archive exceeds 100 GiB or 100,000 files.".into());
        }
        zip.start_file(document.relative(), options)
            .map_err(failure)?;
        checked_copy(&mut input, &mut *zip, document)?;
    }
    Ok(())
}
pub(super) fn verify_zip<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    documents: &BTreeMap<String, Document>,
) -> Result<(), String> {
    for document in documents.values() {
        let mut input = zip.by_name(&document.relative()).map_err(failure)?;
        if input.size() != document.size {
            return Err("Document original size does not match its metadata.".into());
        }
        checked_copy(&mut input, &mut std::io::sink(), document)?;
    }
    Ok(())
}
pub(super) fn extract_originals<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    documents: &BTreeMap<String, Document>,
    stage: &Path,
) -> Result<(), String> {
    if documents.is_empty() {
        return Ok(());
    }
    let root = stage.join("reference-documents");
    std::fs::create_dir(&root).map_err(failure)?;
    for document in documents.values() {
        let mut input = zip.by_name(&document.relative()).map_err(failure)?;
        let mut output = File::create(root.join(&document.storage_name)).map_err(failure)?;
        checked_copy(&mut input, &mut output, document)?;
        output.sync_all().map_err(failure)?;
    }
    Ok(())
}

pub(super) struct Restore {
    pub identities: BTreeMap<String, String>,
    pending: Vec<(PathBuf, PathBuf, Document)>,
}
pub(super) struct Published {
    paths: Vec<PathBuf>,
    kept: bool,
}
impl Published {
    pub fn count(&self) -> usize {
        self.paths.len()
    }
    pub fn keep(mut self) {
        self.kept = true;
    }
}
impl Drop for Published {
    fn drop(&mut self) {
        if !self.kept {
            for path in &self.paths {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}
impl Restore {
    pub fn publish(self) -> Result<Published, String> {
        let mut published = Published {
            paths: Vec::new(),
            kept: false,
        };
        for (source, destination, document) in self.pending {
            let root = destination
                .parent()
                .ok_or("Invalid document restore destination.")?;
            check_root(root)?;
            std::fs::create_dir_all(root).map_err(failure)?;
            if linked(&std::fs::symlink_metadata(root).map_err(failure)?) {
                return Err("Document destination links are unsupported.".into());
            }
            let mut file = tempfile::NamedTempFile::new_in(root).map_err(failure)?;
            checked_copy(&mut regular_file(&source)?, file.as_file_mut(), &document)?;
            file.as_file().sync_all().map_err(failure)?;
            // Atomic no-overwrite publication. Cleanup owns only these new files.
            file.persist_noclobber(&destination).map_err(failure)?;
            published.paths.push(destination);
        }
        Ok(published)
    }
}

async fn same_blocks(
    connection: &mut sqlx::SqliteConnection,
    manifest: &Manifest,
    old: &str,
    existing: &str,
) -> Result<bool, String> {
    let blocks = rows(manifest, "knowledge_document_blocks")
        .iter()
        .filter(|row| row["document_id"] == old)
        .collect::<Vec<_>>();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM knowledge_document_blocks WHERE document_id=?")
            .bind(existing)
            .fetch_one(&mut *connection)
            .await
            .map_err(failure)?;
    if count != blocks.len() as i64 {
        return Ok(false);
    }
    for block in blocks {
        let equal:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM knowledge_document_blocks WHERE document_id=? AND ordinal=? AND page IS ? AND paragraph=? AND text=?)")
            .bind(existing).bind(block["ordinal"].as_i64()).bind(block["page"].as_i64())
            .bind(block["paragraph"].as_i64()).bind(block["text"].as_str())
            .fetch_one(&mut *connection).await.map_err(failure)?;
        if !equal {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) async fn restore_rows(
    connection: &mut sqlx::SqliteConnection,
    manifest: &Manifest,
    documents: &BTreeMap<String, Document>,
    selected: &HashSet<String>,
    stage: &Path,
    root: Option<&Path>,
) -> Result<Restore, String> {
    let mut restore = Restore {
        identities: BTreeMap::new(),
        pending: Vec::new(),
    };
    if !documents.is_empty() {
        let root = root
            .ok_or("Reference document originals are unavailable.")?
            .to_path_buf();
        tokio::task::spawn_blocking(move || check_root(&root))
            .await
            .map_err(failure)??;
    }
    for source in rows(manifest, "knowledge_documents") {
        let old = text(source, "id")?;
        let document = &documents[old];
        let root = root.ok_or("Reference document originals are unavailable.")?;
        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM knowledge_documents WHERE sha256=? AND format=?")
                .bind(&document.hash)
                .bind(&document.format)
                .fetch_optional(&mut *connection)
                .await
                .map_err(failure)?;
        let (id, create) = if let Some(id) = existing {
            if !same_blocks(connection, manifest, old, &id).await? {
                return Err("An existing document with the same hash has different canonical text. Restore into a separate library.".into());
            }
            (id, false)
        } else {
            let collision: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM knowledge_sources WHERE id=?)")
                    .bind(format!("document:{old}"))
                    .fetch_one(&mut *connection)
                    .await
                    .map_err(failure)?;
            let proposed = root.join(&document.storage_name);
            let occupied =
                tokio::task::spawn_blocking(move || match std::fs::symlink_metadata(proposed) {
                    Ok(_) => Ok(true),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                    Err(error) => Err(failure(error)),
                })
                .await
                .map_err(failure)??;
            (
                if collision || occupied {
                    uuid::Uuid::new_v4().to_string()
                } else {
                    old.to_string()
                },
                true,
            )
        };
        let storage = format!("{id}.{}", document.format);
        let original = root.join(&storage);
        let check_path = original.clone();
        let size = document.size;
        let hash = document.hash.clone();
        let present = tokio::task::spawn_blocking(move || -> Result<bool, String> {
            match std::fs::symlink_metadata(&check_path) {
                Ok(_) => {
                    let expected = Document {
                        format: String::new(),
                        storage_name: String::new(),
                        size,
                        hash,
                    };
                    checked_copy(
                        &mut regular_file(&check_path)?,
                        &mut std::io::sink(),
                        &expected,
                    )?;
                    Ok(true)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(failure(error)),
            }
        })
        .await
        .map_err(failure)??;
        if !present {
            restore.pending.push((
                stage
                    .join("reference-documents")
                    .join(&document.storage_name),
                original,
                Document {
                    format: document.format.clone(),
                    storage_name: storage.clone(),
                    size: document.size,
                    hash: document.hash.clone(),
                },
            ));
        }
        if create {
            sqlx::query("INSERT INTO knowledge_sources(id,kind) VALUES (?,'document')")
                .bind(format!("document:{id}"))
                .execute(&mut *connection)
                .await
                .map_err(failure)?;
            let mut row = source.clone();
            row.insert("id".into(), id.clone().into());
            row.insert("source_id".into(), format!("document:{id}").into());
            row.insert("storage_name".into(), storage.into());
            insert_archive_row(connection, "knowledge_documents", &row).await?;
            for block in rows(manifest, "knowledge_document_blocks")
                .iter()
                .filter(|row| row["document_id"] == old)
            {
                let mut row = block.clone();
                row.insert("document_id".into(), id.clone().into());
                row.insert(
                    "id".into(),
                    format!("{id}:{}", block["ordinal"].as_u64().unwrap()).into(),
                );
                insert_archive_row(connection, "knowledge_document_blocks", &row).await?;
            }
        }
        restore.identities.insert(old.into(), id);
    }
    for source in rows(manifest, "knowledge_document_attachments") {
        if !selected.contains(text(source, "meeting_id")?) {
            continue;
        }
        let mut row = source.clone();
        row.insert(
            "document_id".into(),
            restore.identities[text(source, "document_id")?]
                .clone()
                .into(),
        );
        insert_archive_row(connection, "knowledge_document_attachments", &row).await?;
    }
    Ok(restore)
}
