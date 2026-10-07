//! Scoped keyword and local semantic retrieval.
use super::store::{self, CanonicalRow, SourceJob};
use super::types::*;
use sqlx::{FromRow, Row, SqlitePool};
use std::collections::{BTreeMap, BinaryHeap};

pub const CANDIDATES: usize = 64;
pub const VECTOR_PAGE: usize = 512;
pub const RESULT_LIMIT: usize = 12;
pub const RRF_CONSTANT: f64 = 60.;

#[derive(Debug, Clone)]
pub struct FrozenScope {
    pub scope: KnowledgeScope,
    pub meeting_ids: Vec<String>,
}
pub async fn freeze_scope(
    pool: &SqlitePool,
    scope: &KnowledgeScope,
) -> Result<FrozenScope, KnowledgeError> {
    let filter = match scope {
        KnowledgeScope::Meeting { meeting_id } => MeetingFilter {
            meeting_ids: vec![meeting_id.clone()],
            ..Default::default()
        },
        KnowledgeScope::Library { filter } => filter.clone(),
        KnowledgeScope::Live { .. } => return Err(KnowledgeError::InvalidInput),
    };
    if !filter.all_meetings
        && filter.meeting_ids.is_empty()
        && filter.tags.is_empty()
        && !filter.untagged
        && filter.from.is_none()
        && filter.to.is_none()
    {
        return Err(KnowledgeError::InvalidInput);
    }
    if filter.meeting_ids.iter().any(|id| id.trim().is_empty())
        || filter.tags.iter().any(|tag| tag.trim().is_empty())
        || filter.from.as_ref().is_some_and(|v| v.len() < 10)
        || filter.to.as_ref().is_some_and(|v| v.len() < 10)
    {
        return Err(KnowledgeError::InvalidInput);
    }
    let mut ids = std::collections::BTreeMap::<String, (String, Vec<String>)>::new();
    let rows=sqlx::query("SELECT m.id,m.created_at,t.tag FROM meetings m LEFT JOIN meeting_tags t ON t.meeting_id=m.id ORDER BY m.id").fetch_all(pool).await?;
    for row in rows {
        let id: String = row.try_get("id")?;
        let date: String = row.try_get("created_at")?;
        let entry = ids.entry(id).or_insert((date, Vec::new()));
        if let Some(tag) = row.try_get::<Option<String>, _>("tag")? {
            entry.1.push(tag.to_lowercase());
        }
    }
    let meeting_ids = ids
        .into_iter()
        .filter(|(id, (date, tags))| {
            (filter.meeting_ids.is_empty() || filter.meeting_ids.contains(id))
                && filter
                    .from
                    .as_ref()
                    .is_none_or(|from| date.get(..10).unwrap_or(date) >= from.as_str())
                && filter
                    .to
                    .as_ref()
                    .is_none_or(|to| date.get(..10).unwrap_or(date) <= to.as_str())
                && if filter.untagged {
                    tags.is_empty()
                } else if filter.tags.is_empty() {
                    true
                } else {
                    match filter.tag_mode {
                        TagMatch::Any => filter
                            .tags
                            .iter()
                            .any(|tag| tags.contains(&tag.to_lowercase())),
                        TagMatch::All => filter
                            .tags
                            .iter()
                            .all(|tag| tags.contains(&tag.to_lowercase())),
                    }
                }
        })
        .map(|(id, _)| id)
        .collect();
    Ok(FrozenScope {
        scope: scope.clone(),
        meeting_ids,
    })
}

pub async fn recheck_scope(pool: &SqlitePool, frozen: &FrozenScope) -> Result<(), KnowledgeError> {
    let current = freeze_scope(pool, &frozen.scope).await?;
    if frozen
        .meeting_ids
        .iter()
        .any(|id| !current.meeting_ids.contains(id))
    {
        return Err(KnowledgeError::Superseded);
    }
    Ok(())
}

