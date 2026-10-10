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
pub(crate) fn validate_request(request: &AskRequest) -> Result<(), String> {
    if uuid::Uuid::parse_str(&request.request_id)
        .map(|id| id.to_string())
        .ok()
        .as_deref()
        != Some(&request.request_id)
        || request.search.query.trim().is_empty()
        || request.search.query.len() > 1024
        || request.search.document_ids.len() > super::evidence::MAX_EVIDENCE_ENTRIES
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

pub(crate) fn input_fingerprint(
    request: &AskRequest,
    provider: &str,
    model: &str,
) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(json(&(
            "knowledge-input-v1",
            &request.owner,
            &request.search,
            provider,
            model
        ))?)
    ))
}

/// Completed idempotent reads use the original immutable dispatch identity and
/// do not require any currently connected provider or valid credentials.
pub(crate) async fn completed_reply(
    pool: &SqlitePool,
    request: &AskRequest,
) -> Result<Option<AssistantReply>, String> {
    validate_request(request)?;
    let mut tx = pool.begin().await.map_err(failure)?;
    let Some(row) = sqlx::query("SELECT owner_id,input_fingerprint,provider,model,status FROM knowledge_requests WHERE id=?").bind(&request.request_id).fetch_optional(&mut *tx).await.map_err(failure)? else {return Ok(None);};
    if row.get::<String, _>("owner_id") != owner_key(&request.owner)?
        || input_fingerprint(request, row.get("provider"), row.get("model"))?
            != row.get::<String, _>("input_fingerprint")
    {
        return Err("Request identity was already used with different inputs".into());
    }
    match row.get::<String, _>("status").as_str() {
        "completed" => {
            let reply = reply_in(&mut tx, &request.request_id).await?;
            tx.commit().await.map_err(failure)?;
            Ok(Some(reply))
        }
        "failed" | "interrupted" => Ok(None),
        _ => Err("Request is active, cancelled or invalidated".into()),
    }
}

/// Returns the original completed reply or reserves exactly one persisted user turn.
pub async fn reserve(
    pool: &SqlitePool,
    request: &AskRequest,
    provider: &str,
    model: &str,
) -> Result<Option<AssistantReply>, String> {
    reserve_cancellable(pool, request, provider, model, &CancellationToken::new()).await
}

