//! Canonical evidence resolution, independent of derived semantic caches.
use super::types::*;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Current,
    Stale,
    Missing,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedEvidence {
    pub status: EvidenceStatus,
    /// Available only for a verified current canonical passage.
    pub passage: Option<Passage>,
}

pub async fn resolve(
    _pool: &SqlitePool,
    _reference: &EvidenceRef,
) -> Result<ResolvedEvidence, KnowledgeError> {
    todo!("Resolve authoritative canonical evidence")
}

/// A tag has meaning only inside the supplied request-local map.
pub fn tagged_references(_content: &str, _map: &[Passage]) -> Vec<(usize, EvidenceRef)> {
    todo!("Recognize only supplied backend tags")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{retrieval, store};

    async fn fixture() -> (SqlitePool, Passage) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES ('fixture','Public fixture','2026-09-01','2026-09-01')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp,audio_start_time) VALUES ('row','fixture','Ja. Test only.','00:14',14)").execute(&pool).await.unwrap();
        let frozen = retrieval::freeze_scope(
            &pool,
            &KnowledgeScope::Meeting {
                meeting_id: "fixture".into(),
            },
        )
        .await
        .unwrap();
        let mut result = retrieval::search_channels(&pool, &frozen, "Test", None)
            .await
            .unwrap();
        (pool, result.remove(0))
    }

    #[tokio::test]
    async fn unknown_tag_has_no_link() {
        let (_, passage) = fixture().await;
        let refs = tagged_references(
            "Fact [K1]. Invented [K999]. Repeated [K1]. [S2]",
            &[passage.clone()],
        );
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0], (1, passage.evidence));
    }

    #[tokio::test]
    async fn changed_passage_resolves_stale() {
        let (pool, passage) = fixture().await;
        assert_eq!(
            resolve(&pool, &passage.evidence).await.unwrap().status,
            EvidenceStatus::Current
        );
        store::requeue(&pool, &[passage.evidence.source_id.clone()])
            .await
            .unwrap();
        assert_eq!(
            resolve(&pool, &passage.evidence).await.unwrap().status,
            EvidenceStatus::Current,
            "Derived rebuild cannot invalidate citation identity"
        );
        sqlx::query("UPDATE transcripts SET transcript='Nein. Test only.' WHERE id='row'")
            .execute(&pool)
            .await
            .unwrap();
        let stale = resolve(&pool, &passage.evidence).await.unwrap();
        assert_eq!(stale.status, EvidenceStatus::Stale);
        assert!(stale.passage.is_none());
    }

    #[tokio::test]
    async fn forged_time_span_and_cache_id_never_navigate() {
        let (pool, passage) = fixture().await;
        let mut forged = passage.evidence.clone();
        if let EvidenceLocator::Transcript { start_seconds, .. } = &mut forged.locator {
            *start_seconds = Some(999.);
        }
        assert_eq!(
            resolve(&pool, &forged).await.unwrap().status,
            EvidenceStatus::Invalid
        );
        forged = passage.evidence;
        forged.chunk_id = "invented-cache-key".into();
        assert_eq!(
            resolve(&pool, &forged).await.unwrap().status,
            EvidenceStatus::Invalid
        );
    }
}
