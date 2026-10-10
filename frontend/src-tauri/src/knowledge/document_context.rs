//! Explicit reference selection and per-conversation provider permission.
use super::types::*;
use sqlx::{Row, SqliteConnection, SqlitePool};

pub async fn sharing(pool: &SqlitePool, owner: &ConversationOwner) -> Result<bool, String> {
    let key = super::conversations::owner_key(owner)?;
    sqlx::query_scalar(
        "SELECT COALESCE((SELECT enabled FROM knowledge_document_permissions WHERE owner_id=?),0)",
    )
    .bind(key)
    .fetch_one(pool)
    .await
    .map_err(|_| "Reference sharing settings are unavailable".into())
}
pub async fn set_sharing(
    pool: &SqlitePool,
    owner: &ConversationOwner,
    enabled: bool,
) -> Result<(), String> {
    let key = super::conversations::owner_key(owner)?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| "Reference sharing settings are unavailable")?;
    if let ConversationOwner::Meeting(id) = owner {
        sqlx::query("INSERT INTO knowledge_owners(id,kind,meeting_id) SELECT ?,'meeting',id FROM meetings WHERE id=? ON CONFLICT(id) DO NOTHING")
            .bind(&key).bind(id).execute(&mut *tx).await.map_err(|_| "Reference sharing settings are unavailable")?;
    }
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM knowledge_owners WHERE id=?)")
            .bind(&key)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| "Reference sharing settings are unavailable")?;
    if !exists {
        return Err("Choose a saved conversation before enabling reference sharing".into());
    }
    sqlx::query("INSERT INTO knowledge_document_permissions(owner_id,enabled) VALUES (?,?) ON CONFLICT(owner_id) DO UPDATE SET enabled=excluded.enabled")
        .bind(key).bind(enabled).execute(&mut *tx).await.map_err(|_| "Reference sharing settings are unavailable")?;
    tx.commit()
        .await
        .map_err(|_| "Reference sharing settings are unavailable")?;
    Ok(())
}
pub async fn check_sharing(
    pool: &SqlitePool,
    owner: &ConversationOwner,
    provider: &str,
    selected: bool,
) -> Result<(), String> {
    if selected && provider != "builtin-ai" && !sharing(pool, owner).await? {
        return Err("Enable reference sharing for this conversation before sending document excerpts to the configured provider".into());
    }
    Ok(())
}

pub async fn validate_selection(
    connection: &mut SqliteConnection,
    meetings: &[String],
    ids: &[String],
) -> Result<Vec<String>, KnowledgeError> {
    if ids.len() > super::evidence::MAX_EVIDENCE_ENTRIES {
        return Err(KnowledgeError::InvalidInput);
    }
    let mut selected = ids.to_vec();
    selected.sort();
    if selected.windows(2).any(|pair| pair[0] == pair[1])
        || selected.iter().any(|id| {
            uuid::Uuid::parse_str(id)
                .map(|value| value.to_string())
                .ok()
                .as_deref()
                != Some(id.as_str())
        })
    {
        return Err(KnowledgeError::InvalidInput);
    }
    if selected.is_empty() {
        return Ok(selected);
    }
    let meeting_json = serde_json::to_string(meetings).map_err(|_| KnowledgeError::InvalidInput)?;
    let documents_json =
        serde_json::to_string(&selected).map_err(|_| KnowledgeError::InvalidInput)?;
    let found: Vec<String> = sqlx::query_scalar("SELECT d.id FROM knowledge_documents d WHERE d.id IN(SELECT value FROM json_each(?)) AND EXISTS(SELECT 1 FROM knowledge_document_attachments a WHERE a.document_id=d.id AND a.meeting_id IN(SELECT value FROM json_each(?))) ORDER BY d.id")
        .bind(documents_json).bind(meeting_json).fetch_all(connection).await?;
    if found != selected {
        return Err(KnowledgeError::InvalidInput);
    }
    Ok(selected)
}

