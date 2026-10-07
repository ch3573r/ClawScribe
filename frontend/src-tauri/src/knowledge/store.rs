//! Durable source generations and canonical transcript snapshots.
use super::types::*;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqlitePool};
pub const READ_BYTES: usize = 16 * 1024;
pub const DISPLAY_BYTES: usize = 1024;

/// Selection identity is carried through materialization, never re-inferred from
/// a transcript ID that may have moved to a different owner in the meantime.
#[derive(Debug, Clone, FromRow)]
pub struct SelectedRow {
    pub transcript_id: String,
    pub source_id: String,
    pub meeting_id: String,
    pub revision: i64,
    pub generation: i64,
}
impl SelectedRow {
    pub fn for_job(job: &SourceJob, id: String) -> Self {
        Self {
            transcript_id: id,
            source_id: job.source_id.clone(),
            meeting_id: job.meeting_id.clone(),
            revision: job.revision,
            generation: job.generation,
        }
    }
}
#[derive(FromRow)]
struct Snapshot {
    rowid: i64,
    meeting_rowid: i64,
    speaker_null: bool,
    words_null: bool,
    audio_start_time: Option<f64>,
    audio_end_time: Option<f64>,
    duration: Option<f64>,
}
static READERS: once_cell::sync::Lazy<std::sync::Arc<tokio::sync::Semaphore>> =
    once_cell::sync::Lazy::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(1)));
#[cfg(test)]
static SETUP_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
async fn observe_pending<F: std::future::Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| {
        let state = future.as_mut().poll(cx);
        if state.is_pending() {
            SETUP_PENDING.store(true, std::sync::atomic::Ordering::Release);
        }
        state
    })
    .await
}
struct CancelRead(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Drop for CancelRead {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}
struct ReadContext {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    background: bool,
}
impl ReadContext {
    /// Setup has no application-owned raw SQLite resource yet. Dropping its
    /// future follows SQLx's cancellation/transaction rollback paths. Poll the
    /// shared cancellation/foreground state even while SQLx cannot make progress.
    async fn setup<T>(
        &self,
        future: impl std::future::Future<Output = Result<T, sqlx::Error>>,
    ) -> Result<T, KnowledgeError> {
        let mut future = std::pin::pin!(future);
        loop {
            self.check()?;
            tokio::select! {
                biased;
                _ = tokio::time::sleep(std::time::Duration::from_millis(5)) => {}
                result = &mut future => {
                    self.check()?;
                    return result.map_err(Into::into);
                }
            }
        }
    }
    fn check(&self) -> Result<(), KnowledgeError> {
        if self.cancelled.load(std::sync::atomic::Ordering::Acquire)
            || (self.background && super::scheduler::foreground_waiting())
        {
            Err(KnowledgeError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// The blocking closure owns its lease, transaction, locked handle and read
/// permit. Dropping its async waiter signals cancellation but cannot free a
/// SQLite pointer still in use. No handle survives a model await or a write.
async fn read_snapshot<T: Send + 'static>(
    pool: &SqlitePool,
    selected: SelectedRow,
    background: bool,
    work: impl FnOnce(
            &mut sqlx::sqlite::LockedSqliteHandle<'_>,
            &Snapshot,
            &ReadContext,
        ) -> Result<T, KnowledgeError>
        + Send
        + 'static,
) -> Result<T, KnowledgeError> {
    let permit = READERS
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| KnowledgeError::Cancelled)?;
    let pool = pool.clone();
    let context = ReadContext {
        cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        background,
    };
    let waiter = CancelRead(context.cancelled.clone());
    let runtime = tokio::runtime::Handle::current();
    let result=tokio::task::spawn_blocking(move || {
        let _permit=permit;
        context.check()?;
        runtime.block_on(async move {
            #[cfg(test)]
            let mut tx=context.setup(observe_pending(pool.begin())).await?;
            #[cfg(not(test))]
            let mut tx=context.setup(pool.begin()).await?;
            let row:Snapshot=context.setup(sqlx::query_as("SELECT t.rowid,m.rowid AS meeting_rowid,t.speaker IS NULL AS speaker_null,t.word_timestamps_json IS NULL AS words_null,t.audio_start_time,t.audio_end_time,t.duration FROM transcripts t JOIN knowledge_sources s ON s.meeting_id=t.meeting_id JOIN meetings m ON m.id=t.meeting_id WHERE t.id=? AND t.meeting_id=? AND s.id=? AND s.revision=? AND s.generation=?")
                .bind(&selected.transcript_id).bind(&selected.meeting_id).bind(&selected.source_id).bind(selected.revision).bind(selected.generation).fetch_optional(&mut *tx)).await?.ok_or(KnowledgeError::Superseded)?;
            let result={
                let mut handle=context.setup((&mut *tx).lock_handle()).await?;
                context.check()?;
                // Admission covers only synchronous native reads/hashing. Never
                // retain it during acquisition, identity lookup or handle waits.
                let _priority=if background { Some(super::scheduler::claim_snapshot()?) } else { None };
                context.check()?;
                work(&mut handle,&row,&context)
            };
            // Explicitly finish before handing the result back to a caller that
            // may immediately stage a vector using a one-connection pool.
            match result { Ok(value)=>{tx.commit().await?;Ok(value)},Err(error)=>{tx.rollback().await?;Err(error)} }
        })
    }).await.map_err(|_|KnowledgeError::Storage)?;
    drop(waiter);
    result
}

/// Read-only incremental BLOB handles also support SQLite TEXT columns. This
/// avoids materializing a complete TEXT value even inside SQLite's substr/cast.
struct TextBlob<'a> {
    raw: *mut libsqlite3_sys::sqlite3_blob,
    len: usize,
    #[cfg(test)]
    body: bool,
    _guard: std::marker::PhantomData<&'a mut libsqlite3_sys::sqlite3>,
}
impl<'a> TextBlob<'a> {
    fn open(
        handle: &'a mut sqlx::sqlite::LockedSqliteHandle<'_>,
        table: &std::ffi::CStr,
        column: &std::ffi::CStr,
        rowid: i64,
    ) -> Result<Self, KnowledgeError> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: SQLx's locked handle excludes its worker for this entire
        // borrow. Static identifiers and read-only access are used. The BLOB
        // cannot outlive the handle borrow and closes exactly once in Drop.
        let status = unsafe {
            libsqlite3_sys::sqlite3_blob_open(
                handle.as_raw_handle().as_ptr(),
                c"main".as_ptr(),
                table.as_ptr(),
                column.as_ptr(),
                rowid,
                0,
                &mut raw,
            )
        };
        if status != libsqlite3_sys::SQLITE_OK {
            return Err(KnowledgeError::Storage);
        }
        let len = unsafe { libsqlite3_sys::sqlite3_blob_bytes(raw) } as usize;
        Ok(Self {
            raw,
            len,
            #[cfg(test)]
            body: column == c"transcript",
            _guard: std::marker::PhantomData,
        })
    }
    fn window(
        &self,
        start: usize,
        max: usize,
        align_start: bool,
        context: &ReadContext,
    ) -> Result<(usize, String), KnowledgeError> {
        context.check()?;
        if start > self.len || max > READ_BYTES {
            return Err(KnowledgeError::InvalidInput);
        }
        let mut bytes = vec![0u8; max.min(self.len - start)];
        if !bytes.is_empty() {
            let status = unsafe {
                libsqlite3_sys::sqlite3_blob_read(
                    self.raw,
                    bytes.as_mut_ptr().cast(),
                    bytes.len() as i32,
                    start as i32,
                )
            };
            if status != libsqlite3_sys::SQLITE_OK {
                return Err(KnowledgeError::Storage);
            }
        }
        #[cfg(test)]
        if self.body {
            BODY_PEAK.fetch_max(bytes.len(), std::sync::atomic::Ordering::Relaxed);
        }
        #[cfg(test)]
        if !self.body {
            METADATA_PEAK.fetch_max(bytes.len(), std::sync::atomic::Ordering::Relaxed);
        }
        let skipped = if align_start {
            bytes
                .iter()
                .take_while(|byte| (**byte & 0xc0) == 0x80)
                .count()
        } else {
            0
        };
        if skipped > 3 {
            return Err(KnowledgeError::InvalidInput);
        }
        let valid = match std::str::from_utf8(&bytes[skipped..]) {
            Ok(_) => bytes.len() - skipped,
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => return Err(KnowledgeError::InvalidInput),
        };
        bytes.truncate(skipped + valid);
        if skipped > 0 {
            bytes.drain(..skipped);
        }
        Ok((
            start + skipped,
            String::from_utf8(bytes).map_err(|_| KnowledgeError::InvalidInput)?,
        ))
    }
}
impl Drop for TextBlob<'_> {
    fn drop(&mut self) {
        unsafe {
            libsqlite3_sys::sqlite3_blob_close(self.raw);
        }
    }
}

