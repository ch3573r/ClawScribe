//! Local desktop discovery only. These rows are never transcript evidence.
use super::{folded_char, literal, literal_fold};
use crate::knowledge::{
    chunking::floor_boundary,
    retrieval,
    types::{KnowledgeError, KnowledgeScope},
};
use crate::library::saved_text::{editor_text, markdown_text, summary_text, MAX_SAVED_TEXT_BYTES};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqliteConnection, SqlitePool};
use std::{collections::HashSet, time::Duration};

const SCAN_BYTES: usize = 8 * 1024 * 1024;
const SCAN_ROWS: i64 = 10_000;
const HIT_LIMIT: usize = 50;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    note_context: Option<(String, String)>,
}

#[derive(Default, Serialize)]
pub(crate) struct SavedResults {
    pub hits: Vec<SavedHit>,
    pub limited: bool,
}

#[derive(Default, Serialize)]
pub(crate) struct DesktopResults {
    titles: Vec<literal::TitleHit>,
    passages: Vec<literal::LinkedPassage>,
    saved: Vec<SavedHit>,
    limited: bool,
}

#[tauri::command]
pub(crate) async fn search_saved_meetings(
    state: tauri::State<'_, crate::state::AppState>,
    query: String,
    scope: Option<KnowledgeScope>,
    one_per_meeting: bool,
) -> Result<DesktopResults, String> {
    if matches!(&scope, Some(KnowledgeScope::Library { filter }) if !filter.all_meetings && filter.meeting_ids.is_empty())
    {
        return Ok(DesktopResults::default());
    }
    let pool = state.db_manager.pool();
    let linked = literal::linked(pool, &query, scope.as_ref(), one_per_meeting)
        .await
        .map_err(|err| err.to_string())?;
    let saved = match search(pool, query, scope, one_per_meeting).await {
        Ok(saved) => saved,
        Err(KnowledgeError::Busy) => SavedResults {
            limited: true,
            ..Default::default()
        },
        Err(error) => return Err(error.to_string()),
    };
    Ok(DesktopResults {
        titles: linked.titles,
        passages: linked.passages,
        saved: saved.hits,
        limited: linked.limited || saved.limited,
    })
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
        let restriction = if lookup {
            format!("m.id=?1 AND {id}=?2")
        } else {
            "(?1 IS NULL OR m.id IN (SELECT value FROM json_each(?1))) ORDER BY s.rowid DESC LIMIT ?2".to_string()
        };
        format!("SELECT m.id meeting_id, {id} id, substr(m.title,1,256) title, substr(m.created_at,1,64) date,
            {seconds} seconds, {blocks} blocks,
            CASE WHEN {size}<={MAX_SAVED_TEXT_BYTES} THEN {body} END body
            FROM {table} s JOIN meetings m ON m.id=s.meeting_id WHERE {present} AND {restriction}")
    }
    fn text(self, row: &SavedRow) -> Option<String> {
        let raw = row.body.as_deref()?;
        match self {
            Self::Summary => summary_text(raw),
            Self::Note if row.blocks => editor_text(&serde_json::from_str(raw).ok()?),
            Self::Note => Some(markdown_text(raw)),
            Self::Bookmark => Some(raw.to_owned()),
        }
    }
}

// Return original UTF-8 offsets even when a character folds to several scalars.
fn excerpt(text: &str, pattern: &str) -> Option<(usize, usize)> {
    let found = literal_fold(text).find(pattern)?;
    let mut folded_bytes = 0;
    let mut begin = None;
    for (i, ch) in text.char_indices() {
        folded_bytes += folded_char(ch).map(char::len_utf8).sum::<usize>();
        if folded_bytes > found {
            begin.get_or_insert(i);
        }
        if folded_bytes >= found + pattern.len() {
            let start = floor_boundary(text, begin?.saturating_sub(100));
            let end = floor_boundary(text, i + ch.len_utf8() + 100);
            return Some((start, end));
        }
    }
    None
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
        note_context: None,
    })
}

