//! Explicit reference selection and per-conversation provider permission.
use super::types::*;
use sqlx::SqlitePool;

pub async fn sharing(_pool: &SqlitePool, _owner: &ConversationOwner) -> Result<bool, String> {
    Ok(false)
}
pub async fn set_sharing(
    _pool: &SqlitePool,
    _owner: &ConversationOwner,
    _enabled: bool,
) -> Result<(), String> {
    Ok(())
}
pub async fn check_sharing(
    _pool: &SqlitePool,
    _owner: &ConversationOwner,
    _provider: &str,
    _selected: bool,
) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{documents, evidence, retrieval, KnowledgeState};
    use sha2::{Digest, Sha256};
    use tokio_util::sync::CancellationToken;

    async fn fixture() -> (SqlitePool, String) {
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
}
