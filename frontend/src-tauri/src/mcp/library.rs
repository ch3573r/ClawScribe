//! Read-only access to the saved meeting library. Never run application startup or recovery here.
use crate::audio::{constants::AUDIO_EXTENSIONS, recording_preferences};
use crate::database::models::calendar_date;
use crate::exports::actions::{self, ReviewedAction};
use crate::knowledge::{
    embedding::{EmbeddingBackend, OnnxEmbedding},
    model::{VerifiedModel, PINS},
    retrieval,
    store::literal,
    types::{
        EmbeddingPurpose, EvidenceLocator, KnowledgeError, KnowledgeScope, MeetingFilter, TagMatch,
    },
};
use crate::library::saved_text::block_text;
use crate::summary::{processor::validate_saved_summary, sources::strip_source_links};
use futures_util::TryStreamExt;
use serde::Serialize;
use serde_json::{json, Map, Value};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    ConnectOptions, Connection, FromRow, SqliteConnection,
};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{atomic::AtomicBool, Mutex},
    time::{Duration, Instant, SystemTime},
};

const DATABASE: &str = "meeting_minutes.sqlite";
const DB_ERROR: &str = "Could not read the saved library. Open ClawScribe and try again.";
const NOT_FOUND: &str = "Saved meeting not found.";
const TEXT_PAGE: usize = 32_768;
const MAX_STORED_JSON: i64 = 8 * 1024 * 1024;
const INVALID_FILTERS: &str = "Invalid meeting filters.";
const INVALID_PASSAGE_FILTERS: &str = "Invalid passage search filters.";
static SEMANTIC_SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
static EMBEDDING_CACHE: Mutex<Option<CachedEmbedding>> = Mutex::new(None);

#[derive(Debug, PartialEq, Eq)]
struct EmbeddingCacheKey {
    root: PathBuf,
    files: Vec<(u64, SystemTime)>,
}

struct CachedEmbedding {
    key: EmbeddingCacheKey,
    _verified: VerifiedModel,
    backend: OnnxEmbedding,
}

fn embedding_cache_key(root: &Path) -> std::io::Result<EmbeddingCacheKey> {
    let files = PINS
        .files
        .iter()
        .map(|file| {
            let metadata = fs::symlink_metadata(root.join(&file.path))?;
            if !metadata.is_file() || linked(&metadata) {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            Ok((metadata.len(), metadata.modified()?))
        })
        .collect::<std::io::Result<_>>()?;
    Ok(EmbeddingCacheKey {
        root: root.into(),
        files,
    })
}

fn embed_query(root: &Path, query: &str) -> Result<Vec<f32>, KnowledgeError> {
    let mut cache = EMBEDDING_CACHE
        .lock()
        .map_err(|_| KnowledgeError::ModelUnavailable)?;
    let key = match embedding_cache_key(root) {
        Ok(key) => key,
        Err(_) => {
            *cache = None;
            return Err(KnowledgeError::ModelUnavailable);
        }
    };
    if let Some(cached) = cache.as_mut().filter(|cached| cached.key == key) {
        return cached.backend.embed(query, EmbeddingPurpose::Query);
    }
    // Release the old session before loading the new one to cap peak memory.
    *cache = None;
    let verified = VerifiedModel::verify_blocking(root, &AtomicBool::new(false))?;
    let backend = OnnxEmbedding::load(&verified)?;
    if embedding_cache_key(root).ok().as_ref() != Some(&key) {
        return Err(KnowledgeError::ModelUnavailable);
    }
    let cached = cache.insert(CachedEmbedding {
        key,
        _verified: verified,
        backend,
    });
    cached.backend.embed(query, EmbeddingPurpose::Query)
}

#[cfg(test)]
#[test]
fn embedding_cache_key_tracks_root_size_and_modification_time() {
    let root = tempfile::tempdir().unwrap();
    let other_root = tempfile::tempdir().unwrap();
    let baseline = SystemTime::UNIX_EPOCH + Duration::from_secs(86_400);
    for root in [root.path(), other_root.path()] {
        for file in &PINS.files {
            let path = root.join(&file.path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"fixture").unwrap();
            fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(baseline)
                .unwrap();
        }
    }
    let key = embedding_cache_key(root.path()).unwrap();
    let other_key = embedding_cache_key(other_root.path()).unwrap();
    assert_eq!(key, embedding_cache_key(root.path()).unwrap());
    assert_eq!(key.files, other_key.files);
    assert_ne!(key, other_key);

    let path = root.path().join(&PINS.files[0].path);
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(baseline + Duration::from_secs(86_400))
        .unwrap();
    let modified = embedding_cache_key(root.path()).unwrap();
    assert_eq!(key.files[0].0, modified.files[0].0);
    assert_ne!(key, modified);
    file.set_len(1).unwrap();
    file.set_modified(modified.files[0].1).unwrap();
    let resized = embedding_cache_key(root.path()).unwrap();
    assert_eq!(modified.files[0].1, resized.files[0].1);
    assert_ne!(modified, resized);
}

fn read_options(app_data: &Path) -> Result<SqliteConnectOptions, &'static str> {
    let path = app_data.join(DATABASE);
    if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && !linked(&m)) {
        return Err(DB_ERROR);
    }
    Ok(SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false)
        .pragma("query_only", "ON")
        .pragma("trusted_schema", "OFF")
        .busy_timeout(Duration::from_secs(2))
        .disable_statement_logging())
}

async fn connect(app_data: &Path) -> Result<SqliteConnection, &'static str> {
    let mut conn = SqliteConnection::connect_with(&read_options(app_data)?)
        .await
        .map_err(|_| DB_ERROR)?;
    // A literal search can visit every transcript in a large library.
    let deadline = Instant::now() + Duration::from_secs(10);
    conn.lock_handle()
        .await
        .map_err(|_| DB_ERROR)?
        .set_progress_handler(1000, move || Instant::now() < deadline);
    Ok(conn)
}

