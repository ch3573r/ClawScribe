use super::conversation_metrics::{ConversationMetrics, Segment, Side, Snapshot, Summary};
use super::recording_state::RecordingState;
use super::transcription::{worker::SOURCE_ATTRIBUTION_ENABLED, TranscriptUpdate};
use serde::Serialize;
use std::sync::{atomic::Ordering, Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Runtime};

#[derive(Clone, Serialize)]
pub(crate) struct Readout {
    pub session_id: String,
    pub revision: u64,
    #[serde(flatten)]
    snapshot: Snapshot,
}

pub(crate) struct LiveConversation {
    segments: Mutex<ConversationMetrics>,
    published: Mutex<Readout>,
}

impl LiveConversation {
    pub fn new(session_id: &str) -> Self {
        Self {
            segments: Mutex::new(ConversationMetrics::default()),
            published: Mutex::new(Readout {
                session_id: session_id.to_owned(),
                revision: 0,
                snapshot: Summary::default().snapshot(0.0, false),
            }),
        }
    }

    pub fn ingest(&self, update: &TranscriptUpdate, clear_source: bool) {
        if update.is_partial {
            return;
        }
        let side = match (clear_source, update.source.as_str()) {
            (true, "Me") => Side::Me,
            (true, "Participants") => Side::Others,
            _ => Side::Unknown,
        };
        if let Ok(mut segments) = self.segments.lock() {
            segments.ingest(Segment {
                id: update.sequence_id,
                side,
                start: update.audio_start_time,
                end: update.audio_end_time,
                text: &update.text,
                is_partial: update.is_partial,
            });
        }
    }

    pub fn read(&self) -> Option<Readout> {
        self.published.lock().ok().map(|snapshot| snapshot.clone())
    }

    pub fn watch<R: Runtime>(self: Arc<Self>, app: AppHandle<R>, capture: Arc<RecordingState>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut revision = 0;
            let mut summary = Summary::default();
            loop {
                interval.tick().await;
                if !capture.is_recording() {
                    return;
                }
                let updated = self.segments.lock().ok().and_then(|segments| {
                    (segments.revision() != revision).then(|| segments.clone())
                });
                // Merge intervals away from the producer's lock.
                if let Some(segments) = updated {
                    summary = segments.summarize();
                    revision = segments.revision();
                }
                let snapshot = summary.snapshot(
                    capture.speech_clock_seconds(),
                    SOURCE_ATTRIBUTION_ENABLED.load(Ordering::Relaxed),
                );
                let Some(update) = self.publish(snapshot) else {
                    continue;
                };
                let _ = app.emit("live-conversation-metrics", update);
            }
        });
    }

    fn publish(&self, snapshot: Snapshot) -> Option<Readout> {
        let mut published = self.published.lock().ok()?;
        if published.snapshot == snapshot {
            return None;
        }
        published.snapshot = snapshot;
        published.revision += 1;
        Some(published.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::conversation_metrics::Availability;

    fn update(id: u64, source: &str) -> TranscriptUpdate {
        TranscriptUpdate {
            session_id: Some("first".into()),
            text: "Ready?".into(),
            timestamp: String::new(),
            source: source.into(),
            sequence_id: id,
            chunk_start_time: 1.0,
            is_partial: false,
            confidence: None,
            audio_start_time: 1.0,
            audio_end_time: 3.0,
            duration: 2.0,
            word_timestamps: None,
        }
    }

    #[test]
    fn a_display_label_requires_clear_source_evidence_for_metrics() {
        let state = LiveConversation::new("first");
        let snapshot = |enabled| {
            state
                .segments
                .lock()
                .unwrap()
                .summarize()
                .snapshot(3.0, enabled)
        };
        state.ingest(&update(1, "Me"), false);
        assert_eq!(snapshot(true).availability, Availability::UnclearSource);
        assert_eq!(snapshot(false).availability, Availability::LabelsOff);
        assert_eq!(snapshot(true).totals.questions, 1);

        state.ingest(&update(1, "Me"), true);
        assert_eq!(snapshot(true).sides.unwrap().me_seconds, 2.0);
        state.ingest(&update(2, "Participants"), true);
        assert_eq!(snapshot(true).sides.unwrap().others_seconds, 2.0);
        state.ingest(&update(3, ""), true);
        assert!(snapshot(true).sides.is_none());
    }

    #[test]
    fn only_changed_snapshots_advance_the_session_revision() {
        let state = LiveConversation::new("first");
        state.ingest(&update(1, "Me"), true);
        let summary = state.segments.lock().unwrap().summarize();
        assert_eq!(
            state.publish(summary.snapshot(3.0, true)).unwrap().revision,
            1
        );
        assert!(state.publish(summary.snapshot(4.0, true)).is_none());
        assert_eq!(
            state.publish(summary.snapshot(6.0, true)).unwrap().revision,
            2
        );
        let snapshot = state.read().unwrap();
        assert_eq!(snapshot.revision, 2);
        assert_eq!(
            snapshot.snapshot.sides.unwrap().current_monologue_seconds,
            0.0
        );

        let second = LiveConversation::new("second").read().unwrap();
        assert_eq!(second.session_id, "second");
        assert_eq!(second.revision, 0);
        assert_eq!(second.snapshot.availability, Availability::WaitingForSpeech);
    }
}