pub async fn search_channels(
    pool: &SqlitePool,
    scope: &FrozenScope,
    query: &str,
    vector: Option<(&str, &[f32])>,
) -> Result<Vec<Passage>, KnowledgeError> {
    if query.trim().is_empty() || query.len() > 1024 {
        return Err(KnowledgeError::InvalidInput);
    }
    if scope.meeting_ids.is_empty() {
        return Ok(Vec::new());
    }
    let allowed =
        serde_json::to_string(&scope.meeting_ids).map_err(|_| KnowledgeError::InvalidInput)?;
    let active_space = vector
        .map(|(space, _)| space.to_owned())
        .unwrap_or_else(|| super::model::PINS.space().id);
    let mut lexical = Vec::new();
    let terms: Vec<&str> = query
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|term| !term.is_empty())
        .take(64)
        .collect();
    if !terms.is_empty() {
        let expression = terms
            .iter()
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let ids:Vec<String>=sqlx::query_scalar("SELECT f.transcript_id FROM knowledge_fts f JOIN transcripts t ON t.id=f.transcript_id JOIN json_each(?) allowed ON allowed.value=t.meeting_id WHERE knowledge_fts MATCH ? ORDER BY bm25(knowledge_fts),f.transcript_id LIMIT 64").bind(&allowed).bind(expression).fetch_all(pool).await?;
        for id in ids {
            let Some(data) = canonical(pool, &id).await? else {
                continue;
            };
            let Some((start, end)) = match_span(&data.0.transcript, query, &terms) else {
                continue;
            };
            let covering:Option<(i64,i64)>=sqlx::query_as("SELECT c.start_byte,c.end_byte FROM knowledge_chunks c JOIN knowledge_sources s ON s.id=c.source_id WHERE c.transcript_id=? AND c.revision=s.revision AND c.generation=s.generation AND s.semantic_revision=s.revision AND s.semantic_space=? AND c.start_byte<=? AND c.end_byte>=? ORDER BY c.ordinal LIMIT 1").bind(&id).bind(&active_space).bind(start as i64).bind(end as i64).fetch_optional(pool).await?;
            let span = if let Some((start, end)) = covering {
                TextSpan {
                    transcript_id: id,
                    start_byte: start as usize,
                    end_byte: end as usize,
                }
            } else {
                let regular = super::chunking::lexical_spans(&id, &data.0.transcript)
                    .into_iter()
                    .find(|span| span.start_byte <= start && span.end_byte >= end);
                regular.unwrap_or_else(|| {
                    let mut begin =
                        start.saturating_sub((super::chunking::LEXICAL_BYTES - (end - start)) / 2);
                    while !data.0.transcript.is_char_boundary(begin) {
                        begin += 1;
                    }
                    let finish = super::chunking::floor_boundary(
                        &data.0.transcript,
                        begin + super::chunking::LEXICAL_BYTES,
                    );
                    TextSpan {
                        transcript_id: id,
                        start_byte: begin,
                        end_byte: finish,
                    }
                })
            };
            lexical.push(passage(data, span)?);
        }
    }
    let semantic = if let Some((space, vector)) = vector {
        semantic_candidates(pool, &allowed, space, vector).await?
    } else {
        Vec::new()
    };
    let mut merged = BTreeMap::<String, Passage>::new();
    for channel in [lexical, semantic] {
        for (rank, mut passage) in channel.into_iter().take(CANDIDATES).enumerate() {
            let score = 1. / (RRF_CONSTANT + (rank + 1) as f64);
            if let Some(existing) = merged.get_mut(&passage.evidence.chunk_id) {
                existing.rank += score;
            } else {
                passage.rank = score;
                merged.insert(passage.evidence.chunk_id.clone(), passage);
            }
        }
    }
    let mut candidates: Vec<_> = merged.into_values().collect();
    candidates.sort_by(|a, b| {
        b.rank
            .total_cmp(&a.rank)
            .then_with(|| a.evidence.chunk_id.cmp(&b.evidence.chunk_id))
    });
    let mut result: Vec<Passage> = Vec::new();
    for mut candidate in candidates {
        if result
            .iter()
            .any(|existing| contained(existing, &candidate))
        {
            continue;
        }
        result.retain(|existing| {
            if contained(&candidate, existing) {
                candidate.rank = candidate.rank.max(existing.rank);
                false
            } else {
                true
            }
        });
        let revision: Option<i64> =
            sqlx::query_scalar("SELECT revision FROM knowledge_sources WHERE id=?")
                .bind(&candidate.evidence.source_id)
                .fetch_optional(pool)
                .await?;
        if revision != Some(candidate.evidence.source_revision) {
            return Err(KnowledgeError::Superseded);
        }
        result.push(candidate);
    }
    result.sort_by(|a, b| {
        b.rank
            .total_cmp(&a.rank)
            .then_with(|| a.evidence.chunk_id.cmp(&b.evidence.chunk_id))
    });
    result.truncate(RESULT_LIMIT);
    Ok(result)
}