pub(super) async fn call(
    app_data: &Path,
    name: &str,
    args: &Map<String, Value>,
) -> Result<Value, &'static str> {
    let started = Instant::now();
    let mut conn = connect(app_data).await?;
    let page_arg = |key: &str, default: i64, min: i64, max: i64| match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_i64()
            .filter(|n| (min..=max).contains(n))
            .ok_or("Invalid page arguments."),
    };
    let offset = page_arg("offset", 0, 0, i32::MAX.into())?;
    let limit = page_arg("limit", 25, 1, 100)?;
    let text_offset = page_arg("text_offset", 0, 0, i32::MAX.into())?;
    let metadata_offset = page_arg("metadata_offset", 0, 0, i32::MAX.into())?;
    let identifier = || {
        args.get("meeting_id")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty() && s.chars().count() <= 256)
            .ok_or("Invalid meeting ID.")
    };
    match name {
        "list_meetings" => {
            let scope = meeting_scope(args, INVALID_FILTERS)?;
            list_meetings(&mut conn, scope.as_ref(), limit, offset).await
        }
        "list_open_actions" => list_open_actions(&mut conn, limit, offset).await,
        "search_meetings" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.chars().count() <= 256)
                .ok_or("Invalid search text.")?;
            let permit = literal::search_permit()
                .await
                .map_err(|_| "A library search is in progress.")?;
            let query = query.to_owned();
            let runtime = tokio::runtime::Handle::current();
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                runtime.block_on(search_meetings(&mut conn, &query, limit, offset))
            })
            .await
            .map_err(|_| DB_ERROR)?
        }
        "search_passages" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty() && s.chars().count() <= literal::MAX_QUERY_CHARS)
                .ok_or("Invalid search text.")?
                .to_owned();
            let scope = meeting_scope(args, INVALID_PASSAGE_FILTERS)?;
            let use_semantic = match args.get("semantic") {
                Some(value) => value.as_bool().ok_or("Semantic must be true or false.")?,
                None => false,
            };
            let semantic_input = use_semantic.then(|| (query.clone(), scope.clone()));
            let permit = literal::search_permit()
                .await
                .map_err(|_| "A passage search is still in progress. Try again shortly.")?;
            let runtime = tokio::runtime::Handle::current();
            let matches = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                runtime.block_on(literal::search_transcript_terms(
                    &mut conn,
                    &query,
                    scope.as_ref(),
                    25,
                ))
            })
            .await
            .map_err(|_| DB_ERROR)?
            .map_err(|error| match error {
                KnowledgeError::InvalidInput => INVALID_PASSAGE_FILTERS,
                _ => DB_ERROR,
            })?;
            let mut response = json!({ "passages": matches.passages, "limited": matches.limited });
            if let Some((query, scope)) = semantic_input {
                // Leave time to deliver the keyword results if semantic search
                // stalls after an expensive literal scan.
                let budget = Duration::from_secs(14)
                    .saturating_sub(started.elapsed())
                    .min(Duration::from_secs(8));
                let related =
                    tokio::time::timeout(budget, related_passages(app_data, query, scope))
                        .await
                        .ok()
                        .flatten();
                response["mode"] = json!(if related.is_some() {
                    "hybrid"
                } else {
                    "keyword"
                });
                response["related_passages"] = json!(related.unwrap_or_default());
            }
            Ok(response)
        }
        "get_meeting" => {
            meeting(
                &mut conn,
                identifier()?,
                text_offset as usize,
                metadata_offset,
            )
            .await
        }
        "get_transcript" => transcript(&mut conn, identifier()?, limit, offset, text_offset).await,
        "get_audio" => audio(&mut conn, app_data, identifier()?).await,
        "get_meeting_metadata" => limited_metadata(&mut conn, identifier()?).await,
        "list_reviewed_actions" => reviewed_actions(&mut conn, identifier()?, limit, offset).await,
        _ => Err("Unknown meeting tool."),
    }
}

async fn related_passages(
    app_data: &Path,
    query: String,
    scope: Option<KnowledgeScope>,
) -> Option<Vec<Value>> {
    let mut conn = connect(app_data).await.ok()?;
    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM knowledge_settings WHERE singleton=1")
            .fetch_one(&mut conn)
            .await
            .ok()?;
    if !enabled {
        return None;
    }
    let scope = scope.unwrap_or_else(|| KnowledgeScope::Library {
        filter: MeetingFilter {
            all_meetings: true,
            ..Default::default()
        },
    });
    let frozen = retrieval::freeze_scope_rows(&mut conn, &scope, Some(50_000))
        .await
        .ok()?;
    let space = PINS.space();
    let ready: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM knowledge_sources s
         WHERE s.meeting_id IN (SELECT value FROM json_each(?)) AND s.semantic_space=?
         AND s.semantic_revision=s.revision AND EXISTS (
             SELECT 1 FROM knowledge_chunks c JOIN knowledge_vectors v ON v.chunk_id=c.id
             WHERE c.source_id=s.id AND c.revision=s.revision AND c.generation=s.generation AND v.space=s.semantic_space))",
    ).bind(sqlx::types::Json(&frozen.meeting_ids)).bind(&space.id).fetch_one(&mut conn).await.ok()?;
    if !ready {
        return None;
    }
    let model_root = app_data
        .join("knowledge-models")
        .join("multilingual-e5-small");
    let permit = SEMANTIC_SLOT.try_acquire().ok()?;
    let input = query.clone();
    let vector = tokio::task::spawn_blocking(move || {
        // Hold the permit until the native inference returns, even if the tool
        // call times out or the client cancels.
        let _permit = permit;
        embed_query(&model_root, &input)
    })
    .await
    .ok()?
    .ok()?;
    drop(conn);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(read_options(app_data).ok()?)
        .await
        .ok()?;
    let passages = retrieval::search_channels(&pool, &frozen, &query, Some((&space.id, &vector)))
        .await
        .ok()?;
    if !sqlx::query_scalar::<_, bool>("SELECT enabled FROM knowledge_settings WHERE singleton=1")
        .fetch_one(&pool)
        .await
        .ok()?
    {
        return None;
    }
    Some(
        passages
            .into_iter()
            .filter_map(|passage| {
                let EvidenceLocator::Transcript {
                    transcript_ids,
                    start_seconds,
                    ..
                } = passage.evidence.locator
                else {
                    return None;
                };
                Some(json!({
                    "meeting_id": passage.meeting_id,
                    "transcript_ids": transcript_ids,
                    "speaker": passage.speaker,
                    "start_seconds": start_seconds,
                    "text": passage.text.chars().take(2000).collect::<String>(),
                }))
            })
            .collect(),
    )
}

async fn limited_metadata(conn: &mut SqliteConnection, id: &str) -> Result<Value, &'static str> {
    let (title, created_at, duration, link): (String, String, Option<f64>, Option<String>) = sqlx::query_as(
        "SELECT substr(title,1,1024), substr(created_at,1,64), recording_duration_s,
                CASE WHEN length(CAST(calendar_link AS BLOB)) <= ?2 THEN calendar_link ELSE 'null' END
         FROM meetings WHERE id = ?1",
    )
    .bind(id).bind(MAX_STORED_JSON)
    .fetch_optional(conn).await.map_err(|_| DB_ERROR)?.ok_or(NOT_FOUND)?;
    let title =
        crate::library::calendar::stored_title_policy(link.as_deref()).for_clipped(title, true);
    Ok(json!({
        "meeting_id":id, "title":title, "created_at":created_at, "duration_seconds":duration
    }))
}

async fn reviewed_actions(
    conn: &mut SqliteConnection,
    id: &str,
    limit: i64,
    offset: i64,
) -> Result<Value, &'static str> {
    let (stored, oversized): (Option<String>, bool) = sqlx::query_as(
        "SELECT CASE WHEN length(CAST(reviewed_actions AS BLOB)) <= ?2 THEN reviewed_actions END,
                coalesce(length(CAST(reviewed_actions AS BLOB)),0) > ?2
         FROM meetings WHERE id = ?1",
    )
    .bind(id)
    .bind(MAX_STORED_JSON)
    .fetch_optional(conn)
    .await
    .map_err(|_| DB_ERROR)?
    .ok_or(NOT_FOUND)?;
    if oversized {
        return Err(DB_ERROR);
    }
    let mut rows = stored
        .as_deref()
        .map(actions::decode)
        .transpose()
        .map_err(|_| DB_ERROR)?
        .unwrap_or_default()
        .into_iter()
        .filter(|action| !action.done)
        .skip(offset as usize)
        .take(limit as usize + 1)
        .map(action_metadata)
        .collect::<Vec<_>>();
    let next_offset = (rows.len() > limit as usize).then_some(offset + limit);
    rows.truncate(limit as usize);
    Ok(json!({ "meeting_id":id, "actions":rows, "next_offset":next_offset }))
}

fn action_metadata(action: ReviewedAction) -> Value {
    json!({
        "action_id": action.id, "title": action.title, "owner": action.owner, "due_date": action.due_date,
    })
}

