use super::{evidence, EvidenceLocator, SelectedAnswer};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

const MAX_TAG: usize = 12;

fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}

fn label(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 1024
        && !value.contains("://")
        && !value.chars().any(char::is_control)
}

/// Validate the portable payload after the database has proved that the prompt
/// used live sources alone. Citation tags cannot establish that dependency proof.
pub(crate) fn is_portable_live_answer(id: &str, payload: &Value) -> bool {
    let Ok(note) = SelectedAnswer::deserialize(payload) else {
        return false;
    };
    let session_id = &note.context.session_id;
    let through = note.context.finalized_through_seconds;
    if !uuid(id)
        || note.id != id
        || !uuid(session_id)
        || !through.is_finite()
        || through < 0.
        || note.content.len() > 32 * 1024
        || !label(&note.provider)
        || !label(&note.model)
        || note.citations.len() > MAX_TAG
        || note.cited_tags.len() > MAX_TAG
        || note.question_tags.len() > MAX_TAG
        || note.cited_tags.windows(2).any(|pair| pair[0] >= pair[1])
        || note.question_tags.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return false;
    }
    // The native storage shape is the allowlist. Unknown members inside nested
    // context/evidence objects must never carry connection data into an archive.
    if !serde_json::to_value(&note).is_ok_and(|canonical| canonical == *payload) {
        return false;
    }
    let cited_in_text = evidence::tag_numbers(&note.content, MAX_TAG);
    if !note
        .cited_tags
        .iter()
        .all(|tag| cited_in_text.contains(tag))
    {
        return false;
    }
    let expected: BTreeSet<_> = note
        .cited_tags
        .iter()
        .chain(&note.question_tags)
        .copied()
        .collect();
    let mut verified = BTreeSet::new();
    let source_id = format!("live:{session_id}");
    for citation in note.citations {
        let (
            EvidenceLocator::Live {
                session_id: owner,
                sequence_ids,
            },
            Some(excerpt),
        ) = (&citation.locator.locator, citation.live_excerpt)
        else {
            return false;
        };
        let (Some(start), Some(end)) = (excerpt.start_seconds, excerpt.end_seconds) else {
            return false;
        };
        let locator = &citation.locator;
        if !(1..=MAX_TAG).contains(&citation.tag)
            || !verified.insert(citation.tag)
            || citation.attachment_meeting.is_some()
            || locator.historical
            || owner != session_id
            || sequence_ids.as_slice() != [excerpt.sequence_id]
            || locator.source_id != source_id
            || locator.source_revision != 1
            || locator.chunk_id != format!("{source_id}:{}", excerpt.sequence_id)
            || locator.fingerprint.len() != 64
            || !locator
                .fingerprint
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || excerpt.text.len() > 2048
            || !(0. <= start && start <= end && end <= through)
        {
            return false;
        }
    }
    verified == expected
}
