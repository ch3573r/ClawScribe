//! Local desktop discovery only. These rows are never transcript evidence.
use super::literal::{self, LiteralQuery};
use crate::knowledge::{
    chunking::floor_boundary,
    retrieval,
    types::{KnowledgeError, KnowledgeScope},
};
use crate::library::saved_text::{editor_text, markdown_text, summary_text, MAX_SAVED_TEXT_BYTES};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqliteConnection, SqlitePool};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const HIT_LIMIT: usize = 50;
const SOURCES: [SavedSource; 3] = [
    SavedSource::Summary,
    SavedSource::Note,
    SavedSource::Bookmark,
];

// Row IDs cross the JS boundary as decimal strings to preserve their full range.
type SavedCursor = [Option<String>; 3];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SavedSource {
    Summary,
    Note,
    Bookmark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SavedReference {
    pub meeting_id: String,
    pub source: SavedSource,
    pub id: String,
    start: usize,
    end: usize,
    fingerprint: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct SavedHit {
    pub reference: SavedReference,
    pub title: String,
    pub date: String,
    pub text: String,
    pub seconds: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    highlights: Vec<(usize, usize)>,
    clipped_start: bool,
    clipped_end: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    note_context: Option<(String, String)>,
}

#[derive(Default, Serialize)]
pub(crate) struct SavedResults {
    pub hits: Vec<SavedHit>,
    pub limited: bool,
    next: Option<SavedCursor>,
}

#[derive(Default, Serialize)]
pub(crate) struct DesktopResults {
    titles: Vec<literal::TitleHit>,
    passages: Vec<literal::LinkedPassage>,
    saved: Vec<SavedHit>,
    limited: bool,
    saved_next: Option<SavedCursor>,
}

#[tauri::command]
pub(crate) async fn search_saved_meetings(
    state: tauri::State<'_, crate::state::AppState>,
    query: String,
    scope: Option<KnowledgeScope>,
    one_per_meeting: bool,
    saved_cursor: Option<SavedCursor>,
) -> Result<DesktopResults, String> {
    search_archive_page(
        state.db_manager.pool(),
        &query,
        scope,
        one_per_meeting,
        saved_cursor,
    )
    .await
    .map_err(|err| err.to_string())
}

async fn search_archive_page(
    pool: &SqlitePool,
    query: &str,
    scope: Option<KnowledgeScope>,
    one_per_meeting: bool,
    cursor: Option<SavedCursor>,
) -> Result<DesktopResults, KnowledgeError> {
    if matches!(&scope, Some(KnowledgeScope::Library { filter }) if !filter.all_meetings && filter.meeting_ids.is_empty())
    {
        return Ok(DesktopResults::default());
    }
    let query = LiteralQuery::archive(query)?;
    let linked = if cursor.is_none() {
        Some(literal::linked_query(pool, query.clone(), scope.as_ref(), one_per_meeting).await?)
    } else {
        None
    };
    let continued = cursor.is_some();
    let saved = match search_query(pool, query, scope, one_per_meeting, cursor).await {
        Ok(saved) => saved,
        Err(KnowledgeError::Busy) if !continued => SavedResults {
            limited: true,
            ..Default::default()
        },
        Err(error) => return Err(error),
    };
    let mut result = DesktopResults {
        saved: saved.hits,
        limited: saved.limited,
        saved_next: saved.next,
        ..Default::default()
    };
    if let Some(linked) = linked {
        result.titles = linked.titles;
        result.passages = linked.passages;
        result.limited |= linked.limited;
    }
    Ok(result)
}

#[tauri::command]
pub(crate) async fn resolve_saved_search_hit(
    state: tauri::State<'_, crate::state::AppState>,
    reference: SavedReference,
) -> Result<Option<SavedHit>, String> {
    resolve(state.db_manager.pool(), reference)
        .await
        .map_err(|err| err.to_string())
}

#[derive(FromRow)]
struct SavedRow {
    row_id: i64,
    meeting_id: String,
    id: String,
    title: String,
    date: String,
    seconds: Option<f64>,
    blocks: bool,
    body: Option<String>,
}

impl SavedSource {
    fn statement(self, lookup: bool) -> String {
        let (table, id, body, blocks, seconds) = match self {
            Self::Summary => ("summary_processes", "s.meeting_id", "s.result", "0", "NULL"),
            Self::Note => (
                "meeting_notes",
                "s.meeting_id",
                "coalesce(s.notes_markdown,s.notes_json)",
                "s.notes_markdown IS NULL",
                "NULL",
            ),
            Self::Bookmark => ("meeting_bookmarks", "s.id", "s.label", "0", "s.seconds"),
        };
        // octet_length on a bare SQLite column can read its size without
        // materializing an oversized editor document.
        let size = if self == Self::Note {
            "CASE WHEN s.notes_markdown IS NOT NULL THEN octet_length(s.notes_markdown) ELSE octet_length(s.notes_json) END".into()
        } else {
            format!("octet_length({body})")
        };
        let present = if self == Self::Note {
            "(s.notes_markdown IS NOT NULL OR s.notes_json IS NOT NULL)".into()
        } else {
            format!("{body} IS NOT NULL")
        };
        let bounded_body = format!("CASE WHEN {size}<={MAX_SAVED_TEXT_BYTES} THEN {body} END");
        let restriction = if lookup {
            format!("m.id=?1 AND {id}=?2")
        } else {
            format!(
                "(?1 IS NULL OR m.id IN (SELECT value FROM json_each(?1)))
                AND s.rowid<=?2 AND clawscribe_saved_match(s.rowid,m.id,{bounded_body},{blocks})
                ORDER BY s.rowid DESC LIMIT ?3"
            )
        };
        format!(
            "SELECT s.rowid row_id, m.id meeting_id, {id} id, substr(m.title,1,256) title,
            substr(m.created_at,1,64) date, {seconds} seconds, {blocks} blocks, {bounded_body} body
            FROM {table} s CROSS JOIN active_meetings m ON m.id=s.meeting_id
            WHERE {present} AND {restriction}"
        )
    }
    fn text(self, raw: &str, blocks: bool) -> Option<String> {
        match self {
            Self::Summary => summary_text(raw),
            Self::Note if blocks => editor_text(&serde_json::from_str(raw).ok()?),
            Self::Note => Some(markdown_text(raw)),
            Self::Bookmark => Some(raw.to_owned()),
        }
    }
}

struct SavedFilter {
    source: SavedSource,
    query: LiteralQuery,
    deadline: Instant,
    seen: Option<HashSet<String>>,
    first_unsearched: Option<i64>,
    oversized: bool,
}

#[path = "saved_search_filter.rs"]
mod sqlite_filter;

// Return original UTF-8 offsets even when a character folds to several scalars.
fn excerpt(text: &str, query: &LiteralQuery) -> Option<(usize, usize)> {
    let (found_start, found_end) = query.find(text)?;
    let start = floor_boundary(text, found_start.saturating_sub(100));
    let end = floor_boundary(text, found_end + 100);
    let (first, last) = literal::trim_window(
        &text[start..end],
        start > 0,
        end < text.len(),
        (found_start - start, found_end - start),
    );
    Some((start + first, start + last))
}

fn hit(
    row: SavedRow,
    source: SavedSource,
    body: &str,
    start: usize,
    end: usize,
) -> Option<SavedHit> {
    let text = body.get(start..end)?.to_owned();
    let mut hash = Sha256::new();
    // Bind both the locator and the span to the canonical body and timestamp.
    // An otherwise identical bookmark in another meeting is a different source.
    super::hash_value(&mut hash, &(&row.meeting_id, source, &row.id, start, end)).ok()?;
    hash.update(body.as_bytes());
    hash.update(
        row.seconds
            .map(f64::to_bits)
            .unwrap_or_default()
            .to_le_bytes(),
    );
    Some(SavedHit {
        reference: SavedReference {
            meeting_id: row.meeting_id,
            source,
            id: row.id,
            start,
            end,
            fingerprint: format!("{:x}", hash.finalize()),
        },
        title: row.title,
        date: row.date,
        text,
        seconds: row.seconds,
        highlights: Vec::new(),
        clipped_start: start > 0,
        clipped_end: end < body.len(),
        note_context: None,
    })
}

async fn search_rows(
    conn: &mut SqliteConnection,
    query: &LiteralQuery,
    scope: Option<&KnowledgeScope>,
    one_per_meeting: bool,
    cursor: Option<SavedCursor>,
) -> Result<SavedResults, KnowledgeError> {
    let mut result = SavedResults::default();
    if query.is_empty() {
        return Ok(result);
    }
    let allowed = if let Some(scope) = scope {
        match tokio::time::timeout(
            Duration::from_secs(2),
            retrieval::freeze_scope_rows(conn, scope, Some(50_000)),
        )
        .await
        {
            Ok(Ok(scope)) if scope.meeting_ids.is_empty() => return Ok(result),
            Ok(Ok(scope)) => Some(
                serde_json::to_string(&scope.meeting_ids).map_err(|_| KnowledgeError::Storage)?,
            ),
            Ok(Err(error)) if error != KnowledgeError::Busy => return Err(error),
            _ if cursor.is_none() => {
                result.limited = true;
                return Ok(result);
            }
            _ => return Err(KnowledgeError::Busy),
        }
    } else {
        None
    };
    let mut next = SavedCursor::default();
    for (index, source) in SOURCES.into_iter().enumerate() {
        let before = match &cursor {
            Some(cursor) => match &cursor[index] {
                Some(value) => value.parse().map_err(|_| KnowledgeError::InvalidInput)?,
                None => continue,
            },
            None => i64::MAX,
        };
        let deadline = Instant::now() + Duration::from_secs(1);
        let filter = Arc::new(Mutex::new(SavedFilter {
            source,
            query: query.clone(),
            deadline,
            seen: one_per_meeting.then(HashSet::new),
            first_unsearched: None,
            oversized: false,
        }));
        sqlite_filter::install(conn, Some(filter.clone())).await?;
        conn.lock_handle()
            .await?
            .set_progress_handler(2_000, move || {
                Instant::now() < deadline + Duration::from_secs(1)
            });
        let statement = source.statement(false);
        let rows = sqlx::query_as::<_, SavedRow>(&statement)
            .persistent(false)
            .bind(&allowed)
            .bind(before)
            .bind(HIT_LIMIT as i64 + 1)
            .fetch_all(&mut *conn)
            .await;
        conn.lock_handle().await?.remove_progress_handler();
        sqlite_filter::install(conn, None).await?;
        let filter = filter.lock().map_err(|_| KnowledgeError::Storage)?;
        result.limited |= filter.oversized;
        let mut rows = match rows {
            Ok(rows) => rows,
            Err(sqlx::Error::Database(err)) if err.code().as_deref() == Some("9") => {
                if cursor.is_some() {
                    return Err(KnowledgeError::Busy);
                }
                result.limited = true;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if let Some(unsearched) = filter.first_unsearched {
            rows.retain(|row| row.row_id > unsearched);
        }
        let resume = rows
            .get(HIT_LIMIT)
            .map(|row| row.row_id)
            .or(filter.first_unsearched);
        next[index] = resume.map(|id| id.to_string());
        for row in rows.into_iter().take(HIT_LIMIT) {
            let Some(text) = row
                .body
                .as_deref()
                .and_then(|raw| source.text(raw, row.blocks))
            else {
                continue;
            };
            if let Some((start, end)) = excerpt(&text, query) {
                if let Some(mut hit) = hit(row, source, &text, start, end) {
                    hit.highlights = query.highlights(&hit.text);
                    result.hits.push(hit);
                }
            }
        }
    }
    if next.iter().any(Option::is_some) {
        if one_per_meeting {
            result.limited = true;
        } else {
            result.next = Some(next);
        }
    }
    Ok(result)
}

#[cfg(test)]
pub(crate) async fn search(
    pool: &SqlitePool,
    query: String,
    scope: Option<KnowledgeScope>,
    one_per_meeting: bool,
) -> Result<SavedResults, KnowledgeError> {
    search_query(
        pool,
        LiteralQuery::archive(&query)?,
        scope,
        one_per_meeting,
        None,
    )
    .await
}

async fn search_query(
    pool: &SqlitePool,
    query: LiteralQuery,
    scope: Option<KnowledgeScope>,
    one_per_meeting: bool,
    cursor: Option<SavedCursor>,
) -> Result<SavedResults, KnowledgeError> {
    let permit = literal::search_permit().await?;
    let mut tx = tokio::time::timeout(Duration::from_secs(3), pool.begin())
        .await
        .map_err(|_| KnowledgeError::Busy)??;
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        runtime.block_on(search_rows(
            &mut tx,
            &query,
            scope.as_ref(),
            one_per_meeting,
            cursor,
        ))
    })
    .await
    .map_err(|_| KnowledgeError::Storage)?
}

pub(crate) async fn resolve(
    pool: &SqlitePool,
    reference: SavedReference,
) -> Result<Option<SavedHit>, KnowledgeError> {
    if reference.end <= reference.start
        || reference.end - reference.start > 2048
        || reference.fingerprint.len() != 64
    {
        return Err(KnowledgeError::InvalidInput);
    }
    let row = sqlx::query_as::<_, SavedRow>(&reference.source.statement(true))
        .bind(&reference.meeting_id)
        .bind(&reference.id)
        .fetch_optional(pool)
        .await?;
    tokio::task::spawn_blocking(move || {
        let row = row?;
        let body = reference.source.text(row.body.as_deref()?, row.blocks)?;
        let mut current = hit(row, reference.source, &body, reference.start, reference.end)
            .filter(|current| current.reference.fingerprint == reference.fingerprint)?;
        if reference.source == SavedSource::Note {
            current.note_context = Some((
                body[..reference.start].to_owned(),
                body[reference.end..].to_owned(),
            ));
        }
        Some(current)
    })
    .await
    .map_err(|_| KnowledgeError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::types::{MeetingFilter, TagMatch};
    use serde_json::json;

    async fn search_archive(
        pool: &SqlitePool,
        query: &str,
        scope: Option<KnowledgeScope>,
        one_per_meeting: bool,
    ) -> Result<DesktopResults, KnowledgeError> {
        search_archive_page(pool, query, scope, one_per_meeting, None).await
    }

    #[tokio::test]
    async fn archive_finds_an_older_note_after_many_unrelated_notes() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::raw_sql(
            "WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM n WHERE i<10001)
             INSERT INTO meetings(id,title,created_at,updated_at)
             SELECT 'note-'||i,'Meeting',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP FROM n;
             INSERT INTO meeting_notes(meeting_id,notes_markdown,created_at,updated_at)
             SELECT id,CASE WHEN id='note-0' THEN 'Die Äpfel' ELSE 'Unrelated' END,
                    CURRENT_TIMESTAMP,CURRENT_TIMESTAMP FROM meetings;",
        )
        .execute(&pool)
        .await
        .unwrap();

        let results = search(&pool, "ÄPFEL".into(), None, false).await.unwrap();
        assert_eq!(results.hits.len(), 1);
        assert_eq!(results.hits[0].reference.meeting_id, "note-0");
    }

    #[tokio::test]
    async fn archive_continuation_returns_remaining_saved_hits_and_hides_trash() {
        let pool = crate::database::transcript_edits::tests::fixture().await;
        sqlx::raw_sql(
            "WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM n WHERE i<53)
             INSERT INTO meeting_bookmarks(id,meeting_id,label,seconds)
             SELECT 'mark-'||i,'review-test','Budget',i FROM n;",
        )
        .execute(&pool)
        .await
        .unwrap();
        let first = search_archive(&pool, "Budget", None, false).await.unwrap();
        assert_eq!(first.saved.len(), 50);
        assert_eq!(first.saved[0].reference.id, "mark-53");
        let last = search_archive_page(&pool, "Budget", None, false, first.saved_next)
            .await
            .unwrap();
        assert_eq!(
            last.saved
                .iter()
                .map(|hit| hit.reference.id.as_str())
                .collect::<Vec<_>>(),
            ["mark-3", "mark-2", "mark-1", "mark-0"]
        );
        assert!(last.saved_next.is_none());
        let reference = last.saved[0].reference.clone();
        assert!(
            crate::database::repositories::meeting::MeetingsRepository::move_to_trash(
                &pool,
                "review-test",
                true
            )
            .await
            .unwrap()
        );
        let trashed = search_archive(&pool, "Budget", None, false).await.unwrap();
        assert!(trashed.saved.is_empty());
        assert!(resolve(&pool, reference).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn archive_requires_every_literal_in_one_source_and_respects_quoted_order() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO meetings(id,title,created_at,updated_at) VALUES
                ('match','50%_ und die ÄPFEL auf der Straße','2026-01-01','2026-01-01'),
                ('separate','Äpfel','2026-01-02','2026-01-02');
             INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES
                ('a','match','Die Äpfel auf der Straße kosten 50%_.','00:00'),
                ('b','separate','Straße 50%_','00:00');
             INSERT INTO meeting_bookmarks(id,meeting_id,label,seconds) VALUES
                ('bookmark','match','Straße und rote Äpfel 50%_','1');",
        )
        .execute(&pool)
        .await
        .unwrap();

        let results = search_archive(&pool, "50%_ STRAẞE Äpfel", None, false)
            .await
            .unwrap();
        assert_eq!(
            results
                .titles
                .iter()
                .map(|hit| hit.id.as_str())
                .collect::<Vec<_>>(),
            ["match"]
        );
        assert_eq!(results.passages.len(), 1);
        assert_eq!(results.saved.len(), 1);
        let hit = &results.passages[0].hit;
        let highlights = hit
            .highlights
            .iter()
            .map(|&(start, end)| &hit.text[start..end])
            .collect::<Vec<_>>();
        assert_eq!(highlights, ["Äpfel", "Straße", "50%_"]);

        for query in [
            "„Äpfel auf der Straße“ 50%_",
            "„Äpfel auf der Straße” 50%_",
            "„Äpfel auf der Straße\" 50%_",
        ] {
            let results = search_archive(&pool, query, None, false).await.unwrap();
            assert_eq!(results.titles.len(), 1, "{query}");
            assert_eq!(results.passages.len(), 1, "{query}");
            assert!(results.saved.is_empty(), "{query}");
        }

        for query in [
            "\"50%_ Straße Äpfel",
            "„50%_ Straße Äpfel",
            "“50%_ Straße Äpfel",
        ] {
            let results = search_archive(&pool, query, None, false).await.unwrap();
            assert_eq!(results.titles.len(), 1, "{query}");
            assert_eq!(results.passages.len(), 1, "{query}");
            assert_eq!(results.saved.len(), 1, "{query}");
        }
    }

    #[tokio::test]
    async fn saved_hits_use_current_display_text_and_preserve_meeting_boundaries() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::raw_sql("INSERT INTO meetings(id,title,created_at,updated_at) VALUES
            ('a','One','2026-02-02','2026-02-02'),('b','Two','2026-02-01','2026-02-01');
            INSERT INTO meeting_tags(meeting_id,tag) VALUES ('a','Öffnung');
            INSERT INTO meeting_notes(meeting_id,notes_json,created_at,updated_at) VALUES
            ('a','[{\"content\":[{\"text\":\"Notiz 📘 ÄPFEL 50%_\"}],\"props\":{\"id\":\"hidden-prop\"}}]','2026-02-02','2026-02-02');
            INSERT INTO meeting_bookmarks(id,meeting_id,folder_path,seconds,label) VALUES
            ('one','a',NULL,42.5,'Marker ÄPFEL 50%_'),('two','b',NULL,42.5,'Marker ÄPFEL 50%_'),
            ('pending',NULL,'synthetic-folder',42.5,'Pending-only words');").execute(&pool).await.unwrap();
        let stored = json!({"markdown":"obscured-export","summary_json":[{
            "type":"paragraph","content":[{"type":"text","text":"Summary ÄPFEL 50%_"}],"children":[]
        }], "summary_sources":[{"text":"hidden-source"}], "generation":{"model":"hidden-model"}});
        sqlx::query("INSERT INTO summary_processes(meeting_id,status,result,previous_result,created_at,updated_at)
            VALUES ('a','completed',?,'{\"markdown\":\"old-version\"}','2026-02-02','2026-02-02')")
            .bind(stored.to_string()).execute(&pool).await.unwrap();
        let scope = KnowledgeScope::Library {
            filter: MeetingFilter {
                all_meetings: true,
                tags: vec!["ÖFFNUNG".into()],
                tag_mode: TagMatch::All,
                from: Some("2026-02-02".into()),
                to: Some("2026-02-02".into()),
                ..Default::default()
            },
        };
        let results = search(&pool, "äpfel 50%_".into(), Some(scope), false)
            .await
            .unwrap();
        assert!(!results.limited);
        assert_eq!(
            results
                .hits
                .iter()
                .map(|h| h.reference.source)
                .collect::<Vec<_>>(),
            [
                SavedSource::Summary,
                SavedSource::Note,
                SavedSource::Bookmark
            ]
        );
        for h in &results.hits {
            assert_eq!(h.reference.meeting_id, "a");
            assert_eq!(
                resolve(&pool, h.reference.clone())
                    .await
                    .unwrap()
                    .unwrap()
                    .text,
                h.text
            );
        }
        for excluded in [
            "50%X",
            "old-version",
            "obscured-export",
            "hidden-model",
            "hidden-source",
            "hidden-prop",
            "Pending-only",
        ] {
            assert!(
                search(&pool, excluded.into(), None, false)
                    .await
                    .unwrap()
                    .hits
                    .is_empty(),
                "{excluded}"
            );
        }
        let reference = results.hits[2].reference.clone();
        let mut other = reference.clone();
        other.meeting_id = "b".into();
        other.id = "two".into();
        assert!(resolve(&pool, other).await.unwrap().is_none());
        sqlx::query("UPDATE meeting_bookmarks SET seconds=12 WHERE id='one'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(resolve(&pool, reference).await.unwrap().is_none());
        sqlx::query("DELETE FROM meeting_notes WHERE meeting_id='a'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(resolve(&pool, results.hits[1].reference.clone())
            .await
            .unwrap()
            .is_none());
        sqlx::query("UPDATE summary_processes SET result=zeroblob(?) WHERE meeting_id='a'")
            .bind((MAX_SAVED_TEXT_BYTES + 1) as i64)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            search(&pool, "Summary".into(), None, false)
                .await
                .unwrap()
                .limited
        );
    }

    #[test]
    fn excerpt_handles_expanding_lowercase_and_multibyte_boundaries() {
        let text = format!("{}İ Σίσυφος 📘{}", "界".repeat(128), "界".repeat(128));
        let query = LiteralQuery::archive("\"i\u{307} ΣΊΣΥΦΟΣ 📘\"").unwrap();
        let (start, end) = excerpt(&text, &query).unwrap();
        assert!(text[start..end].contains("İ Σίσυφος 📘"));
    }
}