pub(crate) async fn reserve_cancellable(
    pool: &SqlitePool,
    request: &AskRequest,
    provider: &str,
    model: &str,
    token: &CancellationToken,
) -> Result<Option<AssistantReply>, String> {
    validate_request(request)?;
    if token.is_cancelled() {
        return Err("Answer cancelled before reservation".into());
    }
    super::document_context::check_sharing(
        pool,
        &request.owner,
        provider,
        !request.search.document_ids.is_empty(),
    )
    .await?;
    let owner = owner_key(&request.owner)?;
    let fingerprint = input_fingerprint(request, provider, model)?;
    let frozen = retrieval::freeze_search_in_connection(
        &mut *pool.acquire().await.map_err(failure)?,
        &request.search,
    )
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
        sqlx::query("SELECT owner_id,input_fingerprint,status,frozen_ids_json FROM knowledge_requests WHERE id=?")
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
                if token.is_cancelled() {return Err("Answer cancelled".into());}
                tx.commit().await.map_err(failure)?;
                return Ok(Some(reply));
            }
            "failed" | "interrupted" => {
                let original:Vec<String>=serde_json::from_str(row.get("frozen_ids_json")).map_err(|_|"Invalid frozen scope")?;
                if original.iter().any(|id|!frozen.meeting_ids.contains(id)){return Err("Original request scope is no longer available".into());}
                sqlx::query("UPDATE knowledge_requests SET status='preparing',failure=NULL,restored=0 WHERE id=?").bind(&request.request_id).execute(&mut *tx).await.map_err(failure)?;
                for table in ["knowledge_request_evidence", "knowledge_request_sources"] {
                    sqlx::query(&format!("DELETE FROM {table} WHERE request_id=?"))
                        .bind(&request.request_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(failure)?;
                }
                let original_scope = retrieval::FrozenScope {scope: request.search.scope.clone(), meeting_ids: original.clone(), document_ids: frozen.document_ids.clone()};
                retrieval::recheck_scope_in_connection(&mut tx, &original_scope).await.map_err(|error|error.to_string())?;
                reserve_dependencies(&mut tx,&request.request_id,&original_scope).await?;
                if token.is_cancelled() {return Err("Answer cancelled before reservation".into());}
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
    retrieval::recheck_scope_in_connection(&mut tx, &frozen)
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query("INSERT INTO knowledge_requests(id,owner_id,input_fingerprint,input_json,question,scope_json,frozen_ids_json,status,provider,model) VALUES (?,?,?,?,?,?,?,'preparing',?,?)")
        .bind(&request.request_id).bind(owner).bind(fingerprint).bind(json(request)?).bind(&request.search.query).bind(json(&request.search.scope)?).bind(json(&frozen.meeting_ids)?).bind(provider).bind(model).execute(&mut *tx).await.map_err(failure)?;
    reserve_dependencies(&mut tx, &request.request_id, &frozen).await?;
    sqlx::query("INSERT INTO knowledge_messages(id,request_id,role,content) VALUES (?,?,'user',?)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&request.request_id)
        .bind(&request.search.query)
        .execute(&mut *tx)
        .await
        .map_err(failure)?;
    if token.is_cancelled() {
        return Err("Answer cancelled before reservation".into());
    }
    tx.commit().await.map_err(failure)?;
    Ok(None)
}

async fn reserve_dependencies(
    connection: &mut SqliteConnection,
    id: &str,
    frozen: &retrieval::FrozenScope,
) -> Result<(), String> {
    let inserted=sqlx::query("INSERT INTO knowledge_request_sources(request_id,source_id,revision) SELECT ?,s.id,s.revision FROM knowledge_sources s WHERE (s.kind='meeting' AND s.meeting_id IN(SELECT value FROM json_each(?))) OR (s.kind='document' AND s.id IN(SELECT 'document:' || value FROM json_each(?)))")
        .bind(id).bind(json(&frozen.meeting_ids)?).bind(json(&frozen.document_ids)?).execute(connection).await.map_err(failure)?;
    if inserted.rows_affected() != (frozen.meeting_ids.len() + frozen.document_ids.len()) as u64 {
        return Err("Selected source is no longer available".into());
    }
    Ok(())
}

pub async fn frozen_scope(pool: &SqlitePool, id: &str) -> Result<retrieval::FrozenScope, String> {
    let row = sqlx::query(
        "SELECT scope_json,frozen_ids_json,input_json FROM knowledge_requests WHERE id=?",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(failure)?;
    Ok(retrieval::FrozenScope {
        scope: serde_json::from_str(row.get("scope_json")).map_err(|_| "Invalid saved scope")?,
        meeting_ids: serde_json::from_str(row.get("frozen_ids_json"))
            .map_err(|_| "Invalid frozen scope")?,
        document_ids: {
            let input: serde_json::Value = serde_json::from_str(row.get("input_json"))
                .map_err(|_| "Invalid saved selection")?;
            let mut ids: Vec<String> = input
                .get("search")
                .and_then(|search| search.get("document_ids"))
                .map(|value| serde_json::from_value(value.clone()))
                .transpose()
                .map_err(|_| "Invalid saved references")?
                .unwrap_or_default();
            ids.sort();
            ids
        },
    })
}

pub async fn prepare(
    pool: &SqlitePool,
    id: &str,
    frozen: &retrieval::FrozenScope,
    passages: &[Passage],
    inherited: &BTreeMap<String, i64>,
    mode: SearchMode,
) -> Result<Vec<Option<usize>>, String> {
    let mut dependencies = inherited.clone();
    for passage in passages {
        if !frozen.meeting_ids.contains(&passage.meeting_id) || passage.evidence.historical {
            return Err("Evidence is outside the current scope".into());
        }
        if let EvidenceLocator::Document { document_id, .. } = &passage.evidence.locator {
            if !frozen.document_ids.contains(document_id) {
                return Err("Reference is outside the current selection".into());
            }
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
    let contexts = super::evidence::preceding_questions(pool, frozen, passages).await?;
    let mut tx = pool.begin().await.map_err(failure)?;
    let changed=sqlx::query("UPDATE knowledge_requests SET status='running',retrieval_mode=? WHERE id=? AND status='preparing'")
        .bind(if mode==SearchMode::Hybrid {"hybrid"}else{"keyword"}).bind(id).execute(&mut *tx).await.map_err(failure)?;
    if changed.rows_affected() != 1 {
        return Err("Request is no longer active".into());
    }
    retrieval::recheck_scope_in_connection(&mut tx, frozen)
        .await
        .map_err(|e| e.to_string())?;
    let reserved: Vec<(String, i64)> = sqlx::query_as(
        "SELECT source_id,revision FROM knowledge_request_sources WHERE request_id=?",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await
    .map_err(failure)?;
    for (source, revision) in reserved {
        if dependencies
            .insert(source, revision)
            .is_some_and(|other| other != revision)
        {
            return Err("Selected source changed during retrieval".into());
        }
    }
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
            "INSERT INTO knowledge_request_sources(request_id,source_id,revision) VALUES (?,?,?) ON CONFLICT(request_id,source_id) DO NOTHING",
        )
        .bind(id)
        .bind(source)
        .bind(revision)
        .execute(&mut *tx)
        .await
        .map_err(failure)?;
    }
    for (index, passage) in passages.iter().enumerate() {
        let display = serde_json::json!({"title":passage.title,"date":passage.date,"speaker":passage.speaker,"metadata_truncated":passage.metadata_truncated,"preceding_question_tag":contexts[index]});
        sqlx::query("INSERT INTO knowledge_request_evidence(request_id,ordinal,reference_json,display_json) VALUES (?,?,?,?)")
            .bind(id).bind((index+1) as i64).bind(json(&passage.evidence)?).bind(json(&display)?).execute(&mut *tx).await.map_err(failure)?;
    }
    tx.commit().await.map_err(failure)?;
    Ok(contexts)
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
    if !frozen.document_ids.is_empty() {
        let permitted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM knowledge_requests r LEFT JOIN knowledge_document_permissions p ON p.owner_id=r.owner_id WHERE r.id=? AND (r.provider='builtin-ai' OR p.enabled=1))")
            .bind(id).fetch_one(&mut *connection).await.map_err(failure)?;
        if !permitted {
            return Err("Reference sharing was disabled for this conversation".into());
        }
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
    let references = sqlx::query(
        "SELECT reference_json,display_json FROM knowledge_request_evidence WHERE request_id=? ORDER BY ordinal",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await
    .map_err(failure)?;
    let mut evidence = Vec::<EvidenceRef>::new();
    let mut evidence_metadata = Vec::<EvidenceDisplay>::new();
    for reference in references {
        evidence_metadata.push(
            serde_json::from_str(reference.get("display_json"))
                .map_err(|_| "Invalid saved evidence labels")?,
        );
        let mut value: EvidenceRef = serde_json::from_str(reference.get("reference_json"))
            .map_err(|_| "Invalid saved evidence")?;
        value.historical |= row.get::<bool, _>("restored");
        evidence.push(value);
    }
    let content: String = row.get("content");
    super::evidence::validate_context_metadata(&evidence, &evidence_metadata)?;
    let cited_tags = super::evidence::tag_numbers(&content, evidence.len());
    let context_links = cited_tags
        .iter()
        .filter_map(|tag| {
            evidence_metadata[*tag - 1]
                .preceding_question_tag
                .map(|context_tag| CitationContextLink {
                    kind: CitationContextKind::PrecedingQuestion,
                    cited_tag: *tag,
                    context_tag,
                })
        })
        .collect();
    Ok(AssistantReply {
        request_id: id.into(),
        message_id: row.get("id"),
        content,
        evidence,
        evidence_metadata,
        cited_tags,
        context_links,
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
    messages.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then(
                a.request_id
                    .as_ref()
                    .unwrap_or(&a.id)
                    .cmp(b.request_id.as_ref().unwrap_or(&b.id)),
            )
            .then((a.role != "user").cmp(&(b.role != "user")))
            .then(a.id.cmp(&b.id))
    });
    Ok(messages)
}

/// Only verifiable completed turns within the new frozen selection may be sent.
pub async fn eligible_history(
    pool: &SqlitePool,
    owner: &ConversationOwner,
    frozen: &retrieval::FrozenScope,
    budget: usize,
) -> Result<(String, BTreeMap<String, i64>), String> {
    let rows=sqlx::query("SELECT id,frozen_ids_json FROM knowledge_requests WHERE owner_id=? AND status='completed' AND restored=0 ORDER BY created_at DESC,id DESC LIMIT 20")
        .bind(owner_key(owner)?).fetch_all(pool).await.map_err(failure)?;
    let mut history = Vec::new();
    let mut dependencies = BTreeMap::new();
    let mut used = 0;
    for row in rows {
        let id: String = row.get("id");
        let prior_ids: Vec<String> = serde_json::from_str(row.get("frozen_ids_json"))
            .map_err(|_| "Invalid saved history scope")?;
        if prior_ids.iter().any(|id| !frozen.meeting_ids.contains(id)) {
            continue;
        }
        let sources=sqlx::query("SELECT d.source_id,d.revision,s.revision AS current_revision,s.meeting_id FROM knowledge_request_sources d LEFT JOIN knowledge_sources s ON s.id=d.source_id WHERE d.request_id=?")
            .bind(&id).fetch_all(pool).await.map_err(failure)?;
        if sources.is_empty()
            || sources.iter().any(|source| {
                source.get::<Option<i64>, _>("current_revision") != Some(source.get("revision"))
                    || !source
                        .get::<Option<String>, _>("meeting_id")
                        .is_some_and(|id| frozen.meeting_ids.contains(&id))
                        && !source
                            .get::<String, _>("source_id")
                            .strip_prefix("document:")
                            .is_some_and(|id| {
                                frozen.document_ids.iter().any(|selected| selected == id)
                            })
            })
        {
            continue;
        }
        let bytes:i64=sqlx::query_scalar("SELECT coalesce(sum(length(CAST(content AS BLOB))),0) FROM knowledge_messages WHERE request_id=?").bind(&id).fetch_one(pool).await.map_err(failure)?;
        if bytes < 0 || bytes as usize > budget.saturating_sub(used) {
            continue;
        }
        let valid_snapshot = {
            let mut connection = pool.acquire().await.map_err(failure)?;
            reply_in(&mut connection, &id).await.is_ok()
        };
        if !valid_snapshot {
            continue;
        }
        let messages=sqlx::query("SELECT role,content FROM knowledge_messages WHERE request_id=? ORDER BY CASE role WHEN 'user' THEN 0 ELSE 1 END").bind(&id).fetch_all(pool).await.map_err(failure)?;
        let turn=serde_json::to_string(&messages.iter().map(|message|serde_json::json!({"role":message.get::<String,_>("role"),"content":message.get::<String,_>("content").replace("[K","[previous citation ")})).collect::<Vec<_>>()).map_err(|_|"Invalid saved history")?;
        if used + turn.len() + 1 > budget {
            continue;
        }
        used += turn.len() + 1;
        history.push(turn);
        for source in sources {
            dependencies.insert(source.get("source_id"), source.get("revision"));
        }
    }
    history.reverse();
    Ok((history.join("\n"), dependencies))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn meeting_and_library_request_boundaries_remain_separate() {
        let mut input = request(ConversationOwner::Meeting("fixture".into()));
        assert!(
            validate_request(&input).is_err(),
            "meeting owner cannot use library scope"
        );
        input.search.scope = KnowledgeScope::Meeting {
            meeting_id: "fixture".into(),
        };
        assert!(validate_request(&input).is_ok());
        input.owner = ConversationOwner::Library("library".into());
        assert!(
            validate_request(&input).is_err(),
            "library owner cannot use meeting scope"
        );
        input.search.scope = KnowledgeScope::Library {
            filter: MeetingFilter {
                all_meetings: true,
                ..Default::default()
            },
        };
        assert!(validate_request(&input).is_ok());
    }

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

    pub(crate) async fn question_pair_fixture(
        reply: &str,
    ) -> (SqlitePool, AskRequest, retrieval::FrozenScope, Vec<Passage>) {
        use crate::knowledge::store;
        let pool = database().await;
        for (id, text, timestamp) in [
            ("context-question", "Darf der Versuch beginnen?", "00:01"),
            ("context-reply", reply, "00:02"),
        ] {
            sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES (?,'fixture',?,?)").bind(id).bind(text).bind(timestamp).execute(&pool).await.unwrap();
        }
        let request = request(create_library(&pool).await.unwrap());
        reserve(&pool, &request, "builtin-ai", "qwen3.5:4b")
            .await
            .unwrap();
        let frozen = frozen_scope(&pool, &request.request_id).await.unwrap();
        let job:store::SourceJob=sqlx::query_as("SELECT id AS source_id,meeting_id,revision,generation FROM knowledge_sources WHERE meeting_id='fixture'").fetch_one(&pool).await.unwrap();
        let mut passages = Vec::new();
        for (id, text) in [
            ("context-question", "Darf der Versuch beginnen?"),
            ("context-reply", reply),
        ] {
            passages.push(
                store::materialize(
                    &pool,
                    &store::SelectedRow::for_job(&job, id.into()),
                    TextSpan {
                        transcript_id: id.into(),
                        start_byte: 0,
                        end_byte: text.len(),
                    },
                    false,
                )
                .await
                .unwrap(),
            );
        }
        (pool, request, frozen, passages)
    }

    #[tokio::test]
    async fn preceding_question_context_persists_without_rewriting_literal_citations() {
        let (pool, request, frozen, passages) = question_pair_fixture("Nein.").await;
        prepare(
            &pool,
            &request.request_id,
            &frozen,
            &passages,
            &BTreeMap::new(),
            SearchMode::Keyword,
        )
        .await
        .unwrap();
        let reply = finish(
            &pool,
            &request.request_id,
            &frozen,
            "Nicht freigegeben [K2].",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let expected =
            serde_json::json!([{"kind":"preceding_question","cited_tag":2,"context_tag":1}]);
        let original = serde_json::to_value(&reply).unwrap();
        assert_eq!(original["context_links"], expected);
        assert_eq!(reply.cited_tags, vec![2]);
        assert_eq!(reply.content, "Nicht freigegeben [K2].");
        assert_eq!(
            original["evidence_metadata"][1]["preceding_question_tag"],
            1
        );
        let again = completed_reply(&pool, &request).await.unwrap().unwrap();
        assert_eq!(serde_json::to_value(again).unwrap(), original);
        let saved = history(&pool, &request.owner).await.unwrap();
        assert_eq!(
            serde_json::to_value(saved[1].reply.as_ref().unwrap()).unwrap(),
            original
        );
        sqlx::query("UPDATE knowledge_request_evidence SET display_json=json_remove(display_json,'$.preceding_question_tag')").execute(&pool).await.unwrap();
        let legacy = history(&pool, &request.owner).await.unwrap();
        assert_eq!(
            serde_json::to_value(legacy[1].reply.as_ref().unwrap()).unwrap()["context_links"],
            serde_json::json!([])
        );
    }

    #[tokio::test]
    async fn question_context_uses_complete_selected_rows_and_utf8_reply_limit() {
        for (reply, selected_question, expected) in [
            ("ä".repeat(32), true, Some(1)),
            ("ä".repeat(33), true, None),
            ("Ja.".into(), false, None),
        ] {
            let (pool, request, frozen, mut passages) = question_pair_fixture(&reply).await;
            if !selected_question {
                passages.remove(0);
            }
            prepare(
                &pool,
                &request.request_id,
                &frozen,
                &passages,
                &BTreeMap::new(),
                SearchMode::Keyword,
            )
            .await
            .unwrap();
            let display:String=sqlx::query_scalar("SELECT display_json FROM knowledge_request_evidence WHERE request_id=? ORDER BY ordinal DESC LIMIT 1").bind(&request.request_id).fetch_one(&pool).await.unwrap();
            let value: serde_json::Value = serde_json::from_str(&display).unwrap();
            assert_eq!(value["preceding_question_tag"].as_u64(), expected);
        }
    }

    #[tokio::test]
    async fn saved_context_rejects_forged_map_links() {
        for tag in [0, 2, 999] {
            let (pool, request, frozen, passages) = question_pair_fixture("Ja.").await;
            prepare(
                &pool,
                &request.request_id,
                &frozen,
                &passages,
                &BTreeMap::new(),
                SearchMode::Keyword,
            )
            .await
            .unwrap();
            finish(
                &pool,
                &request.request_id,
                &frozen,
                "Ja [K2].",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE knowledge_request_evidence SET display_json=json_set(display_json,'$.preceding_question_tag',?) WHERE request_id=? AND ordinal=2").bind(tag).bind(&request.request_id).execute(&pool).await.unwrap();
            assert!(
                history(&pool, &request.owner).await.is_err(),
                "Forged context tag {tag}"
            );
            assert!(eligible_history(&pool, &request.owner, &frozen, 8192)
                .await
                .unwrap()
                .0
                .is_empty());
        }
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
    async fn meeting_and_expanded_library_histories_remain_independently_readable() {
        let pool = database().await;
        let library = create_library(&pool).await.unwrap();
        let meeting = ConversationOwner::Meeting("fixture".into());
        for (owner, answer) in [
            (meeting.clone(), "Meeting-only answer"),
            (library.clone(), "Expanded answer"),
        ] {
            let mut input = request(owner.clone());
            if matches!(owner, ConversationOwner::Meeting(_)) {
                input.search.scope = KnowledgeScope::Meeting {
                    meeting_id: "fixture".into(),
                };
            }
            reserve(&pool, &input, "builtin-ai", "fixture")
                .await
                .unwrap();
            let frozen = frozen_scope(&pool, &input.request_id).await.unwrap();
            prepare(
                &pool,
                &input.request_id,
                &frozen,
                &[],
                &BTreeMap::new(),
                SearchMode::Keyword,
            )
            .await
            .unwrap();
            finish(
                &pool,
                &input.request_id,
                &frozen,
                answer,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        }
        assert_eq!(
            history(&pool, &meeting).await.unwrap()[1].content,
            "Meeting-only answer"
        );
        assert_eq!(
            history(&pool, &library).await.unwrap()[1].content,
            "Expanded answer"
        );
        assert_eq!(list_libraries(&pool).await.unwrap().len(), 1);
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
    async fn history_keeps_user_before_assistant_when_timestamps_tie() {
        let pool = database().await;
        let (request, frozen) = active(&pool).await;
        finish(
            &pool,
            &request.request_id,
            &frozen,
            "Bounded answer",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE knowledge_messages SET created_at='2026-09-01T00:00:00Z',id=CASE role WHEN 'user' THEN 'z-user' ELSE 'a-assistant' END").execute(&pool).await.unwrap();
        let history = history(&pool, &request.owner).await.unwrap();
        assert_eq!(history[0].role, "user");
        assert_eq!(history[1].role, "assistant");
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