fn contained(a: &Passage, b: &Passage) -> bool {
    if a.evidence.source_id != b.evidence.source_id
        || a.evidence.source_revision != b.evidence.source_revision
    {
        return false;
    }
    match (&a.evidence.locator, &b.evidence.locator) {
        (
            EvidenceLocator::Transcript { spans: a, .. },
            EvidenceLocator::Transcript { spans: b, .. },
        ) if a.len() == 1 && b.len() == 1 => {
            a[0].transcript_id == b[0].transcript_id
                && a[0].start_byte <= b[0].start_byte
                && a[0].end_byte >= b[0].end_byte
        }
        _ => false,
    }
}
fn folded_match(text: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let mut folded = String::new();
    let mut map = Vec::new();
    for (start, ch) in text.char_indices() {
        for lower in ch.to_lowercase() {
            folded.push(lower);
            for _ in 0..lower.len_utf8() {
                map.push((start, start + ch.len_utf8()));
            }
        }
    }
    let needle = needle.to_lowercase();
    let start = folded.find(&needle)?;
    Some((map[start].0, map[start + needle.len() - 1].1))
}
fn match_span(text: &str, query: &str, terms: &[&str]) -> Option<(usize, usize)> {
    if let Some(span) = folded_match(text, query.trim()) {
        return Some(span);
    }
    let mut terms = terms.to_vec();
    terms.sort_by_key(|term| std::cmp::Reverse(term.len()));
    terms.into_iter().find_map(|term| folded_match(text, term))
}
async fn canonical(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<(CanonicalRow, SourceJob, String, String)>, KnowledgeError> {
    let row=sqlx::query("SELECT t.*,s.id AS source_id,s.revision,s.generation,m.title,m.created_at AS date FROM transcripts t JOIN knowledge_sources s ON s.meeting_id=t.meeting_id JOIN meetings m ON m.id=t.meeting_id WHERE t.id=?").bind(id).fetch_optional(pool).await?;
    row.map(|row| {
        Ok((
            CanonicalRow::from_row(&row)?,
            SourceJob {
                source_id: row.try_get("source_id")?,
                meeting_id: row.try_get("meeting_id")?,
                revision: row.try_get("revision")?,
                generation: row.try_get("generation")?,
            },
            row.try_get("title")?,
            row.try_get("date")?,
        ))
    })
    .transpose()
}
fn passage(
    data: (CanonicalRow, SourceJob, String, String),
    span: TextSpan,
) -> Result<Passage, KnowledgeError> {
    let (row, job, title, date) = data;
    let evidence = store::evidence(&job, &row, &span)?;
    Ok(Passage {
        evidence,
        meeting_id: row.meeting_id,
        title,
        date,
        speaker: row.speaker,
        text: row.transcript[span.start_byte..span.end_byte].into(),
        rank: 0.,
    })
}
#[derive(Debug)]
struct Scored {
    score: f64,
    id: String,
}
impl PartialEq for Scored {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score && self.id == other.id
    }
}
impl Eq for Scored {}
impl PartialOrd for Scored {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Scored {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then_with(|| self.id.cmp(&other.id))
    }
}
async fn semantic_candidates(
    pool: &SqlitePool,
    allowed: &str,
    space: &str,
    query: &[f32],
) -> Result<Vec<Passage>, KnowledgeError> {
    let query = super::embedding::normalize(query.to_vec())?;
    let mut heap = BinaryHeap::<Scored>::new();
    let mut after = String::new();
    loop {
        let page:Vec<(String,Vec<u8>)>=sqlx::query_as("SELECT c.id,v.vector FROM knowledge_vectors v JOIN knowledge_chunks c ON c.id=v.chunk_id JOIN knowledge_sources s ON s.id=c.source_id JOIN json_each(?) allowed ON allowed.value=s.meeting_id WHERE v.space=? AND s.semantic_space=? AND s.semantic_revision=s.revision AND c.revision=s.revision AND c.generation=s.generation AND c.id>? ORDER BY c.id LIMIT ?").bind(allowed).bind(space).bind(space).bind(&after).bind(VECTOR_PAGE as i64).fetch_all(pool).await?;
        if page.is_empty() {
            break;
        }
        after = page.last().unwrap().0.clone();
        let query = query.clone();
        heap = tokio::task::spawn_blocking(move || {
            for (id, bytes) in page {
                if bytes.len() != 384 * 4 {
                    continue;
                }
                let mut score = 0.;
                for (value, weight) in bytes.chunks_exact(4).zip(&query) {
                    score += f32::from_le_bytes(value.try_into().unwrap()) as f64 * *weight as f64;
                }
                if !score.is_finite() {
                    continue;
                }
                heap.push(Scored { score, id });
                if heap.len() > CANDIDATES {
                    heap.pop();
                }
            }
            heap
        })
        .await
        .map_err(|_| KnowledgeError::ProviderFailure)?;
    }
    let mut best = heap.into_vec();
    best.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    let mut result = Vec::new();
    for hit in best {
        let chunk:Option<(String,i64,i64,String)>=sqlx::query_as("SELECT c.transcript_id,c.start_byte,c.end_byte,c.fingerprint FROM knowledge_chunks c JOIN knowledge_sources s ON s.id=c.source_id WHERE c.id=? AND c.revision=s.revision AND c.generation=s.generation AND s.semantic_revision=s.revision AND s.semantic_space=?").bind(&hit.id).bind(space).fetch_optional(pool).await?;
        if let Some((id, start, end, fingerprint)) = chunk {
            if let Some(data) = canonical(pool, &id).await? {
                let item = passage(
                    data,
                    TextSpan {
                        transcript_id: id,
                        start_byte: start as usize,
                        end_byte: end as usize,
                    },
                )?;
                if item.evidence.fingerprint == fingerprint && item.evidence.chunk_id == hit.id {
                    result.push(item);
                }
            }
        }
    }
    Ok(result)
}

