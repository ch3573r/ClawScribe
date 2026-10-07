//! Canonical evidence resolution, independent of derived semantic caches.
use super::types::*;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

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
    pool: &SqlitePool,
    reference: &EvidenceRef,
) -> Result<ResolvedEvidence, KnowledgeError> {
    let outcome = |status| {
        Ok(ResolvedEvidence {
            status,
            passage: None,
        })
    };
    if reference.historical {
        return outcome(EvidenceStatus::Stale);
    }
    let EvidenceLocator::Transcript {
        meeting_id,
        transcript_ids,
        spans,
        start_seconds,
    } = &reference.locator
    else {
        return outcome(EvidenceStatus::Invalid);
    };
    if transcript_ids.len() != 1
        || spans.len() != 1
        || transcript_ids[0] != spans[0].transcript_id
        || reference.source_id != format!("meeting:{meeting_id}")
        || spans[0].end_byte <= spans[0].start_byte
        || spans[0].end_byte - spans[0].start_byte > super::store::READ_BYTES
        || start_seconds.is_some_and(|time| !time.is_finite() || time < 0.)
    {
        return outcome(EvidenceStatus::Invalid);
    }
    let source=sqlx::query("SELECT revision,generation FROM knowledge_sources WHERE id=? AND meeting_id=? AND kind='meeting'")
        .bind(&reference.source_id).bind(meeting_id).fetch_optional(pool).await?;
    let Some(source) = source else {
        return outcome(EvidenceStatus::Missing);
    };
    let revision: i64 = source.get("revision");
    if revision != reference.source_revision {
        return outcome(EvidenceStatus::Stale);
    }
    let selected = super::store::SelectedRow {
        transcript_id: transcript_ids[0].clone(),
        source_id: reference.source_id.clone(),
        meeting_id: meeting_id.clone(),
        revision,
        generation: source.get("generation"),
    };
    let passage = match super::store::materialize(pool, &selected, spans[0].clone(), false).await {
        Ok(passage) => passage,
        Err(KnowledgeError::Superseded) => return outcome(EvidenceStatus::Stale),
        Err(KnowledgeError::InvalidInput) => return outcome(EvidenceStatus::Invalid),
        Err(error) => return Err(error),
    };
    if passage.evidence.fingerprint != reference.fingerprint {
        return outcome(EvidenceStatus::Stale);
    }
    if &passage.evidence != reference {
        return outcome(EvidenceStatus::Invalid);
    }
    Ok(ResolvedEvidence {
        status: EvidenceStatus::Current,
        passage: Some(passage),
    })
}

/// A tag has meaning only inside the supplied request-local map.
pub fn tagged_references(content: &str, map: &[Passage]) -> Vec<(usize, EvidenceRef)> {
    tag_numbers(content, map.len())
        .into_iter()
        .map(|tag| (tag, map[tag - 1].evidence.clone()))
        .collect()
}

pub fn tag_numbers(content: &str, count: usize) -> Vec<usize> {
    let mut tags = std::collections::BTreeSet::new();
    for part in content.split("[K").skip(1) {
        let Some(end) = part.find(']') else { continue };
        let digits = &part[..end];
        if digits.is_empty()
            || digits.len() > 3
            || digits.starts_with('0')
            || !digits.bytes().all(|b| b.is_ascii_digit())
        {
            continue;
        }
        if let Ok(tag) = digits.parse::<usize>() {
            if tag > 0 && tag <= count {
                tags.insert(tag);
            }
        }
    }
    tags.into_iter().collect()
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
        store::requeue(&pool, &[passage.meeting_id.clone()])
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

    #[tokio::test]
    async fn restored_reference_stays_stale_with_matching_source_counter() {
        let (pool, mut passage) = fixture().await;
        passage.evidence.historical = true;
        let restored = resolve(&pool, &passage.evidence).await.unwrap();
        assert_eq!(restored.status, EvidenceStatus::Stale);
        assert!(restored.passage.is_none());
    }
}
