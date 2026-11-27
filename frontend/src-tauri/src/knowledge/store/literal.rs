//! Small literal-search results from current saved rows. No model or index counts.
use super::{
    display, folded_char, literal_fold, materialize, scan_patterns, ReadContext, SelectedRow,
    TextBlob, READ_BYTES,
};
use crate::knowledge::{retrieval, types::*};
use serde::Serialize;
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};
use std::{
    cell::Cell,
    collections::HashSet,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

pub(crate) const MAX_QUERY_CHARS: usize = 256;
const MAX_ROWS: usize = 50_000;
const SCAN_BYTES: usize = 32 * 1024 * 1024;
const PAGE: usize = 128;
const CONTEXT_BYTES: usize = 100;
const APP_LIMIT: usize = 50;
static SEARCH_SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[derive(Clone)]
pub(super) struct LiteralQuery {
    patterns: Vec<String>,
    archive: bool,
}

impl LiteralQuery {
    fn new(query: &str, archive: bool) -> Result<Self, KnowledgeError> {
        let mut text = query.trim();
        if text.chars().take(MAX_QUERY_CHARS + 1).count() > MAX_QUERY_CHARS {
            return Err(KnowledgeError::InvalidInput);
        }
        let mut patterns = Vec::new();
        while let Some(first) = text.chars().next() {
            let (term, rest) = if archive {
                let closers: &[char] = match first {
                    '"' => &['"'],
                    '„' => &['“', '”', '"'],
                    '“' => &['”', '"'],
                    _ => &[],
                };
                if !closers.is_empty() {
                    let after_open = &text[first.len_utf8()..];
                    let Some(phrase) = after_open.split_once(closers) else {
                        text = after_open.trim_start();
                        continue;
                    };
                    phrase
                } else {
                    text.split_once(char::is_whitespace).unwrap_or((text, ""))
                }
            } else {
                (text, "")
            };
            let pattern = literal_fold(term.trim());
            if !pattern.is_empty() && !patterns.contains(&pattern) {
                patterns.push(pattern);
            }
            text = rest.trim_start();
        }
        Ok(Self { patterns, archive })
    }

    pub(super) fn archive(query: &str) -> Result<Self, KnowledgeError> {
        Self::new(query, true)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    // Store original UTF-8 offsets even for case folds that expand a character.
    fn folded(text: &str) -> (String, Vec<(usize, usize)>) {
        let mut folded = String::with_capacity(text.len());
        let mut offsets = Vec::with_capacity(text.len());
        for (start, ch) in text.char_indices() {
            for lower in folded_char(ch) {
                folded.push(lower);
                offsets.resize(
                    offsets.len() + lower.len_utf8(),
                    (start, start + ch.len_utf8()),
                );
            }
        }
        (folded, offsets)
    }

    fn record(&self, text: &str, start: usize, found: &mut [Option<(usize, usize)>]) {
        let (folded, offsets) = Self::folded(text);
        for (pattern, position) in self.patterns.iter().zip(found) {
            if position.is_none() {
                if let Some(index) = folded.find(pattern) {
                    *position = Some((
                        start + offsets[index].0,
                        start + offsets[index + pattern.len() - 1].1,
                    ));
                }
            }
        }
    }

    pub(super) fn find(&self, text: &str) -> Option<(usize, usize)> {
        let mut found = vec![None; self.patterns.len()];
        self.record(text, 0, &mut found);
        Self::context(&found)
    }

    pub(super) fn matches(&self, text: &str) -> bool {
        let folded = literal_fold(text);
        self.patterns.iter().all(|pattern| folded.contains(pattern))
    }

    // Prefer a window with several terms when they're nearby. A distant term
    // still qualifies the row, without yielding an unbounded excerpt.
    fn context(found: &[Option<(usize, usize)>]) -> Option<(usize, usize)> {
        if found.iter().any(Option::is_none) {
            return None;
        }
        let (start, first_end) = found.iter().flatten().min().copied()?;
        let end = found
            .iter()
            .flatten()
            .filter(|(next, _)| *next <= first_end + CONTEXT_BYTES)
            .map(|(_, end)| *end)
            .max()
            .unwrap_or(first_end);
        Some((start, end))
    }

    fn scan(
        &self,
        blob: &TextBlob<'_>,
        context: &ReadContext,
    ) -> Result<Option<(usize, usize)>, KnowledgeError> {
        if self.patterns.len() == 1 {
            return scan_patterns(blob, &self.patterns, context);
        }
        let mut found = vec![None; self.patterns.len()];
        let mut start = 0;
        while start < blob.len {
            let (_, text) = blob.window(start, READ_BYTES, false, context)?;
            self.record(&text, start, &mut found);
            if let Some(span) = Self::context(&found) {
                return Ok(Some(span));
            }
            if start + text.len() == blob.len {
                break;
            }
            // Keep the same overlap as the legacy literal scanner.
            let mut advance = text.len().saturating_sub(4096);
            while !text.is_char_boundary(advance) {
                advance += 1;
            }
            if advance == 0 {
                return Err(KnowledgeError::InvalidInput);
            }
            start += advance;
        }
        Ok(None)
    }

    pub(super) fn highlights(&self, text: &str) -> Vec<(usize, usize)> {
        if !self.archive {
            return Vec::new();
        }
        let (folded, offsets) = Self::folded(text);
        let mut ranges = Vec::new();
        for pattern in &self.patterns {
            ranges.extend(
                folded
                    .match_indices(pattern)
                    .map(|(index, _)| (offsets[index].0, offsets[index + pattern.len() - 1].1)),
            );
        }
        ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (start, end) in ranges {
            if let Some(last) = merged.last_mut().filter(|last| start < last.1) {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        merged
    }
}

/// Keep natural boundaries when a clipped edge is near whitespace. The byte
/// range still refers to the exact saved source for citation and consent checks.
pub(super) fn trim_window(
    text: &str,
    clipped_start: bool,
    clipped_end: bool,
    matched: (usize, usize),
) -> (usize, usize) {
    let start = if clipped_start {
        text[..matched.0]
            .find(char::is_whitespace)
            .filter(|start| *start <= 24)
            .unwrap_or(0)
    } else {
        0
    };
    let end = if clipped_end {
        text[matched.1..]
            .rfind(char::is_whitespace)
            .map(|end| matched.1 + end)
            .filter(|end| text.len() - end <= 24)
            .unwrap_or(text.len())
    } else {
        text.len()
    };
    let trimmed = text[start..end].trim_start();
    let start = end - trimmed.len();
    (start, start + trimmed.trim_end().len())
}

pub(super) struct Budget {
    deadline: Instant,
    remaining: Cell<usize>,
}

impl Budget {
    pub(super) fn charge(&self, bytes: usize) -> Result<(), KnowledgeError> {
        let remaining = self.remaining.get();
        if remaining < bytes || Instant::now() >= self.deadline {
            return Err(KnowledgeError::Busy);
        }
        self.remaining.set(remaining - bytes);
        Ok(())
    }
}

/// The read-only meeting-list tool pages in meeting order and shares the
/// desktop matcher, including its Unicode folding and incremental blob reads.
pub(crate) struct MeetingMatcher {
    pattern: [String; 1],
    context: ReadContext,
    rows_left: usize,
}

impl MeetingMatcher {
    pub(crate) fn new(query: &str) -> Self {
        Self {
            pattern: [literal_fold(query)],
            context: ReadContext {
                cancelled: Arc::new(AtomicBool::new(false)),
                background: false,
                literal_budget: Some(Budget {
                    deadline: Instant::now() + Duration::from_secs(10),
                    remaining: Cell::new(SCAN_BYTES),
                }),
            },
            rows_left: MAX_ROWS,
        }
    }

    pub(crate) fn matches(
        &mut self,
        handle: &mut sqlx::sqlite::LockedSqliteHandle<'_>,
        rowid: i64,
        title: bool,
    ) -> Result<bool, KnowledgeError> {
        self.rows_left = self.rows_left.checked_sub(1).ok_or(KnowledgeError::Busy)?;
        self.context.literal_budget.as_ref().unwrap().charge(0)?;
        let (table, column) = if title {
            (c"meetings", c"title")
        } else {
            (c"transcripts", c"transcript")
        };
        let blob = TextBlob::open(handle, table, column, rowid)?;
        Ok(scan_patterns(&blob, &self.pattern, &self.context)?.is_some())
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct TitleHit {
    pub id: String,
    pub title: String,
    pub date: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub highlights: Vec<(usize, usize)>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TranscriptHit {
    pub meeting_id: String,
    pub transcript_id: String,
    pub title: String,
    pub date: String,
    pub text: String,
    pub timestamp: String,
    pub speaker: Option<String>,
    pub start_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub highlights: Vec<(usize, usize)>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    clipped_start: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    clipped_end: bool,
    #[serde(skip)]
    selected: SelectedRow,
    #[serde(skip)]
    span: TextSpan,
}

#[derive(Default, Serialize)]
pub(crate) struct LiteralHits {
    pub titles: Vec<TitleHit>,
    pub passages: Vec<TranscriptHit>,
    pub limited: bool,
}

#[derive(Serialize)]
pub(crate) struct LinkedPassage {
    #[serde(flatten)]
    pub hit: TranscriptHit,
    pub evidence: EvidenceRef,
}

#[derive(Serialize)]
pub(crate) struct LinkedHits {
    pub titles: Vec<TitleHit>,
    pub passages: Vec<LinkedPassage>,
    pub limited: bool,
}

#[derive(FromRow)]
struct Candidate {
    rowid: i64,
    meeting_id: String,
    meeting_rowid: i64,
    transcript_id: Option<String>,
    speaker_present: bool,
    start_seconds: Option<f64>,
    revision: Option<i64>,
    generation: Option<i64>,
}

/// An ASCII substring of the folded query is a necessary condition for a match.
/// The index expands the two non-ASCII letters whose lowercase contains ASCII
/// (dotted I and Kelvin sign). Exact Unicode folding and offsets stay canonical.
fn indexed_anchor(folded: &str) -> Option<String> {
    folded
        .split(|ch: char| !ch.is_ascii() || ch == '\0')
        .filter(|part| part.len() >= 3)
        .max_by_key(|part| part.len())
        .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
}

pub(crate) async fn available_anchor(
    conn: &mut SqliteConnection,
    query: &str,
) -> Result<Option<String>, KnowledgeError> {
    query_anchor(conn, &LiteralQuery::new(query, false)?).await
}

async fn query_anchor(
    conn: &mut SqliteConnection,
    query: &LiteralQuery,
) -> Result<Option<String>, KnowledgeError> {
    let anchor = query
        .patterns
        .iter()
        .filter_map(|part| indexed_anchor(part))
        .max_by_key(String::len);
    if anchor.is_some() && !sqlx::query_scalar::<_, bool>(
        "SELECT count(*)=2 FROM sqlite_schema WHERE type='table' AND name IN ('knowledge_title_literal','knowledge_transcript_literal')"
    ).fetch_one(conn).await? { return Ok(None); }
    Ok(anchor)
}

pub(crate) async fn search_transcript_terms(
    conn: &mut SqliteConnection,
    query: &str,
    scope: Option<&KnowledgeScope>,
    limit: usize,
) -> Result<LiteralHits, KnowledgeError> {
    search_query(
        conn,
        &LiteralQuery::archive(query)?,
        scope,
        limit,
        false,
        false,
    )
    .await
}

async fn search_query(
    conn: &mut SqliteConnection,
    query: &LiteralQuery,
    scope: Option<&KnowledgeScope>,
    limit: usize,
    titles: bool,
    one_per_meeting: bool,
) -> Result<LiteralHits, KnowledgeError> {
    let mut hits = LiteralHits::default();
    if query.is_empty() {
        return Ok(hits);
    }
    let mut tx = conn.begin().await?;
    // The standalone read-only tool can open a library before desktop migration.
    let anchor = query_anchor(&mut tx, query).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let allowed = match scope {
        Some(scope) => match tokio::time::timeout_at(
            deadline,
            retrieval::freeze_scope_rows(&mut tx, scope, Some(MAX_ROWS as i64)),
        )
        .await
        {
            Ok(Ok(scope)) if scope.meeting_ids.is_empty() => return Ok(hits),
            Ok(Ok(scope)) => Some(
                serde_json::to_string(&scope.meeting_ids).map_err(|_| KnowledgeError::Storage)?,
            ),
            Ok(Err(error)) if error != KnowledgeError::Busy => return Err(error),
            _ => {
                hits.limited = true;
                return Ok(hits);
            }
        },
        None => None,
    };
    let context = ReadContext {
        cancelled: Arc::new(AtomicBool::new(false)),
        background: false,
        literal_budget: Some(Budget {
            deadline: deadline.into_std(),
            remaining: Cell::new(SCAN_BYTES),
        }),
    };
    let mut seen = HashSet::new();
    for title in [true, false] {
        if title && !titles {
            continue;
        }
        let (mut before, mut rows_read) = (None, 0);
        loop {
            if rows_read >= MAX_ROWS || context.literal_budget.as_ref().unwrap().charge(0).is_err()
            {
                hits.limited = true;
                break;
            }
            let page = PAGE.min(MAX_ROWS - rows_read);
            // Separate title and passage bounds keep a busy transcript from hiding title matches.
            // Keyset batching reads metadata only and avoids building an unbounded result set.
            let (columns, tables, rowid) = if title {
                (
                    "m.rowid,m.id AS meeting_id,m.rowid AS meeting_rowid,NULL AS transcript_id,
                  0 AS speaker_present,NULL AS start_seconds,NULL AS revision,NULL AS generation",
                    "active_meetings m",
                    "m.rowid",
                )
            } else {
                ("t.rowid,m.id AS meeting_id,m.rowid AS meeting_rowid,t.id AS transcript_id,
                  t.speaker IS NOT NULL AS speaker_present,t.audio_start_time AS start_seconds,s.revision,s.generation",
                 "transcripts t JOIN active_meetings m ON m.id=t.meeting_id LEFT JOIN knowledge_sources s ON s.meeting_id=m.id", "t.rowid")
            };
            let statement = if anchor.is_some() {
                let index = if title {
                    "knowledge_title_literal"
                } else {
                    "knowledge_transcript_literal"
                };
                format!("SELECT {columns} FROM {index}(?4) hit CROSS JOIN {tables}
                    WHERE {rowid}=hit.rowid AND hit.rowid<=?1 AND (?2 IS NULL OR m.id IN(SELECT value FROM json_each(?2)))
                    ORDER BY hit.rowid DESC LIMIT ?3")
            } else {
                format!("SELECT {columns} FROM {tables} WHERE {rowid}<=?1 AND (?2 IS NULL OR m.id IN(SELECT value FROM json_each(?2)))
                    ORDER BY {rowid} DESC LIMIT ?3")
            };
            let mut statement = sqlx::query_as(&statement)
                .bind(before.unwrap_or(i64::MAX))
                .bind(allowed.as_deref())
                .bind(page as i64);
            if let Some(expression) = &anchor {
                statement = statement.bind(expression);
            }
            let rows: Vec<Candidate> =
                match tokio::time::timeout_at(deadline, statement.fetch_all(&mut *tx)).await {
                    Ok(rows) => rows?,
                    Err(_) => {
                        hits.limited = true;
                        return Ok(hits);
                    }
                };
            if rows.is_empty() {
                break;
            }
            before = rows.last().and_then(|row| row.rowid.checked_sub(1));
            let end = rows.len() < page || before.is_none();
            rows_read += rows.len();
            let mut handle = tx.lock_handle().await?;
            match collect(
                &mut handle,
                rows,
                query,
                &context,
                &mut hits,
                &mut seen,
                limit,
                title,
                one_per_meeting,
            ) {
                Ok(true) | Err(KnowledgeError::Busy) => {
                    hits.limited = true;
                    break;
                }
                Ok(false) if end => break,
                Ok(false) => {}
                Err(error) => return Err(error),
            }
            drop(handle);
            tokio::task::yield_now().await;
        }
    }
    tx.commit().await?;
    Ok(hits)
}

#[allow(clippy::too_many_arguments)]
fn collect(
    handle: &mut sqlx::sqlite::LockedSqliteHandle<'_>,
    rows: Vec<Candidate>,
    query: &LiteralQuery,
    context: &ReadContext,
    hits: &mut LiteralHits,
    seen: &mut HashSet<String>,
    limit: usize,
    title: bool,
    one_per_meeting: bool,
) -> Result<bool, KnowledgeError> {
    for row in rows {
        if !title && one_per_meeting && seen.contains(&row.meeting_id) {
            continue;
        }
        if title {
            let matches = {
                let blob = TextBlob::open(handle, c"meetings", c"title", row.meeting_rowid)?;
                query.scan(&blob, context)?.is_some()
            };
            if matches {
                if hits.titles.len() == limit {
                    return Ok(true);
                }
                let title = display(handle, c"meetings", c"title", row.meeting_rowid, context)?.0;
                hits.titles.push(TitleHit {
                    id: row.meeting_id,
                    highlights: query.highlights(&title),
                    title,
                    date: display(
                        handle,
                        c"meetings",
                        c"created_at",
                        row.meeting_rowid,
                        context,
                    )?
                    .0,
                });
            }
            continue;
        }
        let (start_byte, text, clipped_start, clipped_end) = {
            let blob = TextBlob::open(handle, c"transcripts", c"transcript", row.rowid)?;
            let Some((start, end)) = query.scan(&blob, context)? else {
                continue;
            };
            if hits.passages.len() == limit {
                return Ok(true);
            }
            let begin = start.saturating_sub(CONTEXT_BYTES);
            let length = end.saturating_add(CONTEXT_BYTES) - begin;
            let (offset, text) = blob.window(begin, length, true, context)?;
            if query.archive {
                let (first, last) = trim_window(
                    &text,
                    offset > 0,
                    offset + text.len() < blob.len,
                    (start - offset, end - offset),
                );
                let start = offset + first;
                (
                    start,
                    text[first..last].to_owned(),
                    start > 0,
                    offset + last < blob.len,
                )
            } else {
                (offset, text, false, false)
            }
        };
        let id = row.transcript_id.ok_or(KnowledgeError::Storage)?;
        let span = TextSpan {
            transcript_id: id.clone(),
            start_byte,
            end_byte: start_byte + text.len(),
        };
        let speaker = if row.speaker_present {
            Some(display(handle, c"transcripts", c"speaker", row.rowid, context)?.0)
        } else {
            None
        };
        let selected = SelectedRow {
            transcript_id: id.clone(),
            source_id: format!("meeting:{}", row.meeting_id),
            meeting_id: row.meeting_id.clone(),
            revision: row.revision.unwrap_or_default(),
            generation: row.generation.unwrap_or_default(),
        };
        if one_per_meeting {
            seen.insert(row.meeting_id.clone());
        }
        hits.passages.push(TranscriptHit {
            meeting_id: row.meeting_id,
            transcript_id: id,
            title: display(handle, c"meetings", c"title", row.meeting_rowid, context)?.0,
            date: display(
                handle,
                c"meetings",
                c"created_at",
                row.meeting_rowid,
                context,
            )?
            .0,
            timestamp: display(handle, c"transcripts", c"timestamp", row.rowid, context)?.0,
            speaker,
            start_seconds: row.start_seconds,
            highlights: query.highlights(&text),
            clipped_start,
            clipped_end,
            text,
            selected,
            span,
        });
    }
    Ok(false)
}

pub(crate) async fn linked(
    pool: &SqlitePool,
    query: &str,
    scope: Option<&KnowledgeScope>,
    one_per_meeting: bool,
) -> Result<LinkedHits, KnowledgeError> {
    linked_query(
        pool,
        LiteralQuery::new(query, false)?,
        scope,
        one_per_meeting,
    )
    .await
}

pub(super) async fn linked_query(
    pool: &SqlitePool,
    query: LiteralQuery,
    scope: Option<&KnowledgeScope>,
    one_per_meeting: bool,
) -> Result<LinkedHits, KnowledgeError> {
    let hits = desktop_query(pool, query, scope, true, one_per_meeting).await?;
    let mut result = LinkedHits {
        titles: hits.titles,
        passages: Vec::new(),
        limited: hits.limited,
    };
    // The existing resolver checks this same canonical identity again on arrival.
    // Skip hits invalidated between the bounded search snapshot and this read.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    for hit in hits.passages {
        match tokio::time::timeout_at(
            deadline,
            materialize(pool, &hit.selected, hit.span.clone(), false),
        )
        .await
        {
            Ok(Ok(passage)) => result.passages.push(LinkedPassage {
                hit,
                evidence: passage.evidence,
            }),
            Ok(Err(KnowledgeError::Superseded)) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                result.limited = true;
                break;
            }
        }
    }
    Ok(result)
}

pub(crate) async fn desktop(
    pool: &SqlitePool,
    query: &str,
    scope: Option<&KnowledgeScope>,
    titles: bool,
    one_per_meeting: bool,
) -> Result<LiteralHits, KnowledgeError> {
    desktop_query(
        pool,
        LiteralQuery::new(query, false)?,
        scope,
        titles,
        one_per_meeting,
    )
    .await
}

async fn desktop_query(
    pool: &SqlitePool,
    query: LiteralQuery,
    scope: Option<&KnowledgeScope>,
    titles: bool,
    one_per_meeting: bool,
) -> Result<LiteralHits, KnowledgeError> {
    let permit = search_permit().await?;
    let mut conn = tokio::time::timeout(Duration::from_secs(3), pool.acquire())
        .await
        .map_err(|_| KnowledgeError::Busy)??;
    let runtime = tokio::runtime::Handle::current();
    let scope = scope.cloned();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        runtime.block_on(search_query(
            &mut conn,
            &query,
            scope.as_ref(),
            APP_LIMIT,
            titles,
            one_per_meeting,
        ))
    })
    .await
    .map_err(|_| KnowledgeError::Storage)?
}

pub(crate) async fn search_permit() -> Result<tokio::sync::SemaphorePermit<'static>, KnowledgeError>
{
    tokio::time::timeout(Duration::from_secs(5), SEARCH_SLOT.acquire())
        .await
        .map_err(|_| KnowledgeError::Busy)?
        .map_err(|_| KnowledgeError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::evidence::{resolve, EvidenceStatus};

    #[tokio::test]
    async fn searches_literal_unicode_and_resolves_a_verified_late_segment() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO meetings(id,title,created_at,updated_at) VALUES
                ('fixture','Öffnung 50%_', '2024-02-01','2024-02-01'),
                ('title-only','Öffnung 50%_', '2024-02-02','2024-02-02');
             WITH RECURSIVE sequence(n) AS (SELECT 0 UNION ALL SELECT n+1 FROM sequence WHERE n<205)
             INSERT INTO transcripts(id,meeting_id,transcript,timestamp,audio_start_time)
                SELECT printf('row-%03d',n),'fixture','Ordinary fixture text','00:00',n FROM sequence",
        ).execute(&pool).await.unwrap();
        let text = format!(
            "{}📘İ ÄPFEL 50%_ τέλος {}",
            "界".repeat(5_460),
            "ö".repeat(110)
        );
        sqlx::query("UPDATE transcripts SET transcript=? WHERE id='row-205'")
            .bind(&text)
            .execute(&pool)
            .await
            .unwrap();

        let results = linked(&pool, "äpfel 50%_", None, false).await.unwrap();
        assert!(results.titles.is_empty());
        assert_eq!(results.passages.len(), 1);
        let hit = &results.passages[0];
        assert!(hit.hit.text.contains("📘İ ÄPFEL 50%_ τέλος"));
        assert!(hit.hit.text.len() <= 220);
        assert!(!results.limited);
        assert_eq!(
            linked(&pool, "öFFNUNG 50%_", None, false)
                .await
                .unwrap()
                .titles
                .len(),
            2
        );
        assert_eq!(
            linked(&pool, "i\u{307} äpfel", None, false)
                .await
                .unwrap()
                .passages
                .len(),
            1
        );
        assert_eq!(
            linked(&pool, "ΤΈΛΟΣ", None, false)
                .await
                .unwrap()
                .passages
                .len(),
            1
        );
        assert!(linked(&pool, "50%X", None, false)
            .await
            .unwrap()
            .passages
            .is_empty());
        assert!(matches!(
            linked(&pool, &"📘".repeat(MAX_QUERY_CHARS + 1), None, false).await,
            Err(KnowledgeError::InvalidInput)
        ));

        let target = resolve(&pool, &hit.evidence).await.unwrap();
        assert_eq!(target.status, EvidenceStatus::Current);
        let navigation = target.navigation.unwrap();
        assert_eq!(navigation.transcript_index, 205);
        assert_eq!(navigation.transcript_id, "row-205");
        sqlx::query("UPDATE transcripts SET transcript='Replacement' WHERE id='row-205'")
            .execute(&pool)
            .await
            .unwrap();
        let stale = resolve(&pool, &hit.evidence).await.unwrap();
        assert_eq!(stale.status, EvidenceStatus::Stale);
        assert!(stale.navigation.is_none());
    }
}