async fn list_open_actions(
    conn: &mut SqliteConnection,
    limit: i64,
    offset: i64,
) -> Result<Value, &'static str> {
    let mut meetings = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT substr(id,1,256), substr(title,1,512),
                CASE WHEN length(CAST(reviewed_actions AS BLOB)) <= ? THEN reviewed_actions END
         FROM meetings WHERE reviewed_actions IS NOT NULL AND reviewed_actions != '[]'
         ORDER BY julianday(created_at) DESC, id",
    )
    .bind(MAX_STORED_JSON)
    .fetch(conn);
    let mut actions = Vec::new();
    let mut skip = offset;
    let mut skipped_meetings = 0;
    'meetings: while let Some((id, title, stored)) =
        meetings.try_next().await.map_err(|_| DB_ERROR)?
    {
        let Some(saved) = stored.as_deref().and_then(|raw| actions::decode(raw).ok()) else {
            skipped_meetings += 1;
            continue;
        };
        for action in saved.into_iter().filter(|action| !action.done) {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let mut row = action_metadata(action);
            row["meeting_id"] = json!(id);
            row["meeting_title"] = json!(title);
            actions.push(row);
            if actions.len() > limit as usize {
                break 'meetings;
            }
        }
    }
    let next_offset = (actions.len() > limit as usize).then_some(offset + limit);
    actions.truncate(limit as usize);
    Ok(
        json!({ "actions": actions, "next_offset": next_offset, "skipped_meetings": skipped_meetings }),
    )
}

#[cfg(test)]
#[tokio::test]
async fn follow_through_reads_only_open_actions_and_filtered_metadata_for_the_requested_meeting() {
    let mut conn = SqliteConnection::connect(":memory:").await.unwrap();
    sqlx::raw_sql(
        "CREATE TABLE meetings(id TEXT, title TEXT, created_at TEXT, recording_duration_s REAL,
            calendar_link TEXT, reviewed_actions TEXT);
        INSERT INTO meetings VALUES('other','Other title','2026-12-01',10,NULL,NULL);",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    let raw = json!([
        {"id":uuid::Uuid::from_u128(1).to_string(),"title":"Open item","owner":"Dana","dueDate":"2026-12-02","done":false,"sourceKey":"12ab"},
        {"id":uuid::Uuid::from_u128(2).to_string(),"title":"Closed item","done":true}
    ]);
    let link = json!({"eventId":"private-event","subject":"Calendar-only title",
        "attendees":[{"email":"attendee@example.com","attendance":"unreviewed"}]});
    sqlx::query("INSERT INTO meetings VALUES ('a','Calendar-only title','2026-12-01',20,?,?)")
        .bind(link.to_string())
        .bind(raw.to_string())
        .execute(&mut conn)
        .await
        .unwrap();
    let info = limited_metadata(&mut conn, "a").await.unwrap();
    assert_eq!(
        info,
        json!({"meeting_id":"a","title":null,"created_at":"2026-12-01","duration_seconds":20.0})
    );
    let rows = reviewed_actions(&mut conn, "a", 1, 0).await.unwrap();
    assert_eq!(
        rows,
        json!({"meeting_id":"a","actions":[
        {"action_id":uuid::Uuid::from_u128(1).to_string(),"title":"Open item","owner":"Dana","due_date":"2026-12-02"}
    ],"next_offset":null})
    );
    assert_eq!(
        reviewed_actions(&mut conn, "other", 1, 0).await.unwrap()["actions"],
        json!([])
    );
    assert_eq!(
        reviewed_actions(&mut conn, "absent", 1, 0)
            .await
            .unwrap_err(),
        NOT_FOUND
    );
}

fn meeting_scope(
    args: &Map<String, Value>,
    invalid: &'static str,
) -> Result<Option<KnowledgeScope>, &'static str> {
    let date = |key| {
        args.get(key)
            .map(|value| {
                value
                    .as_str()
                    .filter(|date| calendar_date(date).is_some())
                    .map(str::to_owned)
                    .ok_or(invalid)
            })
            .transpose()
    };
    let (from, to) = (date("from")?, date("to")?);
    if matches!((&from, &to), (Some(from), Some(to)) if from > to) {
        return Err(invalid);
    }
    let tags = match args.get("tags") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .filter(|tags| tags.len() <= 20)
            .ok_or(invalid)?
            .iter()
            .map(|tag| {
                tag.as_str()
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty() && tag.chars().count() <= 60)
                    .map(str::to_owned)
                    .ok_or(invalid)
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    let tag_mode = match args.get("tag_mode") {
        None => TagMatch::Any,
        Some(value) if value.as_str() == Some("any") => TagMatch::Any,
        Some(value) if value.as_str() == Some("all") => TagMatch::All,
        _ => return Err(invalid),
    };
    let untagged = args
        .get("untagged")
        .map(|v| v.as_bool().ok_or(invalid))
        .transpose()?
        .unwrap_or_default();
    Ok(
        (untagged || !tags.is_empty() || from.is_some() || to.is_some()).then_some(
            KnowledgeScope::Library {
                filter: MeetingFilter {
                    all_meetings: true,
                    tags,
                    tag_mode,
                    untagged,
                    from,
                    to,
                    ..Default::default()
                },
            },
        ),
    )
}

#[derive(FromRow, Serialize)]
struct Meeting {
    id: String,
    title: String,
    created_at: String,
    updated_at: String,
}

async fn list_meetings(
    conn: &mut SqliteConnection,
    scope: Option<&KnowledgeScope>,
    limit: i64,
    offset: i64,
) -> Result<Value, &'static str> {
    let mut tx = conn.begin().await.map_err(|_| DB_ERROR)?;
    let allowed = if let Some(scope) = scope {
        let scope = retrieval::freeze_scope_in_connection(&mut tx, scope)
            .await
            .map_err(|_| DB_ERROR)?;
        Some(sqlx::types::Json(scope.meeting_ids))
    } else {
        None
    };
    let meetings = sqlx::query_as::<_, Meeting>(
        "SELECT substr(m.id,1,256) id, substr(m.title,1,1024) title, substr(m.created_at,1,64) created_at,
                substr(m.updated_at,1,64) updated_at FROM meetings m
         WHERE (?3 IS NULL OR m.id IN (SELECT value FROM json_each(?3)))
         ORDER BY m.created_at DESC, m.id ASC LIMIT ?1 OFFSET ?2",
    )
    .bind(limit + 1)
    .bind(offset)
    .bind(allowed)
    .fetch_all(&mut *tx)
    .await
    .map_err(|_| DB_ERROR)?;
    Ok(meeting_page(meetings, limit, offset))
}

fn meeting_page(mut meetings: Vec<Meeting>, limit: i64, offset: i64) -> Value {
    let next = (meetings.len() > limit as usize).then_some(offset + limit);
    meetings.truncate(limit as usize);
    json!({ "meetings": meetings, "next_offset": next })
}

