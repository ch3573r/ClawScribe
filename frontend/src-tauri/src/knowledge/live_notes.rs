//! Explicitly selected recording notes. No requests or conversation owners are persisted here.
use super::{
    evidence::{self, EvidenceStatus},
    live::{LiveEvidenceSegment, LiveReplyContext, LiveSnapshot},
    types::{AssistantReply, EvidenceLocator, EvidenceRef, Passage},
    KnowledgeState,
};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, SqliteConnection, SqlitePool, Transaction};
use std::collections::BTreeSet;

#[path = "live_notes/portable.rs"]
mod portable;
pub(crate) use portable::is_portable_live_answer;

const SAVE_ERROR: &str = "Could not save this answer to meeting notes.";
const READ_ERROR: &str = "Could not load saved live answers.";
const SOURCE_CHANGED: &str = "Saved evidence used for this answer changed or was removed.";
const MAX_NOTES: i64 = 32;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Citation {
    tag: usize,
    locator: EvidenceRef,
    live_excerpt: Option<LiveEvidenceSegment>,
    attachment_meeting: Option<String>,
}

pub(super) fn citations(
    reply: &AssistantReply,
    snapshot: &LiveSnapshot,
    selected: &[Passage],
) -> Vec<Citation> {
    let mut tags: BTreeSet<_> = reply.cited_tags.iter().copied().collect();
    tags.extend(reply.context_links.iter().map(|link| link.context_tag));
    tags.into_iter()
        .filter_map(|tag| {
            let passage = selected.get(tag.checked_sub(1)?)?;
            let reference = &passage.evidence;
            let live_excerpt = match &reference.locator {
                EvidenceLocator::Live {
                    session_id,
                    sequence_ids,
                } => {
                    let [sequence] = sequence_ids.as_slice() else {
                        return None;
                    };
                    if session_id != &snapshot.session_id {
                        return None;
                    }
                    let segment = snapshot
                        .segments
                        .iter()
                        .find(|segment| segment.sequence_id == *sequence)?;
                    if super::live_context::passage(snapshot, segment).evidence != *reference {
                        return None;
                    }
                    Some(segment.clone())
                }
                _ => None,
            };
            let attachment_meeting = matches!(reference.locator, EvidenceLocator::Document { .. })
                .then(|| passage.meeting_id.clone())
                .filter(|id| !id.is_empty());
            Some(Citation {
                tag,
                locator: reference.clone(),
                live_excerpt,
                attachment_meeting,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct SelectedAnswer {
    pub(super) id: String,
    pub(super) content: String,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) context: LiveReplyContext,
    pub(super) citations: Vec<Citation>,
    pub(super) cited_tags: Vec<usize>,
    pub(super) question_tags: Vec<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct NoteCandidate {
    pub(super) answer: SelectedAnswer,
    pub(super) dependencies: Vec<SourceDependency>,
}

#[derive(Clone, Debug)]
pub(super) struct SourceDependency {
    reference: EvidenceRef,
    attachment_meeting: Option<String>,
    incarnation: (String, Option<String>),
}

impl SourceDependency {
    fn document_id(&self) -> Option<&str> {
        match &self.reference.locator {
            EvidenceLocator::Document { document_id, .. } => Some(document_id),
            _ => None,
        }
    }
}

async fn source_incarnation(
    conn: &mut SqliteConnection,
    reference: &EvidenceRef,
    attachment_meeting: Option<&str>,
) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
    let (meeting, document) = match &reference.locator {
        EvidenceLocator::Transcript { meeting_id, .. } => (Some(meeting_id.as_str()), None),
        EvidenceLocator::Document { document_id, .. } => {
            (attachment_meeting, Some(document_id.as_str()))
        }
        EvidenceLocator::Live { .. } => return Ok(None),
    };
    sqlx::query_as(
        "SELECT s.live_answer_incarnation, a.incarnation FROM knowledge_sources s
         LEFT JOIN saved_live_answer_attachment_instances a ON a.meeting_id = ?2 AND a.document_id = ?3
         WHERE s.id = ?1 AND s.revision = ?4 AND s.live_answer_incarnation <> '' AND
           ((?3 IS NULL AND s.kind = 'meeting' AND s.meeting_id = ?2)
            OR (?3 IS NOT NULL AND s.kind = 'document' AND a.incarnation IS NOT NULL))",
    )
    .bind(&reference.source_id)
    .bind(meeting)
    .bind(document)
    .bind(reference.source_revision)
    .fetch_optional(conn)
    .await
}

pub(super) async fn source_dependencies(
    pool: &SqlitePool,
    selected: &[Passage],
) -> Result<Vec<SourceDependency>, String> {
    if selected
        .iter()
        .all(|passage| matches!(passage.evidence.locator, EvidenceLocator::Live { .. }))
    {
        return Ok(Vec::new());
    }
    let mut conn = pool.acquire().await.map_err(|_| SOURCE_CHANGED)?;
    let mut dependencies = Vec::new();
    for passage in selected {
        let reference = &passage.evidence;
        if matches!(reference.locator, EvidenceLocator::Live { .. }) {
            continue;
        }
        let attachment_meeting = matches!(reference.locator, EvidenceLocator::Document { .. })
            .then_some(passage.meeting_id.as_str());
        let incarnation = source_incarnation(&mut conn, reference, attachment_meeting)
            .await
            .map_err(|_| SOURCE_CHANGED)?
            .ok_or(SOURCE_CHANGED)?;
        dependencies.push(SourceDependency {
            reference: reference.clone(),
            attachment_meeting: attachment_meeting.map(str::to_owned),
            incarnation,
        });
    }
    Ok(dependencies)
}

pub(super) async fn recheck_dependencies(
    conn: &mut SqliteConnection,
    dependencies: &[SourceDependency],
) -> Result<(), String> {
    for dependency in dependencies {
        let current = source_incarnation(
            conn,
            &dependency.reference,
            dependency.attachment_meeting.as_deref(),
        )
        .await
        .map_err(|_| SOURCE_CHANGED)?;
        if current.as_ref() != Some(&dependency.incarnation) {
            return Err(SOURCE_CHANGED.into());
        }
    }
    Ok(())
}

impl SelectedAnswer {
    pub(super) fn from_reply(reply: &AssistantReply, citations: Vec<Citation>) -> Option<Self> {
        let question_tags = reply
            .context_links
            .iter()
            .map(|link| link.context_tag)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Some(Self {
            id: reply.message_id.clone(),
            content: reply.content.clone(),
            provider: reply.provider.clone(),
            model: reply.model.clone(),
            context: reply.live_context.clone()?,
            citations,
            cited_tags: reply.cited_tags.clone(),
            question_tags,
        })
    }
}

#[derive(Serialize)]
pub(crate) struct AnswerNote {
    id: String,
    content: String,
    provider: String,
    model: String,
    context: LiveReplyContext,
    cited_tags: Vec<usize>,
    question_tags: Vec<usize>,
}

#[tauri::command]
pub(crate) async fn save_selected_live_answer(
    state: tauri::State<'_, AppState>,
    runtime: tauri::State<'_, KnowledgeState>,
    session_id: String,
    answer_id: String,
) -> Result<(), String> {
    // Reserve the same writer lane as recording save. If Stop wins before
    // selection, the native session checks reject it; if selection wins, Stop
    // attaches this exact recording's note in its existing meeting transaction.
    let mut tx = state
        .db_manager
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| SAVE_ERROR)?;
    let recording_id = crate::audio::recording_commands::calendar_recording_id(&session_id)?;
    let answer = runtime.live.select_note(&session_id, &answer_id)?;
    store(&mut tx, &recording_id, &answer).await?;
    tx.commit().await.map_err(|_| SAVE_ERROR.into())
}

pub(super) async fn store(
    tx: &mut Transaction<'_, Sqlite>,
    recording_id: &str,
    candidate: &NoteCandidate,
) -> Result<(), String> {
    let answer = &candidate.answer;
    let (count, exists): (i64, bool) = sqlx::query_as(
        "SELECT count(*), coalesce(max(id = ?2), 0) FROM saved_live_answers WHERE recording_id = ?1",
    )
    .bind(recording_id).bind(&answer.id)
    .fetch_one(&mut **tx).await.map_err(|_| SAVE_ERROR)?;
    if exists {
        return Ok(());
    }
    if count >= MAX_NOTES {
        return Err("This recording already has 32 saved live answers.".into());
    }
    // Dispatch verified the exact canonical passages. Check their revisions and
    // incarnations before inserting the answer in this writer transaction.
    recheck_dependencies(tx, &candidate.dependencies).await?;
    let json = serde_json::to_string(answer).map_err(|_| SAVE_ERROR)?;
    sqlx::query(
        "INSERT INTO saved_live_answers (id, recording_id, answer, source_dependencies_complete)
         VALUES (?, ?, ?, 1)",
    )
    .bind(&answer.id)
    .bind(recording_id)
    .bind(json)
    .execute(&mut **tx)
    .await
    .map_err(|_| SAVE_ERROR)?;
    for (index, dependency) in candidate.dependencies.iter().enumerate() {
        sqlx::query(
            "INSERT INTO saved_live_answer_dependencies
             (answer_id, ordinal, source_id, document_id, attachment_meeting_id, reference_json)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&answer.id)
        .bind(index as i64 + 1)
        .bind(&dependency.reference.source_id)
        .bind(dependency.document_id())
        .bind(&dependency.attachment_meeting)
        .bind(serde_json::to_string(&dependency.reference).map_err(|_| SAVE_ERROR)?)
        .execute(&mut **tx)
        .await
        .map_err(|_| SAVE_ERROR)?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn list_saved_live_answers(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<AnswerNote>, String> {
    list(state.db_manager.pool(), &meeting_id).await
}

async fn list(pool: &SqlitePool, meeting_id: &str) -> Result<Vec<AnswerNote>, String> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, answer FROM saved_live_answers
         WHERE meeting_id = ? ORDER BY saved_at, id LIMIT ?",
    )
    .bind(meeting_id)
    .bind(MAX_NOTES)
    .fetch_all(pool)
    .await
    .map_err(|_| READ_ERROR)?;
    rows.into_iter()
        .map(|(id, raw)| {
            let note: SelectedAnswer = serde_json::from_str(&raw).map_err(|_| READ_ERROR)?;
            Ok(AnswerNote {
                id,
                content: note.content,
                provider: note.provider,
                model: note.model,
                context: note.context,
                cited_tags: note.cited_tags,
                question_tags: note.question_tags,
            })
        })
        .collect()
}

#[derive(Serialize)]
pub(crate) struct CitationExcerpt {
    text: String,
    start_seconds: Option<f64>,
    historical: bool,
}

#[tauri::command]
pub(crate) async fn resolve_saved_live_answer_citation(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    answer_id: String,
    tag: usize,
) -> Result<Option<CitationExcerpt>, String> {
    resolve(state.db_manager.pool(), &meeting_id, &answer_id, tag).await
}

async fn resolve(
    pool: &SqlitePool,
    meeting_id: &str,
    answer_id: &str,
    tag: usize,
) -> Result<Option<CitationExcerpt>, String> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT answer FROM saved_live_answers WHERE id = ? AND meeting_id = ?")
            .bind(answer_id)
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| READ_ERROR)?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let answer: SelectedAnswer = serde_json::from_str(&raw).map_err(|_| READ_ERROR)?;
    let Some(citation) = answer
        .citations
        .into_iter()
        .find(|citation| citation.tag == tag)
    else {
        return Ok(None);
    };
    if let Some(segment) = citation.live_excerpt {
        return Ok(Some(CitationExcerpt {
            text: segment.text,
            start_seconds: segment.start_seconds,
            historical: true,
        }));
    }
    let resolved = evidence::resolve(pool, &citation.locator)
        .await
        .map_err(|_| READ_ERROR)?;
    if let EvidenceLocator::Document { document_id, .. } = &citation.locator.locator {
        let Some(owner) = citation.attachment_meeting else {
            return Ok(None);
        };
        let attached: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM knowledge_document_attachments WHERE document_id = ? AND meeting_id = ?)",
        )
        .bind(document_id).bind(owner).fetch_one(pool).await.map_err(|_| READ_ERROR)?;
        if !attached {
            return Ok(None);
        }
    }
    Ok(resolved
        .passage
        .filter(|_| resolved.status == EvidenceStatus::Current)
        .map(|passage| CitationExcerpt {
            text: passage.text,
            start_seconds: None,
            historical: false,
        }))
}

