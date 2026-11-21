//! One bounded JSON object per line. Streams are staged on disk so SQLite's
//! snapshot can close before copying recordings, and restore can await inserts.
use super::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};

pub(super) const TABLES: &[&str] = &[
    "transcripts",
    "transcript_chunks",
    "transcript_revisions",
    "transcript_edit_batches",
];
pub(super) const STAGING: &str = "table-records";
const MAX_RECORD: u64 = super::MAX_MANIFEST;

pub(super) fn entry_name(table: &str) -> String {
    format!("tables/{table}.jsonl")
}

pub(super) fn validate_layout(manifest: &Manifest) -> bool {
    if manifest.version < 3 {
        return manifest.table_records.is_empty();
    }
    manifest.table_records.len() == TABLES.len()
        && TABLES.iter().all(|table| {
            manifest.table_records.contains_key(*table) && !manifest.tables.contains_key(*table)
        })
}

pub(super) async fn snapshot(
    tx: &mut sqlx::SqliteConnection,
    path: &Path,
    table: &str,
    select: &str,
    total: &mut u64,
) -> Result<u64, String> {
    let mut file = BufWriter::new(
        tokio::fs::File::create(path.join(table))
            .await
            .map_err(failure)?,
    );
    let mut count = 0;
    // Limit the size before SQLite transfers a single oversized value to Rust.
    let projection = format!(
        "SELECT record_id, CASE WHEN length(CAST(record AS BLOB)) < {MAX_RECORD} THEN record END
         FROM ({select})"
    );
    let first = format!("{projection} ORDER BY record_id LIMIT 1");
    let next = format!("{projection} WHERE record_id > ? ORDER BY record_id LIMIT 1");
    let mut row = sqlx::query_as::<_, (i64, Option<String>)>(&first)
        .fetch_optional(&mut *tx)
        .await
        .map_err(failure)?;
    while let Some((id, record)) = row {
        let record = record.ok_or("A transcript record exceeds the 128 MiB record limit.")?;
        // The legacy writer rejected values that JSON cannot round-trip,
        // including SQLite numeric values outside JSON's representable range.
        serde_json::from_str::<Map<String, Value>>(&record).map_err(failure)?;
        *total = total
            .checked_add(record.len() as u64 + 1)
            .filter(|total| *total <= MAX_BYTES)
            .ok_or("Archive exceeds 100 GiB.")?;
        file.write_all(record.as_bytes()).await.map_err(failure)?;
        file.write_all(b"\n").await.map_err(failure)?;
        count += 1;
        row = sqlx::query_as(&next)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(failure)?;
    }
    file.flush().await.map_err(failure)?;
    Ok(count)
}

pub(super) fn write(
    zip: &mut zip::ZipWriter<&mut File>,
    root: &Path,
    options: SimpleFileOptions,
) -> Result<u64, String> {
    let mut total = 0;
    for table in TABLES {
        let mut file = File::open(root.join(table)).map_err(failure)?;
        let size = file.metadata().map_err(failure)?.len();
        zip.start_file(entry_name(table), options)
            .map_err(failure)?;
        super::copy_entry(&mut file, zip, size)?;
        total += size;
    }
    Ok(total)
}

pub(super) fn extract(
    zip: &mut zip::ZipArchive<File>,
    stage: &Path,
    counts: &BTreeMap<String, u64>,
) -> Result<(), String> {
    if counts.is_empty() {
        return Ok(());
    }
    let root = stage.join(STAGING);
    std::fs::create_dir(&root).map_err(failure)?;
    for table in counts.keys() {
        let mut entry = zip.by_name(&entry_name(table)).map_err(failure)?;
        let size = entry.size();
        let mut output = File::create(root.join(table)).map_err(failure)?;
        super::copy_entry(&mut entry, &mut output, size)?;
    }
    Ok(())
}

pub(super) struct Reader {
    file: BufReader<tokio::fs::File>,
    buffer: Vec<u8>,
    remaining: u64,
}

impl Reader {
    pub(super) async fn open(stage: &Path, table: &str, count: u64) -> Result<Self, String> {
        let file = tokio::fs::File::open(stage.join(STAGING).join(table))
            .await
            .map_err(failure)?;
        Ok(Self {
            file: BufReader::new(file),
            buffer: Vec::new(),
            remaining: count,
        })
    }

    pub(super) async fn next(&mut self) -> Result<Option<Map<String, Value>>, String> {
        self.buffer.clear();
        let size = (&mut self.file)
            .take(MAX_RECORD + 1)
            .read_until(b'\n', &mut self.buffer)
            .await
            .map_err(failure)?;
        match (size, self.remaining) {
            (0, 0) => return Ok(None),
            (0, _) | (_, 0) => return Err("Archive table record count does not match.".into()),
            _ => {}
        }
        if size as u64 > MAX_RECORD || self.buffer.pop() != Some(b'\n') {
            return Err("Invalid or oversized archive table record.".into());
        }
        let row = serde_json::from_slice(&self.buffer).map_err(failure)?;
        self.remaining -= 1;
        Ok(Some(row))
    }
}