async fn search_meetings(
    conn: &mut SqliteConnection,
    query: &str,
    limit: i64,
    offset: i64,
) -> Result<Value, &'static str> {
    let mut tx = conn.begin().await.map_err(|_| DB_ERROR)?;
    let anchor = literal::available_anchor(&mut tx, query)
        .await
        .map_err(|_| DB_ERROR)?;
    let candidates = if anchor.is_some() {
        "WHERE meetings.rowid IN (SELECT rowid FROM knowledge_title_literal(?2))
         OR meetings.id IN (SELECT t.meeting_id FROM knowledge_transcript_literal(?2) hit
                            CROSS JOIN transcripts t WHERE t.rowid=hit.rowid)"
    } else {
        ""
    };
    let meeting_sql = format!("SELECT meetings.rowid,substr(id,1,256) id,substr(title,1,1024) title,
        substr(created_at,1,64) created_at,substr(updated_at,1,64) updated_at FROM meetings {candidates}
        ORDER BY meetings.created_at DESC,meetings.id ASC LIMIT 64 OFFSET ?1");
    let transcript_filter = if anchor.is_some() {
        // Probe the index for this segment. Materializing the global hit list
        // once per meeting makes common terms time out on later result pages.
        "AND EXISTS (SELECT 1 FROM knowledge_transcript_literal(?4) hit WHERE hit.rowid=t.rowid)"
    } else {
        ""
    };
    let segment_sql = format!("SELECT t.rowid FROM transcripts t WHERE t.meeting_id=(SELECT id FROM meetings WHERE rowid=?1)
        AND (?2 IS NULL OR t.rowid>?2) {transcript_filter} ORDER BY t.rowid LIMIT ?3");
    let mut matcher = literal::MeetingMatcher::new(query);
    let mut page = Vec::new();
    let mut skip = offset;
    let mut scanned = 0_i64;
    let search_error = |error| match error {
        KnowledgeError::Busy => {
            "Meeting search reached its limit. Narrow the search or use passage filters."
        }
        _ => DB_ERROR,
    };
    loop {
        let mut query = sqlx::query_as::<_, MeetingCandidate>(&meeting_sql).bind(scanned);
        if let Some(anchor) = &anchor {
            query = query.bind(anchor);
        }
        let rows = query.fetch_all(&mut *tx).await.map_err(|_| DB_ERROR)?;
        if rows.is_empty() {
            break;
        }
        scanned += rows.len() as i64;
        for MeetingCandidate { rowid, meeting } in rows {
            let mut matched = matcher
                .matches(
                    &mut tx.lock_handle().await.map_err(|_| DB_ERROR)?,
                    rowid,
                    true,
                )
                .map_err(search_error)?;
            let mut after = None::<i64>;
            while !matched {
                // One confirmed segment is sufficient to include a meeting.
                // For common indexed terms, the first candidate usually matches.
                let batch_size = if anchor.is_some() && after.is_none() {
                    1
                } else {
                    128
                };
                let mut query = sqlx::query_scalar(&segment_sql)
                    .bind(rowid)
                    .bind(after)
                    .bind(batch_size);
                if let Some(anchor) = &anchor {
                    query = query.bind(anchor);
                }
                let ids: Vec<i64> = query.fetch_all(&mut *tx).await.map_err(|_| DB_ERROR)?;
                let mut handle = tx.lock_handle().await.map_err(|_| DB_ERROR)?;
                for &id in &ids {
                    if matcher
                        .matches(&mut handle, id, false)
                        .map_err(search_error)?
                    {
                        matched = true;
                        break;
                    }
                }
                if ids.len() < batch_size as usize {
                    break;
                }
                after = ids.last().copied();
            }
            if matched {
                if skip > 0 {
                    skip -= 1;
                } else {
                    page.push(meeting);
                }
                if page.len() > limit as usize {
                    return Ok(meeting_page(page, limit, offset));
                }
            }
        }
    }
    Ok(meeting_page(page, limit, offset))
}

#[derive(FromRow)]
struct MeetingCandidate {
    rowid: i64,
    #[sqlx(flatten)]
    meeting: Meeting,
}

fn visible_summary(raw: &str) -> Option<String> {
    let saved: Value = serde_json::from_str(raw).ok()?;
    validate_saved_summary(&saved).ok()?;
    if let Some(markdown) = saved
        .get("markdown")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Some(strip_source_links(markdown));
    }
    let mut text = String::new();
    if let Some(blocks) = saved.get("summary_json").and_then(Value::as_array) {
        for block in blocks {
            block_text(block, &mut text);
        }
    } else {
        // Historical summaries stored named sections with a title and plain-text bullets.
        for section in saved.as_object()?.values() {
            if let (Some(title), Some(blocks)) = (
                section.get("title").and_then(Value::as_str),
                section.get("blocks").and_then(Value::as_array),
            ) {
                text.push_str(title);
                text.push('\n');
                for block in blocks {
                    if let Some(content) = block.get("content") {
                        block_text(content, &mut text);
                        text.push('\n');
                    }
                }
            }
        }
    }
    (!text.trim().is_empty()).then(|| strip_source_links(text.trim_end()))
}

fn text_page(text: &str, offset: usize) -> (String, bool) {
    let mut chars = text.chars().skip(offset);
    let page = chars.by_ref().take(TEXT_PAGE).collect();
    (page, chars.next().is_some())
}

async fn meeting(
    conn: &mut SqliteConnection,
    id: &str,
    offset: usize,
    metadata_offset: i64,
) -> Result<Value, &'static str> {
    #[derive(FromRow)]
    struct Details {
        title: String,
        created: String,
        updated: String,
        has_summary: bool,
        summary: Option<String>,
        notes: Option<String>,
        blocks: Option<String>,
        notes_oversize: bool,
    }
    let mut tx = conn.begin().await.map_err(|_| DB_ERROR)?;
    let row = sqlx::query_as::<_, Details>(
        "SELECT substr(m.title,1,1024) title, substr(m.created_at,1,64) created, substr(m.updated_at,1,64) updated,
                s.result IS NOT NULL has_summary,
                CASE WHEN length(CAST(s.result AS BLOB))<=?2 THEN s.result END summary,
                substr(n.notes_markdown,?3+1,?4) notes,
                CASE WHEN n.notes_markdown IS NULL AND length(CAST(n.notes_json AS BLOB))<=?2 THEN n.notes_json END blocks,
                (n.notes_markdown IS NULL AND coalesce(length(CAST(n.notes_json AS BLOB)),0)>?2) notes_oversize
         FROM meetings m LEFT JOIN summary_processes s ON s.meeting_id=m.id
         LEFT JOIN meeting_notes n ON n.meeting_id=m.id WHERE m.id=?1",
    )
    .bind(id)
    .bind(MAX_STORED_JSON)
    .bind(offset as i64)
    .bind(TEXT_PAGE as i64 + 1)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| DB_ERROR)?
    .ok_or(NOT_FOUND)?;
    let metadata = meeting_metadata_page(&mut tx, id, metadata_offset)
        .await
        .map_err(|_| DB_ERROR)?;
    tx.commit().await.map_err(|_| DB_ERROR)?;
    let id = id.to_owned();
    // Parsing and normalizing large saved editor documents must not stall the stdio loop.
    tokio::task::spawn_blocking(move || {
        let summary = row.summary.as_deref().and_then(visible_summary);
        let summary_unavailable = row.has_summary && summary.is_none();
        let (summary, more_summary) = summary.as_deref().map(|s| text_page(s, offset)).unzip();
        let (notes, more_notes) = if let Some(notes) = row.notes {
            Some(text_page(&notes, 0))
        } else {
            row.blocks
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .and_then(|blocks| {
                    let mut notes = String::new();
                    block_text(&blocks, &mut notes);
                    (!notes.trim().is_empty()).then(|| text_page(&notes, offset))
                })
        }
        .unzip();
        let more = more_notes.unwrap_or(false) || more_summary.unwrap_or(false);
        json!({
            "id": id,
            "title": row.title,
            "created_at": row.created,
            "updated_at": row.updated,
            "summary": summary,
            "summary_unavailable": summary_unavailable,
            "notes": notes,
            "notes_unavailable": row.notes_oversize,
            "tags": metadata.tags,
            "bookmarks": metadata.bookmarks,
            "summary_bookmarked": metadata.summary_bookmarked,
            "attachments": metadata.attachments,
            "next_metadata_offset": metadata.next_offset,
            "text_offset": offset,
            "next_text_offset": more.then_some(offset + TEXT_PAGE)
        })
    })
    .await
    .map_err(|_| DB_ERROR)
}