async fn search_rows(
    conn: &mut SqliteConnection,
    query: &str,
    scope: Option<&KnowledgeScope>,
    one_per_meeting: bool,
) -> Result<SavedResults, KnowledgeError> {
    let mut result = SavedResults::default();
    let pattern = literal_fold(query.trim());
    if pattern.is_empty() {
        return Ok(result);
    }
    if query.trim().chars().count() > literal::MAX_QUERY_CHARS {
        return Err(KnowledgeError::InvalidInput);
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
            _ => {
                result.limited = true;
                return Ok(result);
            }
        }
    } else {
        None
    };
    for source in [
        SavedSource::Summary,
        SavedSource::Note,
        SavedSource::Bookmark,
    ] {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        let statement = source.statement(false);
        let mut rows = sqlx::query_as::<_, SavedRow>(&statement)
            .bind(&allowed)
            .bind(SCAN_ROWS + 1)
            .fetch(&mut *conn);
        let mut seen = HashSet::new();
        let (mut count, mut remaining, mut matches) = (0, SCAN_BYTES, 0);
        loop {
            if tokio::time::Instant::now() >= deadline {
                result.limited = true;
                break;
            }
            let row = match tokio::time::timeout_at(deadline, rows.try_next()).await {
                Ok(Ok(Some(row))) => row,
                Ok(Ok(None)) => break,
                Ok(Err(sqlx::Error::Database(err))) if err.code().as_deref() == Some("9") => {
                    result.limited = true;
                    break;
                }
                Ok(Err(error)) => return Err(error.into()),
                Err(_) => {
                    result.limited = true;
                    break;
                }
            };
            count += 1;
            if count > SCAN_ROWS {
                result.limited = true;
                break;
            }
            let Some(raw) = &row.body else {
                result.limited = true;
                continue;
            };
            let Some(left) = remaining.checked_sub(raw.len()) else {
                result.limited = true;
                break;
            };
            remaining = left;
            if one_per_meeting && seen.contains(&row.meeting_id) {
                continue;
            }
            let Some(text) = source.text(&row) else {
                continue;
            };
            if let Some((start, end)) = excerpt(&text, &pattern) {
                if matches >= HIT_LIMIT {
                    result.limited = true;
                    break;
                }
                if one_per_meeting {
                    seen.insert(row.meeting_id.clone());
                }
                result.hits.extend(hit(row, source, &text, start, end));
                matches += 1;
            }
        }
    }
    Ok(result)
}

pub(crate) async fn search(
    pool: &SqlitePool,
    query: String,
    scope: Option<KnowledgeScope>,
    one_per_meeting: bool,
) -> Result<SavedResults, KnowledgeError> {
    let permit = literal::search_permit().await?;
    let mut tx = tokio::time::timeout(Duration::from_secs(3), pool.begin())
        .await
        .map_err(|_| KnowledgeError::Busy)??;
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        runtime.block_on(async {
            // A SQL statement can still be scanning filtered-out rows when
            // the consumer times out. Also bound the SQLite worker itself.
            let active = std::sync::Arc::new(());
            let running = std::sync::Arc::downgrade(&active);
            let deadline = std::time::Instant::now() + Duration::from_secs(6);
            tx.lock_handle()
                .await?
                .set_progress_handler(2_000, move || {
                    running.strong_count() == 0 || std::time::Instant::now() < deadline
                });
            let result = search_rows(&mut tx, &query, scope.as_ref(), one_per_meeting).await;
            drop(active);
            tx.lock_handle().await?.remove_progress_handler();
            result
        })
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
        let body = reference.source.text(&row)?;
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
        let (start, end) = excerpt(&text, &literal_fold("i\u{307} ΣΊΣΥΦΟΣ 📘")).unwrap();
        assert!(text[start..end].contains("İ Σίσυφος 📘"));
    }
}
