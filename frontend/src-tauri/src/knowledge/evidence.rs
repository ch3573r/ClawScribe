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
    let mut remaining = content;
    while let Some(start) = remaining.find('[') {
        remaining = &remaining[start + 1..];
        let mut depth = 1usize;
        let mut nested = false;
        let end = remaining.bytes().position(|byte| {
            if byte == b'[' {
                depth += 1;
                nested = true;
            }
            if byte == b']' {
                depth -= 1;
            }
            depth == 0
        });
        let Some(end) = end else {
            break;
        };
        if !nested {
            if let Some(group) = citation_group(&remaining[..end], count) {
                tags.extend(group);
            }
        }
        remaining = &remaining[end + 1..];
    }
    tags.into_iter().collect()
}

/// Establish positional context only from complete, already-selected canonical
/// rows. No text, row, or scope is added to the request by this relation.
pub async fn preceding_questions(
    pool: &SqlitePool,
    frozen: &super::retrieval::FrozenScope,
    passages: &[Passage],
) -> Result<Vec<Option<usize>>, String> {
    if passages.len() > 64 {
        return Err("Too many evidence rows".into());
    }
    let mut complete = vec![false; passages.len()];
    let mut ids = vec![None; passages.len()];
    for (index, passage) in passages.iter().enumerate() {
        if !frozen.meeting_ids.contains(&passage.meeting_id) || passage.evidence.historical {
            continue;
        }
        let EvidenceLocator::Transcript {
            transcript_ids,
            spans,
            ..
        } = &passage.evidence.locator
        else {
            continue;
        };
        if transcript_ids.len() != 1 || spans.len() != 1 || spans[0].start_byte != 0 {
            continue;
        }
        let canonical = resolve(pool, &passage.evidence)
            .await
            .map_err(|e| e.to_string())?;
        if canonical.status != EvidenceStatus::Current
            || canonical
                .passage
                .as_ref()
                .is_none_or(|p| p.text != passage.text)
        {
            continue;
        }
        let job:Option<super::store::SourceJob>=sqlx::query_as("SELECT id AS source_id,meeting_id,revision,generation FROM knowledge_sources WHERE id=? AND meeting_id=?")
            .bind(&passage.evidence.source_id).bind(&passage.meeting_id).fetch_optional(pool).await.map_err(|_|"Evidence storage unavailable")?;
        let Some(job) = job else { continue };
        if job.revision != passage.evidence.source_revision {
            continue;
        }
        let (_, total) = super::store::body_window(
            pool,
            &super::store::SelectedRow::for_job(&job, transcript_ids[0].clone()),
            0,
        )
        .await
        .map_err(|e| e.to_string())?;
        complete[index] = spans[0].end_byte == total && passage.text.len() == total;
        ids[index] = Some(transcript_ids[0].clone());
    }
    let mut contexts = vec![None; passages.len()];
    for (index, reply) in passages.iter().enumerate() {
        if !complete[index]
            || reply.text.trim().is_empty()
            || reply.text.len() > 64
            || reply.text.trim_end().ends_with('?')
        {
            continue;
        }
        let predecessor:Option<String>=sqlx::query_scalar("SELECT id FROM transcripts WHERE meeting_id=? AND (timestamp,id)<(SELECT timestamp,id FROM transcripts WHERE id=? AND meeting_id=?) ORDER BY timestamp DESC,id DESC LIMIT 1")
            .bind(&reply.meeting_id).bind(ids[index].as_deref()).bind(&reply.meeting_id).fetch_optional(pool).await.map_err(|_|"Evidence storage unavailable")?;
        let Some(predecessor) = predecessor else {
            continue;
        };
        let question = passages.iter().enumerate().find(|(other, p)| {
            complete[*other]
                && ids[*other].as_deref() == Some(predecessor.as_str())
                && p.meeting_id == reply.meeting_id
                && p.evidence.source_id == reply.evidence.source_id
                && p.evidence.source_revision == reply.evidence.source_revision
                && p.text.trim_end().ends_with('?')
        });
        if let Some((other, _)) = question {
            contexts[index] = Some(other + 1);
        }
    }
    Ok(contexts)
}