#[derive(Default)]
struct MetadataPage {
    tags: Vec<Value>,
    bookmarks: Vec<Value>,
    attachments: Vec<Value>,
    summary_bookmarked: bool,
    next_offset: Option<i64>,
}

async fn meeting_metadata_page(
    conn: &mut SqliteConnection,
    id: &str,
    offset: i64,
) -> Result<MetadataPage, sqlx::Error> {
    const LIMIT: usize = 25;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_schema WHERE type='table' AND name IN
            ('meeting_tags','meeting_bookmarks','meeting_summary_bookmarks','knowledge_document_attachments')",
    ).fetch_all(&mut *conn).await?;
    let mut page = MetadataPage::default();
    for (table, statement, entries) in [
        ("meeting_tags",
         "SELECT json_quote(substr(tag,1,60)) FROM meeting_tags WHERE meeting_id=?1 ORDER BY tag LIMIT ?2 OFFSET ?3",
         &mut page.tags),
        ("meeting_bookmarks",
         "SELECT json_object('id',substr(id,1,256),'seconds',seconds,'label',substr(label,1,512))
          FROM meeting_bookmarks WHERE meeting_id=?1 AND seconds >= 0 AND seconds < 1e18 ORDER BY seconds,id LIMIT ?2 OFFSET ?3",
         &mut page.bookmarks),
        ("knowledge_document_attachments",
         "SELECT json_object('id',substr(d.id,1,256),'display_name',substr(d.display_name,1,512),
                    'format',substr(d.format,1,16),'file_size',d.file_size)
          FROM knowledge_document_attachments a JOIN knowledge_documents d ON d.id=a.document_id
          WHERE a.meeting_id=?1 ORDER BY a.created_at,d.id LIMIT ?2 OFFSET ?3",
         &mut page.attachments),
    ] {
        if !tables.iter().any(|name| name == table) {
            continue;
        }
        let values: Vec<sqlx::types::Json<Value>> = sqlx::query_scalar(statement)
            .bind(id).bind((LIMIT + 1) as i64).bind(offset)
            .fetch_all(&mut *conn).await?;
        if values.len() > LIMIT {
            page.next_offset = Some(offset + LIMIT as i64);
        }
        entries.extend(values.into_iter().take(LIMIT).map(|entry| entry.0));
    }
    if tables
        .iter()
        .any(|name| name == "meeting_summary_bookmarks")
    {
        page.summary_bookmarked = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM meeting_summary_bookmarks WHERE meeting_id=?)",
        )
        .bind(id)
        .fetch_one(conn)
        .await?;
    }
    Ok(page)
}

async fn transcript(
    conn: &mut SqliteConnection,
    id: &str,
    limit: i64,
    offset: i64,
    text_offset: i64,
) -> Result<Value, &'static str> {
    #[derive(FromRow)]
    struct Segment {
        id: String,
        text: String,
        timestamp: String,
        speaker: Option<String>,
        start: Option<f64>,
        end: Option<f64>,
    }
    let mut tx = conn.begin().await.map_err(|_| DB_ERROR)?;
    if sqlx::query("SELECT 1 FROM meetings WHERE id=?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| DB_ERROR)?
        .is_none()
    {
        return Err(NOT_FOUND);
    }
    // A resumed segment gets a page to itself. On the following call all later rows start at character zero.
    let row_limit = if text_offset > 0 { 1 } else { limit };
    let mut rows = sqlx::query_as::<_, Segment>(
        "SELECT substr(id,1,256) id, substr(transcript,?2+1,?3) text, substr(timestamp,1,64) timestamp,
            substr(speaker,1,256) speaker, audio_start_time start, audio_end_time end
         FROM transcripts AS t WHERE meeting_id=?1 ORDER BY t.audio_start_time,t.id LIMIT ?4 OFFSET ?5",
    )
    .bind(id)
    .bind(text_offset)
    .bind(TEXT_PAGE as i64 + 1)
    .bind(row_limit + 1)
    .bind(offset)
    .fetch(&mut *tx);
    let mut segments = Vec::new();
    let mut remaining = TEXT_PAGE;
    let mut next = None;
    let mut next_text = 0;
    while let Some(row) = rows.try_next().await.map_err(|_| DB_ERROR)? {
        let index = offset + segments.len() as i64;
        if segments.len() == row_limit as usize || remaining == 0 {
            next = Some(index);
            break;
        }
        let mut chars = row.text.chars();
        let text: String = chars.by_ref().take(remaining).collect();
        let count = text.chars().count();
        let continued = chars.next().is_some();
        segments.push(json!({
            "id": row.id,
            "text": text,
            "speaker": row.speaker,
            "timestamp": row.timestamp,
            "audio_start_time": row.start,
            "audio_end_time": row.end,
            "text_offset": text_offset,
            "continued": continued
        }));
        remaining -= count;
        if continued {
            next = Some(index);
            next_text = text_offset + count as i64;
            break;
        }
    }
    drop(rows);
    tx.commit().await.map_err(|_| DB_ERROR)?;
    Ok(json!({"meeting_id":id,"segments":segments,"next_offset":next,"next_text_offset":next_text}))
}

fn linked(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

/// A folder that may be looked at: absolute, without `..`, on a drive, or, when
/// it is on the network, spelled inside a trusted root (a folder the user chose).
fn local_directory(path: &Path, trusted: &[PathBuf]) -> bool {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return false;
    }
    #[cfg(windows)]
    {
        use std::path::Prefix;
        let on_a_drive = matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)));
        let in_a_trusted_root = trusted
            .iter()
            .any(|root| crate::local_path::lexically_inside(&path.to_string_lossy(), root));
        if !on_a_drive && !in_a_trusted_root {
            return false;
        }
    }
    #[cfg(not(windows))]
    let _ = trusted;
    // Check ancestors first so a junction is rejected before probing its target.
    path.ancestors()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .all(|dir| fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir() && !linked(&m)))
}

