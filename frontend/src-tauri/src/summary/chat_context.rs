//! Bounded Unicode-safe source excerpts. The limit is characters, NOT tokens.
//! Keep the beginning/end without making a second full-transcript allocation.
use std::collections::VecDeque;

pub struct ContextExcerpt {
    pub text: String,
    pub truncated: bool,
    pub source_chars: usize,
}

pub struct ContextBuilder {
    head: String,
    tail: VecDeque<char>,
    head_limit: usize,
    tail_limit: usize,
    source_chars: usize,
}

impl ContextBuilder {
    pub fn new(limit: usize) -> Self {
        let head_limit = limit / 3 * 2;
        Self { head: String::new(), tail: VecDeque::new(), head_limit,
            tail_limit: limit - head_limit, source_chars: 0 }
    }
    pub fn push_str(&mut self, source: &str) {
        for character in source.chars() {
            if self.source_chars < self.head_limit {
                self.head.push(character);
            } else if self.tail_limit > 0 {
                if self.tail.len() == self.tail_limit { self.tail.pop_front(); }
                self.tail.push_back(character);
            }
            self.source_chars = self.source_chars.saturating_add(1);
        }
    }
    pub fn finish(self) -> ContextExcerpt {
        let truncated = self.source_chars > self.head_limit + self.tail_limit;
        let mut text = self.head;
        if truncated { text.push_str("\n[Middle of source omitted for length]\n"); }
        text.extend(self.tail);
        ContextExcerpt { text, truncated, source_chars: self.source_chars }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_unicode_is_not_duplicated_or_marked_truncated() {
        let mut builder = ContextBuilder::new(12);
        builder.push_str("😀日本語äöü");
        let result = builder.finish();
        assert_eq!(result.text, "😀日本語äöü");
        assert_eq!(result.source_chars, 7);
        assert!(!result.truncated);
    }
    #[test]
    fn exactly_at_limit_retains_every_character() {
        for limit in 0..30 {
            let source = "😀".repeat(limit);
            let mut builder = ContextBuilder::new(limit);
            builder.push_str(&source);
            let result = builder.finish();
            assert!(!result.truncated);
            assert_eq!(result.text, source);
        }
    }
    #[test]
    fn long_source_is_bounded_and_has_explicit_coverage() {
        let mut builder = ContextBuilder::new(9);
        builder.push_str("Opening-");
        for _ in 0..10_000 { builder.push_str("中"); }
        builder.push_str("END");
        let result = builder.finish();
        assert!(result.truncated);
        assert!(result.text.starts_with("Openin"));
        assert!(result.text.ends_with("END"));
        assert_eq!(result.source_chars, 10_011);
        assert_eq!(result.text.chars().count(), 9 + "\n[Middle of source omitted for length]\n".len());
    }
    #[test]
    fn streaming_and_single_input_have_identical_results() {
        let parts = ["Er kommt. ", "日本語 😀", "Final decision."];
        let mut streamed = ContextBuilder::new(15);
        for part in parts { streamed.push_str(part); }
        let mut whole = ContextBuilder::new(15);
        whole.push_str(&parts.concat());
        assert_eq!(streamed.finish().text, whole.finish().text);
    }
}