#[derive(serde::Serialize)]
pub struct ScopedDocument {
    pub attachment: super::documents::DocumentAttachment,
    pub meeting_ids: Vec<String>,
}
pub async fn scoped_documents(
    pool: &SqlitePool,
    scope: &KnowledgeScope,
) -> Result<Vec<ScopedDocument>, KnowledgeError> {
    let mut frozen = super::retrieval::freeze_scope(pool, scope).await?;
    let allowed =
        serde_json::to_string(&frozen.meeting_ids).map_err(|_| KnowledgeError::InvalidInput)?;
    let rows = sqlx::query("SELECT document_id,meeting_id FROM knowledge_document_attachments WHERE meeting_id IN(SELECT value FROM json_each(?)) ORDER BY document_id,meeting_id").bind(allowed).fetch_all(pool).await?;
    let mut owners = std::collections::BTreeMap::<String, Vec<String>>::new();
    for row in rows {
        owners
            .entry(row.get("document_id"))
            .or_default()
            .push(row.get("meeting_id"));
    }
    let mut result = Vec::new();
    for (id, meeting_ids) in owners {
        let attachment = super::documents::store::get(pool, &meeting_ids[0], &id)
            .await
            .map_err(|_| KnowledgeError::Superseded)?;
        result.push(ScopedDocument {
            attachment,
            meeting_ids,
        });
    }
    frozen.document_ids = result
        .iter()
        .map(|entry| entry.attachment.id.clone())
        .collect();
    super::retrieval::recheck_scope(pool, &frozen).await?;
    Ok(result)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::knowledge::{documents, evidence, retrieval, KnowledgeState};
    use sha2::{Digest, Sha256};
    use tokio_util::sync::CancellationToken;

    pub(crate) async fn fixture() -> (SqlitePool, String) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        for id in ["aster", "birch"] {
            sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES (?,?,'2026-09-01','2026-09-01')").bind(id).bind(id).execute(&pool).await.unwrap();
        }
        // A realistic one-hour meeting, at one finalized segment every six seconds.
        let mut tx = pool.begin().await.unwrap();
        for row in 0..600 {
            sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp,audio_start_time,audio_end_time) VALUES (?,'aster',?,'2026-09-01',?,?)")
                .bind(format!("segment-{row:04}")).bind(format!("Public synthetic minute {}: supplier review, meeting_budget, owner team {}, recorded meeting decision.",row/10,row%12))
                .bind(row as f64*6.).bind(row as f64*6.+5.).execute(&mut *tx).await.unwrap();
        }
        tx.commit().await.unwrap();
        let bytes = documents::fixtures::pdf(24, true, false);
        let extracted =
            documents::extract::extract(documents::DocumentFormat::Pdf, &bytes).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        documents::store::publish(
            &pool,
            "aster",
            &id,
            "Supplier reference.pdf",
            bytes.len() as u64,
            &format!("{:x}", Sha256::digest(&bytes)),
            &extracted,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        (pool, id)
    }
    fn request(meeting: &str, document_ids: Vec<String>) -> SearchRequest {
        SearchRequest {
            scope: KnowledgeScope::Meeting {
                meeting_id: meeting.into(),
            },
            query: "decision_p24".into(),
            document_ids,
            mode: SearchMode::Keyword,
        }
    }
    #[tokio::test]
    async fn unselected_document_not_retrieved() {
        let (pool, id) = fixture().await;
        let runtime = KnowledgeState::default();
        assert!(
            retrieval::retrieve(&pool, &runtime, &request("aster", vec![]))
                .await
                .unwrap()
                .passages
                .is_empty()
        );
        let result = retrieval::retrieve(&pool, &runtime, &request("aster", vec![id.clone()]))
            .await
            .expect("Selected reference must be searchable without a model");
        assert!(!result.passages.is_empty());
        assert!(result.passages.iter().all(|p| matches!(&p.evidence.locator,EvidenceLocator::Document {document_id,page:Some(24),paragraph,..} if document_id==&id && *paragraph>0)));
        assert!(result
            .passages
            .iter()
            .all(|p| p.text.contains("decision_p24")));
        let resolved = evidence::resolve(&pool, &result.passages[0].evidence)
            .await
            .unwrap();
        assert_eq!(resolved.status, evidence::EvidenceStatus::Current);
        assert!(resolved.navigation.is_none());
    }
    #[tokio::test]
    async fn foreign_attachment_rejected() {
        let (pool, id) = fixture().await;
        assert!(matches!(
            retrieval::retrieve(
                &pool,
                &KnowledgeState::default(),
                &request("birch", vec![id])
            )
            .await,
            Err(KnowledgeError::InvalidInput)
        ));
    }
    #[tokio::test]
    async fn replacement_marks_citation_stale_without_derived_chunks() {
        let (pool, id) = fixture().await;
        let result = retrieval::retrieve(
            &pool,
            &KnowledgeState::default(),
            &request("aster", vec![id.clone()]),
        )
        .await
        .unwrap();
        let reference = result.passages[0].evidence.clone();
        sqlx::query("UPDATE knowledge_document_blocks SET text='Replacement reference decision.' WHERE document_id=? AND page=24").bind(id).execute(&pool).await.unwrap();
        assert_eq!(
            evidence::resolve(&pool, &reference).await.unwrap().status,
            evidence::EvidenceStatus::Stale
        );
    }
    #[tokio::test]
    async fn external_context_requires_enablement_and_permission_is_owner_specific() {
        let (pool, _) = fixture().await;
        let owner = ConversationOwner::Meeting("aster".into());
        let other = ConversationOwner::Meeting("birch".into());
        assert!(!sharing(&pool, &owner).await.unwrap());
        assert!(
            check_sharing(&pool, &owner, "openai", true).await.is_err(),
            "No external provider may receive an excerpt before permission"
        );
        assert!(check_sharing(&pool, &owner, "builtin-ai", true)
            .await
            .is_ok());
        assert!(check_sharing(&pool, &owner, "openai", false).await.is_ok());
        set_sharing(&pool, &owner, true).await.unwrap();
        assert!(sharing(&pool, &owner).await.unwrap());
        assert!(!sharing(&pool, &other).await.unwrap());
        assert!(check_sharing(&pool, &owner, "openai", true).await.is_ok());
        set_sharing(&pool, &owner, false).await.unwrap();
        assert!(check_sharing(&pool, &owner, "openai", true).await.is_err());
    }
    #[tokio::test]
    async fn document_vectors_obey_selection_and_resolve_without_cache() {
        use crate::knowledge::store;
        let (pool, document) = fixture().await;
        let job:store::SourceJob=sqlx::query_as("SELECT id AS source_id,'' AS meeting_id,revision,generation FROM knowledge_sources WHERE id=?")
            .bind(format!("document:{document}")).fetch_one(&pool).await.unwrap();
        let mut after = None;
        let mut ordinal = 0;
        loop {
            let ids = store::row_ids_page(&pool, &job, after.as_deref())
                .await
                .unwrap();
            if ids.is_empty() {
                break;
            }
            after = ids.last().cloned();
            for id in ids {
                let selected = store::SelectedRow::for_job(&job, id.clone());
                let (text, total) = store::body_window(&pool, &selected, 0).await.unwrap();
                assert_eq!(text.len(), total);
                let passage = store::materialize(
                    &pool,
                    &selected,
                    TextSpan {
                        transcript_id: id,
                        start_byte: 0,
                        end_byte: total,
                    },
                    false,
                )
                .await
                .unwrap();
                let mut vector = vec![0.; 384];
                vector[if text.contains("decision_p24") { 0 } else { 1 }] = 1.;
                store::stage_evidence(
                    &pool,
                    &job,
                    &passage.evidence,
                    &text,
                    ordinal,
                    &vector,
                    "synthetic-document-space",
                )
                .await
                .unwrap();
                ordinal += 1;
            }
        }
        assert_eq!(ordinal, 24);
        store::publish(&pool, &job, "synthetic-document-space")
            .await
            .unwrap();
        let mut vector = vec![0.; 384];
        vector[0] = 1.;
        let mut scope = retrieval::freeze_scope(&pool, &request("aster", vec![]).scope)
            .await
            .unwrap();
        assert!(retrieval::search_channels(
            &pool,
            &scope,
            "unmatched",
            Some(("synthetic-document-space", &vector))
        )
        .await
        .unwrap()
        .is_empty());
        scope.document_ids = vec![document.clone()];
        let selected = retrieval::search_channels(
            &pool,
            &scope,
            "unmatched",
            Some(("synthetic-document-space", &vector)),
        )
        .await
        .unwrap();
        assert!(selected[0].text.contains("decision_p24"));
        let reference = selected[0].evidence.clone();
        sqlx::query("DELETE FROM knowledge_chunks WHERE source_id=?")
            .bind(&job.source_id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            evidence::resolve(&pool, &reference).await.unwrap().status,
            evidence::EvidenceStatus::Current
        );
        documents::store::detach(&pool, "aster", &document)
            .await
            .unwrap();
        assert_eq!(
            retrieval::recheck_scope(&pool, &scope).await.unwrap_err(),
            KnowledgeError::Superseded
        );
    }
}
