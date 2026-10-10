//! Recording-scoped, ephemeral assistance; saved conversations are independent.
use super::types::*;
use crate::audio::transcription::TranscriptUpdate;
use crate::summary::llm_client::ConfiguredTextReply;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LiveEvidenceSegment {
    pub sequence_id: u64,
    pub text: String,
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LiveSnapshot {
    pub session_id: String,
    pub finalized_through_seconds: f64,
    pub segments: Vec<LiveEvidenceSegment>,
    pub transcription_incomplete: bool,
    pub transcription_available: bool,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LiveReplyContext {
    pub session_id: String,
    pub finalized_through_seconds: f64,
    pub transcription_incomplete: bool,
}
#[derive(Default, Debug)]
pub struct LiveState {
    inner: Mutex<Inner>,
    consent_io: tokio::sync::Mutex<()>,
}
#[derive(Default, Debug)]
struct Inner {
    session: Option<Session>,
    sharing: bool,
    consent_revision: u64,
    consent_pending: bool,
    consent_failed: bool,
    early_cancellations: VecDeque<String>,
}
#[derive(Debug)]
struct Session {
    id: String,
    transcribes: bool,
    finalized_through: f64,
    segments: BTreeMap<u64, LiveEvidenceSegment>,
    bytes: usize,
    incomplete: bool,
    document_sharing: bool,
    request: Option<(String, CancellationToken)>,
    history: VecDeque<HistoryMessage>,
}
const CONTEXT_SECONDS: f64 = 600.;
const MAX_SEGMENTS: usize = 1024;
const MAX_CONTEXT_BYTES: usize = 256 * 1024;
const MAX_HISTORY_BYTES: usize = 64 * 1024;
#[derive(Debug)]
pub struct LiveLease {
    pub session_id: String,
    pub request_id: String,
    pub token: CancellationToken,
    pub deadline: tokio::time::Instant,
    provider: String,
    state: Arc<LiveState>,
    pub(crate) snapshot: Option<LiveSnapshot>,
}
pub struct LiveStartup {
    pub session_id: String,
    state: Arc<LiveState>,
    committed: bool,
}
impl LiveStartup {
    pub fn commit(mut self) -> String {
        self.committed = true;
        self.session_id.clone()
    }
}
impl Drop for LiveStartup {
    fn drop(&mut self) {
        if !self.committed {
            self.state.stop_session(&self.session_id);
        }
    }
}
impl Drop for LiveLease {
    fn drop(&mut self) {
        self.token.cancel();
        if let Ok(mut inner) = self.state.inner.lock() {
            if let Some(session) = inner.session.as_mut().filter(|s| s.id == self.session_id) {
                if session
                    .request
                    .as_ref()
                    .is_some_and(|(id, _)| id == &self.request_id)
                {
                    session.request = None;
                }
            }
        }
    }
}
impl LiveState {
    pub fn prepare(self: &Arc<Self>, transcribes: bool) -> LiveStartup {
        LiveStartup {
            session_id: self.start(transcribes),
            state: self.clone(),
            committed: false,
        }
    }
    pub fn start(&self, transcribes: bool) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let mut inner = self.inner.lock().unwrap();
        inner.early_cancellations.clear();
        if let Some(session) = inner.session.take() {
            if let Some((_, token)) = session.request {
                token.cancel();
            }
        }
        inner.session = Some(Session {
            id: id.clone(),
            transcribes,
            finalized_through: 0.,
            segments: BTreeMap::new(),
            bytes: 0,
            incomplete: false,
            document_sharing: false,
            request: None,
            history: VecDeque::new(),
        });
        id
    }
    pub fn stop(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.early_cancellations.clear();
            if let Some(session) = inner.session.take() {
                if let Some((_, token)) = session.request {
                    token.cancel();
                }
            }
        }
    }
    pub(crate) fn stop_session(&self, id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner.session.as_ref().is_some_and(|s| s.id == id) {
                inner.early_cancellations.clear();
                if let Some(session) = inner.session.take() {
                    if let Some((_, token)) = session.request {
                        token.cancel();
                    }
                }
            }
        }
    }
    pub fn ingest(&self, session_id: &str, update: &TranscriptUpdate) {
        if update.is_partial || update.text.trim().is_empty() {
            return;
        }
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(session) = inner
            .session
            .as_mut()
            .filter(|s| s.id == session_id && s.transcribes)
        else {
            return;
        };
        if session.segments.contains_key(&update.sequence_id) {
            return;
        }
        if !update.audio_start_time.is_finite()
            || !update.audio_end_time.is_finite()
            || update.audio_start_time < 0.
            || update.audio_end_time < update.audio_start_time
        {
            session.incomplete = true;
            return;
        }
        session.finalized_through = session.finalized_through.max(update.audio_end_time);
        let cutoff = session.finalized_through - CONTEXT_SECONDS;
        if update.audio_end_time < cutoff {
            return;
        }
        let text = super::live_context::prefix(&update.text, 2048).to_owned();
        session.incomplete |= text.len() < update.text.len();
        session.bytes += text.len();
        session.segments.insert(
            update.sequence_id,
            LiveEvidenceSegment {
                sequence_id: update.sequence_id,
                text,
                start_seconds: Some(update.audio_start_time),
                end_seconds: Some(update.audio_end_time),
            },
        );
        session.segments.retain(|_, s| {
            let keep = s.end_seconds.is_some_and(|end| end >= cutoff);
            if !keep {
                session.bytes -= s.text.len();
            }
            keep
        });
        while session.segments.len() > MAX_SEGMENTS || session.bytes > MAX_CONTEXT_BYTES {
            if let Some((_, segment)) = session.segments.pop_first() {
                session.bytes -= segment.text.len();
                session.incomplete = true;
            }
        }
    }
    pub fn snapshot(&self, id: &str) -> Result<LiveSnapshot, String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "Live recording state unavailable")?;
        let session = inner
            .session
            .as_ref()
            .filter(|s| s.id == id)
            .ok_or("This recording session has ended")?;
        Ok(LiveSnapshot {
            session_id: session.id.clone(),
            finalized_through_seconds: session.finalized_through,
            segments: session.segments.values().cloned().collect(),
            transcription_incomplete: session.incomplete,
            transcription_available: session.transcribes,
        })
    }
    pub fn set_sharing(&self, enabled: bool) {
        let mut inner = self.inner.lock().unwrap();
        inner.consent_revision = inner.consent_revision.wrapping_add(1);
        inner.sharing = enabled;
        if !enabled {
            if let Some((_, token)) = inner.session.as_ref().and_then(|s| s.request.as_ref()) {
                token.cancel();
            }
        }
    }
    pub fn current_session_id(&self) -> Option<String> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.session.as_ref().map(|session| session.id.clone()))
    }
    pub fn sharing(&self) -> bool {
        self.inner
            .lock()
            .map(|s| s.sharing && !s.consent_pending && !s.consent_failed)
            .unwrap_or(false)
    }
    pub fn set_document_sharing(&self, id: &str, enabled: bool) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Live recording state unavailable")?;
        let session = inner
            .session
            .as_mut()
            .filter(|s| s.id == id)
            .ok_or("This recording session has ended")?;
        session.document_sharing = enabled;
        if !enabled {
            if let Some((_, token)) = &session.request {
                token.cancel();
            }
        }
        Ok(())
    }
    pub fn document_sharing(&self, id: &str) -> Result<bool, String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "Live recording state unavailable")?;
        Ok(inner
            .session
            .as_ref()
            .filter(|s| s.id == id)
            .ok_or("This recording session has ended")?
            .document_sharing)
    }
    pub fn claim(
        self: &Arc<Self>,
        id: &str,
        request: &str,
        provider: &str,
    ) -> Result<LiveLease, String> {
        if uuid::Uuid::parse_str(request)
            .map(|id| id.to_string())
            .ok()
            .as_deref()
            != Some(request)
        {
            return Err("Invalid live request identity".into());
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Live recording state unavailable")?;
        if inner.early_cancellations.iter().any(|key| key == request) {
            return Err("Live request cancelled".into());
        }
        let sharing = inner.sharing && !inner.consent_pending && !inner.consent_failed;
        let session = inner
            .session
            .as_mut()
            .filter(|s| s.id == id)
            .ok_or("This recording session has ended")?;
        if provider == "builtin-ai" {
            return Err("Built-in AI is unavailable during capture. Ask after recording.".into());
        }
        if !session.transcribes {
            return Err(
                "Audio-only recording has no live transcript. Transcribe after recording.".into(),
            );
        }
        if !sharing {
            return Err("Enable live text sharing before asking the configured provider".into());
        }
        if session.request.is_some() {
            return Err("One live request is already in flight".into());
        }
        if session
            .history
            .iter()
            .any(|row| row.request_id.as_deref() == Some(request))
        {
            return Err("This live request has already completed".into());
        }
        let token = CancellationToken::new();
        session.request = Some((request.to_owned(), token.clone()));
        Ok(LiveLease {
            session_id: id.into(),
            request_id: request.into(),
            token,
            deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            provider: provider.into(),
            state: self.clone(),
            snapshot: None,
        })
    }
    pub fn validate(&self, lease: &LiveLease, documents: bool) -> Result<(), String> {
        if lease.token.is_cancelled() {
            return Err("Live request cancelled".into());
        }
        if tokio::time::Instant::now() >= lease.deadline {
            return Err("Live request deadline expired".into());
        }
        let inner = self
            .inner
            .lock()
            .map_err(|_| "Live recording state unavailable")?;
        validate_in(&inner, lease, documents)
    }
    pub fn cancel(&self, id: &str) -> bool {
        if uuid::Uuid::parse_str(id)
            .map(|id| id.to_string())
            .ok()
            .as_deref()
            != Some(id)
        {
            return false;
        }
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if !inner.early_cancellations.iter().any(|key| key == id) {
            if inner.early_cancellations.len() == 32 {
                inner.early_cancellations.pop_front();
            }
            inner.early_cancellations.push_back(id.into());
        }
        if let Some((_, token)) = inner
            .session
            .as_ref()
            .and_then(|s| s.request.as_ref())
            .filter(|(key, _)| key == id)
        {
            token.cancel();
            true
        } else {
            false
        }
    }
    pub fn history(&self, id: &str) -> Result<Vec<HistoryMessage>, String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "Live recording state unavailable")?;
        Ok(inner
            .session
            .as_ref()
            .filter(|s| s.id == id)
            .ok_or("This recording session has ended")?
            .history
            .iter()
            .cloned()
            .collect())
    }
    pub fn clear_history(&self, id: &str) -> Result<u64, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Live recording state unavailable")?;
        let session = inner
            .session
            .as_mut()
            .filter(|s| s.id == id)
            .ok_or("This recording session has ended")?;
        if let Some((_, token)) = &session.request {
            token.cancel();
        }
        let count = session.history.len() as u64;
        session.history.clear();
        Ok(count)
    }
    pub fn resolve(
        &self,
        reference: &EvidenceRef,
    ) -> Result<super::evidence::ResolvedEvidence, String> {
        use super::evidence::{EvidenceStatus, ResolvedEvidence};
        let missing = |status| ResolvedEvidence {
            status,
            passage: None,
            navigation: None,
        };
        if reference.historical {
            return Ok(missing(EvidenceStatus::Stale));
        }
        let EvidenceLocator::Live {
            session_id,
            sequence_ids,
        } = &reference.locator
        else {
            return Ok(missing(EvidenceStatus::Invalid));
        };
        if sequence_ids.len() != 1 {
            return Ok(missing(EvidenceStatus::Invalid));
        }
        let Ok(snapshot) = self.snapshot(session_id) else {
            return Ok(missing(EvidenceStatus::Missing));
        };
        let Some(segment) = snapshot
            .segments
            .iter()
            .find(|s| s.sequence_id == sequence_ids[0])
        else {
            return Ok(missing(EvidenceStatus::Missing));
        };
        let passage = super::live_context::passage(&snapshot, segment);
        if &passage.evidence != reference {
            return Ok(missing(EvidenceStatus::Invalid));
        }
        Ok(ResolvedEvidence {
            status: EvidenceStatus::Current,
            passage: Some(passage),
            navigation: None,
        })
    }
    fn complete(
        &self,
        lease: &LiveLease,
        request: &AskRequest,
        reply: &AssistantReply,
    ) -> Result<(), String> {
        if lease.token.is_cancelled() || tokio::time::Instant::now() >= lease.deadline {
            return Err("Live request cancelled or deadline expired".into());
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Live recording state unavailable")?;
        validate_in(&inner, lease, !request.search.document_ids.is_empty())?;
        let session = inner
            .session
            .as_mut()
            .ok_or("This recording session has ended")?;
        let timestamp = chrono::Utc::now().to_rfc3339();
        for (id, role, content, result) in [
            (
                uuid::Uuid::new_v4().to_string(),
                "user",
                request.search.query.clone(),
                None,
            ),
            (
                reply.message_id.clone(),
                "assistant",
                reply.content.clone(),
                Some(reply.clone()),
            ),
        ] {
            session.history.push_back(HistoryMessage {
                id,
                request_id: Some(lease.request_id.clone()),
                role: role.into(),
                content,
                created_at: timestamp.clone(),
                status: "completed".into(),
                legacy: false,
                reply: result,
            });
        }
        while session.history.len() > 32
            || serde_json::to_vec(&session.history)
                .map_or(MAX_HISTORY_BYTES + 1, |bytes| bytes.len())
                > MAX_HISTORY_BYTES
        {
            session.history.pop_front();
            session.history.pop_front();
        }
        Ok(())
    }
}
fn validate_in(inner: &Inner, lease: &LiveLease, documents: bool) -> Result<(), String> {
    let session = inner
        .session
        .as_ref()
        .filter(|s| s.id == lease.session_id)
        .ok_or("This recording session has ended")?;
    if !inner.sharing || inner.consent_pending || inner.consent_failed {
        return Err("Live text sharing is disabled".into());
    }
    if !session
        .request
        .as_ref()
        .is_some_and(|(id, _)| id == &lease.request_id)
    {
        return Err("Live request superseded".into());
    }
    if documents && !session.document_sharing {
        return Err(
            "Enable reference sharing for this live session before sending document excerpts"
                .into(),
        );
    }
    Ok(())
}
pub(crate) async fn answer<F, Fut>(
    pool: &sqlx::SqlitePool,
    state: &Arc<LiveState>,
    request: AskRequest,
    lease: LiveLease,
    budget: usize,
    dispatch: F,
) -> Result<AssistantReply, String>
where
    F: FnOnce(String, CancellationToken) -> Fut,
    Fut: std::future::Future<Output = Result<ConfiguredTextReply, String>>,
{
    let operation = async {
        validate_request(&request, &lease)?;
        let documents = !request.search.document_ids.is_empty();
        state.validate(&lease, documents)?;
        let snapshot = lease
            .snapshot
            .clone()
            .unwrap_or(state.snapshot(&lease.session_id)?);
        if snapshot.session_id != lease.session_id {
            return Err("Live snapshot belongs to another recording".into());
        }
        if snapshot.segments.is_empty() {
            return Err("No finalized live transcript is available yet".into());
        }
        let (saved_scope, saved) = super::live_context::saved(pool, &request).await?;
        let live_limit = if saved.is_empty() { 12 } else { 6 };
        let live = super::live_context::lexical(&snapshot, &request.search.query, live_limit);
        let mut passages: Vec<_> = live.iter().take(live_limit).cloned().collect();
        passages.extend(saved.into_iter().take(12 - passages.len()));
        for passage in live {
            if passages.len() == 12 {
                break;
            }
            if !passages.iter().any(|p| p.evidence == passage.evidence) {
                passages.push(passage);
            }
        }
        let (prompt, selected) = super::answers::build_prompt(
            &request.search.query,
            &passages,
            "",
            budget.saturating_sub(512),
        )?;
        let mut envelope: serde_json::Value =
            serde_json::from_str(&prompt).map_err(|_| "Invalid live evidence prompt")?;
        envelope["live_context"] = serde_json::json!({"session_id":snapshot.session_id,"finalized_through_seconds":snapshot.finalized_through_seconds,
            "transcription_incomplete":snapshot.transcription_incomplete,"context_window_seconds":600,
            "notice":"Evidence is finalized transcript through the stated recording time, not a real-time description of the current moment."});
        let prompt = envelope.to_string();
        if prompt.len() > budget {
            return Err("Live context exceeds the configured model budget".into());
        }
        super::live_context::recheck(pool, saved_scope.as_ref(), &selected).await?;
        load_sharing(pool, state).await?;
        state.validate(&lease, documents)?;
        let output = dispatch(prompt, lease.token.child_token()).await?;
        if output.provider != lease.provider {
            return Err("Configured provider changed during the live request".into());
        }
        if output.text.len() > MAX_HISTORY_BYTES / 2 {
            return Err("Live answer exceeded the bounded response limit".into());
        }
        super::live_context::recheck(pool, saved_scope.as_ref(), &selected).await?;
        load_sharing(pool, state).await?;
        state.validate(&lease, documents)?;
        let reply = AssistantReply {
            request_id: lease.request_id.clone(),
            message_id: uuid::Uuid::new_v4().to_string(),
            content: output.text,
            evidence: selected.iter().map(|p| p.evidence.clone()).collect(),
            evidence_metadata: selected
                .iter()
                .map(|p| EvidenceDisplay {
                    title: p.title.clone(),
                    date: p.date.clone(),
                    speaker: p.speaker.clone(),
                    metadata_truncated: p.metadata_truncated,
                    preceding_question_tag: None,
                })
                .collect(),
            cited_tags: vec![],
            context_links: vec![],
            retrieval_mode: SearchMode::Keyword,
            provider: output.provider,
            model: output.model,
            live_context: Some(LiveReplyContext {
                session_id: snapshot.session_id,
                finalized_through_seconds: snapshot.finalized_through_seconds,
                transcription_incomplete: snapshot.transcription_incomplete,
            }),
        };
        let mut reply = reply;
        reply.cited_tags = super::evidence::tag_numbers(&reply.content, reply.evidence.len());
        state.complete(&lease, &request, &reply)?;
        Ok(reply)
    };
    tokio::select! {biased;
        _=lease.token.cancelled()=>Err("Live request cancelled".into()),
        _=tokio::time::sleep_until(lease.deadline)=>{lease.token.cancel();Err("Live request deadline expired".into())},
        result=operation=>result,
    }
}
fn validate_request(request: &AskRequest, lease: &LiveLease) -> Result<(), String> {
    if request.request_id != lease.request_id
        || request.search.query.trim().is_empty()
        || request.search.query.len() > 1024
        || request.search.document_ids.len() > super::evidence::MAX_EVIDENCE_ENTRIES
    {
        return Err("Invalid live answer request".into());
    }
    match (&request.owner, &request.search.scope) {
        (ConversationOwner::Live(owner), KnowledgeScope::Live { session_id })
            if owner == session_id && owner == &lease.session_id =>
        {
            Ok(())
        }
        _ => Err("Live owner and recording session do not match".into()),
    }
}
pub(crate) async fn ask<F>(
    pool: &sqlx::SqlitePool,
    runtime: &super::KnowledgeState,
    request: AskRequest,
    environment: F,
) -> Result<AssistantReply, String>
where
    F: FnOnce(
        &crate::summary::llm_client::LLMProvider,
    ) -> Result<crate::summary::llm_client::TextEnvironment, String>,
{
    use crate::summary::llm_client;
    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_secs(30);
    let session_id = match (&request.owner, &request.search.scope) {
        (ConversationOwner::Live(owner), KnowledgeScope::Live { session_id })
            if owner == session_id =>
        {
            owner.clone()
        }
        _ => return Err("Live owner and recording session do not match".into()),
    };
    let setup = async {
        load_sharing(pool, &runtime.live).await?;
        let settings =
            crate::database::repositories::setting::SettingsRepository::get_model_config(pool)
                .await
                .map_err(|_| "Provider settings unavailable")?
                .ok_or("Configure a summary provider before asking a question")?;
        Ok::<_, String>(settings)
    };
    let settings = tokio::time::timeout(Duration::from_secs(1), setup)
        .await
        .map_err(|_| "Live provider settings lookup timed out")??;
    let provider = llm_client::LLMProvider::from_str(&settings.provider)?;
    let mut lease = runtime.live.claim(
        &session_id,
        &request.request_id,
        llm_client::canonical_provider(&provider),
    )?;
    lease.deadline = deadline;
    validate_request(&request, &lease)?;
    runtime
        .live
        .validate(&lease, !request.search.document_ids.is_empty())?;
    // Read the backend manager's finalized snapshot and health for this exact session.
    let snapshot = crate::audio::recording_commands::live_snapshot()?;
    if snapshot.session_id != session_id {
        return Err("Live recording session changed".into());
    }
    lease.snapshot = Some(snapshot);
    let mut resolved = tokio::select! {biased;
        _=lease.token.cancelled()=>return Err("Live request cancelled".into()),
        _=tokio::time::sleep_until(deadline)=>return Err("Live request deadline expired".into()),
        result=llm_client::resolve_configured_text(pool,environment(&provider)?,&settings.provider,&settings.model,started,&lease.token)=>result?,
    };
    resolved.deadline = resolved.deadline.min(deadline);
    lease.deadline = resolved.deadline;
    lease.provider = llm_client::canonical_provider(&resolved.provider).into();
    let budget = resolved
        .budget
        .input(
            &resolved.provider,
            &resolved.model,
            super::answers::SYSTEM.len() + 1024,
        )?
        .min(512 * 1024);
    let expected_model = resolved.model.clone();
    answer(pool,&runtime.live,request,lease,budget,move |prompt,token|async move {
        let system=format!("{}\nLIVE RECORDING: Use only finalized evidence through live_context.finalized_through_seconds. Transcription may lag capture. Never claim this is what is happening now; qualify incomplete transcription and the bounded ten-minute window.",super::answers::SYSTEM);
        let result=llm_client::dispatch_resolved_text(resolved,system,prompt,&token).await?;
        if result.model != expected_model { return Err("Configured model changed during the live request".into()); }
        Ok(result)
    }).await
}
pub(crate) async fn load_sharing(
    pool: &sqlx::SqlitePool,
    state: &LiveState,
) -> Result<bool, String> {
    let revision = state
        .inner
        .lock()
        .map_err(|_| "Live sharing settings unavailable")?
        .consent_revision;
    let _io = state.consent_io.lock().await;
    let enabled: bool =
        sqlx::query_scalar("SELECT live_text_sharing=1 FROM knowledge_settings WHERE singleton=1")
            .fetch_one(pool)
            .await
            .map_err(|_| "Live sharing settings unavailable")?;
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "Live sharing settings unavailable")?;
    if inner.consent_revision == revision && !inner.consent_pending && !inner.consent_failed {
        inner.sharing = enabled;
        if !enabled {
            if let Some((_, token)) = inner.session.as_ref().and_then(|s| s.request.as_ref()) {
                token.cancel();
            }
        }
    }
    Ok(inner.sharing && !inner.consent_pending && !inner.consent_failed)
}
pub(crate) async fn save_sharing(
    pool: &sqlx::SqlitePool,
    state: &LiveState,
    enabled: bool,
) -> Result<(), String> {
    // Fail closed and signal cancellation before any database or configuration wait.
    let revision = {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "Live sharing settings unavailable")?;
        inner.consent_revision = inner.consent_revision.wrapping_add(1);
        inner.sharing = false;
        inner.consent_pending = true;
        if let Some((_, token)) = inner.session.as_ref().and_then(|s| s.request.as_ref()) {
            token.cancel();
        }
        inner.consent_revision
    };
    let _io = state.consent_io.lock().await;
    if state
        .inner
        .lock()
        .map_err(|_| "Live sharing settings unavailable")?
        .consent_revision
        != revision
    {
        return Ok(());
    }
    let result = sqlx::query("UPDATE knowledge_settings SET live_text_sharing=? WHERE singleton=1")
        .bind(enabled)
        .execute(pool)
        .await;
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "Live sharing settings unavailable")?;
    if inner.consent_revision == revision {
        inner.consent_pending = false;
        inner.consent_failed = result.as_ref().map_or(true, |row| row.rows_affected() != 1);
        inner.sharing = enabled && !inner.consent_failed;
    }
    if result
        .map_err(|_| "Could not save live sharing. Sharing remains disabled for this session.")?
        .rows_affected()
        != 1
    {
        return Err(
            "Could not save live sharing. Sharing remains disabled for this session.".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn update(sequence_id: u64, end: f64, text: &str, partial: bool) -> TranscriptUpdate {
        TranscriptUpdate {
            session_id: None,
            text: text.into(),
            timestamp: "12:00:00".into(),
            source: "Me".into(),
            sequence_id,
            chunk_start_time: end - 1.,
            is_partial: partial,
            confidence: Some(0.01),
            audio_start_time: end - 1.,
            audio_end_time: end,
            duration: 1.,
            word_timestamps: None,
        }
    }
    fn ready() -> (Arc<LiveState>, String) {
        let state = Arc::new(LiveState::default());
        let id = state.start(true);
        state.set_sharing(true);
        state.ingest(&id, &update(1, 7., "Ja", false));
        (state, id)
    }
    fn request(id: &str, request_id: &str) -> AskRequest {
        AskRequest {
            live_reference_scope: None,
            request_id: request_id.into(),
            owner: ConversationOwner::Live(id.into()),
            search: SearchRequest {
                scope: KnowledgeScope::Live {
                    session_id: id.into(),
                },
                query: "What was confirmed?".into(),
                document_ids: vec![],
                mode: SearchMode::Keyword,
            },
        }
    }
    async fn pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }
    async fn enabled_pool(state: &LiveState) -> sqlx::SqlitePool {
        let pool = pool().await;
        save_sharing(&pool, state, true).await.unwrap();
        pool
    }
    #[test]
    fn partial_segments_excluded() {
        let (state, id) = ready();
        state.ingest(&id, &update(2, 12., "Pending hypothesis", true));
        let snapshot = state.snapshot(&id).unwrap();
        assert_eq!(snapshot.segments.len(), 1);
        assert_eq!(
            snapshot.segments[0].text, "Ja",
            "Short low-confidence finalized words must survive"
        );
        assert_eq!(snapshot.finalized_through_seconds, 7.);
    }
    #[test]
    fn context_uses_last_ten_minutes() {
        let (state, id) = ready();
        state.ingest(&id, &update(2, 500., "Boundary", false));
        state.ingest(&id, &update(3, 1100., "Recent", false));
        let snapshot = state.snapshot(&id).unwrap();
        assert_eq!(
            snapshot
                .segments
                .iter()
                .map(|s| s.sequence_id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(snapshot.finalized_through_seconds, 1100.);
    }
    #[test]
    fn realistic_hour_keeps_only_recent_finalized_context() {
        let (state, id) = ready();
        for sequence in 2..=601 {
            state.ingest(
                &id,
                &update(
                    sequence,
                    (sequence - 1) as f64 * 6.,
                    "Public synthetic meeting decision",
                    false,
                ),
            );
        }
        let snapshot = state.snapshot(&id).unwrap();
        assert_eq!(snapshot.finalized_through_seconds, 3600.);
        assert_eq!(snapshot.segments.first().unwrap().sequence_id, 501);
        assert_eq!(snapshot.segments.last().unwrap().sequence_id, 601);
        assert_eq!(snapshot.segments.len(), 101);
    }
    #[test]
    fn failed_start_clears_live_state_without_clearing_remembered_consent() {
        let state = Arc::new(LiveState::default());
        state.set_sharing(true);
        let startup = state.prepare(true);
        let id = startup.session_id.clone();
        assert!(state.snapshot(&id).is_ok());
        drop(startup);
        assert!(state.snapshot(&id).is_err());
        assert!(state.sharing());
        let next = state.prepare(true).commit();
        assert_ne!(id, next);
        assert!(state.snapshot(&next).is_ok());
    }
    #[test]
    fn audio_only_has_no_live_context() {
        let state = Arc::new(LiveState::default());
        let id = state.start(false);
        state.set_sharing(true);
        state.ingest(&id, &update(1, 7., "Unrelated", false));
        assert!(state.snapshot(&id).unwrap().segments.is_empty());
        assert!(state
            .claim(&id, &uuid::Uuid::new_v4().to_string(), "openai")
            .unwrap_err()
            .contains("Audio-only"));
    }
    #[test]
    fn local_builtin_rejected_during_capture() {
        let (state, id) = ready();
        assert!(state
            .claim(&id, &uuid::Uuid::new_v4().to_string(), "builtin-ai")
            .unwrap_err()
            .contains("after recording"));
    }
    #[test]
    fn every_recording_has_a_fresh_uuid_and_rejects_old_ingestion() {
        let (state, first) = ready();
        assert_eq!(uuid::Uuid::parse_str(&first).unwrap().to_string(), first);
        state.stop();
        let second = state.start(true);
        assert_ne!(first, second);
        state.ingest(&first, &update(9, 15., "Late old result", false));
        assert!(state.snapshot(&second).unwrap().segments.is_empty());
    }
    #[test]
    fn one_request_in_flight_and_thirty_second_deadline() {
        let (state, id) = ready();
        let now = tokio::time::Instant::now();
        let lease = state
            .claim(&id, &uuid::Uuid::new_v4().to_string(), "openai")
            .unwrap();
        assert!(lease.deadline >= now + Duration::from_secs(30));
        assert!(lease.deadline < now + Duration::from_secs(31));
        assert!(state
            .claim(&id, &uuid::Uuid::new_v4().to_string(), "openai")
            .is_err());
        drop(lease);
        assert!(state
            .claim(&id, &uuid::Uuid::new_v4().to_string(), "openai")
            .is_ok());
    }
    #[test]
    fn live_sharing_is_independent_off_by_default_and_revocation_cancels() {
        let state = Arc::new(LiveState::default());
        let id = state.start(true);
        state.ingest(&id, &update(1, 7., "Ja", false));
        assert!(!state.sharing());
        assert!(state
            .claim(&id, &uuid::Uuid::new_v4().to_string(), "openai")
            .is_err());
        state.set_sharing(true);
        assert!(!state.document_sharing(&id).unwrap());
        let lease = state
            .claim(&id, &uuid::Uuid::new_v4().to_string(), "openai")
            .unwrap();
        state.set_sharing(false);
        assert!(lease.token.is_cancelled());
        assert!(state.validate(&lease, false).is_err());
        assert_eq!(state.snapshot(&id).unwrap().segments[0].text, "Ja");
    }
    #[test]
    fn document_permission_is_separate_and_session_specific() {
        let (state, id) = ready();
        state.set_document_sharing(&id, true).unwrap();
        assert!(state.document_sharing(&id).unwrap());
        let lease = state
            .claim(&id, &uuid::Uuid::new_v4().to_string(), "openai")
            .unwrap();
        state.set_document_sharing(&id, false).unwrap();
        assert!(lease.token.is_cancelled());
        state.stop();
        let second = state.start(true);
        assert!(state.sharing(), "Remembered live opt-in survives sessions");
        assert!(!state.document_sharing(&second).unwrap());
        assert!(state.set_document_sharing(&id, true).is_err());
    }
    #[test]
    fn old_session_reply_discarded() {
        let (state, id) = ready();
        let lease = state
            .claim(&id, &uuid::Uuid::new_v4().to_string(), "openai")
            .unwrap();
        state.stop();
        let _second = state.start(true);
        assert!(lease.token.is_cancelled());
        assert!(state.validate(&lease, false).is_err());
    }
    #[test]
    fn context_has_hard_byte_and_segment_caps() {
        let (state, id) = ready();
        for sequence in 2..1500 {
            state.ingest(&id, &update(sequence, 10., &"ü".repeat(2048), false));
        }
        let snapshot = state.snapshot(&id).unwrap();
        assert!(snapshot.segments.len() <= 1024);
        assert!(
            snapshot
                .segments
                .iter()
                .map(|s| s.text.len())
                .sum::<usize>()
                <= 256 * 1024
        );
        assert!(snapshot.segments.iter().all(|s| s.text.len() <= 2048));
        assert!(
            snapshot.transcription_incomplete,
            "Truncated live context must be disclosed"
        );
    }
    #[tokio::test]
    async fn stop_does_not_wait_for_provider() {
        let (state, id) = ready();
        let pool = enabled_pool(&state).await;
        let request_id = uuid::Uuid::new_v4().to_string();
        let lease = state.claim(&id, &request_id, "openai").unwrap();
        let token = lease.token.clone();
        let task_state = state.clone();
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            answer(
                &pool,
                &task_state,
                request(&id, &request_id),
                lease,
                8192,
                move |_, _| async move {
                    let _ = started.send(());
                    std::future::pending::<Result<ConfiguredTextReply, String>>().await
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .unwrap()
            .unwrap();
        let before = std::time::Instant::now();
        state.stop();
        assert!(before.elapsed() < Duration::from_millis(100));
        assert!(token.is_cancelled());
        assert!(tokio::time::timeout(Duration::from_millis(100), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
    }
    #[tokio::test]
    async fn request_deadline_bounds_a_never_ending_provider() {
        let (state, id) = ready();
        let pool = enabled_pool(&state).await;
        let request_id = uuid::Uuid::new_v4().to_string();
        let mut lease = state.claim(&id, &request_id, "openai").unwrap();
        lease.deadline = tokio::time::Instant::now() + Duration::from_millis(20);
        let result = tokio::time::timeout(
            Duration::from_millis(200),
            answer(
                &pool,
                &state,
                request(&id, &request_id),
                lease,
                8192,
                |_, _| std::future::pending::<Result<ConfiguredTextReply, String>>(),
            ),
        )
        .await
        .unwrap();
        assert!(result.unwrap_err().contains("deadline"));
    }
    #[tokio::test]
    async fn live_history_is_never_persisted_and_clears_on_stop() {
        let (state, id) = ready();
        let pool = enabled_pool(&state).await;
        let request_id = uuid::Uuid::new_v4().to_string();
        let lease = state.claim(&id, &request_id, "openai").unwrap();
        let reply = answer(
            &pool,
            &state,
            request(&id, &request_id),
            lease,
            8192,
            |prompt, _| async move {
                let value: serde_json::Value = serde_json::from_str(&prompt).unwrap();
                assert_eq!(value["live_context"]["finalized_through_seconds"], 7.);
                Ok(ConfiguredTextReply {
                    text: "Confirmed [K1]".into(),
                    provider: "openai".into(),
                    model: "synthetic".into(),
                })
            },
        )
        .await
        .unwrap();
        assert_eq!(reply.evidence.len(), 1);
        let context = reply.live_context.as_ref().unwrap();
        assert_eq!(context.session_id, id);
        assert_eq!(context.finalized_through_seconds, 7.);
        assert!(!context.transcription_incomplete);
        assert_eq!(state.history(&id).unwrap().len(), 2);
        for table in [
            "knowledge_owners",
            "knowledge_requests",
            "knowledge_messages",
            "knowledge_request_evidence",
            "knowledge_request_sources",
            "knowledge_document_permissions",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(count, 0, "Live state must not create durable {table}");
        }
        state.stop();
        assert!(state.history(&id).is_err());
        let second = state.start(true);
        assert!(state.history(&second).unwrap().is_empty());
    }
    #[tokio::test]
    async fn remembered_consent_is_off_by_default_persists_and_reloads_independently() {
        let pool = pool().await;
        let state = LiveState::default();
        assert!(!load_sharing(&pool, &state).await.unwrap());
        save_sharing(&pool, &state, true).await.unwrap();
        let restarted = LiveState::default();
        assert!(load_sharing(&pool, &restarted).await.unwrap());
        assert!(restarted.sharing());
        let id = restarted.start(true);
        assert!(!restarted.document_sharing(&id).unwrap());
        save_sharing(&pool, &restarted, false).await.unwrap();
        assert!(!load_sharing(&pool, &LiveState::default()).await.unwrap());
        let durable: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_document_permissions")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(durable, 0);
    }
    #[tokio::test]
    async fn revoked_consent_cancels_a_never_ending_request() {
        let (state, id) = ready();
        let pool = enabled_pool(&state).await;
        let consent_pool = pool.clone();
        let request_id = uuid::Uuid::new_v4().to_string();
        let lease = state.claim(&id, &request_id, "openai").unwrap();
        let task_state = state.clone();
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            answer(
                &pool,
                &task_state,
                request(&id, &request_id),
                lease,
                8192,
                move |_, _| async move {
                    let _ = started.send(());
                    std::future::pending::<Result<ConfiguredTextReply, String>>().await
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .unwrap()
            .unwrap();
        save_sharing(&consent_pool, &state, false).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(100), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
    }
    #[tokio::test]
    async fn pending_index_cannot_delay_stop() {
        let (state, id) = ready();
        let runtime = super::super::KnowledgeState {
            live: state.clone(),
            ..Default::default()
        };
        runtime
            .scheduler
            .notifications
            .lock()
            .unwrap()
            .notify("meeting:public-fixture".into(), 1)
            .unwrap();
        let _pending_index_lock = runtime.configuration.lock().await;
        assert_eq!(
            crate::audio::recording_commands::stop_live_assistance(&runtime),
            Some(id.clone())
        );
        assert!(state.snapshot(&id).is_err());
        assert_eq!(
            runtime
                .scheduler
                .notifications
                .lock()
                .unwrap()
                .next()
                .unwrap()
                .source_id,
            "meeting:public-fixture"
        );
    }
    #[tokio::test]
    async fn stale_consent_read_cannot_undo_revocation() {
        let (state, id) = ready();
        let pool = enabled_pool(&state).await;
        let lease = state
            .claim(&id, &uuid::Uuid::new_v4().to_string(), "openai")
            .unwrap();
        let io = state.consent_io.lock().await;
        let load = load_sharing(&pool, &state);
        tokio::pin!(load);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut load)
            .await
            .is_err());
        let revoke = save_sharing(&pool, &state, false);
        tokio::pin!(revoke);
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut revoke)
            .await
            .is_err());
        assert!(lease.token.is_cancelled());
        assert!(!state.sharing());
        drop(io);
        assert!(
            !load.await.unwrap(),
            "The stale persisted true must not reopen sharing"
        );
        revoke.await.unwrap();
        assert!(!load_sharing(&pool, &state).await.unwrap());
        assert!(state
            .claim(&id, &uuid::Uuid::new_v4().to_string(), "openai")
            .is_err());
    }
    #[tokio::test]
    async fn consent_persistence_failure_is_visible_and_stays_fail_closed() {
        let (state, id) = ready();
        let pool = enabled_pool(&state).await;
        let lease = state
            .claim(&id, &uuid::Uuid::new_v4().to_string(), "openai")
            .unwrap();
        sqlx::query("CREATE TRIGGER deny_live_setting BEFORE UPDATE OF live_text_sharing ON knowledge_settings BEGIN SELECT RAISE(ABORT,'synthetic write failure'); END").execute(&pool).await.unwrap();
        assert!(save_sharing(&pool, &state, false).await.is_err());
        assert!(lease.token.is_cancelled());
        assert!(!load_sharing(&pool, &state).await.unwrap());
        assert!(!state.sharing());
        let durable: bool =
            sqlx::query_scalar("SELECT live_text_sharing=1 FROM knowledge_settings")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            durable,
            "Exercise a failed durable revocation rather than a successful write"
        );
    }
    #[tokio::test]
    async fn upgraded_settings_keep_semantic_enablement_but_do_not_enable_live_sharing() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE knowledge_settings(singleton INTEGER PRIMARY KEY,enabled INTEGER NOT NULL); INSERT INTO knowledge_settings VALUES(1,1)").execute(&pool).await.unwrap();
        sqlx::query(include_str!(
            "../../migrations/20261010000000_live_text_sharing.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let flags: (bool, bool) =
            sqlx::query_as("SELECT enabled=1,live_text_sharing=1 FROM knowledge_settings")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(flags, (true, false));
        assert!(
            sqlx::query("UPDATE knowledge_settings SET live_text_sharing=2")
                .execute(&pool)
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn live_owner_scope_mismatch_never_dispatches() {
        let (state, id) = ready();
        let pool = enabled_pool(&state).await;
        let request_id = uuid::Uuid::new_v4().to_string();
        let lease = state.claim(&id, &request_id, "openai").unwrap();
        let mut input = request(&id, &request_id);
        input.search.scope = KnowledgeScope::Live {
            session_id: uuid::Uuid::new_v4().to_string(),
        };
        assert!(answer(&pool, &state, input, lease, 8192, |_, _| async {
            panic!("Mismatched sessions must never dispatch")
        })
        .await
        .is_err());
        assert!(state.history(&id).unwrap().is_empty());
    }
    #[tokio::test]
    async fn configured_builtin_branch_is_rejected_before_model_or_durable_work() {
        let runtime = super::super::KnowledgeState::default();
        let id = runtime.live.start(true);
        let pool = enabled_pool(&runtime.live).await;
        sqlx::query("INSERT INTO settings(id,provider,model,whisperModel) VALUES('1','builtin-ai','synthetic','tiny')").execute(&pool).await.unwrap();
        let result = super::super::answers::ask(
            &pool,
            &runtime,
            request(&id, &uuid::Uuid::new_v4().to_string()),
            |_| panic!("Built-in AI must never resolve or load during capture"),
        )
        .await;
        assert!(result.unwrap_err().contains("after recording"));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_requests")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    #[tokio::test]
    async fn provider_completion_after_session_replacement_cannot_enter_new_history() {
        let (state, id) = ready();
        let pool = enabled_pool(&state).await;
        let request_id = uuid::Uuid::new_v4().to_string();
        let lease = state.claim(&id, &request_id, "openai").unwrap();
        let changing = state.clone();
        let result = answer(
            &pool,
            &state,
            request(&id, &request_id),
            lease,
            8192,
            move |_, _| async move {
                changing.stop();
                changing.start(true);
                Ok(ConfiguredTextReply {
                    text: "Old completed answer [K1]".into(),
                    provider: "openai".into(),
                    model: "synthetic".into(),
                })
            },
        )
        .await;
        assert!(result.is_err());
        let second = state.current_session_id().unwrap();
        assert_ne!(id, second);
        assert!(state.history(&second).unwrap().is_empty());
    }
    #[tokio::test]
    async fn selected_saved_references_require_independent_live_document_permission() {
        let (pool, document) = super::super::document_context::tests::fixture().await;
        let (state, id) = ready();
        save_sharing(&pool, &state, true).await.unwrap();
        let make_request = |request_id: &str| {
            let mut input = request(&id, request_id);
            input.search.query = "decision_p24".into();
            input.live_reference_scope = Some(MeetingFilter {
                meeting_ids: vec!["aster".into()],
                ..Default::default()
            });
            input.search.document_ids = vec![document.clone()];
            input
        };
        let request_id = uuid::Uuid::new_v4().to_string();
        let lease = state.claim(&id, &request_id, "openai").unwrap();
        assert!(answer(
            &pool,
            &state,
            make_request(&request_id),
            lease,
            16384,
            |_, _| async { panic!("Document consent is separate from live text sharing") }
        )
        .await
        .is_err());
        // Durable saved-owner consent must not grant this recording permission.
        super::super::document_context::set_sharing(
            &pool,
            &ConversationOwner::Meeting("aster".into()),
            true,
        )
        .await
        .unwrap();
        assert!(!state.document_sharing(&id).unwrap());
        state.set_document_sharing(&id, true).unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        let lease = state.claim(&id, &request_id, "openai").unwrap();
        let reply = answer(
            &pool,
            &state,
            make_request(&request_id),
            lease,
            16384,
            |prompt, _| async move {
                let value: serde_json::Value = serde_json::from_str(&prompt).unwrap();
                assert_eq!(value["document_evidence"].as_array().unwrap().len(), 1);
                assert!(value["document_evidence"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("decision_p24"));
                assert_eq!(value["transcript_evidence"][0]["text"], "Ja");
                Ok(ConfiguredTextReply {
                    text: "Reference decision [K2]".into(),
                    provider: "openai".into(),
                    model: "synthetic".into(),
                })
            },
        )
        .await
        .unwrap();
        assert_eq!(reply.cited_tags, vec![2]);
        assert!(
            matches!(&reply.evidence[1].locator,EvidenceLocator::Document {document_id,..} if document_id == &document)
        );
        assert_eq!(
            super::super::evidence::resolve(&pool, &reply.evidence[1])
                .await
                .unwrap()
                .status,
            super::super::evidence::EvidenceStatus::Current
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM knowledge_requests")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    #[tokio::test]
    async fn foreign_and_unselected_saved_references_cannot_enter_live_prompt() {
        let (pool, document) = super::super::document_context::tests::fixture().await;
        let (state, id) = ready();
        save_sharing(&pool, &state, true).await.unwrap();
        state.set_document_sharing(&id, true).unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        let lease = state.claim(&id, &request_id, "openai").unwrap();
        let mut foreign = request(&id, &request_id);
        foreign.search.document_ids = vec![document.clone()];
        foreign.live_reference_scope = Some(MeetingFilter {
            meeting_ids: vec!["birch".into()],
            ..Default::default()
        });
        assert!(answer(&pool, &state, foreign, lease, 16384, |_, _| async {
            panic!("Foreign document must not reach the provider")
        })
        .await
        .is_err());
        let request_id = uuid::Uuid::new_v4().to_string();
        let lease = state.claim(&id, &request_id, "openai").unwrap();
        let mut unselected = request(&id, &request_id);
        unselected.search.query = "decision_p24".into();
        let reply = answer(
            &pool,
            &state,
            unselected,
            lease,
            16384,
            |prompt, _| async move {
                let value: serde_json::Value = serde_json::from_str(&prompt).unwrap();
                assert!(value["document_evidence"].as_array().unwrap().is_empty());
                assert!(!prompt.contains("Supplier reference"));
                Ok(ConfiguredTextReply {
                    text: "Not established in the live transcript.".into(),
                    provider: "openai".into(),
                    model: "synthetic".into(),
                })
            },
        )
        .await
        .unwrap();
        assert!(reply
            .evidence
            .iter()
            .all(|e| matches!(e.locator, EvidenceLocator::Live { .. })));
    }
    #[tokio::test]
    async fn detached_selected_reference_discards_provider_completion() {
        let (pool, document) = super::super::document_context::tests::fixture().await;
        let (state, id) = ready();
        save_sharing(&pool, &state, true).await.unwrap();
        state.set_document_sharing(&id, true).unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        let lease = state.claim(&id, &request_id, "openai").unwrap();
        let mut input = request(&id, &request_id);
        input.search.query = "decision_p24".into();
        input.search.document_ids = vec![document.clone()];
        input.live_reference_scope = Some(MeetingFilter {
            meeting_ids: vec!["aster".into()],
            ..Default::default()
        });
        let changed_pool = pool.clone();
        let result = answer(&pool, &state, input, lease, 16384, move |_, _| async move {
            super::super::documents::store::detach(&changed_pool, "aster", &document)
                .await
                .unwrap();
            Ok(ConfiguredTextReply {
                text: "Stale reference response [K2]".into(),
                provider: "openai".into(),
                model: "synthetic".into(),
            })
        })
        .await;
        assert!(result.is_err());
        assert!(state.history(&id).unwrap().is_empty());
    }
    #[test]
    fn live_citations_are_canonical_and_expire_with_the_session() {
        let (state, id) = ready();
        let snapshot = state.snapshot(&id).unwrap();
        let passage = super::super::live_context::passage(&snapshot, &snapshot.segments[0]);
        assert_eq!(
            state.resolve(&passage.evidence).unwrap().status,
            super::super::evidence::EvidenceStatus::Current
        );
        let mut invented = passage.evidence.clone();
        invented.fingerprint = "invented".into();
        assert_eq!(
            state.resolve(&invented).unwrap().status,
            super::super::evidence::EvidenceStatus::Invalid
        );
        state.stop();
        assert_eq!(
            state.resolve(&passage.evidence).unwrap().status,
            super::super::evidence::EvidenceStatus::Missing
        );
    }
    #[tokio::test]
    async fn live_history_remains_bounded_over_repeated_manual_requests() {
        let (state, id) = ready();
        let pool = enabled_pool(&state).await;
        for _ in 0..40 {
            let request_id = uuid::Uuid::new_v4().to_string();
            let lease = state.claim(&id, &request_id, "openai").unwrap();
            answer(
                &pool,
                &state,
                request(&id, &request_id),
                lease,
                8192,
                |_, _| async {
                    Ok(ConfiguredTextReply {
                        text: "Public synthetic answer ".repeat(100),
                        provider: "openai".into(),
                        model: "synthetic".into(),
                    })
                },
            )
            .await
            .unwrap();
        }
        let history = state.history(&id).unwrap();
        assert!(history.len() <= 32);
        assert!(serde_json::to_vec(&history).unwrap().len() <= 64 * 1024);
        assert!(!history.is_empty());
    }
}