pub async fn retrieve(
    pool: &SqlitePool,
    runtime: &super::KnowledgeState,
    request: &SearchRequest,
) -> Result<SearchResponse, KnowledgeError> {
    if request.query.trim().is_empty()
        || request.query.len() > 1024
        || !request.document_ids.is_empty()
    {
        return Err(KnowledgeError::InvalidInput);
    }
    let scope = freeze_scope(pool, &request.scope).await?;
    let space = super::model::PINS.space();
    let mut status = super::store::status(pool, runtime.scheduler.is_enabled(), &space.id).await?;
    let vector = if request.mode == SearchMode::Hybrid {
        match runtime
            .scheduler
            .embed(request.query.clone(), EmbeddingPurpose::Query)
            .await
        {
            Ok(vector) => Some(vector),
            Err(error) => {
                status.reason = Some(
                    match error {
                        KnowledgeError::Busy => "busy",
                        KnowledgeError::Cancelled => "cancelled",
                        KnowledgeError::Disabled => "disabled",
                        KnowledgeError::InvalidInput => "query_token_limit",
                        _ => "model_unavailable",
                    }
                    .into(),
                );
                None
            }
        }
    } else {
        None
    };
    let passages = search_channels(
        pool,
        &scope,
        &request.query,
        vector.as_ref().map(|v| (space.id.as_str(), v.as_slice())),
    )
    .await?;
    recheck_scope(pool, &scope).await?;
    Ok(SearchResponse {
        passages,
        mode: if vector.is_some() && status.semantic_ready > 0 {
            SearchMode::Hybrid
        } else {
            SearchMode::Keyword
        },
        index_status: status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn add_text(pool: &SqlitePool, id: &str, meeting: &str, text: &str) {
        sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp,audio_start_time) VALUES(?,?,?,'00:01',1)").bind(id).bind(meeting).bind(text).execute(pool).await.unwrap();
    }
    #[tokio::test]
    async fn keyword_search_uses_current_canonical_rows_and_backend_scope() {
        let pool = fixture().await;
        add_text(&pool, "a", "one", "ATLAS-42 release is Friday").await;
        add_text(
            &pool,
            "b",
            "two",
            "ATLAS-42 ATLAS-42 ATLAS-42 unrelated project",
        )
        .await;
        let scope = freeze_scope(
            &pool,
            &KnowledgeScope::Meeting {
                meeting_id: "one".into(),
            },
        )
        .await
        .unwrap();
        let hits = search_channels(&pool, &scope, "ATLAS-42", None)
            .await
            .unwrap();
        assert_eq!(
            hits.len(),
            1,
            "keyword evidence cannot depend on embeddings"
        );
        assert_eq!(hits[0].meeting_id, "one");
        assert!(hits[0].text.contains("Friday"));
        sqlx::query("UPDATE transcripts SET transcript='Corrected to Monday', original_transcript='ATLAS-42' WHERE id='a'").execute(&pool).await.unwrap();
        assert!(search_channels(&pool, &scope, "ATLAS-42", None)
            .await
            .unwrap()
            .is_empty());
        assert!(search_channels(&pool, &scope, "\" OR * : )", None)
            .await
            .unwrap()
            .is_empty());
    }
    #[tokio::test]
    async fn hybrid_preserves_identifier_hits_and_fuses_only_covering_chunks() {
        use super::super::store;
        let pool = fixture().await;
        let text = format!(
            "{} ATLAS-42 schedule is Friday",
            "General status. ".repeat(30)
        );
        add_text(&pool, "a", "one", &text).await;
        let job:store::SourceJob=sqlx::query_as("SELECT j.source_id,s.meeting_id,j.revision,j.generation FROM knowledge_index_jobs j JOIN knowledge_sources s ON s.id=j.source_id WHERE s.meeting_id='one'").fetch_one(&pool).await.unwrap();
        let row = store::rows_page(&pool, &job, None).await.unwrap().remove(0);
        let vector = super::super::embedding::normalize(vec![1.; 384]).unwrap();
        let span = TextSpan {
            transcript_id: "a".into(),
            start_byte: 0,
            end_byte: 15,
        };
        store::stage(&pool, &job, &row, &span, 0, &vector, "test-space")
            .await
            .unwrap();
        store::publish(&pool, &job, "test-space").await.unwrap();
        let scope = freeze_scope(
            &pool,
            &KnowledgeScope::Meeting {
                meeting_id: "one".into(),
            },
        )
        .await
        .unwrap();
        let hits = search_channels(&pool, &scope, "ATLAS-42", Some(("test-space", &vector)))
            .await
            .unwrap();
        assert!(
            hits.iter().any(|hit| hit.text.contains("ATLAS-42")),
            "partial semantic neighbor must not replace exact lexical evidence"
        );
        let wrong_space = search_channels(
            &pool,
            &scope,
            "nonmatchingword",
            Some(("new-space", &vector)),
        )
        .await
        .unwrap();
        assert!(
            wrong_space.is_empty(),
            "model_space_change_requires_reindex"
        );
    }
    #[tokio::test]
    async fn frozen_scope_recheck_detects_removal_without_adding_new_members() {
        let pool = fixture().await;
        let scope = KnowledgeScope::Library {
            filter: MeetingFilter {
                tags: vec!["beta".into()],
                ..Default::default()
            },
        };
        let frozen = freeze_scope(&pool, &scope).await.unwrap();
        sqlx::query("INSERT INTO meeting_tags(meeting_id,tag) VALUES('one','Beta')")
            .execute(&pool)
            .await
            .unwrap();
        recheck_scope(&pool, &frozen).await.unwrap();
        assert_eq!(frozen.meeting_ids, vec!["two"]);
        sqlx::query("DELETE FROM meeting_tags WHERE meeting_id='two' AND tag='Beta'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            recheck_scope(&pool, &frozen).await,
            Err(KnowledgeError::Superseded)
        );
    }
    async fn fixture() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        for (id, date) in [
            ("one", "2026-01-01"),
            ("two", "2026-09-01"),
            ("three", "2026-10-01"),
        ] {
            sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES (?, 'Public fixture', ?, ?)").bind(id).bind(date).bind(date).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO meeting_tags(meeting_id,tag) VALUES ('one','ÄPFEL'),('two','Äpfel'),('two','Beta')").execute(&pool).await.unwrap();
        pool
    }
    #[tokio::test]
    async fn tag_scope_applies_before_ranking() {
        let pool = fixture().await;
        let scope = KnowledgeScope::Library {
            filter: MeetingFilter {
                tags: vec!["äpfel".into(), "beta".into()],
                tag_mode: TagMatch::All,
                ..Default::default()
            },
        };
        assert_eq!(
            freeze_scope(&pool, &scope).await.unwrap().meeting_ids,
            vec!["two"]
        );
        let scope = KnowledgeScope::Library {
            filter: MeetingFilter {
                tags: vec!["äpfel".into()],
                ..Default::default()
            },
        };
        assert_eq!(
            freeze_scope(&pool, &scope).await.unwrap().meeting_ids,
            vec!["one", "two"]
        );
    }
    #[tokio::test]
    async fn empty_scope_never_broadens_and_untagged_has_precedence() {
        let pool = fixture().await;
        assert!(freeze_scope(
            &pool,
            &KnowledgeScope::Library {
                filter: MeetingFilter::default()
            }
        )
        .await
        .is_err());
        let scope = KnowledgeScope::Library {
            filter: MeetingFilter {
                tags: vec!["äpfel".into()],
                untagged: true,
                ..Default::default()
            },
        };
        assert_eq!(
            freeze_scope(&pool, &scope).await.unwrap().meeting_ids,
            vec!["three"]
        );
    }
    #[tokio::test]
    async fn selected_ids_and_dates_intersect() {
        let pool = fixture().await;
        let scope = KnowledgeScope::Library {
            filter: MeetingFilter {
                meeting_ids: vec!["one".into(), "two".into()],
                from: Some("2026-06-01".into()),
                to: Some("2026-09-30".into()),
                ..Default::default()
            },
        };
        assert_eq!(
            freeze_scope(&pool, &scope).await.unwrap().meeting_ids,
            vec!["two"]
        );
        assert!(freeze_scope(
            &pool,
            &KnowledgeScope::Live {
                session_id: "live".into()
            }
        )
        .await
        .is_err());
    }
}
