//! Dependency-free transcript chunking; also tested directly with rustc in CI.

pub(crate) fn summary_chunk_budget(token_threshold: usize) -> Result<usize, String> {
    token_threshold
        .checked_sub(300)
        .filter(|budget| *budget > 0)
        .ok_or_else(|| {
            "Summary token threshold must exceed the 300-token prompt reserve.".to_string()
        })
}

/// Rough token count estimation using character count
pub fn rough_token_count(s: &str) -> usize {
    let char_count = s.chars().count();
    (char_count as f64 * 0.35).ceil() as usize
}

/// Split on UTF-8 boundaries without skipping source text at sentence boundaries.
/// Token sizes are estimates, not a substitute for provider-specific tokenization.
/// Overlap is capped at half a window to bound redundant work on small budgets.
/// Only the current window is scanned; no full character copy or prefix sums are needed.
pub fn chunk_text(text: &str, chunk_size_tokens: usize, overlap_tokens: usize) -> Vec<String> {
    if text.is_empty() || chunk_size_tokens == 0 {
        return vec![];
    }

    let chunk_size_chars = ((chunk_size_tokens as f64 / 0.35).floor() as usize).max(1);
    let overlap_chars = ((overlap_tokens as f64 / 0.35).floor() as usize)
        .min(chunk_size_chars / 2);
    let mut chunks = Vec::new();
    let mut start_byte = 0;

    while start_byte < text.len() {
        let remaining = &text[start_byte..];
        let mut end_offset = remaining
            .char_indices()
            .nth(chunk_size_chars)
            .map_or(remaining.len(), |(offset, _)| offset);

        if end_offset < remaining.len() {
            let window = &remaining[..end_offset];
            // Avoid tiny chunks when an early sentence ends near the start.
            let minimum_end = end_offset / 2;
            let sentence_end = window
                .rfind(". ")
                .map(|offset| offset + 2)
                .filter(|end| *end >= minimum_end);
            let word_end = || {
                window.rfind(char::is_whitespace).map(|offset| {
                    offset + window[offset..].chars().next().unwrap().len_utf8()
                }).filter(|end| *end >= minimum_end)
            };
            if let Some(boundary) = sentence_end.or_else(word_end) {
                end_offset = boundary;
            }
        }

        let chunk = &remaining[..end_offset];
        chunks.push(chunk.to_string());
        if end_offset == remaining.len() {
            break;
        }

        // Advance from the ACTUAL emitted end, not the original window end.
        // Otherwise shortening a chunk at a sentence boundary silently loses text.
        let next_offset = if overlap_chars == 0 {
            end_offset
        } else {
            chunk.char_indices().rev().nth(overlap_chars - 1)
                .map_or(end_offset, |(offset, _)| offset)
        };
        // Unusual short/Unicode windows must still make forward progress.
        start_byte += if next_offset > 0 { next_offset } else { end_offset };
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunking_does_not_skip_text_after_sentence_boundaries() {
        let text = format!("Opening. {}", (0..80).map(|i| format!("word{i:03} ")).collect::<String>());
        assert_eq!(chunk_text(&text, 20, 0).concat(), text);
    }

    #[test]
    fn chunking_preserves_unicode_and_whitespace_without_overlap() {
        for text in ["Grüße. 日本語の会議 😀\n", "e\u{301}\tNein. Да! ", "長い単語に空白はありません", "\r\n \t"] {
            let text = text.repeat(30);
            for budget in [1, 2, 5, 20, 100] {
                let chunks = chunk_text(&text, budget, 0);
                assert_eq!(chunks.concat(), text);
                assert!(chunks.iter().all(|chunk| !chunk.is_empty()));
                assert!(chunks.iter().all(|chunk| rough_token_count(chunk) <= budget));
            }
        }
    }

    #[test]
    fn overlapping_chunks_retain_every_numbered_fact() {
        let text = (0..200).map(|i| format!("Decision{i:03}. ")).collect::<String>();
        let chunks = chunk_text(&text, 30, 5);
        for i in 0..200 {
            assert!(chunks.iter().any(|chunk| chunk.contains(&format!("Decision{i:03}."))));
        }
    }

    #[test]
    fn chunking_caps_excessive_overlap_and_handles_extreme_budgets() {
        let text = "abcdefghij".repeat(100);
        let chunks = chunk_text(&text, 5, usize::MAX);
        assert!(chunks.len() < 200);
        assert!(chunks.iter().all(|chunk| !chunk.is_empty()));
        assert_eq!(chunk_text(&text, usize::MAX, usize::MAX), vec![text]);
        assert!(chunk_text("text", 0, 10).is_empty());
        assert!(chunk_text("", 10, 0).is_empty());
    }

    #[test]
    fn invalid_summary_budget_is_an_error_not_an_underflow() {
        for threshold in [0, 1, 299, 300] {
            assert!(summary_chunk_budget(threshold).is_err());
        }
        assert_eq!(summary_chunk_budget(301).unwrap(), 1);
        assert_eq!(summary_chunk_budget(4000).unwrap(), 3700);
    }

}
