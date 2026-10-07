//! Durable conversation ownership, request identity and source dependencies.
use super::{retrieval, types::*};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection, SqlitePool};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

fn failure(_: sqlx::Error) -> String {
    "Conversation storage failed".into()
}
fn json(value: &impl serde::Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(|_| "Invalid conversation data".into())
}
pub fn owner_key(owner: &ConversationOwner) -> Result<String, String> {
    match owner {
        ConversationOwner::Meeting(id) if !id.trim().is_empty() => Ok(format!("meeting:{id}")),
        ConversationOwner::Library(id) if !id.trim().is_empty() => Ok(format!("library:{id}")),
        _ => Err("Unsupported conversation owner".into()),
    }
}
pub async fn create_library(pool: &SqlitePool) -> Result<ConversationOwner, String> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO knowledge_owners(id,kind) VALUES (?,'library')")
        .bind(format!("library:{id}"))
        .execute(pool)
        .await
        .map_err(failure)?;
    Ok(ConversationOwner::Library(id))
}
pub async fn list_libraries(pool: &SqlitePool) -> Result<Vec<ConversationOwner>, String> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM knowledge_owners WHERE kind='library' ORDER BY created_at,id",
    )
    .fetch_all(pool)
    .await
    .map_err(failure)?;
    Ok(ids
        .into_iter()
        .filter_map(|id| {
            id.strip_prefix("library:")
                .map(|id| ConversationOwner::Library(id.to_owned()))
        })
        .collect())
}
fn validate_request(request: &AskRequest) -> Result<(), String> {
    if uuid::Uuid::parse_str(&request.request_id)
        .map(|id| id.to_string())
        .ok()
        .as_deref()
        != Some(&request.request_id)
        || request.search.query.trim().is_empty()
        || request.search.query.len() > 1024
        || !request.search.document_ids.is_empty()
    {
        return Err("Invalid saved-answer request".into());
    }
    match (&request.owner, &request.search.scope) {
        (ConversationOwner::Meeting(id), KnowledgeScope::Meeting { meeting_id })
            if id == meeting_id =>
        {
            Ok(())
        }
        (ConversationOwner::Library(_), KnowledgeScope::Library { .. }) => Ok(()),
        _ => Err("Conversation owner and scope do not match".into()),
    }
}