pub async fn row_ids_page(
    pool: &SqlitePool,
    job: &SourceJob,
    after: Option<&str>,
) -> Result<Vec<String>, KnowledgeError> {
    if !current(pool, job).await? {
        return Err(KnowledgeError::Superseded);
    }
    Ok(sqlx::query_scalar("SELECT id FROM transcripts WHERE meeting_id=? AND (? IS NULL OR (timestamp,id)>(SELECT timestamp,id FROM transcripts WHERE id=?)) ORDER BY timestamp,id LIMIT 32").bind(&job.meeting_id).bind(after).bind(after).fetch_all(pool).await?)
}
pub async fn body_window(
    pool: &SqlitePool,
    selected: &SelectedRow,
    start: usize,
) -> Result<(String, usize), KnowledgeError> {
    read_snapshot(pool, selected.clone(), true, move |handle, row, context| {
        let blob = TextBlob::open(handle, c"transcripts", c"transcript", row.rowid)?;
        let (_, text) = blob.window(start, READ_BYTES, false, context)?;
        Ok((text, blob.len))
    })
    .await
}
pub struct LocatedHit {
    pub start: usize,
    pub end: usize,
    pub lexical: TextSpan,
}
pub async fn locate(
    pool: &SqlitePool,
    selected: &SelectedRow,
    query: String,
    terms: Vec<String>,
    raw_terms: Vec<String>,
) -> Result<Option<LocatedHit>, KnowledgeError> {
    let id = selected.transcript_id.clone();
    read_snapshot(
        pool,
        selected.clone(),
        false,
        move |handle, row, context| {
            let hit = {
                let body = TextBlob::open(handle, c"transcripts", c"transcript", row.rowid)?;
                scan(&body, &query, &terms, context)?
            };
            let hit = if hit.is_some() {
                hit
            } else if !row.speaker_null {
                let speaker_hit = {
                    let speaker = TextBlob::open(handle, c"transcripts", c"speaker", row.rowid)?;
                    scan(&speaker, &query, &raw_terms, context)?.is_some()
                };
                if speaker_hit {
                    let body = TextBlob::open(handle, c"transcripts", c"transcript", row.rowid)?;
                    let (_, text) =
                        body.window(0, super::chunking::LEXICAL_BYTES, false, context)?;
                    Some((0, text.len()))
                } else {
                    None
                }
            } else {
                None
            };
            let Some((start, end)) = hit else {
                return Ok(None);
            };
            let body = TextBlob::open(handle, c"transcripts", c"transcript", row.rowid)?;
            let begin = if body.len <= super::chunking::LEXICAL_BYTES {
                0
            } else {
                start.saturating_sub(super::chunking::LEXICAL_BYTES.saturating_sub(end - start) / 2)
            };
            let (begin, text) =
                body.window(begin, super::chunking::LEXICAL_BYTES, true, context)?;
            Ok(Some(LocatedHit {
                start,
                end,
                lexical: TextSpan {
                    transcript_id: id,
                    start_byte: begin,
                    end_byte: begin + text.len(),
                },
            }))
        },
    )
    .await
}
fn scan(
    blob: &TextBlob<'_>,
    query: &str,
    terms: &[String],
    context: &ReadContext,
) -> Result<Option<(usize, usize)>, KnowledgeError> {
    let mut terms = terms.to_vec();
    terms.sort_by_key(|term| std::cmp::Reverse(term.len()));
    let mut patterns = vec![query.trim().to_lowercase()];
    patterns.extend(terms.into_iter().map(|term| term.to_lowercase()));
    let mut best: Option<(usize, usize, usize)> = None;
    let mut start = 0;
    while start < blob.len {
        let (_, text) = blob.window(start, READ_BYTES, false, context)?;
        if text.is_empty() {
            break;
        }
        let mut folded = String::with_capacity(text.len() * 3);
        let mut map = Vec::with_capacity(text.len() * 3);
        for (offset, ch) in text.char_indices() {
            for lower in ch.to_lowercase() {
                folded.push(lower);
                for _ in 0..lower.len_utf8() {
                    map.push((start + offset, start + offset + ch.len_utf8()));
                }
            }
        }
        #[cfg(test)]
        super::retrieval::MAPPING_PEAK.fetch_max(
            map.capacity() * std::mem::size_of::<(usize, usize)>(),
            std::sync::atomic::Ordering::Relaxed,
        );
        for (rank, pattern) in patterns.iter().enumerate() {
            if pattern.is_empty() {
                continue;
            }
            if let Some(offset) = folded.find(pattern) {
                if best.is_none_or(|found| rank < found.0) {
                    best = Some((rank, map[offset].0, map[offset + pattern.len() - 1].1));
                }
            }
        }
        if best.is_some_and(|hit| hit.0 == 0) || start + text.len() == blob.len {
            break;
        }
        let mut advance = text.len().saturating_sub(4096);
        while !text.is_char_boundary(advance) {
            advance += 1;
        }
        if advance == 0 {
            return Err(KnowledgeError::InvalidInput);
        }
        start += advance;
    }
    Ok(best.map(|(_, start, end)| (start, end)))
}
struct HashWriter<'a>(&'a mut Sha256);
impl std::io::Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn hash_value(hash: &mut Sha256, value: &impl serde::Serialize) -> Result<(), KnowledgeError> {
    serde_json::to_writer(HashWriter(hash), value).map_err(|_| KnowledgeError::InvalidInput)
}
fn hash_text(
    hash: &mut Sha256,
    blob: &TextBlob<'_>,
    context: &ReadContext,
) -> Result<(), KnowledgeError> {
    hash.update(b"\"");
    let mut start = 0;
    while start < blob.len {
        let (_, text) = blob.window(start, READ_BYTES, false, context)?;
        if text.is_empty() {
            return Err(KnowledgeError::InvalidInput);
        }
        start += text.len();
        let bytes = text.as_bytes();
        let mut begin = 0;
        for (index, byte) in bytes.iter().copied().enumerate() {
            if byte == b'"' || byte == b'\\' || byte < 32 {
                hash.update(&bytes[begin..index]);
                match byte {
                    b'"' => hash.update(b"\\\""),
                    b'\\' => hash.update(b"\\\\"),
                    8 => hash.update(b"\\b"),
                    12 => hash.update(b"\\f"),
                    10 => hash.update(b"\\n"),
                    13 => hash.update(b"\\r"),
                    9 => hash.update(b"\\t"),
                    _ => {
                        const HEX: &[u8] = b"0123456789abcdef";
                        hash.update([
                            b'\\',
                            b'u',
                            b'0',
                            b'0',
                            HEX[(byte >> 4) as usize],
                            HEX[(byte & 15) as usize],
                        ]);
                    }
                }
                begin = index + 1;
            }
        }
        hash.update(&bytes[begin..]);
    }
    hash.update(b"\"");
    Ok(())
}
fn display(
    handle: &mut sqlx::sqlite::LockedSqliteHandle<'_>,
    table: &std::ffi::CStr,
    column: &std::ffi::CStr,
    rowid: i64,
    context: &ReadContext,
) -> Result<(String, bool), KnowledgeError> {
    let blob = TextBlob::open(handle, table, column, rowid)?;
    let (_, text) = blob.window(0, DISPLAY_BYTES, false, context)?;
    let clipped = text.len() < blob.len;
    Ok((text, clipped))
}
pub async fn materialize(
    pool: &SqlitePool,
    selected: &SelectedRow,
    span: TextSpan,
    background: bool,
) -> Result<Passage, KnowledgeError> {
    let selected = selected.clone();
    let pinned = selected.clone();
    read_snapshot(pool, pinned, background, move |handle, row, context| {
        if span.transcript_id != selected.transcript_id
            || span.end_byte < span.start_byte
            || span.end_byte - span.start_byte > READ_BYTES
        {
            return Err(KnowledgeError::InvalidInput);
        }
        let text = {
            let blob = TextBlob::open(handle, c"transcripts", c"transcript", row.rowid)?;
            let (_, text) = blob.window(
                span.start_byte,
                span.end_byte - span.start_byte,
                false,
                context,
            )?;
            if text.len() != span.end_byte - span.start_byte {
                return Err(KnowledgeError::Superseded);
            }
            text
        };
        let mut hash = Sha256::new();
        hash.update(b"[");
        hash_value(&mut hash, &"canonical-transcript-v1")?;
        hash.update(b",");
        hash_value(&mut hash, &selected.transcript_id)?;
        hash.update(b",");
        hash_value(&mut hash, &selected.meeting_id)?;
        hash.update(b",");
        hash_value(&mut hash, &span)?;
        hash.update(b",");
        hash_value(&mut hash, &text)?;
        hash.update(b",");
        if row.speaker_null {
            hash.update(b"null");
        } else {
            let blob = TextBlob::open(handle, c"transcripts", c"speaker", row.rowid)?;
            hash_text(&mut hash, &blob, context)?;
        }
        hash.update(b",");
        {
            let blob = TextBlob::open(handle, c"transcripts", c"timestamp", row.rowid)?;
            hash_text(&mut hash, &blob, context)?;
        }
        hash.update(b",");
        hash_value(&mut hash, &row.audio_start_time)?;
        hash.update(b",");
        hash_value(&mut hash, &row.audio_end_time)?;
        hash.update(b",");
        hash_value(&mut hash, &row.duration)?;
        hash.update(b",");
        if row.words_null {
            hash.update(b"null");
        } else {
            let blob = TextBlob::open(handle, c"transcripts", c"word_timestamps_json", row.rowid)?;
            hash_text(&mut hash, &blob, context)?;
        }
        hash.update(b"]");
        let fingerprint = format!("{:x}", hash.finalize());
        let identity = serde_json::to_vec(&(
            "evidence-v1",
            &selected.source_id,
            selected.revision,
            &span,
            &fingerprint,
        ))
        .map_err(|_| KnowledgeError::InvalidInput)?;
        let (title, title_clipped) =
            display(handle, c"meetings", c"title", row.meeting_rowid, context)?;
        let (date, date_clipped) = display(
            handle,
            c"meetings",
            c"created_at",
            row.meeting_rowid,
            context,
        )?;
        let (speaker, speaker_clipped) = if row.speaker_null {
            (None, false)
        } else {
            let (value, clipped) = display(handle, c"transcripts", c"speaker", row.rowid, context)?;
            (Some(value), clipped)
        };
        Ok(Passage {
            evidence: EvidenceRef {
                historical: false,
                source_id: selected.source_id,
                source_revision: selected.revision,
                chunk_id: format!("{:x}", Sha256::digest(identity)),
                fingerprint,
                locator: EvidenceLocator::Transcript {
                    meeting_id: selected.meeting_id.clone(),
                    transcript_ids: vec![selected.transcript_id],
                    spans: vec![span],
                    start_seconds: row.audio_start_time,
                },
            },
            meeting_id: selected.meeting_id,
            title,
            date,
            speaker,
            metadata_truncated: title_clipped || date_clipped || speaker_clipped,
            text,
            rank: 0.,
        })
    })
    .await
}
#[cfg(test)]
pub(crate) static BODY_PEAK: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub(crate) static METADATA_PEAK: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub(crate) fn track_row(row: &CanonicalRow) {
    use std::sync::atomic::Ordering;
    BODY_PEAK.fetch_max(row.transcript.len(), Ordering::Relaxed);
    METADATA_PEAK.fetch_max(
        row.speaker.as_ref().map_or(0, String::len)
            + row.word_timestamps_json.as_ref().map_or(0, String::len),
        Ordering::Relaxed,
    );
}