/// Historical snapshots remain readable without consulting mutated source text,
/// but their serialized relation must still fit the immutable canonical map.
pub fn validate_context_metadata(
    evidence: &[EvidenceRef],
    metadata: &[EvidenceDisplay],
) -> Result<(), String> {
    if evidence.len() != metadata.len() || evidence.len() > 64 {
        return Err("Invalid evidence map".into());
    }
    for (index, display) in metadata.iter().enumerate() {
        if display.title.len() > 1024
            || display.date.len() > 1024
            || display.speaker.as_ref().is_some_and(|s| s.len() > 1024)
        {
            return Err("Invalid evidence labels".into());
        }
        let Some(tag) = display.preceding_question_tag else {
            continue;
        };
        let question = tag
            .checked_sub(1)
            .filter(|n| *n < evidence.len() && *n != index)
            .ok_or("Invalid preceding-question tag")?;
        let reply = &evidence[index];
        let anchor = &evidence[question];
        if metadata[question].preceding_question_tag.is_some()
            || reply.source_id != anchor.source_id
            || reply.source_revision != anchor.source_revision
            || reply.historical != anchor.historical
        {
            return Err("Invalid preceding-question relation".into());
        }
        let (
            EvidenceLocator::Transcript {
                meeting_id: a,
                transcript_ids: ai,
                spans: aspan,
                ..
            },
            EvidenceLocator::Transcript {
                meeting_id: b,
                transcript_ids: bi,
                spans: bspan,
                ..
            },
        ) = (&reply.locator, &anchor.locator)
        else {
            return Err("Invalid context locator".into());
        };
        if a != b
            || reply.source_id != format!("meeting:{a}")
            || ai.len() != 1
            || bi.len() != 1
            || ai == bi
            || aspan.len() != 1
            || bspan.len() != 1
            || aspan[0].transcript_id != ai[0]
            || bspan[0].transcript_id != bi[0]
            || aspan[0].start_byte != 0
            || bspan[0].start_byte != 0
            || aspan[0].end_byte == 0
            || aspan[0].end_byte > 64
            || bspan[0].end_byte == 0
            || bspan[0].end_byte > super::store::READ_BYTES
        {
            return Err("Invalid context span".into());
        }
    }
    Ok(())
}

