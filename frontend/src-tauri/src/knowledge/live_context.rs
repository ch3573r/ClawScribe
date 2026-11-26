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
        provider_title: Some("Current recording".into()),
        date: String::new(),
        speaker: None,
        metadata_truncated: false,
        text: segment.text.clone(),
        rank: 0.,
    }
}
/// What a live question asks of the rolling transcript window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveIntent {
    /// A specific question: rank finalized segments by relevance.
    Targeted,
    /// A recap of the window: use every finalized segment that fits.
    Summary,
    /// Open questions across the window: every segment that fits.
    OpenQuestions,
}

impl LiveIntent {
    pub(crate) fn uses_window(self) -> bool {
        !matches!(self, Self::Targeted)
    }
}

pub(crate) fn intent(query: &str) -> LiveIntent {
    let query = query.to_lowercase();
    const OPEN: &[&str] = &[
        "open question",
        "unresolved",
        "unanswered",
        "remains open",
        "still open",
        "offene frage",
        "offenen frage",
        "noch offen",
        "was ist offen",
        "ungeklärt",
        "unbeantwortet",
    ];
    // A recap needs both a summarising verb and a whole-window scope, so a
    // targeted "summarize what Peter said about the budget" stays targeted.
    const SUMMARY_VERB: &[&str] = &["summar", "recap", "zusammenfass", "überblick"];
    const WINDOW_SCOPE: &[&str] = &[
        "last 10 minutes",
        "last ten minutes",
        "so far",
        "the conversation",
        "the meeting",
        "the discussion",
        "everything",
        "letzten 10 minuten",
        "letzten zehn minuten",
        "bisher",
        "das gespräch",
        "das meeting",
        "die besprechung",
        "alles",
    ];
    let summarises = SUMMARY_VERB.iter().any(|verb| query.contains(verb))
        || (query.contains("fasse ") && query.contains("zusammen"));
    let whole_window = WINDOW_SCOPE.iter().any(|scope| query.contains(scope));
    if OPEN.iter().any(|needle| query.contains(needle)) {
        LiveIntent::OpenQuestions
    } else if summarises && whole_window {
        LiveIntent::Summary
    } else {
        LiveIntent::Targeted
    }
}

/// Common English and German function words. They match nearly every segment
/// and would otherwise drown out the words that carry the question.
const STOPWORDS: &[&str] = &[
    "a", "about", "after", "all", "also", "am", "an", "and", "any", "are", "as", "at", "be",
    "been", "but", "by", "can", "could", "did", "do", "does", "for", "from", "had", "has", "have",
    "he", "her", "his", "how", "i", "if", "in", "into", "is", "it", "its", "me", "my", "of", "on",
    "or", "our", "she", "so", "than", "that", "the", "their", "them", "then", "there", "these",
    "they", "this", "to", "us", "was", "we", "were", "what", "when", "where", "which", "who",
    "why", "will", "with", "would", "you", "your", "aber", "als", "auch", "auf", "aus", "bei",
    "bin", "bis", "das", "dass", "dem", "den", "der", "des", "die", "dies", "diese", "du", "ein",
    "eine", "einen", "einer", "es", "für", "hat", "ich", "ihr", "im", "ist", "mit", "nach",
    "nicht", "noch", "oder", "sich", "sie", "sind", "über", "um", "und", "uns", "von", "vor",
    "war", "wer", "wie", "wir", "wird", "zu", "zum", "zur",
];

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
}

fn query_terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = words(query)
        .filter(|term| !STOPWORDS.contains(&term.as_str()))
        .take(64)
        .collect();
    terms.sort();
    terms.dedup();
    terms
}

/// Whole-word matches count double; a prefix match (for inflections such as
/// "supplier"/"suppliers" or "Lieferant"/"Lieferanten") counts once.
fn relevance(terms: &[String], text: &str) -> usize {
    let segment: Vec<String> = words(text).collect();
    terms
        .iter()
        .map(|term| {
            if segment.iter().any(|word| word == term) {
                2
            } else if term.chars().count() >= 4
                && segment.iter().any(|word| word.starts_with(term.as_str()))
            {
                1
            } else {
                0
            }
        })
        .sum::<usize>()
}

/// Every finalized segment of the rolling window in chronological order,
/// capped so saved references still fit the evidence entry limit.
pub(crate) fn window(snapshot: &LiveSnapshot) -> Vec<Passage> {
    const LIVE_WINDOW_LIMIT: usize = 1000;
    let skip = snapshot.segments.len().saturating_sub(LIVE_WINDOW_LIMIT);
    snapshot
        .segments
        .iter()
        .skip(skip)
        .map(|segment| passage(snapshot, segment))
        .collect()
}

/// Bytes reserved per row for the `preceding_question_tag` field that
/// `build_prompt` may add (`,"preceding_question_tag":1024`).
const CONTEXT_FIELD_RESERVE: usize = 32;

