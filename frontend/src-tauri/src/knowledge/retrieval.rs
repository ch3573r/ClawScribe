//! Scoped keyword and local semantic retrieval.
use super::store;
#[cfg(test)]
use super::store::{CanonicalRow, SourceJob};
use super::types::*;
use sqlx::{FromRow, Row, SqlitePool};
use std::collections::{BTreeMap, BinaryHeap};

pub const CANDIDATES: usize = 64;
pub const VECTOR_PAGE: usize = 512;
pub const RESULT_LIMIT: usize = 12;
pub const RRF_CONSTANT: f64 = 60.;
#[cfg(test)]
tokio::task_local! { static MOVE_AFTER_FTS: std::cell::RefCell<Option<(String,String)>>; }
#[cfg(test)]
pub(crate) static MAPPING_PEAK: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[derive(Debug, Clone)]
pub struct FrozenScope {
    pub scope: KnowledgeScope,
    pub meeting_ids: Vec<String>,
}
fn valid_scope_date(value: &str) -> bool {
    value.len() == 10
        && value.bytes().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
        && chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}
pub async fn freeze_scope(
    pool: &SqlitePool,
    scope: &KnowledgeScope,
) -> Result<FrozenScope, KnowledgeError> {
    freeze_scope_in_connection(&mut *pool.acquire().await?, scope).await
}
pub async fn freeze_scope_in_connection(
    connection: &mut sqlx::SqliteConnection,
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
        || filter.from.as_ref().is_some_and(|v| !valid_scope_date(v))
        || filter.to.as_ref().is_some_and(|v| !valid_scope_date(v))
        || matches!((&filter.from,&filter.to),(Some(from),Some(to)) if from>to)
    {
        return Err(KnowledgeError::InvalidInput);
    }
    let mut ids = std::collections::BTreeMap::<String, (String, Vec<String>)>::new();
    let rows=sqlx::query("SELECT m.id,m.created_at,t.tag FROM meetings m LEFT JOIN meeting_tags t ON t.meeting_id=m.id ORDER BY m.id").fetch_all(&mut *connection).await?;
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
    recheck_scope_in_connection(&mut *pool.acquire().await?, frozen).await
}
pub async fn recheck_scope_in_connection(
    connection: &mut sqlx::SqliteConnection,
    frozen: &FrozenScope,
) -> Result<(), KnowledgeError> {
    let current = freeze_scope_in_connection(connection, &frozen.scope).await?;
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
    let raw_terms: Vec<&str> = query
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|term| !term.is_empty())
        .take(64)
        .collect();
    let content_terms: Vec<&str> = raw_terms
        .iter()
        .copied()
        .filter(|term| {
            !matches!(
                term.to_lowercase().as_str(),
                "the"
                    | "a"
                    | "an"
                    | "of"
                    | "to"
                    | "in"
                    | "on"
                    | "at"
                    | "and"
                    | "or"
                    | "for"
                    | "is"
                    | "are"
                    | "was"
                    | "were"
                    | "will"
                    | "would"
                    | "can"
                    | "could"
                    | "should"
                    | "must"
                    | "be"
                    | "been"
                    | "do"
                    | "does"
                    | "did"
                    | "what"
                    | "which"
                    | "where"
                    | "when"
                    | "who"
                    | "how"
                    | "der"
                    | "die"
                    | "das"
                    | "den"
                    | "dem"
                    | "des"
                    | "ein"
                    | "eine"
                    | "einer"
                    | "einem"
                    | "einen"
                    | "und"
                    | "oder"
                    | "für"
                    | "im"
                    | "am"
                    | "ist"
                    | "sind"
                    | "wird"
                    | "werden"
                    | "wurde"
                    | "wurden"
                    | "wie"
                    | "wo"
                    | "wann"
                    | "wer"
                    | "welche"
                    | "welcher"
                    | "welches"
                    | "mit"
                    | "von"
                    | "zu"
                    | "zum"
                    | "zur"
            )
        })
        .collect();
    let terms = if content_terms.is_empty() {
        raw_terms.clone()
    } else {
        content_terms
    };
    if !terms.is_empty() {
        let body_expression = terms
            .iter()
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        // Speaker names can also be ordinary function words (for example Will).
        // Search their dedicated metadata column with the unfiltered tokens.
        let speaker_expression = raw_terms
            .iter()
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let expression = format!("text: ({body_expression}) OR speaker: ({speaker_expression})");
        let selected:Vec<store::SelectedRow>=sqlx::query_as("SELECT f.transcript_id,s.id AS source_id,s.meeting_id,s.revision,s.generation FROM knowledge_fts f JOIN transcripts t ON t.id=f.transcript_id JOIN knowledge_sources s ON s.meeting_id=t.meeting_id JOIN json_each(?) allowed ON allowed.value=t.meeting_id WHERE knowledge_fts MATCH ? ORDER BY bm25(knowledge_fts),f.transcript_id LIMIT 64").bind(&allowed).bind(expression).fetch_all(pool).await?;
        #[cfg(test)]
        if let Ok(Some((id, owner))) = MOVE_AFTER_FTS.try_with(|slot| slot.borrow_mut().take()) {
            sqlx::query("UPDATE transcripts SET meeting_id=? WHERE id=?")
                .bind(owner)
                .bind(id)
                .execute(pool)
                .await?;
        }
        for selected in selected {
            let Some(hit) = store::locate(
                pool,
                &selected,
                query.into(),
                terms.iter().map(|s| (*s).into()).collect(),
                raw_terms.iter().map(|s| (*s).into()).collect(),
            )
            .await?
            else {
                continue;
            };
            let covering:Option<(i64,i64)>=sqlx::query_as("SELECT c.start_byte,c.end_byte FROM knowledge_chunks c JOIN knowledge_sources s ON s.id=c.source_id WHERE c.transcript_id=? AND s.id=? AND s.revision=? AND s.generation=? AND c.revision=s.revision AND c.generation=s.generation AND s.semantic_revision=s.revision AND s.semantic_space=? AND c.start_byte<=? AND c.end_byte>=? ORDER BY c.ordinal LIMIT 1").bind(&selected.transcript_id).bind(&selected.source_id).bind(selected.revision).bind(selected.generation).bind(&active_space).bind(hit.start as i64).bind(hit.end as i64).fetch_optional(pool).await?;
            let span = if let Some((start, end)) = covering {
                TextSpan {
                    transcript_id: selected.transcript_id.clone(),
                    start_byte: start as usize,
                    end_byte: end as usize,
                }
            } else {
                hit.lexical
            };
            lexical.push(store::materialize(pool, &selected, span, false).await?);
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
        if !scope.meeting_ids.contains(&candidate.meeting_id) {
            return Err(KnowledgeError::Superseded);
        }
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
#[derive(Debug)]
struct Scored {
    score: f64,
    id: String,
    source_id: String,
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
    // Empty or stale generations do not reduce another source's budget.
    let eligible: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT s.id) FROM knowledge_sources s JOIN knowledge_chunks c ON c.source_id=s.id JOIN knowledge_vectors v ON v.chunk_id=c.id JOIN json_each(?) allowed ON allowed.value=s.meeting_id WHERE s.semantic_revision=s.revision AND s.semantic_space=? AND c.revision=s.revision AND c.generation=s.generation AND v.space=s.semantic_space")
        .bind(allowed).bind(space).fetch_one(pool).await?;
    let per_source = if eligible >= 2 { 3 } else { CANDIDATES };
    let mut heap = BinaryHeap::<Scored>::new();
    let mut after = String::new();
    loop {
        let page:Vec<(String,Vec<u8>,String)>=sqlx::query_as("SELECT c.id,v.vector,s.id FROM knowledge_vectors v JOIN knowledge_chunks c ON c.id=v.chunk_id JOIN knowledge_sources s ON s.id=c.source_id JOIN json_each(?) allowed ON allowed.value=s.meeting_id WHERE v.space=? AND s.semantic_space=? AND s.semantic_revision=s.revision AND c.revision=s.revision AND c.generation=s.generation AND c.id>? ORDER BY c.id LIMIT ?").bind(allowed).bind(space).bind(space).bind(&after).bind(VECTOR_PAGE as i64).fetch_all(pool).await?;
        if page.is_empty() {
            break;
        }
        debug_assert!(page.len() <= 512);
        after = page.last().unwrap().0.clone();
        let query = query.clone();
        heap = tokio::task::spawn_blocking(move || {
            for (id, bytes, source_id) in page {
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
                let hit = Scored {
                    score,
                    id,
                    source_id,
                };
                if per_source < CANDIDATES {
                    let same_source = || heap.iter().filter(|item| item.source_id == hit.source_id);
                    if same_source().count() >= per_source {
                        let weakest = same_source().max().unwrap();
                        if hit >= *weakest {
                            continue;
                        }
                        let remove = weakest.id.clone();
                        heap.retain(|item| item.id != remove);
                    }
                }
                heap.push(hit);
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
        let chunk=sqlx::query("SELECT c.transcript_id,c.start_byte,c.end_byte,c.fingerprint,s.id AS source_id,s.meeting_id,s.revision,s.generation FROM knowledge_chunks c JOIN knowledge_sources s ON s.id=c.source_id JOIN json_each(?) allowed ON allowed.value=s.meeting_id WHERE c.id=? AND c.revision=s.revision AND c.generation=s.generation AND s.semantic_revision=s.revision AND s.semantic_space=?").bind(allowed).bind(&hit.id).bind(space).fetch_optional(pool).await?;
        if let Some(chunk) = chunk {
            let selected = store::SelectedRow::from_row(&chunk)?;
            let fingerprint: String = chunk.try_get("fingerprint")?;
            let span = TextSpan {
                transcript_id: selected.transcript_id.clone(),
                start_byte: chunk.try_get::<i64, _>("start_byte")? as usize,
                end_byte: chunk.try_get::<i64, _>("end_byte")? as usize,
            };
            let item = store::materialize(pool, &selected, span, false).await?;
            if item.evidence.fingerprint == fingerprint && item.evidence.chunk_id == hit.id {
                result.push(item);
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
    let allowed =
        serde_json::to_string(&scope.meeting_ids).map_err(|_| KnowledgeError::InvalidInput)?;
    let scoped_ready: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM knowledge_sources s JOIN knowledge_chunks c ON c.source_id=s.id JOIN knowledge_vectors v ON v.chunk_id=c.id JOIN json_each(?) allowed ON allowed.value=s.meeting_id WHERE s.semantic_revision=s.revision AND s.semantic_space=? AND c.revision=s.revision AND c.generation=s.generation AND v.space=s.semantic_space)")
        .bind(allowed).bind(&space.id).fetch_one(pool).await?;
    let vector = if request.mode == SearchMode::Hybrid && scoped_ready {
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
                        KnowledgeError::Disabled if status.semantic_enabled => "model_unavailable",
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
        mode: if vector.is_some() {
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
    #[tokio::test]
    async fn moving_a_selected_fts_row_cannot_escape_search_or_public_retrieval_scope() {
        let pool = fixture().await;
        add_text(&pool, "moving", "one", "ATLAS-42 scoped evidence").await;
        let request = SearchRequest {
            scope: KnowledgeScope::Meeting {
                meeting_id: "one".into(),
            },
            query: "ATLAS-42".into(),
            document_ids: Vec::new(),
            mode: SearchMode::Keyword,
        };
        for public in [false, true] {
            sqlx::query("UPDATE transcripts SET meeting_id='one' WHERE id='moving'")
                .execute(&pool)
                .await
                .unwrap();
            let frozen = freeze_scope(&pool, &request.scope).await.unwrap();
            let result = MOVE_AFTER_FTS
                .scope(
                    std::cell::RefCell::new(Some(("moving".into(), "two".into()))),
                    async {
                        if public {
                            retrieve(&pool, &super::super::KnowledgeState::default(), &request)
                                .await
                                .map(|response| response.passages)
                        } else {
                            search_channels(&pool, &frozen, &request.query, None).await
                        }
                    },
                )
                .await;
            match result {
                Ok(hits) => assert!(
                    hits.iter().all(|hit| hit.meeting_id == "one"),
                    "a post-selection move cannot disclose unselected evidence; public={public}"
                ),
                Err(KnowledgeError::Superseded) => {}
                Err(error) => panic!("unexpected scoped race error: {error}"),
            }
        }
    }
    #[tokio::test]
    async fn late_unicode_identifier_uses_bounded_inputs_and_maps_during_recording() {
        use std::sync::atomic::Ordering;
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = fixture().await;
        let text = format!(
            "{} ATLAS-42 Überprüfung",
            "Ä e\u{301} 🙂 ordinary material. ".repeat(40000)
        );
        add_text(&pool, "large", "one", &text).await;
        let scope = freeze_scope(
            &pool,
            &KnowledgeScope::Meeting {
                meeting_id: "one".into(),
            },
        )
        .await
        .unwrap();
        store::BODY_PEAK.store(0, Ordering::Relaxed);
        MAPPING_PEAK.store(0, Ordering::Relaxed);
        let recording = crate::audio::inference::claim_job().unwrap();
        let started = std::time::Instant::now();
        let hits = search_channels(&pool, &scope, "ATLAS-42", None)
            .await
            .unwrap();
        let elapsed = started.elapsed().as_millis();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("ATLAS-42"));
        if let EvidenceLocator::Transcript { spans, .. } = &hits[0].evidence.locator {
            assert!(spans[0].start_byte > 16384);
            assert_eq!(&text[spans[0].start_byte..spans[0].end_byte], hits[0].text);
        } else {
            panic!("canonical transcript locator required");
        }
        drop(recording);
        let body = store::BODY_PEAK.load(Ordering::Relaxed);
        let mapping = MAPPING_PEAK.load(Ordering::Relaxed);
        println!("KNOWLEDGE_LARGE_KEYWORD canonical_bytes={} retained_body_bytes={body} mapping_bytes={mapping} elapsed_ms={elapsed} recording_contended=true",text.len());
        assert!(
            body <= 16384,
            "canonical body loading must be byte bounded, got {body}"
        );
        assert!(
            mapping <= 16384 * 3 * std::mem::size_of::<(usize, usize)>(),
            "case-fold mapping must be bounded, got {mapping}"
        );
    }
    #[tokio::test]
    async fn oversized_metadata_is_explicitly_clipped_but_fingerprint_remains_complete() {
        let pool = fixture().await;
        add_text(&pool, "metadata", "one", "ATLAS-42 body").await;
        let speaker = "Änne🙂".repeat(10000);
        let timing = format!("{{\"synthetic\":\"{}\"}}", "x".repeat(256 * 1024));
        sqlx::query("UPDATE transcripts SET speaker=?,word_timestamps_json=? WHERE id='metadata'")
            .bind(&speaker)
            .bind(&timing)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE meetings SET title=? WHERE id='one'")
            .bind("Ü".repeat(2000))
            .execute(&pool)
            .await
            .unwrap();
        let row: CanonicalRow = sqlx::query_as("SELECT * FROM transcripts WHERE id='metadata'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let scope = freeze_scope(
            &pool,
            &KnowledgeScope::Meeting {
                meeting_id: "one".into(),
            },
        )
        .await
        .unwrap();
        store::METADATA_PEAK.store(0, std::sync::atomic::Ordering::Relaxed);
        let hits = search_channels(&pool, &scope, "ATLAS-42", None)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        let EvidenceLocator::Transcript { spans, .. } = &hits[0].evidence.locator else {
            panic!("transcript locator")
        };
        assert_eq!(
            hits[0].evidence.fingerprint,
            store::fingerprint(&row, &spans[0]).unwrap(),
            "complete canonical-v1 fingerprint must remain stable"
        );
        assert!(
            hits[0].metadata_truncated,
            "clipped display fields must be explicitly marked"
        );
        assert!(hits[0].speaker.as_ref().unwrap().len() <= 1024 && hits[0].title.len() <= 1024);
        assert!(
            store::METADATA_PEAK.load(std::sync::atomic::Ordering::Relaxed) <= 16384,
            "canonical metadata hashing must stream bounded buffers"
        );
    }
    #[tokio::test]
    async fn multiple_sources_balance_semantic_candidates_without_losing_identifier_hits() {
        let pool = fixture().await;
        for n in 0..70 {
            add_text(
                &pool,
                &format!("noise-{n}"),
                "one",
                &format!("Distinct inventory identifier ITEM-{n}"),
            )
            .await;
        }
        add_text(&pool, "identifier", "one", "ATLAS-42 exact identifier").await;
        add_text(&pool, "target", "two", "A separate relevant decision").await;
        while let Some(job) = store::next_job(&pool).await.unwrap() {
            let rows: Vec<CanonicalRow> =
                sqlx::query_as("SELECT * FROM transcripts WHERE meeting_id=? ORDER BY id")
                    .bind(&job.meeting_id)
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            for (n, row) in rows.iter().enumerate() {
                let mut vector = vec![0.; 384];
                vector[usize::from(!row.id.starts_with("noise-"))] = 1.;
                let span = TextSpan {
                    transcript_id: row.id.clone(),
                    start_byte: 0,
                    end_byte: row.transcript.len(),
                };
                store::stage(&pool, &job, row, &span, n as i64, &vector, "test-space")
                    .await
                    .unwrap();
            }
            store::publish(&pool, &job, "test-space").await.unwrap();
        }
        let mut query = vec![0.; 384];
        query[0] = 1.;
        let both = semantic_candidates(&pool, "[\"one\",\"two\"]", "test-space", &query)
            .await
            .unwrap();
        assert_eq!(both.iter().filter(|hit| hit.meeting_id == "one").count(), 3);
        assert!(
            both.iter().any(|hit| hit.meeting_id == "two"),
            "one source cannot starve another indexed source"
        );
        let sole = semantic_candidates(&pool, "[\"one\"]", "test-space", &query)
            .await
            .unwrap();
        assert_eq!(
            sole.len(),
            64,
            "a sole eligible source retains the full candidate budget"
        );
        let scope = freeze_scope(
            &pool,
            &KnowledgeScope::Library {
                filter: MeetingFilter {
                    all_meetings: true,
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
        let hits = search_channels(&pool, &scope, "ATLAS-42", Some(("test-space", &query)))
            .await
            .unwrap();
        assert!(
            hits.iter().any(|hit| hit.text.contains("ATLAS-42")),
            "semantic source balancing cannot remove lexical identifiers"
        );
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_vectors")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            stored, 72,
            "source balancing never merges or deletes distinct evidence"
        );
    }
    #[tokio::test]
    async fn hybrid_mode_requires_a_ready_generation_inside_selected_scope() {
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let pool = fixture().await;
        add_text(&pool, "a", "one", "Selected meeting contains ATLAS-42").await;
        sqlx::query("UPDATE knowledge_settings SET enabled=1 WHERE singleton=1")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE knowledge_sources SET semantic_revision=revision,semantic_space=? WHERE meeting_id='two'").bind(super::super::model::PINS.space().id).execute(&pool).await.unwrap();
        let mut runtime = super::super::KnowledgeState::default();
        runtime.scheduler = super::super::scheduler::synthetic_query_scheduler();
        let request = SearchRequest {
            scope: KnowledgeScope::Meeting {
                meeting_id: "one".into(),
            },
            query: "ATLAS-42".into(),
            document_ids: Vec::new(),
            mode: SearchMode::Hybrid,
        };
        let result = retrieve(&pool, &runtime, &request).await.unwrap();
        assert_eq!(result.passages.len(), 1);
        assert_eq!(
            result.mode,
            SearchMode::Keyword,
            "another meeting's ready index cannot determine this search's mode"
        );
    }
    #[tokio::test]
    async fn renamed_unicode_speaker_search_returns_current_canonical_evidence() {
        let pool = fixture().await;
        add_text(
            &pool,
            "speaker-row",
            "one",
            "Ja. Die Lieferung kommt am Freitag.",
        )
        .await;
        sqlx::query("UPDATE transcripts SET speaker='Özlem' WHERE id='speaker-row'")
            .execute(&pool)
            .await
            .unwrap();
        let scope = freeze_scope(
            &pool,
            &KnowledgeScope::Meeting {
                meeting_id: "one".into(),
            },
        )
        .await
        .unwrap();
        let hits = search_channels(&pool, &scope, "özlem", None).await.unwrap();
        assert_eq!(
            hits.len(),
            1,
            "speaker-only matches must retain canonical row evidence"
        );
        assert_eq!(hits[0].speaker.as_deref(), Some("Özlem"));
        assert_eq!(hits[0].text, "Ja. Die Lieferung kommt am Freitag.");
        let old = hits[0].evidence.clone();
        sqlx::query("UPDATE transcripts SET speaker='İpek' WHERE id='speaker-row'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(search_channels(&pool, &scope, "özlem", None)
            .await
            .unwrap()
            .is_empty());
        let hits = search_channels(&pool, &scope, "İpek", None).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].speaker.as_deref(), Some("İpek"));
        assert!(hits[0].text.len() <= 2048);
        assert_ne!(hits[0].evidence.fingerprint, old.fingerprint);
        assert!(hits[0].evidence.source_revision > old.source_revision);
        sqlx::query("UPDATE transcripts SET speaker='Will' WHERE id='speaker-row'")
            .execute(&pool)
            .await
            .unwrap();
        let hits = search_channels(&pool, &scope, "What did Will decide?", None)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].speaker.as_deref(), Some("Will"));
    }
    #[tokio::test]
    async fn date_scope_rejects_partial_invalid_timestamp_and_reversed_bounds() {
        let pool = fixture().await;
        for (from, to) in [
            (Some("2026-09"), None),
            (Some("2026-02-30"), None),
            (None, Some("2026-13-01")),
            (Some("2026-09-01T12:00:00Z"), None),
            (Some("2026-10-01"), Some("2026-09-01")),
            (Some("2026-9-01"), None),
            (Some("2026- 9-01"), None),
            (None, Some("2026-09- 1")),
        ] {
            let scope = KnowledgeScope::Library {
                filter: MeetingFilter {
                    all_meetings: true,
                    from: from.map(str::to_owned),
                    to: to.map(str::to_owned),
                    ..Default::default()
                },
            };
            assert!(matches!(
                freeze_scope(&pool, &scope).await,
                Err(KnowledgeError::InvalidInput)
            ));
        }
        let scope = KnowledgeScope::Library {
            filter: MeetingFilter {
                from: Some("2026-09-01".into()),
                to: Some("2026-09-01".into()),
                ..Default::default()
            },
        };
        assert_eq!(
            freeze_scope(&pool, &scope).await.unwrap().meeting_ids,
            vec!["two"]
        );
    }
    #[tokio::test]
    async fn vector_scan_reaches_last_page_with_bounded_candidates_and_results() {
        let pool = fixture().await;
        for n in 0..600 {
            add_text(
                &pool,
                &format!("row-{n:03}"),
                "one",
                &format!("Public fixture item {n}"),
            )
            .await;
        }
        let job:SourceJob=sqlx::query_as("SELECT j.source_id,s.meeting_id,j.revision,j.generation FROM knowledge_index_jobs j JOIN knowledge_sources s ON s.id=j.source_id WHERE s.meeting_id='one'").fetch_one(&pool).await.unwrap();
        let rows:Vec<CanonicalRow>=sqlx::query_as("SELECT id,meeting_id,transcript,speaker,timestamp,audio_start_time,audio_end_time,duration,word_timestamps_json FROM transcripts WHERE meeting_id='one' ORDER BY id").fetch_all(&pool).await.unwrap();
        let spans: Vec<_> = rows
            .iter()
            .map(|row| TextSpan {
                transcript_id: row.id.clone(),
                start_byte: 0,
                end_byte: row.transcript.len(),
            })
            .collect();
        let ids: Vec<_> = rows
            .iter()
            .zip(&spans)
            .map(|(row, span)| store::evidence(&job, row, span).unwrap().chunk_id)
            .collect();
        let last = ids.iter().max().unwrap();
        for (n, ((row, span), id)) in rows.iter().zip(&spans).zip(&ids).enumerate() {
            let mut vector = vec![0.; 384];
            vector[usize::from(id != last)] = 1.;
            store::stage(&pool, &job, row, span, n as i64, &vector, "test-space")
                .await
                .unwrap();
        }
        store::publish(&pool, &job, "test-space").await.unwrap();
        let mut query = vec![0.; 384];
        query[0] = 1.;
        let candidates = semantic_candidates(&pool, "[\"one\"]", "test-space", &query)
            .await
            .unwrap();
        assert_eq!(candidates.len(), 64);
        assert_eq!(&candidates[0].evidence.chunk_id, last);
        let scope = freeze_scope(
            &pool,
            &KnowledgeScope::Meeting {
                meeting_id: "one".into(),
            },
        )
        .await
        .unwrap();
        let results = search_channels(
            &pool,
            &scope,
            "nonmatchingword",
            Some(("test-space", &query)),
        )
        .await
        .unwrap();
        assert_eq!(results.len(), 12);
        assert_eq!(&results[0].evidence.chunk_id, last);
    }
    #[tokio::test]
    async fn identical_dated_meetings_and_short_german_replies_remain_distinct() {
        let pool = fixture().await;
        add_text(&pool, "a", "one", "Ja. İ Äpfel bestätigen.").await;
        add_text(&pool, "b", "two", "Ja. İ Äpfel bestätigen.").await;
        let scope = freeze_scope(
            &pool,
            &KnowledgeScope::Library {
                filter: MeetingFilter {
                    all_meetings: true,
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
        let hits = search_channels(&pool, &scope, "Ja", None).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_ne!(hits[0].evidence.chunk_id, hits[1].evidence.chunk_id);
        assert_ne!(hits[0].date, hits[1].date);
    }
    #[tokio::test]
    async fn covering_chunk_receives_both_channel_ranks() {
        let pool = fixture().await;
        add_text(&pool, "a", "one", "ATLAS-42 ships Friday").await;
        let job:SourceJob=sqlx::query_as("SELECT j.source_id,s.meeting_id,j.revision,j.generation FROM knowledge_index_jobs j JOIN knowledge_sources s ON s.id=j.source_id WHERE s.meeting_id='one'").fetch_one(&pool).await.unwrap();
        let row = store::rows_page(&pool, &job, None).await.unwrap().remove(0);
        let span = TextSpan {
            transcript_id: row.id.clone(),
            start_byte: 0,
            end_byte: row.transcript.len(),
        };
        let vector = super::super::embedding::normalize(vec![1.; 384]).unwrap();
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
        assert_eq!(hits.len(), 1);
        assert!((hits[0].rank - 2. / 61.).abs() < 1e-12);
        assert_eq!(
            hits[0].evidence,
            store::evidence(&job, &row, &span).unwrap()
        );
    }
    #[tokio::test]
    #[ignore = "manual trusted-runner 10000-passage native retrieval acceptance"]
    async fn synthetic_retrieval_workload() {
        use std::{
            sync::{
                atomic::{AtomicBool, AtomicUsize, Ordering},
                Arc,
            },
            time::{Duration, Instant},
        };
        let _serial = crate::audio::inference::GLOBAL_JOB_TEST_LOCK.lock().await;
        let fixtures=[
            ("Sicherungskopien werden neunzig Tage lang im Rechenzentrum Dublin aufbewahrt.","How long are backup copies retained?"),
            ("Lieferantenrechnungen benötigen zwei unabhängige Freigaben durch Einkauf und Finanzabteilung.","Who must authorize payment of vendor invoices?"),
            ("Die Blutproben müssen im Labor bei minus achtzig Grad Celsius gelagert werden.","At what temperature should biological samples be stored?"),
            ("Auf dem Dach des Verwaltungsgebäudes installieren wir Solarmodule zur Stromerzeugung.","Where will the renewable electricity panels be mounted?"),
            ("Während der Bahnsperrung fahren Ersatzbusse zwischen Bahnhof und Messegelände.","How will visitors travel during the railway closure?"),
            ("Vergessene Passwörter können Kunden über einen zeitlich begrenzten Link per E-Mail zurücksetzen.","How does an account holder regain access after forgetting their secret?"),
            ("Für Beschäftigte werden überdachte Fahrradstellplätze neben dem Haupteingang gebaut.","What sheltered parking will be provided for staff who cycle?"),
            ("Bei Dienstreisen sind rollstuhlgerechte Hotelzimmer ohne Stufen verbindlich zu buchen.","What accessibility requirement applies to overnight business accommodation?"),
            ("Wegen dichten Nebels dürfen Flugzeuge erst nach Verbesserung der Sicht starten.","What weather condition is preventing aircraft departures?"),
            ("Die Kantine muss Milch, Eier und Nüsse in jedem Tagesgericht als Allergene kennzeichnen.","Which food sensitivities must the cafeteria disclose?"),
            ("Customer records must be encrypted before they leave the device, using keys held by the organization.","Wie werden Kundendaten vor dem Verlassen des Geräts geschützt?"),
            ("Fire evacuation drills are scheduled twice each year, and the assembly point is the north car park.","Wo sollen sich Mitarbeitende bei einer Brandübung versammeln?"),
            ("The museum will loan the bronze sculpture for six months, provided transport is insured.","Unter welcher Bedingung darf die Bronzeskulptur ausgeliehen werden?"),
            ("The orchard irrigation system opens at dawn and uses collected rainwater instead of drinking water.","Womit und wann werden die Obstbäume bewässert?"),
            ("The scholarship covers tuition fees but excludes accommodation and daily meals.","Welche Kosten übernimmt das Stipendium nicht?"),
            ("We postponed the launch until the independent penetration assessment is complete.","Which security review is blocking shipment of the product?"),
            ("The reception desk will lend reusable umbrellas to guests caught in bad weather.","What can visitors borrow when it starts raining?"),
            ("New hires receive a mentor from a different department during their first three months.","Who helps recently recruited employees settle into the organization?"),
            ("The cardiology clinic reserves urgent appointments for patients reporting chest pain.","Which symptoms qualify for priority assessment at the heart service?"),
            ("The auditorium will replace its old seats with adjustable chairs to accommodate different body sizes.","How is the venue improving audience seating comfort?"),
            ("Incident ATLAS-42 requires replacing the expired gateway certificate on Friday.","ATLAS-42"),
            ("Ticket BOREAL-731 assigns the database migration to the evening maintenance window.","BOREAL-731"),
            ("Der Fehler FALKE-908 wird durch ein Update des Druckertreibers behoben.","FALKE-908"),
            ("Purchase order PO-88217 includes laboratory glassware and protective gloves.","PO-88217"),
            ("Die Anlage mit Seriennummer XR-6619 benötigt einen neuen Temperatursensor.","XR-6619"),
            ("Apollo deploys its payments service in the Frankfurt region after the resilience review.","Where will Apollo host payment processing?"),
            ("Apollo deploys its payments service in the Stockholm region after the resilience review.","Where will Apollo host payment processing?"),
            ("The earlier decision used paper tickets for entry to the conference.","How are attendees admitted to the conference?"),
            ("The revised decision uses digital QR codes for entry to the conference.","How are attendees admitted to the conference?"),
            ("Unassigned research approved a reusable glass container for specimen transport.","Which packaging was approved for moving samples?")
        ];
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let seeded = Instant::now();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES('noise','Synthetic inventory','2026-01-01','2026-01-01')").execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO meeting_tags(meeting_id,tag) VALUES('noise','Inventory')")
            .execute(&mut *tx)
            .await
            .unwrap();
        for n in 0..9970 {
            let text = if n % 2 == 0 {
                format!("Warehouse inventory report {n}: aisle {}, shelf {} holds {} ordinary cardboard cartons. The stock clerk completed a routine count.",n%71,n%23,n%119)
            } else {
                format!("Lagerbestandsmeldung {n}: Gang {}, Regal {} enthält {} gewöhnliche Kartons. Die Bestandszählung wurde abgeschlossen.",n%71,n%23,n%119)
            };
            sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp) VALUES(?,'noise',?,'00:01')").bind(format!("noise-{n:05}")).bind(text).execute(&mut *tx).await.unwrap();
        }
        for (n, (text, _)) in fixtures.iter().enumerate() {
            let id = format!("target-{n:02}");
            let date = format!("2026-09-{:02}", n + 1);
            sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES(?, 'Public acceptance fixture',?,?)").bind(&id).bind(&date).bind(&date).execute(&mut *tx).await.unwrap();
            sqlx::query("INSERT INTO transcripts(id,meeting_id,transcript,timestamp,audio_start_time) VALUES(?,?,?,'00:01',1)").bind(&id).bind(&id).bind(text).execute(&mut *tx).await.unwrap();
            if n != 29 {
                sqlx::query("INSERT INTO meeting_tags(meeting_id,tag) VALUES(?,?)")
                    .bind(&id)
                    .bind(if n == 25 {
                        "Äpfel"
                    } else if n == 26 {
                        "Beta"
                    } else {
                        "Research"
                    })
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
        }
        tx.commit().await.unwrap();
        let bulk_ms = seeded.elapsed().as_millis();
        let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_index_jobs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(jobs, 31);
        println!("KNOWLEDGE_BULK rows=10000 sources=31 jobs={jobs} elapsed_ms={bulk_ms}");
        let root = std::path::PathBuf::from(
            std::env::var_os("CLAWSCRIBE_KNOWLEDGE_MODEL").expect("manual runner model directory"),
        );
        super::super::model::ModelDownloads::default()
            .download(&root, tokio_util::sync::CancellationToken::new())
            .await
            .unwrap();
        let verified = super::super::model::VerifiedModel::verify(&root)
            .await
            .unwrap();
        let runtime = super::super::KnowledgeState::default();
        let baseline = memory_stats::memory_stats().unwrap();
        let peak = Arc::new(AtomicUsize::new(baseline.virtual_mem));
        let running = Arc::new(AtomicBool::new(true));
        let monitor = {
            let peak = peak.clone();
            let running = running.clone();
            std::thread::spawn(move || {
                while running.load(Ordering::Acquire) {
                    if let Some(m) = memory_stats::memory_stats() {
                        peak.fetch_max(m.virtual_mem, Ordering::AcqRel);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
        };
        sqlx::query("UPDATE knowledge_settings SET enabled=1 WHERE singleton=1")
            .execute(&pool)
            .await
            .unwrap();
        runtime.scheduler.enable(verified).unwrap();
        let started = Instant::now();
        while let Some(job) = store::next_job(&pool).await.unwrap() {
            super::super::indexer::index_source(&pool, &runtime.scheduler, &job)
                .await
                .unwrap();
            assert!(
                started.elapsed() < Duration::from_secs(2400),
                "index workload exceeded bounded deadline"
            );
        }
        let index_ms = started.elapsed().as_millis();
        let chunks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_chunks")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(chunks, 10000);
        let unicode = "Äpfel e\u{301} 🙂 bestätigen die Änderung. ".repeat(150);
        let spans = runtime
            .scheduler
            .spans("unicode".into(), unicode.clone())
            .await
            .unwrap();
        assert!(spans.len() > 1);
        assert_eq!(spans[0].start_byte, 0);
        assert_eq!(spans.last().unwrap().end_byte, unicode.len());
        for span in spans {
            assert!(unicode.get(span.start_byte..span.end_byte).is_some());
        }
        let mut times = Vec::new();
        let mut hits = 0;
        for (n, (_, query)) in fixtures.iter().enumerate() {
            let filter = match n {
                25 => MeetingFilter {
                    tags: vec!["äpfel".into()],
                    ..Default::default()
                },
                26 => MeetingFilter {
                    tags: vec!["Beta".into()],
                    ..Default::default()
                },
                27 => MeetingFilter {
                    from: Some("2026-09-28".into()),
                    to: Some("2026-09-28".into()),
                    ..Default::default()
                },
                28 => MeetingFilter {
                    meeting_ids: vec!["target-28".into()],
                    ..Default::default()
                },
                29 => MeetingFilter {
                    untagged: true,
                    ..Default::default()
                },
                _ => MeetingFilter {
                    all_meetings: true,
                    ..Default::default()
                },
            };
            let request = SearchRequest {
                scope: KnowledgeScope::Library { filter },
                query: (*query).into(),
                document_ids: Vec::new(),
                mode: SearchMode::Hybrid,
            };
            let started = Instant::now();
            let result = retrieve(&pool, &runtime, &request).await.unwrap();
            times.push(started.elapsed().as_millis());
            assert_eq!(result.mode, SearchMode::Hybrid);
            assert!(result.passages.len() <= 12);
            let found = result
                .passages
                .iter()
                .take(5)
                .any(|p| p.meeting_id == format!("target-{n:02}"));
            hits += usize::from(found);
            if !found {
                let frozen = freeze_scope(&pool, &request.scope).await.unwrap();
                let allowed = serde_json::to_string(&frozen.meeting_ids).unwrap();
                let vector = runtime
                    .scheduler
                    .embed(request.query.clone(), EmbeddingPurpose::Query)
                    .await
                    .unwrap();
                let semantic = semantic_candidates(
                    &pool,
                    &allowed,
                    &super::super::model::PINS.space().id,
                    &vector,
                )
                .await
                .unwrap();
                let lexical = search_channels(&pool, &frozen, &request.query, None)
                    .await
                    .unwrap();
                let expected = format!("target-{n:02}");
                println!("KNOWLEDGE_MISS case={n} semantic_expected_rank={:?} semantic_top10={:?} lexical_top12={:?} fused_top12={:?}",semantic.iter().position(|p|p.meeting_id==expected).map(|rank|rank+1),semantic.iter().take(10).map(|p|p.meeting_id.as_str()).collect::<Vec<_>>(),lexical.iter().map(|p|p.meeting_id.as_str()).collect::<Vec<_>>(),result.passages.iter().map(|p|p.meeting_id.as_str()).collect::<Vec<_>>());
            }
            println!(
                "KNOWLEDGE_QUERY case={n} expected_top5={found} elapsed_ms={}",
                times.last().unwrap()
            );
        }
        running.store(false, Ordering::Release);
        monitor.join().unwrap();
        let private_delta = peak
            .load(Ordering::Acquire)
            .saturating_sub(baseline.virtual_mem);
        times.sort_unstable();
        let p95 = times[28];
        println!("KNOWLEDGE_RETRIEVAL passages={chunks} queries=30 expected_top5={hits} warm_p95_ms={p95} indexing_ms={index_ms} peak_private_delta_bytes={private_delta} engine=onnx backend=cpu threads=2 batch=1");
        runtime.scheduler.disable().await;
        assert!(hits >= 27, "retrieval recall gate failed");
        assert!(p95 <= 2000, "warm retrieval p95 gate failed");
    }
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