#[derive(Debug, Clone, FromRow)]
pub struct SourceJob {
    pub source_id: String,
    pub meeting_id: String,
    pub revision: i64,
    pub generation: i64,
}
#[derive(Debug, Clone, FromRow)]
pub struct CanonicalRow {
    pub id: String,
    pub meeting_id: String,
    pub transcript: String,
    pub speaker: Option<String>,
    pub timestamp: String,
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
    pub duration: Option<f64>,
    pub word_timestamps_json: Option<String>,
}
pub fn fingerprint(row: &CanonicalRow, span: &TextSpan) -> Result<String, KnowledgeError> {
    let text = row
        .transcript
        .get(span.start_byte..span.end_byte)
        .ok_or(KnowledgeError::InvalidInput)?;
    if span.transcript_id != row.id {
        return Err(KnowledgeError::InvalidInput);
    }
    let bytes = serde_json::to_vec(&(
        "canonical-transcript-v1",
        &row.id,
        &row.meeting_id,
        span,
        text,
        &row.speaker,
        &row.timestamp,
        row.audio_start_time,
        row.audio_end_time,
        row.duration,
        &row.word_timestamps_json,
    ))
    .map_err(|_| KnowledgeError::InvalidInput)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
pub fn evidence(
    job: &SourceJob,
    row: &CanonicalRow,
    span: &TextSpan,
) -> Result<EvidenceRef, KnowledgeError> {
    if job.meeting_id != row.meeting_id || job.source_id != format!("meeting:{}", row.meeting_id) {
        return Err(KnowledgeError::InvalidInput);
    }
    let fingerprint = fingerprint(row, span)?;
    let identity = serde_json::to_vec(&(
        "evidence-v1",
        &job.source_id,
        job.revision,
        span,
        &fingerprint,
    ))
    .map_err(|_| KnowledgeError::InvalidInput)?;
    Ok(EvidenceRef {
        historical: false,
        source_id: job.source_id.clone(),
        source_revision: job.revision,
        chunk_id: format!("{:x}", Sha256::digest(identity)),
        fingerprint,
        locator: EvidenceLocator::Transcript {
            meeting_id: row.meeting_id.clone(),
            transcript_ids: vec![row.id.clone()],
            spans: vec![span.clone()],
            start_seconds: row.audio_start_time,
        },
    })
}
pub async fn next_job(pool: &SqlitePool) -> Result<Option<SourceJob>, KnowledgeError> {
    Ok(sqlx::query_as("SELECT j.source_id,s.meeting_id,j.revision,j.generation FROM knowledge_index_jobs j JOIN knowledge_sources s ON s.id=j.source_id AND s.revision=j.revision AND s.generation=j.generation WHERE s.kind='meeting' AND j.attempts<3 AND j.paused=0 ORDER BY j.attempts,j.source_id LIMIT 1").fetch_optional(pool).await?)
}
pub async fn current(pool: &SqlitePool, job: &SourceJob) -> Result<bool, KnowledgeError> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM knowledge_sources s JOIN knowledge_index_jobs j ON j.source_id=s.id WHERE s.id=? AND s.revision=? AND s.generation=? AND j.generation=s.generation AND j.paused=0").bind(&job.source_id).bind(job.revision).bind(job.generation).fetch_one(pool).await? == 1)
}
#[cfg(test)]
pub async fn rows_page(
    pool: &SqlitePool,
    job: &SourceJob,
    after: Option<(&str, &str)>,
) -> Result<Vec<CanonicalRow>, KnowledgeError> {
    if !current(pool, job).await? {
        return Err(KnowledgeError::Superseded);
    }
    let (timestamp, id) = after.unwrap_or(("", ""));
    let rows=sqlx::query_as("SELECT id,meeting_id,transcript,speaker,timestamp,audio_start_time,audio_end_time,duration,word_timestamps_json FROM transcripts WHERE meeting_id=? AND (timestamp>? OR (timestamp=? AND id>?)) ORDER BY timestamp,id LIMIT 32").bind(&job.meeting_id).bind(timestamp).bind(timestamp).bind(id).fetch_all(pool).await?;
    #[cfg(test)]
    for row in &rows {
        track_row(row);
    }
    Ok(rows)
}
#[cfg(test)]
pub async fn stage(
    pool: &SqlitePool,
    job: &SourceJob,
    row: &CanonicalRow,
    span: &TextSpan,
    ordinal: i64,
    vector: &[f32],
    space: &str,
) -> Result<(), KnowledgeError> {
    let reference = evidence(job, row, span)?;
    stage_evidence(
        pool,
        job,
        &reference,
        &row.transcript[span.start_byte..span.end_byte],
        ordinal,
        vector,
        space,
    )
    .await
}
pub async fn stage_evidence(
    pool: &SqlitePool,
    job: &SourceJob,
    reference: &EvidenceRef,
    text: &str,
    ordinal: i64,
    vector: &[f32],
    space: &str,
) -> Result<(), KnowledgeError> {
    let EvidenceLocator::Transcript { spans, .. } = &reference.locator else {
        return Err(KnowledgeError::InvalidInput);
    };
    if spans.len() != 1
        || reference.source_id != job.source_id
        || reference.source_revision != job.revision
    {
        return Err(KnowledgeError::InvalidInput);
    }
    let span = &spans[0];
    let vector = super::embedding::normalize(vector.to_vec())?;
    let mut tx = pool.begin().await?;
    // This conditional write obtains SQLite's writer lock before checking the
    // generation; later commands cannot dirty or replace it during publication.
    let guarded=sqlx::query("UPDATE knowledge_index_jobs SET failure=failure WHERE source_id=? AND revision=? AND generation=? AND paused=0 AND EXISTS(SELECT 1 FROM knowledge_sources s WHERE s.id=source_id AND s.revision=? AND s.generation=?)").bind(&job.source_id).bind(job.revision).bind(job.generation).bind(job.revision).bind(job.generation).execute(&mut *tx).await?;
    if guarded.rows_affected() != 1 {
        return Err(KnowledgeError::Superseded);
    }
    sqlx::query("INSERT INTO knowledge_chunks(id,source_id,revision,generation,ordinal,transcript_id,start_byte,end_byte,fingerprint,text) VALUES(?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET generation=excluded.generation,ordinal=excluded.ordinal")
        .bind(&reference.chunk_id).bind(&job.source_id).bind(job.revision).bind(job.generation).bind(ordinal).bind(&span.transcript_id).bind(span.start_byte as i64).bind(span.end_byte as i64).bind(&reference.fingerprint).bind(text).execute(&mut *tx).await?;
    let bytes: Vec<u8> = vector.iter().flat_map(|v| v.to_le_bytes()).collect();
    sqlx::query("INSERT INTO knowledge_vectors(chunk_id,space,dimensions,vector) VALUES(?,?,384,?) ON CONFLICT(chunk_id) DO UPDATE SET space=excluded.space,vector=excluded.vector")
        .bind(&reference.chunk_id).bind(space).bind(bytes).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn publish(
    pool: &SqlitePool,
    job: &SourceJob,
    space: &str,
) -> Result<(), KnowledgeError> {
    let mut tx = pool.begin().await?;
    let changed=sqlx::query("UPDATE knowledge_sources SET semantic_revision=revision,semantic_space=? WHERE id=? AND revision=? AND generation=? AND EXISTS(SELECT 1 FROM knowledge_index_jobs j WHERE j.source_id=knowledge_sources.id AND j.generation=knowledge_sources.generation AND j.paused=0)")
        .bind(space).bind(&job.source_id).bind(job.revision).bind(job.generation).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(KnowledgeError::Superseded);
    }
    sqlx::query("DELETE FROM knowledge_chunks WHERE source_id=? AND generation!=?")
        .bind(&job.source_id)
        .bind(job.generation)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "DELETE FROM knowledge_index_jobs WHERE source_id=? AND revision=? AND generation=?",
    )
    .bind(&job.source_id)
    .bind(job.revision)
    .bind(job.generation)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