/// Returns the original completed reply or reserves exactly one persisted user turn.
pub async fn reserve(
    pool: &SqlitePool,
    request: &AskRequest,
    provider: &str,
    model: &str,
) -> Result<Option<AssistantReply>, String> {
    validate_request(request)?;
    let owner = owner_key(&request.owner)?;
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(json(&(
            "knowledge-input-v1",
            &request.owner,
            &request.search,
            provider,
            model
        ))?)
    );
    let frozen = retrieval::freeze_scope(pool, &request.search.scope)
        .await
        .map_err(|e| e.to_string())?;
    let mut tx = pool.begin().await.map_err(failure)?;
    // Obtain writer ownership before checking or creating any request identity.
    sqlx::query("UPDATE knowledge_owners SET id=id WHERE id=?")
        .bind(&owner)
        .execute(&mut *tx)
        .await
        .map_err(failure)?;
    if let Some(row) =
        sqlx::query("SELECT owner_id,input_fingerprint,status FROM knowledge_requests WHERE id=?")
            .bind(&request.request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(failure)?
    {
        if row.get::<String, _>("owner_id") != owner
            || row.get::<String, _>("input_fingerprint") != fingerprint
        {
            return Err("Request identity was already used with different inputs".into());
        }
        match row.get::<String, _>("status").as_str() {
            "completed" => {
                let reply = reply_in(&mut tx, &request.request_id).await?;
                tx.commit().await.map_err(failure)?;
                return Ok(Some(reply));
            }
            "failed" | "interrupted" => {
                sqlx::query("UPDATE knowledge_requests SET status='preparing',failure=NULL,restored=0,frozen_ids_json=? WHERE id=?").bind(json(&frozen.meeting_ids)?).bind(&request.request_id).execute(&mut *tx).await.map_err(failure)?;
                for table in ["knowledge_request_evidence", "knowledge_request_sources"] {
                    sqlx::query(&format!("DELETE FROM {table} WHERE request_id=?"))
                        .bind(&request.request_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(failure)?;
                }
                tx.commit().await.map_err(failure)?;
                return Ok(None);
            }
            _ => return Err("Request is active, cancelled or invalidated".into()),
        }
    }
    if let ConversationOwner::Meeting(id) = &request.owner {
        sqlx::query("INSERT INTO knowledge_owners(id,kind,meeting_id) SELECT ?,'meeting',id FROM meetings WHERE id=? ON CONFLICT(id) DO NOTHING")
            .bind(&owner).bind(id).execute(&mut *tx).await.map_err(failure)?;
    }
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM knowledge_owners WHERE id=?)")
            .bind(&owner)
            .fetch_one(&mut *tx)
            .await
            .map_err(failure)?;
    if !exists {
        return Err("Conversation owner does not exist".into());
    }
    sqlx::query("INSERT INTO knowledge_requests(id,owner_id,input_fingerprint,question,scope_json,frozen_ids_json,status,provider,model) VALUES (?,?,?,?,?,?,'preparing',?,?)")
        .bind(&request.request_id).bind(owner).bind(fingerprint).bind(&request.search.query).bind(json(&request.search.scope)?).bind(json(&frozen.meeting_ids)?).bind(provider).bind(model).execute(&mut *tx).await.map_err(failure)?;
    sqlx::query("INSERT INTO knowledge_messages(id,request_id,role,content) VALUES (?,?,'user',?)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&request.request_id)
        .bind(&request.search.query)
        .execute(&mut *tx)
        .await
        .map_err(failure)?;
    tx.commit().await.map_err(failure)?;
    Ok(None)
}

pub async fn frozen_scope(pool: &SqlitePool, id: &str) -> Result<retrieval::FrozenScope, String> {
    let row = sqlx::query("SELECT scope_json,frozen_ids_json FROM knowledge_requests WHERE id=?")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(failure)?;
    Ok(retrieval::FrozenScope {
        scope: serde_json::from_str(row.get("scope_json")).map_err(|_| "Invalid saved scope")?,
        meeting_ids: serde_json::from_str(row.get("frozen_ids_json"))
            .map_err(|_| "Invalid frozen scope")?,
    })
}

pub async fn prepare(
    pool: &SqlitePool,
    id: &str,
    frozen: &retrieval::FrozenScope,
    passages: &[Passage],
    inherited: &BTreeMap<String, i64>,
    mode: SearchMode,
) -> Result<(), String> {
    let mut dependencies = inherited.clone();
    for passage in passages {
        if !frozen.meeting_ids.contains(&passage.meeting_id) || passage.evidence.historical {
            return Err("Evidence is outside the current scope".into());
        }
        if dependencies
            .insert(
                passage.evidence.source_id.clone(),
                passage.evidence.source_revision,
            )
            .is_some_and(|revision| revision != passage.evidence.source_revision)
        {
            return Err("Evidence changed while preparing the answer".into());
        }
    }
    let mut tx = pool.begin().await.map_err(failure)?;
    let changed=sqlx::query("UPDATE knowledge_requests SET status='running',retrieval_mode=? WHERE id=? AND status='preparing'")
        .bind(if mode==SearchMode::Hybrid {"hybrid"}else{"keyword"}).bind(id).execute(&mut *tx).await.map_err(failure)?;
    if changed.rows_affected() != 1 {
        return Err("Request is no longer active".into());
    }
    retrieval::recheck_scope_in_connection(&mut tx, frozen)
        .await
        .map_err(|e| e.to_string())?;
    for (source, revision) in dependencies {
        let current: Option<i64> =
            sqlx::query_scalar("SELECT revision FROM knowledge_sources WHERE id=?")
                .bind(&source)
                .fetch_optional(&mut *tx)
                .await
                .map_err(failure)?;
        if current != Some(revision) {
            return Err("Evidence changed before provider dispatch".into());
        }
        sqlx::query(
            "INSERT INTO knowledge_request_sources(request_id,source_id,revision) VALUES (?,?,?)",
        )
        .bind(id)
        .bind(source)
        .bind(revision)
        .execute(&mut *tx)
        .await
        .map_err(failure)?;
    }
    for (index, passage) in passages.iter().enumerate() {
        let display = serde_json::json!({"title":passage.title,"date":passage.date,"speaker":passage.speaker,"metadata_truncated":passage.metadata_truncated});
        sqlx::query("INSERT INTO knowledge_request_evidence(request_id,ordinal,reference_json,display_json) VALUES (?,?,?,?)")
            .bind(id).bind((index+1) as i64).bind(json(&passage.evidence)?).bind(json(&display)?).execute(&mut *tx).await.map_err(failure)?;
    }
    tx.commit().await.map_err(failure)
}

async fn check_in(
    connection: &mut SqliteConnection,
    id: &str,
    frozen: &retrieval::FrozenScope,
) -> Result<(), String> {
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM knowledge_requests WHERE id=? AND status='running' AND restored=0)").bind(id).fetch_one(&mut *connection).await.map_err(failure)?;
    if !active {
        return Err("Request is no longer active".into());
    }
    retrieval::recheck_scope_in_connection(connection, frozen)
        .await
        .map_err(|e| e.to_string())?;
    let invalid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM knowledge_request_sources d LEFT JOIN knowledge_sources s ON s.id=d.source_id WHERE d.request_id=? AND (s.id IS NULL OR s.revision!=d.revision))")
        .bind(id).fetch_one(&mut *connection).await.map_err(failure)?;
    if invalid {
        return Err("Evidence changed during generation".into());
    }
    Ok(())
}
pub async fn check_ready(
    pool: &SqlitePool,
    id: &str,
    frozen: &retrieval::FrozenScope,
) -> Result<(), String> {
    check_in(&mut *pool.acquire().await.map_err(failure)?, id, frozen).await
}
pub async fn finish(
    pool: &SqlitePool,
    id: &str,
    frozen: &retrieval::FrozenScope,
    content: &str,
    token: &CancellationToken,
) -> Result<AssistantReply, String> {
    if content.len() > 256 * 1024 {
        return Err("Answer exceeds the saved-answer limit".into());
    }
    let mut tx = pool.begin().await.map_err(failure)?;
    sqlx::query("UPDATE knowledge_requests SET status=status WHERE id=?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(failure)?;
    if token.is_cancelled() {
        return Err("Answer cancelled".into());
    }
    check_in(&mut tx, id, frozen).await?;
    sqlx::query(
        "INSERT INTO knowledge_messages(id,request_id,role,content) VALUES (?,?,'assistant',?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(id)
    .bind(content)
    .execute(&mut *tx)
    .await
    .map_err(failure)?;
    sqlx::query("UPDATE knowledge_requests SET status='completed',completed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=? AND status='running'").bind(id).execute(&mut *tx).await.map_err(failure)?;
    let reply = reply_in(&mut tx, id).await?;
    if token.is_cancelled() {
        return Err("Answer cancelled".into());
    }
    tx.commit().await.map_err(failure)?;
    Ok(reply)
}
async fn reply_in(connection: &mut SqliteConnection, id: &str) -> Result<AssistantReply, String> {
    let row=sqlx::query("SELECT m.id,m.content,r.provider,r.model,r.retrieval_mode,r.restored FROM knowledge_messages m JOIN knowledge_requests r ON r.id=m.request_id WHERE r.id=? AND m.role='assistant'").bind(id).fetch_one(&mut *connection).await.map_err(failure)?;
    let references: Vec<String> = sqlx::query_scalar(
        "SELECT reference_json FROM knowledge_request_evidence WHERE request_id=? ORDER BY ordinal",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await
    .map_err(failure)?;
    let mut evidence = Vec::<EvidenceRef>::new();
    for reference in references {
        let mut reference: EvidenceRef =
            serde_json::from_str(&reference).map_err(|_| "Invalid saved evidence")?;
        reference.historical |= row.get::<bool, _>("restored");
        evidence.push(reference);
    }
    let content: String = row.get("content");
    let cited_tags = super::evidence::tag_numbers(&content, evidence.len());
    Ok(AssistantReply {
        request_id: id.into(),
        message_id: row.get("id"),
        content,
        evidence,
        cited_tags,
        retrieval_mode: if row.get::<String, _>("retrieval_mode") == "hybrid" {
            SearchMode::Hybrid
        } else {
            SearchMode::Keyword
        },
        provider: row.get("provider"),
        model: row.get("model"),
    })
}
pub async fn cancel(pool: &SqlitePool, id: &str) -> Result<(), String> {
    sqlx::query("UPDATE knowledge_requests SET status='cancelled',failure='cancelled' WHERE id=? AND status IN ('preparing','running','failed','interrupted')").bind(id).execute(pool).await.map_err(failure)?;
    Ok(())
}
pub async fn fail(pool: &SqlitePool, id: &str) -> Result<(), String> {
    sqlx::query("UPDATE knowledge_requests SET status='failed',failure='generation_failed' WHERE id=? AND status IN ('preparing','running')").bind(id).execute(pool).await.map_err(failure)?;
    Ok(())
}
pub async fn clear(pool: &SqlitePool, owner: &ConversationOwner) -> Result<u64, String> {
    let key = owner_key(owner)?;
    let mut tx = pool.begin().await.map_err(failure)?;
    sqlx::query("UPDATE knowledge_requests SET status='cancelled' WHERE owner_id=?")
        .bind(&key)
        .execute(&mut *tx)
        .await
        .map_err(failure)?;
    let mut count = sqlx::query("DELETE FROM knowledge_requests WHERE owner_id=?")
        .bind(key)
        .execute(&mut *tx)
        .await
        .map_err(failure)?
        .rows_affected();
    if let ConversationOwner::Meeting(id) = owner {
        count += sqlx::query("DELETE FROM ai_chat_messages WHERE meeting_id=?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(failure)?
            .rows_affected();
    }
    tx.commit().await.map_err(failure)?;
    Ok(count)
}
pub async fn history(
    pool: &SqlitePool,
    owner: &ConversationOwner,
) -> Result<Vec<HistoryMessage>, String> {
    let key = owner_key(owner)?;
    let mut connection = pool.acquire().await.map_err(failure)?;
    let rows=sqlx::query("SELECT m.id,m.request_id,m.role,m.content,m.created_at,r.status FROM knowledge_messages m JOIN knowledge_requests r ON r.id=m.request_id WHERE r.owner_id=? ORDER BY m.created_at,m.id").bind(key).fetch_all(&mut *connection).await.map_err(failure)?;
    let mut messages = Vec::new();
    for row in rows {
        let role: String = row.get("role");
        let status: String = row.get("status");
        let id: String = row.get("request_id");
        let reply = if role == "assistant" && status == "completed" {
            Some(reply_in(&mut connection, &id).await?)
        } else {
            None
        };
        messages.push(HistoryMessage {
            id: row.get("id"),
            request_id: Some(id),
            role,
            content: row.get("content"),
            created_at: row.get("created_at"),
            status,
            legacy: false,
            reply,
        });
    }
    if let ConversationOwner::Meeting(id) = owner {
        let rows = sqlx::query(
            "SELECT id,role,content,created_at FROM ai_chat_messages WHERE meeting_id=?",
        )
        .bind(id)
        .fetch_all(&mut *connection)
        .await
        .map_err(failure)?;
        messages.extend(rows.into_iter().map(|row| HistoryMessage {
            id: row.get("id"),
            request_id: None,
            role: row.get("role"),
            content: row.get("content"),
            created_at: row.get("created_at"),
            status: "legacy".into(),
            legacy: true,
            reply: None,
        }));
    }
    messages.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    Ok(messages)
}

/// Only verifiable completed turns within the new frozen selection may be sent.
pub async fn eligible_history(
    _pool: &SqlitePool,
    _owner: &ConversationOwner,
    _frozen: &retrieval::FrozenScope,
    _budget: usize,
) -> Result<(String, BTreeMap<String, i64>), String> {
    todo!("Constrain inherited turns and normalize their source dependencies")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn database() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('fixture','Public fixture','2026-09-01','2026-09-01')")
            .execute(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn title_and_date_edits_invalidate_prompt_metadata_snapshot() {
        let pool = database().await;
        let before: i64 =
            sqlx::query_scalar("SELECT revision FROM knowledge_sources WHERE meeting_id='fixture'")
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query("UPDATE meetings SET title='Revised public title' WHERE id='fixture'")
            .execute(&pool)
            .await
            .unwrap();
        let renamed: i64 =
            sqlx::query_scalar("SELECT revision FROM knowledge_sources WHERE meeting_id='fixture'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            renamed,
            before + 1,
            "A queued prompt must detect changed title metadata"
        );
        sqlx::query("UPDATE meetings SET created_at='2026-09-03' WHERE id='fixture'")
            .execute(&pool)
            .await
            .unwrap();
        let redated: i64 =
            sqlx::query_scalar("SELECT revision FROM knowledge_sources WHERE meeting_id='fixture'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            redated,
            renamed + 1,
            "A changed meeting date cannot preserve a dated prompt snapshot"
        );
    }

    #[tokio::test]
    async fn durable_library_history_has_authoritative_tables_without_cache_foreign_keys() {
        let pool = database().await;
        for table in [
            "knowledge_owners",
            "knowledge_requests",
            "knowledge_messages",
            "knowledge_request_evidence",
            "knowledge_request_sources",
        ] {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
            )
            .bind(table)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(exists, "Missing authoritative conversation table: {table}");
            use sqlx::Row;
            let references = sqlx::query(&format!("PRAGMA foreign_key_list({table})"))
                .fetch_all(&pool)
                .await
                .unwrap();
            assert!(
                references.iter().all(|row| !matches!(
                    row.get::<String, _>("table").as_str(),
                    "knowledge_chunks" | "knowledge_vectors" | "knowledge_sources"
                )),
                "History must survive cache/source association rebuilds"
            );
        }
    }

    async fn pending_request(pool: &SqlitePool) {
        sqlx::query("INSERT INTO knowledge_owners(id,kind) VALUES ('library:fixture','library')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO knowledge_requests(id,owner_id,input_fingerprint,question,scope_json,frozen_ids_json,status,provider,model) VALUES ('00000000-0000-4000-8000-000000000001','library:fixture','input','Private dependent question','{}','[\"fixture\"]','running','builtin-ai','qwen3.5:4b')").execute(pool).await.unwrap();
        sqlx::query("INSERT INTO knowledge_messages(id,request_id,role,content) VALUES ('user','00000000-0000-4000-8000-000000000001','user','Private dependent question')").execute(pool).await.unwrap();
        sqlx::query("INSERT INTO knowledge_request_sources(request_id,source_id,revision) VALUES ('00000000-0000-4000-8000-000000000001','meeting:fixture',1)").execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn deleted_source_redacts_dependent_request_before_associations_disappear() {
        let pool = database().await;
        pending_request(&pool).await;
        sqlx::query("DELETE FROM meetings WHERE id='fixture'")
            .execute(&pool)
            .await
            .unwrap();
        let (status, question): (String, String) =
            sqlx::query_as("SELECT status,question FROM knowledge_requests")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "invalidated");
        assert!(question.is_empty());
        let content: String = sqlx::query_scalar("SELECT content FROM knowledge_messages")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(content.is_empty());
        let owners: i64 = sqlx::query_scalar("SELECT count(*) FROM knowledge_owners")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            owners, 1,
            "Independent durable library owner survives deletion"
        );
    }

    #[tokio::test]
    async fn cancelled_request_cannot_commit() {
        let pool = database().await;
        pending_request(&pool).await;
        sqlx::query("UPDATE knowledge_requests SET status='cancelled' WHERE id='00000000-0000-4000-8000-000000000001'").execute(&pool).await.unwrap();
        let insert=sqlx::query("INSERT INTO knowledge_messages(id,request_id,role,content) VALUES ('late','00000000-0000-4000-8000-000000000001','assistant','Late result')").execute(&pool).await;
        assert!(
            insert.is_err(),
            "Cancelled requests must reject late assistant inserts"
        );
    }

    fn request(owner: ConversationOwner) -> AskRequest {
        AskRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            owner,
            search: SearchRequest {
                scope: KnowledgeScope::Library {
                    filter: MeetingFilter {
                        meeting_ids: vec!["fixture".into()],
                        ..Default::default()
                    },
                },
                query: "What was agreed?".into(),
                document_ids: vec![],
                mode: SearchMode::Keyword,
            },
        }
    }
    async fn active(pool: &SqlitePool) -> (AskRequest, retrieval::FrozenScope) {
        let request = request(create_library(pool).await.unwrap());
        assert!(reserve(pool, &request, "builtin-ai", "qwen3.5:4b")
            .await
            .unwrap()
            .is_none());
        let frozen = frozen_scope(pool, &request.request_id).await.unwrap();
        let sources = BTreeMap::from([("meeting:fixture".into(), 1)]);
        prepare(
            pool,
            &request.request_id,
            &frozen,
            &[],
            &sources,
            SearchMode::Keyword,
        )
        .await
        .unwrap();
        (request, frozen)
    }
    #[tokio::test]
    async fn duplicate_request_returns_same_reply() {
        let pool = database().await;
        let (mut request, frozen) = active(&pool).await;
        let first = finish(
            &pool,
            &request.request_id,
            &frozen,
            "Evidence is insufficient.",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let again = reserve(&pool, &request, "builtin-ai", "qwen3.5:4b")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(again.message_id, first.message_id);
        assert_eq!(again.content, first.content);
        let messages = history(&pool, &request.owner).await.unwrap();
        assert_eq!(messages.len(), 2);
        request.search.query = "Different immutable input".into();
        assert!(reserve(&pool, &request, "builtin-ai", "qwen3.5:4b")
            .await
            .is_err());
    }
    #[tokio::test]
    async fn deleted_source_aborts_queued_dispatch() {
        let pool = database().await;
        let (request, frozen) = active(&pool).await;
        sqlx::query("DELETE FROM meetings WHERE id='fixture'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(check_ready(&pool, &request.request_id, &frozen)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn deletion_invalidates_a_preparing_request_before_retrieval_finishes() {
        let pool = database().await;
        let request = request(create_library(&pool).await.unwrap());
        reserve(&pool, &request, "builtin-ai", "qwen3.5:4b")
            .await
            .unwrap();
        sqlx::query("DELETE FROM meetings WHERE id='fixture'")
            .execute(&pool)
            .await
            .unwrap();
        let messages = history(&pool, &request.owner).await.unwrap();
        assert_eq!(messages[0].status, "invalidated");
        assert!(messages[0].content.is_empty());
    }
    #[tokio::test]
    async fn deleted_source_discards_completed_answer() {
        let pool = database().await;
        let (request, frozen) = active(&pool).await;
        sqlx::query("DELETE FROM meetings WHERE id='fixture'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(finish(
            &pool,
            &request.request_id,
            &frozen,
            "A late result",
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert!(history(&pool, &request.owner)
            .await
            .unwrap()
            .iter()
            .all(|message| message.content.is_empty()));
    }
    #[tokio::test]
    async fn metadata_change_during_generation_rejects_late_answer() {
        for statement in [
            "UPDATE meetings SET title='Edited title' WHERE id='fixture'",
            "UPDATE meetings SET created_at='2026-09-05' WHERE id='fixture'",
        ] {
            let pool = database().await;
            let (request, frozen) = active(&pool).await;
            sqlx::query(statement).execute(&pool).await.unwrap();
            assert!(check_ready(&pool, &request.request_id, &frozen)
                .await
                .is_err());
            assert!(finish(
                &pool,
                &request.request_id,
                &frozen,
                "Old metadata",
                &CancellationToken::new()
            )
            .await
            .is_err());
        }
    }
    #[tokio::test]
    async fn clear_removes_legacy_and_new_and_prevents_late_commit() {
        let pool = database().await;
        let mut request = request(ConversationOwner::Meeting("fixture".into()));
        request.search.scope = KnowledgeScope::Meeting {
            meeting_id: "fixture".into(),
        };
        reserve(&pool, &request, "builtin-ai", "qwen3.5:4b")
            .await
            .unwrap();
        let frozen = frozen_scope(&pool, &request.request_id).await.unwrap();
        prepare(
            &pool,
            &request.request_id,
            &frozen,
            &[],
            &BTreeMap::new(),
            SearchMode::Keyword,
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO ai_chat_messages(id,meeting_id,role,content,created_at) VALUES ('legacy','fixture','assistant','Legacy readable output','2026-09-01')").execute(&pool).await.unwrap();
        assert_eq!(history(&pool, &request.owner).await.unwrap().len(), 2);
        clear(&pool, &request.owner).await.unwrap();
        assert!(history(&pool, &request.owner).await.unwrap().is_empty());
        assert!(finish(
            &pool,
            &request.request_id,
            &frozen,
            "Late after clear",
            &CancellationToken::new()
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn filter_change_omits_prior_turn_without_erasing_readable_history() {
        let pool = database().await;
        let (request, frozen) = active(&pool).await;
        finish(
            &pool,
            &request.request_id,
            &frozen,
            "Original project fact [K1]",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let (prior, dependencies) = eligible_history(&pool, &request.owner, &frozen, 4096)
            .await
            .unwrap();
        assert!(prior.contains("Original project fact"));
        assert!(
            !prior.contains("[K1]"),
            "Old request-local tags must not collide with new tags"
        );
        assert_eq!(dependencies.get("meeting:fixture"), Some(&1));
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('other','Different project','2026-09-03','2026-09-03')").execute(&pool).await.unwrap();
        let changed = retrieval::freeze_scope(
            &pool,
            &KnowledgeScope::Meeting {
                meeting_id: "other".into(),
            },
        )
        .await
        .unwrap();
        let (prior, dependencies) = eligible_history(&pool, &request.owner, &changed, 4096)
            .await
            .unwrap();
        assert!(prior.is_empty());
        assert!(dependencies.is_empty());
        assert_eq!(history(&pool, &request.owner).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn inherited_dependencies_redact_a_later_uncited_answer() {
        let pool = database().await;
        let (prior, frozen) = active(&pool).await;
        finish(
            &pool,
            &prior.request_id,
            &frozen,
            "Prior supported fact",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let (_, dependencies) = eligible_history(&pool, &prior.owner, &frozen, 4096)
            .await
            .unwrap();
        let next = request(prior.owner.clone());
        reserve(&pool, &next, "builtin-ai", "qwen3.5:4b")
            .await
            .unwrap();
        prepare(
            &pool,
            &next.request_id,
            &frozen,
            &[],
            &dependencies,
            SearchMode::Keyword,
        )
        .await
        .unwrap();
        finish(
            &pool,
            &next.request_id,
            &frozen,
            "A response using prior history",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        sqlx::query("DELETE FROM meetings WHERE id='fixture'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(history(&pool, &prior.owner)
            .await
            .unwrap()
            .iter()
            .all(|message| message.content.is_empty()));
    }
}
