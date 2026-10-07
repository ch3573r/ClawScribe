//! Canonical UTF-8 spans, with distinct lexical-byte and semantic-token budgets.
use super::types::TextSpan;

pub const BODY_TOKENS: usize = 320;
pub const OVERLAP_TOKENS: usize = 48;
pub const LEXICAL_BYTES: usize = 2048;
pub const OVERLAP_BYTES: usize = 128;

pub fn lexical_spans(_id: &str, _text: &str) -> Vec<TextSpan> {
    Vec::new()
}

pub fn semantic_spans(
    _tokenizer: &tokenizers::Tokenizer,
    _id: &str,
    _text: &str,
) -> Result<Vec<TextSpan>, super::types::KnowledgeError> {
    Ok(Vec::new())
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
