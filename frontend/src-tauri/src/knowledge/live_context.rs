//! Bounded lexical live evidence and explicitly selected canonical saved references.
use super::{live::LiveSnapshot, retrieval, types::*};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

pub(crate) fn prefix(value: &str, limit: usize) -> &str {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
pub(crate) fn passage(
    snapshot: &LiveSnapshot,
    segment: &super::live::LiveEvidenceSegment,
) -> Passage {
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                "live-finalized-v1",
                &snapshot.session_id,
                segment.sequence_id,
                &segment.text,
                segment.start_seconds,
                segment.end_seconds
            ))
            .unwrap()
        )
    );
    Passage {
        evidence: EvidenceRef {
            historical: false,
            source_id: format!("live:{}", snapshot.session_id),
            source_revision: 1,
            chunk_id: format!("live:{}:{}", snapshot.session_id, segment.sequence_id),
            fingerprint,
            locator: EvidenceLocator::Live {
                session_id: snapshot.session_id.clone(),
                sequence_ids: vec![segment.sequence_id],
            },
        },
        meeting_id: String::new(),
        title: "Current recording".into(),
        date: String::new(),
        speaker: None,
        metadata_truncated: false,
        text: segment.text.clone(),
        rank: 0.,
    }
}
pub(crate) fn lexical(snapshot: &LiveSnapshot, query: &str, limit: usize) -> Vec<Passage> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|s| !s.is_empty())
        .take(64)
        .map(str::to_lowercase)
        .collect();
    let mut ranked: Vec<_> = snapshot
        .segments
        .iter()
        .map(|segment| {
            let text = segment.text.to_lowercase();
            let score = terms
                .iter()
                .filter(|term| text.contains(term.as_str()))
                .count();
            (score, segment.sequence_id, segment)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    ranked.truncate(retrieval::CANDIDATES);
    let mut selected: Vec<&super::live::LiveEvidenceSegment> = Vec::new();
    let limit = limit.min(retrieval::RESULT_LIMIT);
    // Admit short replies together with their preceding question rather than
    // evicting a selected reply while trying to add its interpretation context.
    for (_, _, reply) in ranked {
        let mut group = vec![reply];
        if reply.text.len() <= 64 && !reply.text.trim_end().ends_with('?') {
            if let Some(previous) = snapshot
                .segments
                .iter()
                .rev()
                .find(|s| s.sequence_id < reply.sequence_id)
                .filter(|s| s.text.trim_end().ends_with('?'))
            {
                group.push(previous);
            }
        }
        group.retain(|segment| {
            !selected
                .iter()
                .any(|s| s.sequence_id == segment.sequence_id)
        });
        if selected.len() + group.len() <= limit {
            selected.extend(group);
        }
        if selected.len() == limit {
            break;
        }
    }
    selected.sort_by_key(|s| s.sequence_id);
    selected.into_iter().map(|s| passage(snapshot, s)).collect()
}

fn preceding_question(snapshot: &LiveSnapshot, row: &Passage) -> Option<EvidenceRef> {
    let EvidenceLocator::Live {
        session_id,
        sequence_ids,
    } = &row.evidence.locator
    else {
        return None;
    };
    if session_id != &snapshot.session_id
        || sequence_ids.len() != 1
        || row.text.len() > 64
        || row.text.trim_end().ends_with('?')
    {
        return None;
    }
    snapshot
        .segments
        .iter()
        .rev()
        .find(|segment| segment.sequence_id < sequence_ids[0])
        .filter(|segment| segment.text.trim_end().ends_with('?'))
        .map(|segment| passage(snapshot, segment).evidence)
}

/// Keep positional short-reply context atomic through the final byte budget.
/// Reuse saved-answer serialization without changing its selection behavior.
pub(crate) fn build_prompt(
    snapshot: &LiveSnapshot,
    question: &str,
    passages: &[Passage],
    budget: usize,
) -> Result<(String, Vec<Passage>, Vec<Option<usize>>), String> {
    let sources: Vec<_> = passages
        .iter()
        .map(|row| {
            preceding_question(snapshot, row).and_then(|reference| {
                passages
                    .iter()
                    .position(|source| source.evidence == reference)
            })
        })
        .collect();
    let (mut prompt, mut selected) = super::answers::build_prompt(question, &[], "", budget)?;
    let mut contexts = vec![];
    let mut handled = vec![false; passages.len()];
    for index in 0..passages.len() {
        if handled[index] {
            continue;
        }
        let group = if let Some(reply) = sources.iter().position(|source| *source == Some(index)) {
            vec![index, reply]
        } else if let Some(source) = sources[index] {
            vec![source, index]
        } else {
            vec![index]
        };
        let mut candidate = selected.clone();
        for member in group {
            handled[member] = true;
            if !candidate
                .iter()
                .any(|row| row.evidence == passages[member].evidence)
            {
                candidate.push(passages[member].clone());
            }
        }
        let (candidate_prompt, accepted) =
            super::answers::build_prompt(question, &candidate, "", budget)?;
        if accepted.len() != candidate.len() {
            continue;
        }
        let candidate_contexts: Vec<_> = accepted
            .iter()
            .map(|row| {
                preceding_question(snapshot, row)
                    .and_then(|reference| {
                        accepted
                            .iter()
                            .position(|source| source.evidence == reference)
                    })
                    .map(|index| index + 1)
            })
            .collect();
        let mut envelope: serde_json::Value =
            serde_json::from_str(&candidate_prompt).map_err(|_| "Invalid live evidence prompt")?;
        for row in envelope["transcript_evidence"]
            .as_array_mut()
            .into_iter()
            .flatten()
        {
            let index = row["tag"]
                .as_str()
                .and_then(|tag| tag.strip_prefix("[K"))
                .and_then(|tag| tag.strip_suffix(']'))
                .and_then(|tag| tag.parse::<usize>().ok())
                .and_then(|tag| tag.checked_sub(1));
            if let Some(context) = index.and_then(|index| candidate_contexts.get(index)) {
                row["preceding_question_tag"] = serde_json::json!(context);
            }
        }
        let candidate_prompt = envelope.to_string();
        if candidate_prompt.len() > budget {
            continue;
        }
        prompt = candidate_prompt;
        selected = accepted;
        contexts = candidate_contexts;
    }
    Ok((prompt, selected, contexts))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_live_retrieval_keeps_a_short_reply_with_its_source_question() {
        let snapshot = LiveSnapshot {
            session_id: uuid::Uuid::new_v4().to_string(),
            finalized_through_seconds: 30.,
            transcription_incomplete: false,
            transcription_available: true,
            segments: vec![
                super::super::live::LiveEvidenceSegment {
                    sequence_id: 1,
                    text: "Is the supplier approved?".into(),
                    start_seconds: Some(0.),
                    end_seconds: Some(10.),
                },
                super::super::live::LiveEvidenceSegment {
                    sequence_id: 2,
                    text: "Ja".into(),
                    start_seconds: Some(10.),
                    end_seconds: Some(20.),
                },
                super::super::live::LiveEvidenceSegment {
                    sequence_id: 3,
                    text: "A later unrelated agenda item.".into(),
                    start_seconds: Some(20.),
                    end_seconds: Some(30.),
                },
            ],
        };
        let passages = lexical(&snapshot, "Ja", 2);
        assert_eq!(
            passages.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
            vec!["Is the supplier approved?", "Ja"]
        );
        assert!(
            lexical(&snapshot, "Ja", 1).iter().all(|p| p.text != "Ja"),
            "Do not present a contextless short answer when its pair exceeds the limit"
        );
    }
}
pub(crate) async fn saved(
    pool: &SqlitePool,
    request: &AskRequest,
) -> Result<(Option<retrieval::FrozenScope>, Vec<Passage>), String> {
    let Some(filter) = &request.live_reference_scope else {
        if !request.search.document_ids.is_empty() {
            return Err("Select saved meetings that own the reference documents".into());
        }
        return Ok((None, vec![]));
    };
    if filter.all_meetings || filter.meeting_ids.is_empty() || filter.meeting_ids.len() > 64 {
        return Err("Live references require explicitly selected saved meetings".into());
    }
    let search = SearchRequest {
        scope: KnowledgeScope::Library {
            filter: filter.clone(),
        },
        query: request.search.query.clone(),
        document_ids: request.search.document_ids.clone(),
        mode: SearchMode::Keyword,
    };
    let frozen = retrieval::freeze_search_in_connection(
        &mut *pool
            .acquire()
            .await
            .map_err(|_| "Reference storage unavailable")?,
        &search,
    )
    .await
    .map_err(|e| e.to_string())?;
    let mut specified = filter.meeting_ids.clone();
    specified.sort();
    specified.dedup();
    if specified.len() != filter.meeting_ids.len() || specified != frozen.meeting_ids {
        return Err("Selected saved references changed or are unavailable".into());
    }
    let passages = retrieval::search_channels(pool, &frozen, &search.query, None)
        .await
        .map_err(|e| e.to_string())?;
    retrieval::recheck_scope(pool, &frozen)
        .await
        .map_err(|e| e.to_string())?;
    Ok((Some(frozen), passages))
}
pub(crate) async fn recheck(
    pool: &SqlitePool,
    scope: Option<&retrieval::FrozenScope>,
    passages: &[Passage],
) -> Result<(), String> {
    if let Some(scope) = scope {
        retrieval::recheck_scope(pool, scope)
            .await
            .map_err(|e| e.to_string())?;
        for passage in passages
            .iter()
            .filter(|p| !matches!(p.evidence.locator, EvidenceLocator::Live { .. }))
        {
            let resolved = super::evidence::resolve(pool, &passage.evidence)
                .await
                .map_err(|e| e.to_string())?;
            if resolved.status != super::evidence::EvidenceStatus::Current {
                return Err("Selected saved evidence changed during the request".into());
            }
        }
    }
    Ok(())
}