/// A complete bracket group is accepted or rejected together. Never salvage an
/// inner tag from malformed/nested syntax, or expand an unknown range endpoint.
fn citation_group(body: &str, count: usize) -> Option<Vec<usize>> {
    const MAX_GROUP_TAGS: usize = 64;
    if body.len() > 1024 {
        return None;
    }
    let number = |item: &str| {
        let digits = item.trim().strip_prefix('K')?;
        if digits.is_empty()
            || digits.len() > 3
            || digits.starts_with('0')
            || !digits.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let tag = digits.parse::<usize>().ok()?;
        (tag <= count).then_some(tag)
    };
    if body.contains(',') {
        let mut group = Vec::new();
        for item in body.split(',') {
            if group.len() == MAX_GROUP_TAGS {
                return None;
            }
            group.push(number(item)?);
        }
        Some(group)
    } else if let Some((first, last)) = body.split_once('-') {
        let first = number(first)?;
        let last = number(last)?;
        if last < first || last - first >= MAX_GROUP_TAGS {
            return None;
        }
        Some((first..=last).collect())
    } else {
        Some(vec![number(body)?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{retrieval, store};

    #[tokio::test]
    async fn question_context_rejects_partial_nonadjacent_changed_and_unselected_rows() {
        use crate::knowledge::conversations::tests::question_pair_fixture;
        for scenario in [
            "partial_question",
            "partial_reply",
            "not_question",
            "intervening",
            "changed_revision",
            "outside_scope",
            "different_meeting",
        ] {
            let (pool, _, mut frozen, mut passages) = question_pair_fixture("Ja.").await;
            if scenario == "not_question" {
                sqlx::query(
                    "UPDATE transcripts SET transcript='A statement.' WHERE id='context-question'",
                )
                .execute(&pool)
                .await
                .unwrap();
                passages[0].text = "A statement.".into();
            }
            if scenario == "intervening" {
                sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES ('between','fixture','Pause.','00:01:30')").execute(&pool).await.unwrap();
            }
            if scenario == "changed_revision" {
                sqlx::query("UPDATE meetings SET title='Changed' WHERE id='fixture'")
                    .execute(&pool)
                    .await
                    .unwrap();
            } else {
                let job:store::SourceJob=sqlx::query_as("SELECT id AS source_id,meeting_id,revision,generation FROM knowledge_sources WHERE meeting_id='fixture'").fetch_one(&pool).await.unwrap();
                for (index, passage) in passages.iter_mut().enumerate() {
                    let id = if index == 0 {
                        "context-question"
                    } else {
                        "context-reply"
                    };
                    let partial = (scenario == "partial_question" && index == 0)
                        || (scenario == "partial_reply" && index == 1);
                    *passage = store::materialize(
                        &pool,
                        &store::SelectedRow::for_job(&job, id.into()),
                        TextSpan {
                            transcript_id: id.into(),
                            start_byte: usize::from(partial),
                            end_byte: passage.text.len(),
                        },
                        false,
                    )
                    .await
                    .unwrap();
                }
            }
            if scenario == "outside_scope" {
                frozen.meeting_ids.clear();
            }
            if scenario == "different_meeting" {
                passages[0].meeting_id = "other-meeting".into();
            }
            assert_eq!(
                preceding_questions(&pool, &frozen, &passages)
                    .await
                    .unwrap(),
                vec![None, None],
                "{scenario}"
            );
        }
    }

    #[tokio::test]
    async fn context_metadata_rejects_cross_source_revision_chains_and_partial_spans() {
        let (_, _, _, passages) =
            crate::knowledge::conversations::tests::question_pair_fixture("Ja.").await;
        let refs = passages
            .iter()
            .map(|p| p.evidence.clone())
            .collect::<Vec<_>>();
        let metadata = passages
            .iter()
            .enumerate()
            .map(|(index, p)| EvidenceDisplay {
                title: p.title.clone(),
                date: p.date.clone(),
                speaker: p.speaker.clone(),
                metadata_truncated: false,
                preceding_question_tag: (index == 1).then_some(1),
            })
            .collect::<Vec<_>>();
        assert!(validate_context_metadata(&refs, &metadata).is_ok());
        for scenario in [
            "source",
            "revision",
            "chain",
            "partial",
            "same_row",
            "historical",
        ] {
            let mut refs = refs.clone();
            let mut metadata = metadata.clone();
            match scenario {
                "source" => refs[0].source_id = "meeting:other".into(),
                "revision" => refs[0].source_revision += 1,
                "chain" => metadata[0].preceding_question_tag = Some(2),
                "same_row" => refs[0] = refs[1].clone(),
                "historical" => refs[0].historical = true,
                _ => {
                    if let EvidenceLocator::Transcript { spans, .. } = &mut refs[1].locator {
                        spans[0].start_byte = 1;
                    }
                }
            }
            assert!(
                validate_context_metadata(&refs, &metadata).is_err(),
                "{scenario}"
            );
        }
    }

    #[test]
    fn grouped_citations_retain_every_explicit_original_ordinal() {
        assert_eq!(
            tag_numbers("Changed [K3, K1]. Repeated [K3].", 3),
            vec![1, 3]
        );
        assert_eq!(
            tag_numbers("Context [K1-K4]. Limit [K6, K5].", 6),
            vec![1, 2, 3, 4, 5, 6]
        );
        assert_eq!(
            tag_numbers("Facts [K2,K1] and [K3 - K4].", 4),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn nested_sibling_groups_never_salvage_inner_tags() {
        for malformed in ["[K1[K2][K3]]", "[[K1][K2]]", "[K1[K2][K3]"] {
            assert!(tag_numbers(malformed, 3).is_empty());
        }
        assert_eq!(tag_numbers("[K1[K2][K3]] then [K2]", 3), vec![2]);
    }

    #[test]
    fn malformed_or_out_of_map_groups_never_create_links() {
        for content in [
            "[K1, K999]",
            "[K0-K3]",
            "[K3-K1]",
            "[K1-K999]",
            "[K01, K2]",
            "[K1,2]",
            "[K1,,K2]",
            "[K1-K2-K3]",
            "[K1, K2-K3]",
            "[K1; K2]",
            "[K1 and K2]",
            "[K1[K2]",
            "[K1, K2",
        ] {
            assert!(
                tag_numbers(content, 3).is_empty(),
                "Invalid citation group: {content}"
            );
        }
        assert!(tag_numbers("[K1, K2]", 1).is_empty());
        assert!(tag_numbers("[K1-K65]", 999).is_empty());
        assert!(tag_numbers(&format!("[{}]", vec!["K1"; 65].join(",")), 1).is_empty());
        assert_eq!(tag_numbers("[K1-K64]", 64), (1..=64).collect::<Vec<_>>());
        assert_eq!(tag_numbers("Unknown [K999]. Known [K1].", 1), vec![1]);
    }

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