fn read_small_json(path: &Path) -> Option<Value> {
    const MAX: u64 = 64 * 1024;
    let meta = fs::symlink_metadata(path).ok()?;
    if linked(&meta) || !meta.is_file() || meta.len() > MAX {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX as usize {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn saved_audio(app_data: &Path, folder: &Path, id: &str) -> Option<PathBuf> {
    // The same roots the app uses (the defaults, the folders the user trusted,
    // restored backups), read from the core's trust file because this runs
    // without the app.
    let trusted: Vec<PathBuf> = read_small_json(&app_data.join(crate::trusted_roots::STORE_FILE))
        .as_ref()
        .and_then(|state| state.get("trusted"))
        .and_then(Value::as_array)
        .map(|folders| {
            folders
                .iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    let roots = recording_preferences::roots_from(trusted.clone(), Some(app_data.to_path_buf()));
    let inside = roots.iter().any(|root| {
        folder != root
            && (folder.starts_with(root)
                || crate::local_path::lexically_inside(&folder.to_string_lossy(), root)
                    && crate::local_path::comparable_text(&folder.to_string_lossy())
                        != crate::local_path::comparable_text(&root.to_string_lossy()))
    });
    if !inside || !local_directory(folder, &trusted) {
        return None;
    }
    let metadata = read_small_json(&folder.join("metadata.json"));
    if metadata
        .as_ref()
        .and_then(|m| m["meeting_id"].as_str())
        .is_some_and(|owner| owner != id)
    {
        return None;
    }
    if metadata.is_none()
        && !fs::symlink_metadata(folder.join("recording-outcome.json"))
            .is_ok_and(|m| m.is_file() && !linked(&m))
    {
        return None;
    }
    // Only direct names created by recording, import and recovery may be shared.
    // In particular never follow the audio_file value from a restored metadata.json.
    let names = ["audio-recovered.mp4", "audio-recovered.wav"]
        .into_iter()
        .map(String::from)
        .chain(AUDIO_EXTENSIONS.iter().map(|ext| format!("audio.{ext}")))
        .chain(std::iter::once("recording.mp4".to_owned()));
    names.map(|name| folder.join(name)).find(|file| {
        fs::symlink_metadata(file).is_ok_and(|m| m.is_file() && m.len() > 0 && !linked(&m))
    })
}

async fn audio(
    conn: &mut SqliteConnection,
    app_data: &Path,
    id: &str,
) -> Result<Value, &'static str> {
    let folder = sqlx::query_scalar::<_, Option<String>>(
        "SELECT CASE WHEN length(m.folder_path)<=32767 AND NOT EXISTS (
             SELECT 1 FROM meetings o WHERE o.id<>m.id AND o.folder_path=m.folder_path COLLATE NOCASE
         ) THEN m.folder_path END FROM meetings m WHERE m.id=?",
    )
    .bind(id)
    .fetch_optional(conn)
    .await
    .map_err(|_| DB_ERROR)?
    .ok_or(NOT_FOUND)?;
    let data = app_data.to_owned();
    let owner = id.to_owned();
    let path = tokio::task::spawn_blocking(move || saved_audio(&data, Path::new(&folder?), &owner))
        .await
        .map_err(|_| "Could not inspect saved audio.")?;
    Ok(json!({"meeting_id":id,"path":path}))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (tempfile::TempDir, SqliteConnection) {
        let root = tempfile::tempdir().unwrap();
        let mut writer = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(root.path().join(DATABASE))
                .create_if_missing(true),
        )
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE meetings(id TEXT PRIMARY KEY, title TEXT, created_at TEXT, updated_at TEXT, folder_path TEXT,
                 reviewed_actions TEXT);
             CREATE TABLE transcripts(id TEXT PRIMARY KEY, meeting_id TEXT, transcript TEXT, original_transcript TEXT,
                  timestamp TEXT, speaker TEXT, audio_start_time REAL, audio_end_time REAL);
             CREATE TABLE meeting_notes (meeting_id TEXT PRIMARY KEY, notes_markdown TEXT, notes_json TEXT);
             CREATE TABLE summary_processes (meeting_id TEXT PRIMARY KEY, result TEXT);
             CREATE TABLE meeting_tags (meeting_id TEXT, tag TEXT);
             CREATE TABLE meeting_bookmarks (id TEXT, meeting_id TEXT, seconds REAL, label TEXT, folder_path TEXT);
             CREATE TABLE meeting_summary_bookmarks (meeting_id TEXT);
             CREATE TABLE knowledge_documents (id TEXT, display_name TEXT, format TEXT, file_size INTEGER, storage_name TEXT);
             CREATE TABLE knowledge_document_attachments (meeting_id TEXT, document_id TEXT, created_at TEXT);
             CREATE TABLE knowledge_sources (meeting_id TEXT, revision INTEGER, generation INTEGER);
             CREATE TABLE knowledge_document_blocks (text TEXT);
             INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('a','Planning','2024-02-01','2024-02-02'),('b','Review','2024-02-01','2024-02-01');
             INSERT INTO transcripts VALUES ('second','a','Corrected 10%','Superseded','00:01','Alex',1.0,2.0),
                 ('first','a','Early words',NULL,'00:00',NULL,0.0,1.0);
             INSERT INTO knowledge_document_blocks VALUES ('Attached reference');
             INSERT INTO meeting_notes VALUES ('a','Existing meeting note',NULL);"
        ).execute(&mut writer).await.unwrap();
        (root, writer)
    }

    async fn run(root: &Path, name: &str, args: Value) -> Value {
        call(root, name, args.as_object().unwrap()).await.unwrap()
    }

    #[tokio::test]
    async fn never_creates_or_modifies_the_library() {
        let empty = tempfile::tempdir().unwrap();
        assert!(connect(empty.path()).await.is_err());
        assert_eq!(fs::read_dir(empty.path()).unwrap().count(), 0);
        let (root, _writer) = fixture().await;
        let mut reader = connect(root.path()).await.unwrap();
        assert!(sqlx::query("DELETE FROM meetings")
            .execute(&mut reader)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn list_search_and_transcript_use_current_rows_and_stable_pages() {
        let (root, _writer) = fixture().await;
        let data = root.path();
        let first = run(data, "list_meetings", json!({"limit":1})).await;
        assert_eq!(first["meetings"][0]["id"], "a");
        assert_eq!(first["next_offset"], 1);
        let second = run(data, "list_meetings", json!({"limit":1,"offset":1})).await;
        assert_eq!(second["meetings"][0]["id"], "b");
        assert!(second["next_offset"].is_null());
        for (query, count) in [
            ("10%", 1),
            ("%", 1),
            ("rev", 1),
            ("Superseded", 0),
            ("Attached reference", 0),
            ("' OR 1=1 --", 0),
        ] {
            let matches = run(data, "search_meetings", json!({"query":query})).await;
            assert_eq!(matches["meetings"].as_array().unwrap().len(), count);
        }
        let transcript = run(
            data,
            "get_transcript",
            json!({"meeting_id":"a","limit":1,"offset":1}),
        )
        .await;
        let segment = &transcript["segments"][0];
        assert_eq!(segment["text"], "Corrected 10%");
        assert_eq!(segment["speaker"], "Alex");
        assert_eq!(segment["timestamp"], "00:01");
        assert_eq!(segment["audio_start_time"], 1.0);
        assert_eq!(transcript["next_offset"], Value::Null);
        assert!(!transcript.to_string().contains("Superseded"));
        let missing = json!({"meeting_id":"missing"});
        assert_eq!(
            call(data, "get_transcript", missing.as_object().unwrap())
                .await
                .unwrap_err(),
            NOT_FOUND
        );
    }

    #[tokio::test]
    async fn meeting_search_pages_unicode_title_and_current_transcript_matches_only() {
        let (root, mut writer) = fixture().await;
        sqlx::raw_sql("UPDATE meetings SET title='ÄPFEL 50%_' WHERE id='b';
            UPDATE transcripts SET transcript='📘 Äpfel 50%_', original_transcript='history only' WHERE id='second';
            INSERT INTO summary_processes VALUES ('b','{\"markdown\":\"summary only\"}');
            INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('c','Other','2026-01-01','2026-01-01');
            INSERT INTO meeting_notes VALUES ('c','ÄPFEL 50%_',NULL);
            INSERT INTO meeting_bookmarks(id,meeting_id,label) VALUES ('mark','c','bookmark only');")
            .execute(&mut writer).await.unwrap();
        let old: i64 = sqlx::query_scalar("SELECT instr(lower('ÄPFEL 50%_'), lower('äpfel 50%_'))")
            .fetch_one(&mut writer)
            .await
            .unwrap();
        assert_eq!(old, 0);
        for (offset, id, next) in [(0, "a", json!(1)), (1, "b", Value::Null)] {
            let page = run(
                root.path(),
                "search_meetings",
                json!({"query":"äpfel 50%_","limit":1,"offset":offset}),
            )
            .await;
            assert_eq!(page["meetings"][0]["id"], id);
            assert_eq!(page["next_offset"], next);
            assert_eq!(page.as_object().unwrap().len(), 2);
            assert_eq!(page["meetings"][0].as_object().unwrap().len(), 4);
        }
        for query in [
            "history only",
            "summary only",
            "Existing meeting note",
            "bookmark only",
            "Attached reference",
            "50%x",
        ] {
            assert_eq!(
                run(root.path(), "search_meetings", json!({"query":query})).await["meetings"],
                json!([])
            );
        }
    }

    #[tokio::test]
    async fn indexed_search_pages_meetings_after_rejecting_partial_segment_matches() {
        let (root, mut writer) = fixture().await;
        sqlx::raw_sql(
            "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<239)
             INSERT INTO meetings(id,title,created_at,updated_at)
             SELECT printf('m%03d',x),'Planning','2025-03-01','2025-03-01' FROM n;
             WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<15)
             INSERT INTO transcripts(id,meeting_id,transcript)
             SELECT m.id||'-'||x,m.id,CASE WHEN x=0 THEN 'shipping' ELSE 'Ö shipping' END
             FROM meetings m CROSS JOIN n WHERE m.id GLOB 'm[0-9]*' ORDER BY m.id,x;
             CREATE INDEX by_meeting ON transcripts(meeting_id);
             CREATE VIRTUAL TABLE knowledge_title_literal USING fts5(title,content='meetings',tokenize='trigram');
             CREATE VIRTUAL TABLE knowledge_transcript_literal USING fts5(transcript,content='transcripts',tokenize='trigram');
             INSERT INTO knowledge_title_literal(knowledge_title_literal) VALUES('rebuild');
             INSERT INTO knowledge_transcript_literal(knowledge_transcript_literal) VALUES('rebuild');",
        )
        .execute(&mut writer)
        .await
        .unwrap();
        let mut reader = connect(root.path()).await.unwrap();
        let page = search_meetings(&mut reader, "ö shipping", 10, 200)
            .await
            .unwrap();
        let hits = page["meetings"].as_array().unwrap();
        assert_eq!(hits.len(), 10);
        assert_eq!(hits.first().unwrap()["id"], "m200");
        assert_eq!(hits.last().unwrap()["id"], "m209");
        assert_eq!(page["next_offset"], 210);
    }

    #[tokio::test]
    async fn literal_passages_use_inclusive_dates_and_unicode_project_tags() {
        let (root, mut writer) = fixture().await;
        sqlx::raw_sql(
            "INSERT INTO meeting_tags VALUES ('a','Öffnung'),('a','Fixture');
             UPDATE transcripts SET transcript='📘 ÄPFEL 50%_ da', original_transcript='obsolete' WHERE id='second';
             INSERT INTO transcripts VALUES ('other','b','Äpfel 50%_ elsewhere',NULL,'00:00',NULL,0.0,1.0);",
        ).execute(&mut writer).await.unwrap();
        let hits = run(
            root.path(),
            "search_passages",
            json!({
                "query": "50%_ äpfel", "from": "2024-02-01", "to": "2024-02-01",
                "tags": ["ÖFFNUNG", "Fixture"], "tag_mode": "all"
            }),
        )
        .await;
        assert_eq!(hits["passages"].as_array().unwrap().len(), 1);
        assert_eq!(hits["passages"][0]["meeting_id"], "a");
        assert_eq!(hits["passages"][0]["transcript_id"], "second");
        assert_eq!(hits["passages"][0]["text"], "📘 ÄPFEL 50%_ da");
        assert_eq!(hits["limited"], false);
        for args in [
            json!({"query":"obsolete"}),
            json!({"query":"äpfel Early"}),
            json!({"query":"Äpfel", "tags":["Fixture","Missing"], "tag_mode":"all"}),
            json!({"query":"Äpfel", "from":"2024-02-02"}),
        ] {
            assert_eq!(
                run(root.path(), "search_passages", args).await["passages"],
                json!([])
            );
        }
        let invalid = json!({"query":"Äpfel", "from":"2024-02-30"});
        assert_eq!(
            call(root.path(), "search_passages", invalid.as_object().unwrap())
                .await
                .unwrap_err(),
            "Invalid passage search filters."
        );
        let fallback = run(
            root.path(),
            "search_passages",
            json!({"query":"50%_ äpfel", "semantic":true}),
        )
        .await;
        assert_eq!(fallback["mode"], "keyword");
        assert_eq!(fallback["passages"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn lists_filtered_meetings_and_paginates_open_actions_across_meetings() {
        let (root, mut writer) = fixture().await;
        sqlx::raw_sql(
            "UPDATE meetings SET created_at='2024-02-02' WHERE id='b';
            INSERT INTO meeting_tags VALUES ('a','Übung'),('a','Project');",
        )
        .execute(&mut writer)
        .await
        .unwrap();
        for (meeting, action) in [("b", 1_u128), ("a", 2_u128)] {
            let actions = json!([
                {"id":uuid::Uuid::from_u128(action).to_string(),"title":"Review item","owner":"Alex","done":false,"details":"private detail"},
                {"id":uuid::Uuid::from_u128(action + 10).to_string(),"title":"Completed item","done":true}
            ]);
            sqlx::query("UPDATE meetings SET reviewed_actions=? WHERE id=?")
                .bind(actions.to_string())
                .bind(meeting)
                .execute(&mut writer)
                .await
                .unwrap();
        }
        let filtered = run(
            root.path(),
            "list_meetings",
            json!({
                "tags":["ÜBUNG","project"],"tag_mode":"all","from":"2024-02-01","to":"2024-02-01"
            }),
        )
        .await;
        assert_eq!(filtered["meetings"].as_array().unwrap().len(), 1);
        assert_eq!(filtered["meetings"][0]["id"], "a");
        let page = run(root.path(), "list_open_actions", json!({"limit":1})).await;
        assert_eq!(page["actions"][0]["meeting_id"], "b");
        assert_eq!(
            page["actions"][0]["action_id"],
            uuid::Uuid::from_u128(1).to_string()
        );
        assert_eq!(page["next_offset"], 1);
        assert!(!page.to_string().contains("private detail"));
        let next = run(
            root.path(),
            "list_open_actions",
            json!({"limit":1,"offset":1}),
        )
        .await;
        assert_eq!(next["actions"][0]["meeting_id"], "a");
        assert!(next["next_offset"].is_null());
    }

    #[tokio::test]
    async fn meeting_metadata_contains_only_attached_references_and_visible_bookmarks() {
        let (root, mut writer) = fixture().await;
        sqlx::raw_sql("INSERT INTO meeting_tags VALUES ('a','Project');
            INSERT INTO meeting_bookmarks VALUES ('mark','a',91.5,'Decision','private-folder');
            INSERT INTO meeting_summary_bookmarks VALUES ('a');
            INSERT INTO knowledge_documents VALUES
                ('reference','Agenda','pdf',120,'private-storage'), ('other','Private document','txt',20,'private-storage');
            INSERT INTO knowledge_document_attachments VALUES ('a','reference','2024-02-01'),('b','other','2024-02-01');")
            .execute(&mut writer).await.unwrap();
        let details = run(root.path(), "get_meeting", json!({"meeting_id":"a"})).await;
        assert_eq!(details["tags"], json!(["Project"]));
        assert_eq!(
            details["bookmarks"],
            json!([{"id":"mark","seconds":91.5,"label":"Decision"}])
        );
        assert_eq!(details["summary_bookmarked"], true);
        assert_eq!(
            details["attachments"],
            json!([{"id":"reference","display_name":"Agenda","format":"pdf","file_size":120}])
        );
        assert!(!details.to_string().contains("private-"));
        assert!(!details.to_string().contains("Private document"));
    }

    #[tokio::test]
    async fn summary_and_notes_include_only_visible_text_and_continue_by_character() {
        let (root, mut writer) = fixture().await;
        let summary = format!("{}end", "ä".repeat(TEXT_PAGE));
        let stored = json!({"markdown":summary,"codex":{"processing_log_path":"private-provider-log"},"summary_sources":[{"text":"unshared source"}]});
        sqlx::query("INSERT INTO summary_processes VALUES ('a',?)")
            .bind(stored.to_string())
            .execute(&mut writer)
            .await
            .unwrap();
        let details = run(root.path(), "get_meeting", json!({"meeting_id":"a"})).await;
        assert_eq!(
            details["summary"].as_str().unwrap().chars().count(),
            TEXT_PAGE
        );
        assert_eq!(details["notes"], "Existing meeting note");
        assert!(!details.to_string().contains("private-provider-log"));
        assert!(!details.to_string().contains("unshared source"));
        let next = run(
            root.path(),
            "get_meeting",
            json!({"meeting_id":"a","text_offset":details["next_text_offset"]}),
        )
        .await;
        assert_eq!(next["summary"], "end");
        assert!(next["next_text_offset"].is_null());
        let blocks = json!({"summary_json":[{"type":"paragraph","props":{"id":"internal"},"content":[{"type":"link","href":"internal","content":[{"text":"Edited"}]}],"children":[]}]});
        assert_eq!(
            visible_summary(&blocks.to_string()).as_deref(),
            Some("Edited")
        );
        assert_eq!(
            visible_summary(r#"{"markdown":"<think>hidden</think>"}"#),
            None
        );
    }

    #[tokio::test]
    async fn a_long_segment_can_be_reassembled_across_pages_without_skipping_the_following_segment()
    {
        let (root, mut writer) = fixture().await;
        let text = format!("{}finish", "語".repeat(TEXT_PAGE));
        sqlx::query("UPDATE transcripts SET transcript=? WHERE id='first'")
            .bind(&text)
            .execute(&mut writer)
            .await
            .unwrap();
        let page1 = run(root.path(), "get_transcript", json!({"meeting_id":"a"})).await;
        assert_eq!(page1["next_offset"], 0);
        assert_eq!(page1["segments"][0]["continued"], true);
        let page2 = run(root.path(),"get_transcript",json!({"meeting_id":"a","offset":page1["next_offset"],"text_offset":page1["next_text_offset"]})).await;
        assert_eq!(page2["segments"][0]["text"], "finish");
        assert_eq!(page2["segments"][0]["text_offset"], TEXT_PAGE);
        assert_eq!(page2["next_offset"], 1);
        assert_eq!(page2["next_text_offset"], 0);
        let page3 = run(
            root.path(),
            "get_transcript",
            json!({"meeting_id":"a","offset":page2["next_offset"]}),
        )
        .await;
        assert_eq!(page3["segments"][0]["id"], "second");
    }

    #[tokio::test]
    async fn returns_only_existing_meeting_audio_and_never_recovers_or_follows_metadata_paths() {
        let (root, mut writer) = fixture().await;
        let folder = root.path().join("restored-recordings").join("test");
        fs::create_dir_all(folder.join(".checkpoints")).unwrap();
        fs::write(
            folder.join("metadata.json"),
            r#"{"meeting_id":"a","audio_file":"../unrelated.wav"}"#,
        )
        .unwrap();
        fs::write(folder.join("unrelated.wav"), b"other file").unwrap();
        fs::write(folder.join(".checkpoints/audio_chunk_0.mp4"), b"incomplete").unwrap();
        sqlx::query("UPDATE meetings SET folder_path=? WHERE id='a'")
            .bind(folder.to_str())
            .execute(&mut writer)
            .await
            .unwrap();
        let args = json!({"meeting_id":"a"});
        assert!(run(root.path(), "get_audio", args.clone()).await["path"].is_null());
        let file = folder.join("audio.wav");
        fs::write(&file, b"synthetic").unwrap();
        assert_eq!(
            run(root.path(), "get_audio", args.clone()).await["path"],
            file.to_str().unwrap()
        );
        fs::write(folder.join("metadata.json"), r#"{"meeting_id":"b"}"#).unwrap();
        assert!(run(root.path(), "get_audio", args.clone()).await["path"].is_null());
        fs::write(folder.join("metadata.json"), "{}").unwrap();
        sqlx::query("UPDATE meetings SET folder_path=? WHERE id='b'")
            .bind(folder.to_str())
            .execute(&mut writer)
            .await
            .unwrap();
        assert!(run(root.path(), "get_audio", args).await["path"].is_null());
    }

    #[test]
    fn audio_in_a_trusted_recordings_folder_is_found_but_an_unknown_folder_is_not() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("current");
        let old = root.path().join("old-recordings");
        let stranger = root.path().join("stranger");
        for folder in [&current, &old, &stranger] {
            let meeting = folder.join("meeting");
            fs::create_dir_all(&meeting).unwrap();
            fs::write(meeting.join("metadata.json"), "{}").unwrap();
            fs::write(meeting.join("audio.wav"), b"synthetic").unwrap();
        }
        let write_preferences = |trusted: serde_json::Value| {
            fs::write(
                root.path().join(crate::trusted_roots::STORE_FILE),
                serde_json::to_vec(&json!({ "trusted": trusted, "takeover_done": true })).unwrap(),
            )
            .unwrap();
        };
        write_preferences(json!([current, old]));
        assert!(saved_audio(root.path(), &current.join("meeting"), "a").is_some());
        assert!(saved_audio(root.path(), &old.join("meeting"), "a").is_some());
        assert!(saved_audio(root.path(), &stranger.join("meeting"), "a").is_none());
        // Once the folder is no longer trusted, its audio is out of reach again.
        write_preferences(json!([current]));
        assert!(saved_audio(root.path(), &old.join("meeting"), "a").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_linked_audio_and_linked_ancestors() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let recordings = root.path().join("restored-recordings");
        let folder = recordings.join("meeting");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("metadata.json"), "{}").unwrap();
        let outside = root.path().join("other.wav");
        fs::write(&outside, b"synthetic").unwrap();
        symlink(&outside, folder.join("audio.wav")).unwrap();
        assert!(saved_audio(root.path(), &folder, "a").is_none());
        fs::remove_file(folder.join("audio.wav")).unwrap();
        fs::write(folder.join("audio.wav"), b"synthetic").unwrap();
        let alias = recordings.join("alias");
        symlink(&folder, &alias).unwrap();
        assert!(saved_audio(root.path(), &alias, "a").is_none());
    }
}