pub async fn record_failure(
    pool: &SqlitePool,
    job: &SourceJob,
    error: &KnowledgeError,
) -> Result<(), KnowledgeError> {
    let category = match error {
        KnowledgeError::Busy
        | KnowledgeError::Disabled
        | KnowledgeError::Cancelled
        | KnowledgeError::Superseded => return Ok(()),
        KnowledgeError::ModelUnavailable => "model_unavailable",
        KnowledgeError::Storage => "storage",
        _ => "indexing",
    };
    sqlx::query("UPDATE knowledge_index_jobs SET attempts=MIN(attempts+1,3),failure=? WHERE source_id=? AND revision=? AND generation=?").bind(category).bind(&job.source_id).bind(job.revision).bind(job.generation).execute(pool).await?;
    Ok(())
}

pub async fn requeue(pool: &SqlitePool, ids: &[String]) -> Result<(), KnowledgeError> {
    let ids = serde_json::to_string(ids).map_err(|_| KnowledgeError::InvalidInput)?;
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE knowledge_sources SET generation=generation+1,semantic_revision=NULL,semantic_space=NULL WHERE meeting_id IN(SELECT value FROM json_each(?))").bind(&ids).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO knowledge_index_jobs(source_id,revision,generation) SELECT id,revision,generation FROM knowledge_sources WHERE meeting_id IN(SELECT value FROM json_each(?)) ON CONFLICT(source_id) DO UPDATE SET revision=excluded.revision,generation=excluded.generation,attempts=0,failure=NULL,paused=0").bind(ids).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn invalidate_other_spaces(pool: &SqlitePool, space: &str) -> Result<(), KnowledgeError> {
    let ids:Vec<String>=sqlx::query_scalar("SELECT meeting_id FROM knowledge_sources WHERE kind='meeting' AND semantic_space IS NOT NULL AND semantic_space!=?").bind(space).fetch_all(pool).await?;
    if !ids.is_empty() {
        requeue(pool, &ids).await?;
    }
    Ok(())
}
pub async fn status(
    pool: &SqlitePool,
    runtime_ready: bool,
    space: &str,
) -> Result<IndexStatus, KnowledgeError> {
    let enabled =
        sqlx::query_scalar::<_, i64>("SELECT enabled FROM knowledge_settings WHERE singleton=1")
            .fetch_one(pool)
            .await?
            == 1;
    let ready:i64=sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_sources WHERE semantic_revision=revision AND semantic_space=?").bind(space).fetch_one(pool).await?;
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_index_jobs WHERE attempts<3 AND paused=0",
    )
    .fetch_one(pool)
    .await?;
    let failed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_index_jobs WHERE attempts>=3")
            .fetch_one(pool)
            .await?;
    let paused: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_index_jobs WHERE paused=1")
            .fetch_one(pool)
            .await?;
    Ok(IndexStatus {
        keyword_ready: true,
        semantic_enabled: enabled,
        semantic_ready: ready as usize,
        pending: pending as usize,
        failed: failed as usize,
        reason: if !enabled {
            Some("disabled".into())
        } else if !runtime_ready {
            Some("model_unavailable".into())
        } else if failed > 0 {
            Some("indexing_failed".into())
        } else if paused > 0 {
            Some("paused".into())
        } else if pending > 0 {
            Some("indexing".into())
        } else {
            None
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn pending_setup_releases_before_connection_return(materialized: bool, preempt: bool) {
        use std::{
            sync::atomic::Ordering,
            time::{Duration, Instant},
        };
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = database().await;
        meeting(&pool, "one").await;
        transcript(&pool).await;
        let selected = SelectedRow::for_job(&next_job(&pool).await.unwrap().unwrap(), "row".into());
        let held = pool.acquire().await.unwrap();
        SETUP_PENDING.store(false, Ordering::Release);
        BODY_PEAK.store(0, Ordering::Relaxed);
        let reader = {
            let pool = pool.clone();
            tokio::spawn(async move {
                if materialized {
                    materialize(
                        &pool,
                        &selected,
                        TextSpan {
                            transcript_id: "row".into(),
                            start_byte: 0,
                            end_byte: 1,
                        },
                        true,
                    )
                    .await
                    .map(|_| ())
                } else {
                    body_window(&pool, &selected, 0).await.map(|_| ())
                }
            })
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while !SETUP_PENDING.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // The pool-acquire future was actually polled Pending while the only
        // lease remains in this test. No inference permit is needed yet.
        let admission_while_waiting = crate::audio::inference::claim_job().is_ok();
        let started = Instant::now();
        let foreground = if preempt {
            Some(super::super::scheduler::ForegroundPriority::enter())
        } else {
            reader.abort();
            None
        };
        let released = tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if let Ok(slot) = READERS.clone().try_acquire_owned() {
                    if let Ok(admission) = crate::audio::inference::claim_job() {
                        drop((slot, admission));
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .is_ok();
        let elapsed = started.elapsed().as_micros();
        let body = BODY_PEAK.load(Ordering::Relaxed);
        println!("KNOWLEDGE_PENDING_SETUP materialized={materialized} foreground_preemption={preempt} released_before_connection={released} elapsed_us={elapsed} bound_ms=500 body_bytes={body} admission_free_while_waiting={admission_while_waiting}");
        // Clean up even on RED; the assertions describe observations taken
        // BEFORE returning the held connection, not eventual SQL timeout.
        drop(held);
        let outcome = tokio::time::timeout(Duration::from_secs(5), reader)
            .await
            .unwrap();
        drop(foreground);
        tokio::time::timeout(Duration::from_secs(5), async {
            drop(READERS.clone().acquire_owned().await.unwrap());
            pool.close().await;
        })
        .await
        .unwrap();
        assert!(released, "setup must release reader and inference admission within 500 ms while the only database connection is still held");
        assert!(
            admission_while_waiting,
            "blocked acquisition must not hold inference admission"
        );
        assert_eq!(body, 0);
        if preempt {
            assert!(matches!(
                outcome.unwrap(),
                Err(KnowledgeError::Cancelled) | Err(KnowledgeError::Busy)
            ));
        } else {
            assert!(outcome.unwrap_err().is_cancelled());
        }
    }
    #[tokio::test]
    async fn aborted_pending_body_read_releases_admission_and_slot() {
        pending_setup_releases_before_connection_return(false, false).await;
    }
    #[tokio::test]
    async fn preempted_pending_body_read_releases_admission_and_slot() {
        pending_setup_releases_before_connection_return(false, true).await;
    }
    #[tokio::test]
    async fn aborted_pending_materialization_releases_admission_and_slot() {
        pending_setup_releases_before_connection_return(true, false).await;
    }
    #[tokio::test]
    async fn preempted_pending_materialization_releases_admission_and_slot() {
        pending_setup_releases_before_connection_return(true, true).await;
    }
    #[tokio::test]
    async fn aborted_blob_reader_keeps_lease_owned_and_releases_for_recreation_and_close() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let pool = database().await;
        meeting(&pool, "one").await;
        transcript(&pool).await;
        let job = next_job(&pool).await.unwrap().unwrap();
        let selected = SelectedRow::for_job(&job, "row".into());
        let entered = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let reader = {
            let pool = pool.clone();
            let selected = selected.clone();
            let entered = entered.clone();
            let closed = closed.clone();
            tokio::spawn(async move {
                read_snapshot(&pool, selected, false, move |handle, row, context| {
                    let blob = TextBlob::open(handle, c"transcripts", c"transcript", row.rowid)?;
                    assert!(blob.len > 0);
                    entered.store(true, Ordering::Release);
                    while !context.cancelled.load(Ordering::Acquire) {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    drop(blob);
                    closed.store(true, Ordering::Release);
                    context.check()
                })
                .await
            })
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !entered.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            pool.try_acquire().is_none(),
            "worker owns the only connection while its BLOB is open"
        );
        reader.abort();
        assert!(reader.await.unwrap_err().is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(5),async {
            sqlx::query("DELETE FROM transcripts WHERE id='row'").execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO transcripts(rowid,id,meeting_id,transcript,timestamp) VALUES(901,'row','one','Recreated BOREAL-731','00:01')").execute(&pool).await.unwrap();
        }).await.unwrap();
        assert!(
            closed.load(Ordering::Acquire),
            "BLOB closes before the connection is reused"
        );
        let span = TextSpan {
            transcript_id: "row".into(),
            start_byte: 0,
            end_byte: 20,
        };
        assert!(matches!(
            materialize(&pool, &selected, span.clone(), false).await,
            Err(KnowledgeError::Superseded)
        ));
        let current = next_job(&pool).await.unwrap().unwrap();
        let selected = SelectedRow::for_job(&current, "row".into());
        let (text, total) = body_window(&pool, &selected, 0).await.unwrap();
        assert_eq!(text, "Recreated BOREAL-731");
        assert_eq!(text.len(), total);
        let passage = materialize(
            &pool,
            &selected,
            TextSpan {
                end_byte: total,
                ..span
            },
            false,
        )
        .await
        .unwrap();
        stage_evidence(
            &pool,
            &current,
            &passage.evidence,
            &passage.text,
            0,
            &vec![1.; 384],
            "test-space",
        )
        .await
        .unwrap();
        let failed = read_snapshot(&pool, selected, false, |handle, row, context| {
            let blob = TextBlob::open(handle, c"transcripts", c"transcript", row.rowid)?;
            blob.window(0, READ_BYTES + 1, false, context)
        })
        .await;
        assert!(matches!(failed, Err(KnowledgeError::InvalidInput)));
        tokio::time::timeout(std::time::Duration::from_secs(5), pool.close())
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn fts_row_identity_survives_gaps_moves_and_recreated_rows() {
        let pool = database().await;
        meeting(&pool, "one").await;
        meeting(&pool, "two").await;
        sqlx::query("INSERT INTO transcripts(rowid,id,meeting_id,transcript,timestamp) VALUES(101,'gap','one','ATLAS-42','00:01'),(900,'keep','one','BOREAL-731','00:02')").execute(&pool).await.unwrap();
        assert_eq!(count(&pool,"SELECT COUNT(*) FROM knowledge_fts f JOIN transcripts t ON t.rowid=f.rowid AND t.id=f.transcript_id").await,2,"FTS must use canonical row identity for bounded point mutations");
        sqlx::query("UPDATE transcripts SET meeting_id='two',transcript='Moved FALKE-908',speaker='Änne' WHERE id='gap'").execute(&pool).await.unwrap();
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_fts WHERE knowledge_fts MATCH '\"ATLAS-42\"'"
            )
            .await,
            0
        );
        let moved:(i64,String,String)=sqlx::query_as("SELECT rowid,transcript_id,source_id FROM knowledge_fts WHERE knowledge_fts MATCH '\"FALKE-908\"'").fetch_one(&pool).await.unwrap();
        assert_eq!(moved, (101, "gap".into(), "meeting:two".into()));
        sqlx::query("DELETE FROM transcripts WHERE id='gap'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO transcripts(rowid,id,meeting_id,transcript,timestamp) VALUES(1500,'gap','one','Recreated XR-6619','00:03')").execute(&pool).await.unwrap();
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_fts WHERE knowledge_fts MATCH '\"FALKE-908\"'"
            )
            .await,
            0
        );
        let recreated:(i64,String,String)=sqlx::query_as("SELECT rowid,transcript_id,source_id FROM knowledge_fts WHERE knowledge_fts MATCH '\"XR-6619\"'").fetch_one(&pool).await.unwrap();
        assert_eq!(recreated, (1500, "gap".into(), "meeting:one".into()));
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_fts WHERE knowledge_fts MATCH '\"BOREAL-731\"'"
            )
            .await,
            1
        );
        sqlx::query("DELETE FROM meetings WHERE id='one'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM knowledge_fts").await, 0);
    }
    #[tokio::test]
    #[ignore = "manual trusted-runner 10000-row FTS mutation measurement"]
    async fn synthetic_fts_mutation_workload() {
        tokio::time::timeout(std::time::Duration::from_secs(120), async {
            let pool=database().await;
            meeting(&pool,"one").await;
            let mut tx=pool.begin().await.unwrap();
            for n in 0..10000 {
                sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES(?,'one',?,'00:01')").bind(format!("row-{n}")).bind(format!("Synthetic identifier ITEM-{n}")).execute(&mut *tx).await.unwrap();
            }
            tx.commit().await.unwrap();
            let started=std::time::Instant::now();
            sqlx::query("UPDATE transcripts SET transcript=transcript || ' corrected',speaker='Änne'").execute(&pool).await.unwrap();
            let update_ms=started.elapsed().as_millis();
            assert_eq!(count(&pool,"SELECT COUNT(*) FROM knowledge_fts WHERE knowledge_fts MATCH 'corrected AND speaker:Änne'").await,10000);
            let started=std::time::Instant::now();
            sqlx::query("DELETE FROM transcripts").execute(&pool).await.unwrap();
            let delete_ms=started.elapsed().as_millis();
            assert_eq!(count(&pool,"SELECT COUNT(*) FROM knowledge_fts").await,0);
            assert_eq!(count(&pool,"SELECT COUNT(*) FROM knowledge_sources").await,1);
            let jobs=count(&pool,"SELECT COUNT(*) FROM knowledge_index_jobs").await;
            assert_eq!(jobs,1);
            println!("KNOWLEDGE_FTS_MUTATIONS rows=10000 update_ms={update_ms} delete_ms={delete_ms} jobs={jobs} database=in_memory");
        }).await.expect("bounded mutation workload deadline");
    }
    #[tokio::test]
    async fn deletion_removes_populated_vectors_with_both_repository_and_fk_order() {
        for transcripts_first in [false, true] {
            let pool = database().await;
            meeting(&pool, "one").await;
            transcript(&pool).await;
            let job = next_job(&pool).await.unwrap().unwrap();
            let row = rows_page(&pool, &job, None).await.unwrap().remove(0);
            let span = TextSpan {
                transcript_id: row.id.clone(),
                start_byte: 0,
                end_byte: row.transcript.len(),
            };
            stage(&pool, &job, &row, &span, 0, &vec![1.; 384], "test-space")
                .await
                .unwrap();
            assert_eq!(
                count(&pool, "SELECT COUNT(*) FROM knowledge_vectors").await,
                1
            );
            assert_eq!(count(&pool, "SELECT COUNT(*) FROM knowledge_fts").await, 1);
            if transcripts_first {
                sqlx::query("DELETE FROM transcripts WHERE meeting_id='one'")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            sqlx::query("DELETE FROM meetings WHERE id='one'")
                .execute(&pool)
                .await
                .unwrap();
            for table in [
                "knowledge_sources",
                "knowledge_chunks",
                "knowledge_vectors",
                "knowledge_index_jobs",
                "knowledge_fts",
            ] {
                assert_eq!(
                    count(&pool, &format!("SELECT COUNT(*) FROM {table}")).await,
                    0,
                    "orphan in {table}"
                );
            }
        }
    }
    #[tokio::test]
    async fn enabled_without_model_is_distinct_from_disabled() {
        let pool = database().await;
        let disabled = status(&pool, false, "test-space").await.unwrap();
        assert!(!disabled.semantic_enabled);
        assert_eq!(disabled.reason.as_deref(), Some("disabled"));
        sqlx::query("UPDATE knowledge_settings SET enabled=1 WHERE singleton=1")
            .execute(&pool)
            .await
            .unwrap();
        let missing = status(&pool, false, "test-space").await.unwrap();
        assert!(
            missing.semantic_enabled,
            "saved opt-in must be visible without a ready runtime"
        );
        assert_eq!(missing.reason.as_deref(), Some("model_unavailable"));
    }
    #[tokio::test]
    async fn reserved_document_source_does_not_enter_meeting_worker() {
        let pool = database().await;
        let insert=sqlx::query("INSERT INTO knowledge_sources(id,kind,meeting_id) VALUES('document:reserved','document',NULL)").execute(&pool).await;
        assert!(insert.is_ok(),"source ownership must support future document migration without rebuilding evidence foreign keys");
        sqlx::query(
            "INSERT INTO knowledge_index_jobs(source_id,revision) VALUES('document:reserved',1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(next_job(&pool).await.unwrap().is_none());
    }
    #[tokio::test]
    async fn model_space_change_requires_reindex_and_supersedes_same_revision_worker() {
        let pool = database().await;
        meeting(&pool, "one").await;
        transcript(&pool).await;
        let old = next_job(&pool).await.unwrap().unwrap();
        publish(&pool, &old, "old-space").await.unwrap();
        invalidate_other_spaces(&pool, "new-space").await.unwrap();
        let new = next_job(&pool).await.unwrap().unwrap();
        assert_eq!(new.revision, old.revision);
        assert!(new.generation > old.generation);
        assert_eq!(
            publish(&pool, &old, "old-space").await,
            Err(KnowledgeError::Superseded)
        );
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_sources WHERE semantic_revision IS NOT NULL"
            )
            .await,
            0
        );
        assert!(current(&pool, &new).await.unwrap());
    }
    #[tokio::test]
    async fn evidence_identity_survives_cache_rebuild_and_tracks_speaker_timing() {
        let pool = database().await;
        meeting(&pool, "one").await;
        transcript(&pool).await;
        let job = next_job(&pool).await.unwrap().unwrap();
        let row = rows_page(&pool, &job, None).await.unwrap().remove(0);
        let span = TextSpan {
            transcript_id: row.id.clone(),
            start_byte: 0,
            end_byte: row.transcript.len(),
        };
        let original = evidence(&job, &row, &span).unwrap();
        requeue(&pool, &["one".into()]).await.unwrap();
        let again = next_job(&pool).await.unwrap().unwrap();
        assert_eq!(original, evidence(&again, &row, &span).unwrap());
        let mut changed = row.clone();
        changed.speaker = Some("Speaker B".into());
        assert_ne!(
            fingerprint(&row, &span).unwrap(),
            fingerprint(&changed, &span).unwrap()
        );
        changed = row.clone();
        changed.audio_start_time = Some(10.);
        assert_ne!(
            fingerprint(&row, &span).unwrap(),
            fingerprint(&changed, &span).unwrap()
        );
    }
    use sqlx::SqlitePool;

    async fn database() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }
    async fn meeting(pool: &SqlitePool, id: &str) {
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES (?, 'Public fixture', '2026-10-07', '2026-10-07')")
            .bind(id).execute(pool).await.unwrap();
    }
    async fn transcript(pool: &SqlitePool) {
        sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES ('row','one','Release ATLAS-42 on Friday','00:01')")
            .execute(pool).await.unwrap();
    }
    async fn count(pool: &SqlitePool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
    }
    #[tokio::test]
    async fn clean_database_seeds_empty_meetings() {
        let pool = database().await;
        meeting(&pool, "one").await;
        let available = count(
            &pool,
            "SELECT COUNT(*) FROM sqlite_master WHERE name='knowledge_sources'",
        )
        .await;
        assert_eq!(available, 1, "canonical source generations must exist");
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM knowledge_sources").await,
            1
        );
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM knowledge_index_jobs").await,
            1
        );
    }
    #[tokio::test]
    async fn edited_source_cannot_publish_old_generation() {
        let pool = database().await;
        meeting(&pool, "one").await;
        transcript(&pool).await;
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='knowledge_sources'"
            )
            .await,
            1,
            "mutation generations are absent"
        );
        let old = count(
            &pool,
            "SELECT revision FROM knowledge_sources WHERE meeting_id='one'",
        )
        .await;
        sqlx::query("UPDATE knowledge_sources SET semantic_revision=revision, semantic_space='fixture' WHERE meeting_id='one'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE knowledge_index_jobs SET attempts=3")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE transcripts SET speaker='Speaker B', audio_start_time=3 WHERE id='row'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            count(
                &pool,
                "SELECT revision FROM knowledge_sources WHERE meeting_id='one'"
            )
            .await,
            old + 1
        );
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_sources WHERE semantic_revision IS NOT NULL"
            )
            .await,
            0
        );
        assert_eq!(
            count(&pool, "SELECT attempts FROM knowledge_index_jobs").await,
            0
        );
        let stale = sqlx::query("UPDATE knowledge_sources SET semantic_revision=? WHERE meeting_id='one' AND revision=?").bind(old).bind(old).execute(&pool).await.unwrap();
        assert_eq!(stale.rows_affected(), 0);
    }
    #[tokio::test]
    async fn transcript_move_invalidates_both_owners() {
        let pool = database().await;
        meeting(&pool, "one").await;
        meeting(&pool, "two").await;
        transcript(&pool).await;
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='knowledge_sources'"
            )
            .await,
            1,
            "move generations are absent"
        );
        let old = count(&pool, "SELECT SUM(revision) FROM knowledge_sources").await;
        sqlx::query("UPDATE transcripts SET meeting_id='two' WHERE id='row'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            count(&pool, "SELECT SUM(revision) FROM knowledge_sources").await,
            old + 2
        );
    }
    #[tokio::test]
    async fn delete_cascades_index() {
        let pool = database().await;
        meeting(&pool, "one").await;
        transcript(&pool).await;
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='knowledge_sources'"
            )
            .await,
            1,
            "index deletion is absent"
        );
        sqlx::query("DELETE FROM meetings WHERE id='one'")
            .execute(&pool)
            .await
            .unwrap();
        for table in [
            "knowledge_sources",
            "knowledge_chunks",
            "knowledge_vectors",
            "knowledge_index_jobs",
            "knowledge_fts",
        ] {
            assert_eq!(
                count(&pool, &format!("SELECT COUNT(*) FROM {table}")).await,
                0,
                "orphan in {table}"
            );
        }
    }
    #[tokio::test]
    async fn restore_requeues_index() {
        let pool = database().await;
        meeting(&pool, "one").await;
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM sqlite_master WHERE name='knowledge_sources'"
            )
            .await,
            1,
            "restore indexing is absent"
        );
        let id: String = sqlx::query_scalar("SELECT id FROM knowledge_sources")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        meeting(&pool, "one").await;
        transcript(&pool).await;
        let restored: String = sqlx::query_scalar("SELECT id FROM knowledge_sources")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(restored, id);
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_index_jobs WHERE attempts=0"
            )
            .await,
            1
        );
    }

    #[tokio::test]
    async fn old_worker_cannot_publish_or_delete_newer_job() {
        let pool = database().await;
        meeting(&pool, "one").await;
        transcript(&pool).await;
        let old = next_job(&pool).await.unwrap().unwrap();
        let row = rows_page(&pool, &old, None).await.unwrap().remove(0);
        let span = TextSpan {
            transcript_id: row.id.clone(),
            start_byte: 0,
            end_byte: row.transcript.len(),
        };
        let vector = super::super::embedding::normalize(vec![1.; 384]).unwrap();
        stage(&pool, &old, &row, &span, 0, &vector, "old-space")
            .await
            .unwrap();
        sqlx::query("UPDATE transcripts SET transcript='Changed public fixture' WHERE id='row'")
            .execute(&pool)
            .await
            .unwrap();
        let new = next_job(&pool).await.unwrap().unwrap();
        assert_eq!(
            publish(&pool, &old, "old-space").await,
            Err(KnowledgeError::Superseded)
        );
        assert_eq!(
            stage(&pool, &old, &row, &span, 0, &vector, "old-space").await,
            Err(KnowledgeError::Superseded)
        );
        record_failure(&pool, &old, &KnowledgeError::ProviderFailure)
            .await
            .unwrap();
        assert!(current(&pool, &new).await.unwrap());
        assert_eq!(
            count(&pool, "SELECT attempts FROM knowledge_index_jobs").await,
            0
        );
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_sources WHERE semantic_revision IS NOT NULL"
            )
            .await,
            0
        );
    }
    #[tokio::test]
    async fn pauses_do_not_consume_three_failure_attempts() {
        let pool = database().await;
        meeting(&pool, "one").await;
        let job = next_job(&pool).await.unwrap().unwrap();
        for error in [
            KnowledgeError::Busy,
            KnowledgeError::Disabled,
            KnowledgeError::Cancelled,
            KnowledgeError::Superseded,
        ] {
            record_failure(&pool, &job, &error).await.unwrap();
        }
        assert_eq!(
            count(&pool, "SELECT attempts FROM knowledge_index_jobs").await,
            0
        );
        for _ in 0..4 {
            record_failure(&pool, &job, &KnowledgeError::ProviderFailure)
                .await
                .unwrap();
        }
        assert_eq!(
            count(&pool, "SELECT attempts FROM knowledge_index_jobs").await,
            3
        );
        assert!(next_job(&pool).await.unwrap().is_none());
        transcript(&pool).await;
        assert!(next_job(&pool).await.unwrap().is_some());
    }
    #[tokio::test]
    async fn upgrade_seeds_existing_library_and_fts() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let migrations = sqlx::migrate!("./migrations");
        for migration in migrations.iter().filter(|m| m.version < 20261007000000) {
            sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
        }
        meeting(&pool, "one").await;
        meeting(&pool, "empty").await;
        transcript(&pool).await;
        sqlx::query("UPDATE transcripts SET rowid=99 WHERE id='row'")
            .execute(&pool)
            .await
            .unwrap();
        let migration = migrations
            .iter()
            .find(|m| m.version == 20261007000000)
            .unwrap();
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
        assert_eq!(
            count(
                &pool,
                "SELECT rowid FROM knowledge_fts WHERE transcript_id='row'"
            )
            .await,
            99,
            "upgrade backfill preserves canonical row identity"
        );
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM knowledge_sources").await,
            2
        );
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM knowledge_index_jobs").await,
            2
        );
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_fts WHERE knowledge_fts MATCH '\"ATLAS-42\"'"
            )
            .await,
            1
        );
        sqlx::query("UPDATE transcripts SET transcript='Corrected only',original_transcript='ATLAS-42' WHERE id='row'").execute(&pool).await.unwrap();
        assert_eq!(
            count(
                &pool,
                "SELECT COUNT(*) FROM knowledge_fts WHERE knowledge_fts MATCH '\"ATLAS-42\"'"
            )
            .await,
            0
        );
        sqlx::query("DELETE FROM transcripts")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM meetings")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            count(&pool, "SELECT COUNT(*) FROM knowledge_index_jobs").await,
            0
        );
        assert_eq!(count(&pool, "SELECT COUNT(*) FROM knowledge_fts").await, 0);
    }
}
