/*! Local metrics derived from finalized transcript segments on an audio timeline. */

use serde::Serialize;
use std::collections::{btree_map::Entry, BTreeMap};

const MONOLOGUE_GAP_MS: u64 = 2_000;
const RECENT_WINDOW_MS: u64 = 120_000;
/// Finalized segments trail the live clock by the segment still being spoken
/// plus its decoding, so a monologue stays current for this long after its
/// last finalized speech.
const FINALIZATION_LAG_MS: u64 = 10_000;
/// Side metrics stay available while at most this share of speech time has no
/// clear source. Crosstalk makes some unclear speech normal in a real meeting.
const MAX_UNCLEAR_SHARE: f64 = 0.25;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Me,
    Others,
    Unknown,
}

/// Times are seconds on the speech detector's audio clock, with pauses omitted.
pub struct Segment<'a> {
    pub id: u64,
    pub side: Side,
    pub start: f64,
    pub end: f64,
    pub text: &'a str,
    pub is_partial: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    pub availability: Availability,
    pub totals: Totals,
    pub sides: Option<SideMetrics>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    WaitingForSpeech,
    LabelsOff,
    UnclearSource,
    Available,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Totals {
    pub speech_seconds: f64,
    /// Whitespace-delimited tokens containing letters or digits.
    pub words: usize,
    pub questions: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SideMetrics {
    pub me_seconds: f64,
    pub others_seconds: f64,
    /// Share of the attributed speech time, which excludes unclear speech.
    pub me_share: f64,
    /// Share of all speech time that had no clear source.
    pub unclear_share: f64,
    pub pace_wpm: Option<u32>,
    pub recent_pace_wpm: Option<u32>,
    pub me_questions: usize,
    pub others_questions: usize,
    pub current_monologue_seconds: f64,
    pub longest_monologue_seconds: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Speech {
    side: Side,
    start: u64,
    end: u64,
    words: usize,
    questions: usize,
    pace_supported: bool,
}

#[derive(Clone, Default)]
pub struct ConversationMetrics {
    // Store compact contributions, never the transcript. Upserts are O(log n)
    // and do not recalculate the timeline on the producer's thread.
    segments: BTreeMap<u64, Speech>,
    revision: u64,
}

impl ConversationMetrics {
    pub fn ingest(&mut self, segment: Segment<'_>) {
        if segment.is_partial
            || segment.text.trim().is_empty()
            || !segment.start.is_finite()
            || !segment.end.is_finite()
            || segment.start < 0.0
        {
            return;
        }
        let start = millis(segment.start);
        let end = millis(segment.end);
        if end <= start {
            return;
        }
        let speech = Speech {
            side: segment.side,
            start,
            end,
            words: segment
                .text
                .split_whitespace()
                .filter(|token| token.chars().any(char::is_alphanumeric))
                .count(),
            questions: question_count(segment.text),
            pace_supported: !segment.text.chars().any(has_unspaced_script),
        };
        match self.segments.entry(segment.id) {
            Entry::Occupied(mut entry) if *entry.get() != speech => {
                entry.insert(speech);
            }
            Entry::Vacant(entry) => {
                entry.insert(speech);
            }
            _ => return,
        }
        self.revision += 1;
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn summarize(&self) -> Summary {
        let mut summary = Summary::default();
        let mut edges = Vec::with_capacity(self.segments.len() * 2);
        for speech in self.segments.values() {
            summary.totals.words += speech.words;
            summary.totals.questions += speech.questions;
            if speech.side == Side::Me {
                summary.me.push(*speech);
                summary.me_questions += speech.questions;
            } else if speech.side == Side::Others {
                summary.others_questions += speech.questions;
            }
            edges.push((speech.start, speech.side, 1));
            edges.push((speech.end, speech.side, -1));
        }
        summary.me.sort_by_key(|speech| speech.end);

        edges.sort_unstable_by_key(|edge| edge.0);
        let mut active = [0; 3];
        let mut total_ms = 0;
        for (time, side, delta) in edges {
            let span = time - summary.end;
            if span > 0 {
                let me = active[Side::Me as usize] > 0;
                let others = active[Side::Others as usize] > 0;
                let unclear = !me && !others && active[Side::Unknown as usize] > 0;
                if me || others || unclear {
                    total_ms += span;
                }
                if unclear {
                    // Unclear speech may be someone else, so it ends a monologue.
                    summary.unclear_ms += span;
                    summary.monologue = None;
                }
                if me {
                    summary.me_ms += span;
                }
                if others {
                    summary.others_ms += span;
                    summary.monologue = None;
                } else if me {
                    let start = match summary.monologue {
                        Some((start, end)) if summary.end - end <= MONOLOGUE_GAP_MS => start,
                        _ => summary.end,
                    };
                    summary.monologue = Some((start, time));
                    summary.longest_ms = summary.longest_ms.max(time - start);
                }
            }
            active[side as usize] += delta;
            summary.end = time;
        }
        summary.speech_ms = total_ms;
        summary.totals.speech_seconds = seconds(total_ms);
        if summary.me.iter().all(|speech| speech.pace_supported) {
            let words = summary.me.iter().map(|speech| speech.words).sum::<usize>();
            summary.pace_wpm = pace(words as f64, summary.me_ms);
        }
        summary
    }
}

/// The completed timeline, reusable while the recording clock advances
/// without new finalized segments.
#[derive(Default)]
pub struct Summary {
    totals: Totals,
    end: u64,
    speech_ms: u64,
    unclear_ms: u64,
    me: Vec<Speech>,
    me_ms: u64,
    others_ms: u64,
    me_questions: usize,
    pace_wpm: Option<u32>,
    others_questions: usize,
    monologue: Option<(u64, u64)>,
    longest_ms: u64,
}

impl Summary {
    pub fn snapshot(&self, audio_seconds: f64, attribution_enabled: bool) -> Snapshot {
        let availability = if self.end == 0 {
            Availability::WaitingForSpeech
        } else if !attribution_enabled {
            Availability::LabelsOff
        } else if self.unclear_share() > MAX_UNCLEAR_SHARE {
            Availability::UnclearSource
        } else {
            Availability::Available
        };
        let sides = (availability == Availability::Available).then(|| {
            let now = millis(audio_seconds).max(self.end);
            SideMetrics {
                me_seconds: seconds(self.me_ms),
                others_seconds: seconds(self.others_ms),
                me_share: self.me_ms as f64 / (self.me_ms as f64 + self.others_ms as f64),
                unclear_share: self.unclear_share(),
                pace_wpm: self.pace_wpm,
                recent_pace_wpm: self.recent_pace(now.saturating_sub(RECENT_WINDOW_MS)),
                me_questions: self.me_questions,
                others_questions: self.others_questions,
                current_monologue_seconds: self
                    .monologue
                    .filter(|&(_, end)| now - end <= MONOLOGUE_GAP_MS + FINALIZATION_LAG_MS)
                    .map_or(0.0, |(start, end)| seconds(end - start)),
                longest_monologue_seconds: seconds(self.longest_ms),
            }
        });
        Snapshot {
            availability,
            totals: self.totals.clone(),
            sides,
        }
    }

    fn unclear_share(&self) -> f64 {
        if self.speech_ms == 0 {
            return 0.0;
        }
        self.unclear_ms as f64 / self.speech_ms as f64
    }

    fn recent_pace(&self, cutoff: u64) -> Option<u32> {
        let mut words = 0.0;
        let mut ranges = Vec::new();
        for speech in self
            .me
            .iter()
            .rev()
            .take_while(|speech| speech.end > cutoff)
        {
            if !speech.pace_supported {
                return None;
            }
            let start = speech.start.max(cutoff);
            words += speech.words as f64 * (speech.end - start) as f64
                / (speech.end - speech.start) as f64;
            ranges.push((start, speech.end));
        }
        ranges.sort_unstable();
        let mut end = cutoff;
        let mut speech_ms = 0;
        for (start, next_end) in ranges {
            speech_ms += next_end.saturating_sub(start.max(end));
            end = end.max(next_end);
        }
        pace(words, speech_ms)
    }
}

fn pace(words: f64, speech_ms: u64) -> Option<u32> {
    (words > 0.0 && speech_ms > 0).then(|| (words * 60_000.0 / speech_ms as f64).round() as u32)
}

fn millis(seconds: f64) -> u64 {
    (seconds * 1_000.0).round() as u64
}

fn seconds(millis: u64) -> f64 {
    millis as f64 / 1_000.0
}

fn has_unspaced_script(ch: char) -> bool {
    ch.is_alphanumeric()
        && matches!(ch,
        '\u{0E00}'..='\u{0EFF}' | '\u{1000}'..='\u{109F}' | '\u{1780}'..='\u{17FF}' |
        '\u{3040}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' | '\u{3400}'..='\u{9FFF}' |
        '\u{F900}'..='\u{FAFF}' | '\u{FF66}'..='\u{FF9F}' | '\u{20000}'..='\u{323AF}')
}

/// Count runs of closing question punctuation. The Spanish opener does not
/// count, and ASCII semicolon is distinct from the Greek question mark.
fn question_count(text: &str) -> usize {
    let is_question = |ch| matches!(ch, '?' | '？' | '؟' | '\u{037E}');
    let mut chars = text.chars().peekable();
    let mut count = 0;
    while let Some(ch) = chars.next() {
        if is_question(ch) {
            while chars
                .peek()
                .is_some_and(|&next| is_question(next) || matches!(next, '!' | '！'))
            {
                chars.next();
            }
            let next_starts_word = chars
                .peek()
                .is_some_and(|&next| next.is_alphanumeric() && !has_unspaced_script(next));
            if ch != '?' || !next_starts_word {
                count += 1;
            }
        }
    }
    count
}

#[cfg(test)]
mod tests;
