//! Recording-scoped, ephemeral assistance; saved conversations are independent.
use super::types::*;
use crate::audio::transcription::TranscriptUpdate;
use crate::summary::llm_client::ConfiguredTextReply;
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;
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
}
#[derive(Default)]
pub struct LiveState;
#[derive(Debug)]
pub struct LiveLease {
    pub session_id: String,
    pub request_id: String,
    pub token: CancellationToken,
    pub deadline: tokio::time::Instant,
}
pub struct LiveStartup {
    pub session_id: String,
}
impl LiveStartup {
    pub fn commit(self) -> String {
        self.session_id
    }
}
impl LiveState {
    pub fn prepare(self: &Arc<Self>, transcribes: bool) -> LiveStartup {
        LiveStartup {
            session_id: self.start(transcribes),
        }
    }
    pub fn start(&self, _transcribes: bool) -> String {
        String::new()
    }
    pub fn stop(&self) {}
    pub fn ingest(&self, _session: &str, _update: &TranscriptUpdate) {}
    pub fn snapshot(&self, _session: &str) -> Result<LiveSnapshot, String> {
        Err("Live assistance is not implemented".into())
    }
    pub fn set_sharing(&self, _enabled: bool) {}
    pub fn sharing(&self) -> bool {
        false
    }
    pub fn set_document_sharing(&self, _session: &str, _enabled: bool) -> Result<(), String> {
        Err("Live assistance is not implemented".into())
    }
    pub fn document_sharing(&self, _session: &str) -> Result<bool, String> {
        Err("Live assistance is not implemented".into())
    }
    pub fn claim(
        self: &Arc<Self>,
        _session: &str,
        _request: &str,
        _provider: &str,
    ) -> Result<LiveLease, String> {
        Err("Live assistance is not implemented".into())
    }
    pub fn validate(&self, _lease: &LiveLease, _documents: bool) -> Result<(), String> {
        Err("Live assistance is not implemented".into())
    }
    pub fn history(&self, _session: &str) -> Result<Vec<HistoryMessage>, String> {
        Err("Live assistance is not implemented".into())
    }
}
pub(crate) async fn answer<F, Fut>(
    _pool: &sqlx::SqlitePool,
    _state: &Arc<LiveState>,
    _request: AskRequest,
    _lease: LiveLease,
    _budget: usize,
    _dispatch: F,
) -> Result<AssistantReply, String>
where
    F: FnOnce(String, CancellationToken) -> Fut,
    Fut: std::future::Future<Output = Result<ConfiguredTextReply, String>>,
{
    Err("Live assistance is not implemented".into())
}
pub(crate) async fn load_sharing(
    _pool: &sqlx::SqlitePool,
    _state: &LiveState,
) -> Result<bool, String> {
    Err("Live sharing is not implemented".into())
}
pub(crate) async fn save_sharing(
    _pool: &sqlx::SqlitePool,
    _state: &LiveState,
    _enabled: bool,
) -> Result<(), String> {
    Err("Live sharing is not implemented".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn update(sequence_id: u64, end: f64, text: &str, partial: bool) -> TranscriptUpdate {
        TranscriptUpdate {
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
        let pool = pool().await;
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
        let pool = pool().await;
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
        let pool = pool().await;
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
        let pool = pool().await;
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
        state.set_sharing(false);
        assert!(tokio::time::timeout(Duration::from_millis(100), task)
            .await
            .unwrap()
            .unwrap()
            .is_err());
    }
    #[tokio::test]
    async fn pending_index_cannot_delay_stop() {
        let (state, id) = ready();
        let runtime = super::super::KnowledgeState::default();
        runtime
            .scheduler
            .notifications
            .lock()
            .unwrap()
            .notify("meeting:public-fixture".into(), 1)
            .unwrap();
        let _pending_index_lock = runtime.configuration.lock().await;
        state.stop();
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
}