/// Linear check that `rows` fit `budget` in one prompt, with room for the
/// per-row context field.
pub(crate) fn fits(question: &str, rows: &[Passage], budget: usize) -> Result<bool, String> {
    let Some(reserved) = budget.checked_sub(CONTEXT_FIELD_RESERVE * rows.len()) else {
        return Ok(false);
    };
    match super::answers::build_prompt(question, rows, "", reserved) {
        Ok((_, accepted)) => Ok(accepted.len() == rows.len()),
        Err(error) if error == super::answers::PROMPT_BUDGET_EXCEEDED => Ok(false),
        Err(error) => Err(error),
    }
}

/// Keep the longest contiguous run of the newest window segments that fits
/// `budget`, in chronological order. A recap of "the last 10 minutes" loses
/// the oldest speech first and never leaves gaps inside the kept run.
pub(crate) fn fit_newest(
    question: &str,
    window: &[Passage],
    budget: usize,
) -> Result<Vec<Passage>, String> {
    // Prompt size grows with the number of kept rows, so binary search the cut.
    let (mut low, mut high) = (0usize, window.len());
    while low < high {
        let mid = (low + high + 1) / 2;
        if fits(question, &window[window.len() - mid..], budget)? {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    Ok(window[window.len() - low..].to_vec())
}

pub(crate) fn lexical(snapshot: &LiveSnapshot, query: &str, limit: usize) -> Vec<Passage> {
    let terms = query_terms(query);
    let mut ranked: Vec<_> = snapshot
        .segments
        .iter()
        .map(|segment| {
            let score = relevance(&terms, &segment.text);
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

/// Serialize `rows` with their preceding-question context tags, or `None`
/// when the result does not fit `budget`.
fn assemble(
    snapshot: &LiveSnapshot,
    question: &str,
    rows: &[Passage],
    budget: usize,
) -> Result<Option<(String, Vec<Passage>, Vec<Option<usize>>)>, String> {
    let (prompt, accepted) = super::answers::build_prompt(question, rows, "", budget)?;
    if accepted.len() != rows.len() {
        return Ok(None);
    }
    let contexts: Vec<_> = accepted
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
        serde_json::from_str(&prompt).map_err(|_| "Invalid live evidence prompt")?;
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
        if let Some(context) = index.and_then(|index| contexts.get(index)) {
            row["preceding_question_tag"] = serde_json::json!(context);
        }
    }
    let prompt = envelope.to_string();
    Ok((prompt.len() <= budget).then_some((prompt, accepted, contexts)))
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
    // Fast path: everything fits at once (the window path guarantees this).
    // Avoids the quadratic incremental check below for large live windows.
    if fits(question, passages, budget)? {
        if let Some(result) = assemble(snapshot, question, passages, budget)? {
            return Ok(result);
        }
    }
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

    fn segment(sequence_id: u64, text: &str) -> super::super::live::LiveEvidenceSegment {
        super::super::live::LiveEvidenceSegment {
            sequence_id,
            text: text.into(),
            start_seconds: Some(sequence_id as f64 * 5.),
            end_seconds: Some(sequence_id as f64 * 5. + 5.),
        }
    }

    fn snapshot_of(segments: Vec<super::super::live::LiveEvidenceSegment>) -> LiveSnapshot {
        LiveSnapshot {
            session_id: uuid::Uuid::new_v4().to_string(),
            recording_generation: serde_json::from_str("\"1\"").unwrap(),
            finalized_through_seconds: 600.,
            transcription_incomplete: false,
            transcription_available: true,
            segments,
        }
    }

    #[test]
    fn quick_actions_and_german_phrasings_select_window_intents() {
        assert_eq!(
            intent("Summarize the last 10 minutes of finalized speech. Identify decisions and uncertainty, and acknowledge missing or delayed speech."),
            LiveIntent::Summary
        );
        assert_eq!(
            intent("List open questions from the finalized conversation. Do not invent answers, owners or commitments."),
            LiveIntent::OpenQuestions
        );
        assert_eq!(
            intent("Fasse die letzten 10 Minuten zusammen"),
            LiveIntent::Summary
        );
        assert_eq!(
            intent("Summarize what Peter said about the budget"),
            LiveIntent::Targeted
        );
        assert_eq!(
            intent("Was hat Peter bisher zum Budget gesagt?"),
            LiveIntent::Targeted
        );
        assert_eq!(
            intent("Wer soll den Bericht verfassen?"),
            LiveIntent::Targeted
        );
        assert_eq!(
            intent("Welche Fragen sind noch offen?"),
            LiveIntent::OpenQuestions
        );
        assert_eq!(
            intent("Who owns the supplier contract?"),
            LiveIntent::Targeted
        );
    }

    #[test]
    fn summary_window_includes_every_segment_when_the_budget_allows() {
        let segments: Vec<_> = (1..=120)
            .map(|id| {
                segment(
                    id,
                    &format!("Wir besprechen Punkt {id} der Agenda ausführlich."),
                )
            })
            .collect();
        let snapshot = snapshot_of(segments);
        let window = window(&snapshot);
        assert_eq!(window.len(), 120);
        let kept = fit_newest("Summarize the last 10 minutes", &window, 200_000).unwrap();
        assert_eq!(
            kept.len(),
            120,
            "a recap must not be capped at a dozen segments"
        );
        assert!(kept.first().unwrap().text.contains("Punkt 1 "));
        assert!(kept.last().unwrap().text.contains("Punkt 120 "));
    }

    #[test]
    fn recap_fits_the_newest_of_eight_hundred_segments_on_small_models() {
        let snapshot = snapshot_of(
            (1..=800)
                .map(|id| segment(id, "Discuss the timeline."))
                .collect(),
        );
        let rows = window(&snapshot);
        let question = "Summarize the last 10 minutes of finalized speech.";
        for budget in [2_500, 12_628] {
            let kept = fit_newest(question, &rows, budget).unwrap();
            assert!(!kept.is_empty() && kept.len() < rows.len());
            assert_eq!(
                kept.last().unwrap().evidence.chunk_id,
                rows.last().unwrap().evidence.chunk_id
            );
            assert!(fits(question, &kept, budget).unwrap());
            assert!(!fits(question, &rows[rows.len() - kept.len() - 1..], budget).unwrap());
        }
    }

    #[test]
    fn a_small_budget_drops_the_oldest_window_speech_first() {
        let segments: Vec<_> = (1..=80)
            .map(|id| {
                segment(
                    id,
                    &format!("Agenda item {id} was discussed in some detail today."),
                )
            })
            .collect();
        let snapshot = snapshot_of(segments);
        let window = window(&snapshot);
        let kept = fit_newest("Summarize", &window, 4_000).unwrap();
        assert!(!kept.is_empty() && kept.len() < 80);
        assert!(kept.last().unwrap().text.contains("item 80 "));
        let numbers: Vec<u32> = kept
            .iter()
            .map(|p| p.text.split_whitespace().nth(2).unwrap().parse().unwrap())
            .collect();
        let mut sorted = numbers.clone();
        sorted.sort();
        assert_eq!(numbers, sorted, "kept speech stays chronological");
        assert_eq!(*numbers.last().unwrap(), 80);
        assert_eq!(
            numbers.len() as u32,
            80 - numbers[0] + 1,
            "a contiguous newest run"
        );
    }

    #[test]
    fn uneven_segments_keep_a_contiguous_newest_run() {
        let mut segments = Vec::new();
        for id in 1..=40u64 {
            let text = if id == 38 {
                format!("Item {id} {}", "long detail ".repeat(60))
            } else {
                format!("Item {id} short.")
            };
            segments.push(segment(id, &text));
        }
        let snapshot = snapshot_of(segments);
        let window = window(&snapshot);
        let kept = fit_newest("Summarize", &window, 3_000).unwrap();
        let ids: Vec<u64> = kept
            .iter()
            .map(|p| {
                p.text
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .trim_end_matches('.')
                    .parse()
                    .unwrap()
            })
            .collect();
        assert_eq!(*ids.last().unwrap(), 40);
        for pair in ids.windows(2) {
            assert_eq!(pair[1], pair[0] + 1, "no gaps inside the kept run");
        }
        // The final prompt accepts exactly the fitted set.
        let (_, selected, _) = build_prompt(&snapshot, "Summarize", &kept, 3_000).unwrap();
        assert_eq!(selected.len(), kept.len());
    }

    #[test]
    fn stopwords_do_not_drive_lexical_ranking() {
        let snapshot = snapshot_of(vec![
            segment(1, "The supplier contract is approved by legal."),
            segment(2, "In the end of the day the weather in the city was nice."),
            segment(3, "Of the many things in the list, this one is the oldest."),
        ]);
        let passages = lexical(&snapshot, "What is the status of the supplier contract?", 1);
        assert_eq!(passages.len(), 1);
        assert!(passages[0].text.contains("supplier contract"));
    }

    #[test]
    fn word_boundaries_shape_ranking() {
        let snapshot = snapshot_of(vec![
            segment(1, "We still need a cat sitter for the office."),
            segment(2, "Category planning continues next week."),
            segment(3, "Who approves the budget?"),
            segment(4, "The budget review happened yesterday."),
        ]);
        let cat = lexical(&snapshot, "cat", 1);
        assert!(
            cat[0].text.contains("cat sitter"),
            "substring matches must not outrank words"
        );
    }
    #[test]
    fn bounded_live_retrieval_keeps_a_short_reply_with_its_source_question() {
        let snapshot = LiveSnapshot {
            session_id: uuid::Uuid::new_v4().to_string(),
            recording_generation: serde_json::from_str("\"1\"").unwrap(),
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