#[tauri::command]
pub(crate) async fn delete_saved_live_answer(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    answer_id: String,
) -> Result<(), String> {
    sqlx::query("DELETE FROM saved_live_answers WHERE id = ? AND meeting_id = ?")
        .bind(answer_id)
        .bind(meeting_id)
        .execute(state.db_manager.pool())
        .await
        .map_err(|_| "Could not remove the saved answer.")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        audio::{outcome::RecordingOutcome, provenance::RecordingProvenance},
        database::repositories::transcript::TranscriptsRepository,
    };

    #[tokio::test]
    async fn a_selected_answer_follows_only_the_recording_handoff_with_its_original_citation() {
        fn require_send<T: Send>(future: T) -> T {
            future
        }

        let pool = crate::database::transcript_edits::tests::fixture().await;
        let snapshot = LiveSnapshot {
            session_id: uuid::Uuid::new_v4().to_string(),
            recording_generation: serde_json::from_str("\"1\"").unwrap(),
            finalized_through_seconds: 31.,
            segments: vec![LiveEvidenceSegment {
                sequence_id: 19,
                text: "Keep the staging plan.".into(),
                start_seconds: Some(28.),
                end_seconds: Some(31.),
            }],
            transcription_incomplete: true,
            transcription_available: true,
        };
        let passage = super::super::live_context::passage(&snapshot, &snapshot.segments[0]);
        let dependencies = require_send(source_dependencies(&pool, std::slice::from_ref(&passage)))
            .await
            .unwrap();
        assert!(dependencies.is_empty());
        let note = SelectedAnswer {
            id: uuid::Uuid::new_v4().to_string(),
            content: "The staging plan stays [K1]".into(),
            provider: "openai".into(),
            model: "synthetic".into(),
            context: LiveReplyContext {
                session_id: snapshot.session_id.clone(),
                finalized_through_seconds: 31.,
                transcription_incomplete: true,
            },
            citations: vec![Citation {
                tag: 1,
                locator: passage.evidence,
                live_excerpt: Some(snapshot.segments[0].clone()),
                attachment_meeting: None,
            }],
            cited_tags: vec![1],
            question_tags: vec![],
        };
        let id = format!("meeting-{}", uuid::Uuid::new_v4());
        let candidate = NoteCandidate {
            answer: note.clone(),
            dependencies,
        };
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        store(&mut tx, &id, &candidate).await.unwrap();
        store(&mut tx, &id, &candidate).await.unwrap();
        tx.commit().await.unwrap();
        assert!(list(&pool, &id).await.unwrap().is_empty());
        assert!(resolve(&pool, "review-test", &note.id, 1)
            .await
            .unwrap()
            .is_none());
        TranscriptsRepository::save_recording_handoff(
            &pool,
            &id,
            "Synthetic recording",
            &[],
            "synthetic-recording".into(),
            &RecordingOutcome::default(),
            RecordingProvenance::default(),
        )
        .await
        .unwrap();
        let notes = list(&pool, &id).await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].context.session_id, snapshot.session_id);
        assert_eq!(notes[0].context.finalized_through_seconds, 31.);
        let source = resolve(&pool, &id, &note.id, 1).await.unwrap().unwrap();
        assert_eq!(source.text, "Keep the staging plan.");
        assert_eq!(source.start_seconds, Some(28.));
        assert!(source.historical);
        assert!(resolve(&pool, &id, &note.id, 2).await.unwrap().is_none());
        assert!(resolve(&pool, "review-test", &note.id, 1)
            .await
            .unwrap()
            .is_none());
        sqlx::query("DELETE FROM meetings WHERE id = ?")
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM saved_live_answers")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
