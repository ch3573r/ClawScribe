//! Canonical UTF-8 spans, with distinct lexical-byte and semantic-token budgets.
use super::types::TextSpan;

pub const BODY_TOKENS: usize = 320;
pub const OVERLAP_TOKENS: usize = 48;
pub const LEXICAL_BYTES: usize = 2048;
pub const OVERLAP_BYTES: usize = 128;

pub fn floor_boundary(text: &str, mut end: usize) -> usize {
    end = end.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}
pub fn lexical_spans(id: &str, text: &str) -> Vec<TextSpan> {
    let mut result = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = floor_boundary(text, start + LEXICAL_BYTES);
        result.push(TextSpan {
            transcript_id: id.into(),
            start_byte: start,
            end_byte: end,
        });
        if end == text.len() {
            break;
        }
        let mut next = end.saturating_sub(OVERLAP_BYTES);
        while !text.is_char_boundary(next) {
            next += 1;
        }
        start = next.max(start + 1);
    }
    result
}

pub fn semantic_spans(
    tokenizer: &tokenizers::Tokenizer,
    id: &str,
    text: &str,
) -> Result<Vec<TextSpan>, super::types::KnowledgeError> {
    use super::types::KnowledgeError;
    if text.len() > 16 * 1024 {
        return Err(KnowledgeError::InvalidInput);
    }
    let mut result = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let encoded = tokenizer
            .encode(&text[start..], false)
            .map_err(|_| KnowledgeError::InvalidInput)?;
        if encoded.is_empty() {
            break;
        }
        let mut end = if encoded.len() <= BODY_TOKENS {
            text.len()
        } else {
            start + encoded.get_offsets()[BODY_TOKENS - 1].1
        };
        end = floor_boundary(text, end);
        // Retokenize the canonical slice: SentencePiece boundary normalization
        // can change token count when a long row becomes a standalone passage.
        let body = loop {
            if end <= start {
                return Err(KnowledgeError::InvalidInput);
            }
            let body = tokenizer
                .encode(&text[start..end], false)
                .map_err(|_| KnowledgeError::InvalidInput)?;
            if body.len() <= BODY_TOKENS {
                break body;
            }
            end = floor_boundary(text, end - 1);
        };
        let total = tokenizer
            .encode(format!("passage: {}", &text[start..end]), true)
            .map_err(|_| KnowledgeError::InvalidInput)?;
        if total.len() > 512 {
            return Err(KnowledgeError::InvalidInput);
        }
        result.push(TextSpan {
            transcript_id: id.into(),
            start_byte: start,
            end_byte: end,
        });
        if end == text.len() {
            break;
        }
        let mut next = start + body.get_offsets()[body.len().saturating_sub(OVERLAP_TOKENS)].0;
        while !text.is_char_boundary(next) {
            next += 1;
        }
        if next <= start || next >= end {
            return Err(KnowledgeError::InvalidInput);
        }
        start = next;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_body_budget_and_overlap_exclude_prefix_and_special_tokens() {
        let mut tokenizer = tokenizers::Tokenizer::new(
            tokenizers::models::wordlevel::WordLevel::builder()
                .vocab(
                    [("word".into(), 0), ("[UNK]".into(), 1)]
                        .into_iter()
                        .collect(),
                )
                .unk_token("[UNK]".into())
                .build()
                .unwrap(),
        );
        tokenizer.with_pre_tokenizer(Some(
            tokenizers::pre_tokenizers::whitespace::WhitespaceSplit,
        ));
        let text = vec!["word"; 700].join(" ");
        let spans = semantic_spans(&tokenizer, "row", &text).unwrap();
        assert_eq!(
            spans.len(),
            3,
            "320-token bodies must advance by 272 tokens"
        );
        assert_eq!(spans[0].start_byte, 0);
        assert_eq!(spans[0].end_byte, 1599);
        assert_eq!(spans[1].start_byte, 1360);
        assert_eq!(spans[1].end_byte, 2959);
        assert_eq!(spans[2].start_byte, 2720);
        assert_eq!(spans[2].end_byte, 3499);
        for span in spans {
            assert!(
                tokenizer
                    .encode(&text[span.start_byte..span.end_byte], false)
                    .unwrap()
                    .len()
                    <= 320
            );
        }
    }
    #[test]
    fn unicode_chunk_spans_roundtrip() {
        let text = "Äpfel e\u{301} 🙂 Release ATLAS-42. ".repeat(200);
        let spans = lexical_spans("row", &text);
        assert!(
            !spans.is_empty(),
            "keyword evidence must exist without a tokenizer"
        );
        assert_eq!(spans[0].start_byte, 0);
        assert_eq!(spans.last().unwrap().end_byte, text.len());
        for span in &spans {
            assert_eq!(span.transcript_id, "row");
            assert!(span.end_byte - span.start_byte <= 2048);
            assert!(text.get(span.start_byte..span.end_byte).is_some());
        }
        for pair in spans.windows(2) {
            assert!(pair[1].start_byte > pair[0].start_byte);
            assert!(pair[1].start_byte <= pair[0].end_byte);
            assert!(pair[0].end_byte - pair[1].start_byte <= 128);
        }
    }
}
