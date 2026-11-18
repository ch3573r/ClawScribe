use crate::audio::recording_state::DeviceType;
use serde::Serialize;

// Short replies rarely identify a language reliably. A long enough turn always
// uses acoustic auto-detection, even if this input already has a confirmed language.
const PROBE_SECONDS: f64 = 1.8;
const FRESH_SECONDS: f64 = 90.0;

#[derive(Debug, Default)]
struct Evidence {
    confirmed: Option<&'static str>,
    candidate: Option<&'static str>,
    last_evidence: Option<f64>,
}

impl Evidence {
    fn fresh(&self, start: f64) -> bool {
        self.last_evidence
            .is_some_and(|last| (0.0..=FRESH_SECONDS).contains(&(start - last)))
    }

    fn hint(&self, start: f64, duration: f64) -> Option<&'static str> {
        if duration >= PROBE_SECONDS || !self.fresh(start) || self.candidate.is_some() {
            return None;
        }
        self.confirmed
    }

    fn observe(&mut self, language: &'static str, start: f64) -> bool {
        // Replayed or out-of-order evidence must not count as a second turn.
        if self.last_evidence.is_some_and(|last| start <= last) {
            return false;
        }
        let repeated = self.candidate == Some(language) && self.fresh(start);
        self.last_evidence = Some(start);
        if self.confirmed == Some(language) {
            self.candidate = None;
        } else if repeated {
            self.confirmed = Some(language);
            self.candidate = None;
            return true;
        } else {
            self.candidate = Some(language);
        }
        false
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SourceLanguageSnapshot {
    pub session_id: String,
    pub revision: u64,
    pub microphone: Option<&'static str>,
    pub system: Option<&'static str>,
    pub evidence: LanguageEvidence,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LanguageEvidence {
    WhisperAudio,
    TranscriptText,
}

pub(super) fn parakeet_multilingual(model: &str) -> bool {
    model.starts_with("parakeet-tdt-0.6b-v3-") || model.starts_with("parakeet-tdt-0.6b-ultra-")
}

fn reliable_text_language(text: &str) -> Option<&'static str> {
    if text.chars().filter(|c| c.is_alphabetic()).take(32).count() < 32 {
        return None;
    }
    let info = whatlang::detect(text)?;
    if !info.is_reliable() || info.confidence() < 0.8 {
        return None;
    }
    Some(
        crate::summary::language_detection::summary_code_from_whatlang(info.lang())
            .unwrap_or_else(|| info.lang().code()),
    )
}

#[derive(Debug)]
pub(super) struct SourceLanguages {
    session_id: String,
    revision: u64,
    generation: u64,
    evidence: LanguageEvidence,
    microphone: Evidence,
    system: Evidence,
}

impl SourceLanguages {
    pub fn new(session_id: String, evidence: LanguageEvidence) -> Self {
        Self {
            session_id,
            revision: 0,
            generation: 0,
            evidence,
            microphone: Evidence::default(),
            system: Evidence::default(),
        }
    }

    pub fn snapshot(&self) -> SourceLanguageSnapshot {
        SourceLanguageSnapshot {
            session_id: self.session_id.clone(),
            revision: self.revision,
            evidence: self.evidence,
            microphone: self.microphone.confirmed,
            system: self.system.confirmed,
        }
    }

    pub fn reset(&mut self) -> SourceLanguageSnapshot {
        self.microphone = Evidence::default();
        self.system = Evidence::default();
        self.generation += 1;
        self.revision += 1;
        self.snapshot()
    }

    pub fn input(
        &self,
        source: &DeviceType,
        start: f64,
        duration: f64,
    ) -> (u64, Option<&'static str>) {
        let evidence = match source {
            DeviceType::Microphone => &self.microphone,
            DeviceType::System => &self.system,
        };
        let hint = if self.evidence == LanguageEvidence::WhisperAudio {
            evidence.hint(start, duration)
        } else {
            None
        };
        (self.generation, hint)
    }

    pub fn observe(
        &mut self,
        generation: u64,
        source: &DeviceType,
        language: Option<&'static str>,
        start: f64,
        duration: f64,
        confidence: Option<f32>,
        text: &str,
    ) -> Option<SourceLanguageSnapshot> {
        if self.generation != generation || duration < PROBE_SECONDS {
            return None;
        }
        let language = match self.evidence {
            // Token confidence checks decoder output quality. It is not the model's language probability.
            LanguageEvidence::WhisperAudio
                if confidence.is_some_and(|p| p >= 0.55)
                    && text.chars().filter(|c| c.is_alphabetic()).take(4).count() >= 4 =>
            {
                language?
            }
            LanguageEvidence::WhisperAudio => return None,
            LanguageEvidence::TranscriptText => reliable_text_language(text)?,
        };
        let evidence = match source {
            DeviceType::Microphone => &mut self.microphone,
            DeviceType::System => &mut self.system,
        };
        if !evidence.observe(language, start) {
            return None;
        }
        self.revision += 1;
        Some(self.snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DeviceType::{Microphone, System};

    #[test]
    fn independent_sources_need_two_recent_credible_observations_to_change() {
        let mut state = SourceLanguages::new("session".into(), LanguageEvidence::WhisperAudio);
        for (channel, language, start) in [
            (Microphone, "de", 0.),
            (System, "en", 2.),
            (Microphone, "de", 5.),
            (System, "en", 7.),
        ] {
            state.observe(
                0,
                &channel,
                Some(language),
                start,
                2.5,
                Some(0.9),
                "Synthetic speech",
            );
        }
        assert_eq!(state.input(&Microphone, 10., 0.7).1, Some("de"));
        assert_eq!(state.input(&System, 10., 0.7).1, Some("en"));
        assert_eq!(state.input(&Microphone, 10., 3.).1, None);
        state.observe(
            0,
            &Microphone,
            Some("en"),
            12.,
            3.,
            Some(0.9),
            "Example words",
        );
        assert_eq!(state.snapshot().microphone, Some("de"));
        assert_eq!(state.input(&Microphone, 16., 0.7).1, None);
        for (duration, confidence, text) in [
            (0.7, 0.9, "Yes"),
            (2.5, 0.2, "Weak result"),
            (2.5, 0.9, "..."),
        ] {
            assert!(state
                .observe(
                    0,
                    &Microphone,
                    Some("fr"),
                    16.,
                    duration,
                    Some(confidence),
                    text
                )
                .is_none());
        }
        let switched = state
            .observe(
                0,
                &Microphone,
                Some("en"),
                20.,
                3.,
                Some(0.9),
                "English speech",
            )
            .unwrap();
        assert_eq!(switched.microphone, Some("en"));
        assert_eq!(switched.system, Some("en"));
        assert_eq!(state.input(&System, 120., 0.8).1, None);
    }

    #[test]
    fn reset_discards_pending_evidence_and_rejects_results_from_the_old_generation() {
        let mut state = SourceLanguages::new("one".into(), LanguageEvidence::WhisperAudio);
        state.observe(0, &System, Some("fr"), 0., 3., Some(0.9), "Bonjour");
        state.reset();
        assert!(state
            .observe(0, &System, Some("fr"), 5., 3., Some(0.9), "Bonjour")
            .is_none());
        assert!(state
            .observe(1, &System, Some("fr"), 10., 3., Some(0.9), "Bonjour")
            .is_none());
        assert_eq!(state.snapshot().system, None);
        assert_eq!(
            SourceLanguages::new("two".into(), LanguageEvidence::WhisperAudio)
                .snapshot()
                .revision,
            0
        );
    }

    #[test]
    fn stale_or_duplicate_observations_cannot_confirm_a_candidate() {
        let mut evidence = Evidence::default();
        assert!(!evidence.observe("de", 5.0));
        assert!(!evidence.observe("de", 5.0));
        assert!(!evidence.observe("de", 3.0));
        assert_eq!(evidence.confirmed, None);
        assert!(evidence.observe("de", 8.0));
        assert!(!evidence.observe("en", 6.0));
        assert_eq!(evidence.candidate, None);
    }

    #[test]
    fn parakeet_reports_reliable_text_language_without_steering_its_decoder() {
        let mut state = SourceLanguages::new("parakeet".into(), LanguageEvidence::TranscriptText);
        let text = "The members of the committee would like to discuss the proposed schedule for the coming financial year.";
        assert!(parakeet_multilingual("parakeet-tdt-0.6b-ultra-int8"));
        assert!(!parakeet_multilingual("parakeet-tdt-0.6b-v2-int8"));
        assert!(reliable_text_language("Ja.").is_none());
        state.observe(0, &System, None, 0., 5., None, text);
        let snapshot = state.observe(0, &System, None, 8., 5., None, text).unwrap();
        assert_eq!(snapshot.system, Some("en"));
        assert_eq!(snapshot.evidence, LanguageEvidence::TranscriptText);
        assert_eq!(state.input(&System, 15., 1.).1, None);
    }
}
