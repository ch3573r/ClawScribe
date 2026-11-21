// Custom-word matching adapted from Handy (copyright cjpais), MIT licensed:
// https://github.com/cjpais/handy/blob/main/src-tauri/src/audio_toolkit/text.rs

pub const DEFAULT_THRESHOLD: f64 = 0.18;

pub fn apply_custom_words(text: &str, words: &[String], threshold: f64) -> String {
    Vocabulary::new(words.to_vec()).correct(text, threshold).0
}

pub(super) fn normalize_custom_words(words: Vec<String>) -> Result<Vec<String>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut normalized = Vec::new();
    for word in words {
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        if word.chars().count() > 64 {
            return Err("Each vocabulary entry must be at most 64 characters.".into());
        }
        if word.chars().any(char::is_control) {
            return Err(
                "Vocabulary entries cannot contain control characters or line breaks.".into(),
            );
        }
        if !word.chars().any(char::is_alphanumeric) {
            return Err("Vocabulary entries must contain at least one letter or number.".into());
        }
        if seen.insert(word.to_lowercase()) {
            normalized.push(word.to_string());
            if normalized.len() > 500 {
                return Err("Custom vocabulary can contain at most 500 entries.".into());
            }
        }
    }
    Ok(normalized)
}

struct CustomWordMatchKey {
    word_index: usize,
    key: String,
    chars: Vec<char>,
    soundex: Option<[u8; 4]>,
}

/// Token ranges refer to the original whitespace-delimited transcript words.
pub(super) struct Replacement {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Compiled once per recording or batch, including the empty-list fast path.
pub(super) struct Vocabulary {
    words: Vec<String>,
    keys: Vec<CustomWordMatchKey>,
    listed_keys: std::collections::HashSet<String>,
    max_key_len: usize,
}

impl Vocabulary {
    pub fn new(words: Vec<String>) -> Self {
        let keys: Vec<CustomWordMatchKey> = words
            .iter()
            .enumerate()
            .filter_map(|(index, word)| {
                let key = build_match_key(word);
                let chars: Vec<char> = key.chars().collect();
                // Single-character keys lose distinguishing punctuation (C#/C++).
                (chars.len() > 1).then(|| CustomWordMatchKey {
                    word_index: index,
                    soundex: supports_soundex(&key).then(|| soundex(&key)),
                    key,
                    chars,
                })
            })
            .collect();
        let listed_keys = words.iter().map(|word| build_match_key(word)).collect();
        let max_key_len = keys.iter().map(|key| key.chars.len()).max().unwrap_or(0);
        Self {
            words,
            keys,
            listed_keys,
            max_key_len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn correct(&self, text: &str, threshold: f64) -> (String, Vec<Replacement>) {
        if self.is_empty() {
            return (text.to_string(), Vec::new());
        }
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut replacements = Vec::new();
        let mut output = String::with_capacity(text.len());
        let mut cursor = 0;
        let mut i = 0;
        while i < words.len() {
            let mut best_match: Option<(usize, &String, f64, bool)> = None;
            for n in (1..=3).rev() {
                if i + n > words.len() {
                    continue;
                }
                let ngram_words = &words[i..i + n];
                // Never swallow a sentence boundary or opening punctuation.
                if ngram_words[..n - 1]
                    .iter()
                    .any(|w| !split_token(w).2.is_empty())
                    || ngram_words[1..]
                        .iter()
                        .any(|w| !extract_punctuation(w).0.is_empty())
                {
                    continue;
                }
                if let Some((replacement, score, plural)) = find_best_match(
                    &build_ngram(ngram_words),
                    &self.words,
                    &self.keys,
                    &self.listed_keys,
                    self.max_key_len,
                    threshold,
                ) {
                    if best_match
                        .as_ref()
                        .map_or(true, |(_, _, best_score, _)| score < *best_score)
                    {
                        best_match = Some((n, replacement, score, plural));
                    }
                }
            }
            if let Some((n, replacement, _, plural)) = best_match {
                let (prefix, _) = extract_punctuation(words[i]);
                let (_, last_stem, suffix) = split_token(words[i + n - 1]);
                // An entry that is itself a possessive ("McDonald's") already
                // carries the ending the transcript word had.
                let suffix = if ends_with_possessive(replacement) {
                    strip_leading_possessive(suffix)
                } else {
                    suffix
                };
                let plural_suffix = if plural {
                    &last_stem[last_stem.len() - last_stem.chars().last().unwrap().len_utf8()..]
                } else {
                    ""
                };
                let corrected = with_punctuation(
                    prefix,
                    &format!(
                        "{}{}",
                        preserve_case_pattern(words[i], replacement),
                        plural_suffix
                    ),
                    suffix,
                );
                // These are slices of text, so offsets are valid UTF-8 boundaries.
                let start = words[i].as_ptr() as usize - text.as_ptr() as usize;
                let end = words[i + n - 1].as_ptr() as usize - text.as_ptr() as usize
                    + words[i + n - 1].len();
                output.push_str(&text[cursor..start]);
                output.push_str(&corrected);
                if corrected != text[start..end] {
                    replacements.push(Replacement {
                        start: i,
                        end: i + n,
                        text: corrected,
                    });
                }
                cursor = end;
                i += n;
            } else {
                i += 1;
            }
        }
        output.push_str(&text[cursor..]);
        (output, replacements)
    }
}

fn build_ngram(words: &[&str]) -> String {
    words
        .iter()
        .map(|w| build_match_key(split_token(w).1))
        .collect::<String>()
}

// Keep possessives together with the original trailing punctuation. All offsets
// come from character boundaries or strip_suffix on a complete UTF-8 string.
fn split_token(word: &str) -> (&str, &str, &str) {
    let (prefix, suffix) = extract_punctuation(word);
    let end = word.len() - suffix.len();
    let core = &word[prefix.len()..end];
    let stem = core
        .strip_suffix("'s")
        .or_else(|| core.strip_suffix("’s"))
        .or_else(|| core.strip_suffix("'S"))
        .or_else(|| core.strip_suffix("’S"))
        .unwrap_or(core);
    (prefix, stem, &word[prefix.len() + stem.len()..])
}

const POSSESSIVES: [&str; 4] = ["'s", "’s", "'S", "’S"];

fn ends_with_possessive(word: &str) -> bool {
    let (_, trailing) = extract_punctuation(word);
    let core = &word[..word.len() - trailing.len()];
    POSSESSIVES.iter().any(|p| core.ends_with(p))
}

fn strip_leading_possessive(suffix: &str) -> &str {
    POSSESSIVES
        .iter()
        .find_map(|p| suffix.strip_prefix(p))
        .unwrap_or(suffix)
}

fn build_match_key(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn supports_soundex(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|c| c.is_ascii_alphabetic())
}

fn find_best_match<'a>(
    candidate: &str,
    custom_words: &'a [String],
    keys: &[CustomWordMatchKey],
    listed_keys: &std::collections::HashSet<String>,
    max_key_len: usize,
    threshold: f64,
) -> Option<(&'a String, f64, bool)> {
    let candidate_chars: Vec<char> = candidate.chars().collect();
    // The shared length prefilter permits up to 25% of the longer key,
    // including insertions accepted by the long-key phonetic rule.
    let max_candidate_len = max_key_len + (max_key_len / 3).max(2);
    if candidate_chars.is_empty() || candidate_chars.len() > max_candidate_len {
        return None;
    }
    let mut best_match = None;
    let mut best_score = f64::MAX;
    let candidate_soundex = supports_soundex(candidate).then(|| soundex(candidate));
    for word_key in keys {
        if listed_keys.contains(candidate) && candidate != word_key.key {
            continue;
        }
        let key_len = word_key.chars.len();
        let plural = candidate.strip_suffix('s') == Some(word_key.key.as_str());
        let stem = if plural {
            &candidate_chars[..candidate_chars.len() - 1]
        } else {
            &candidate_chars[..]
        };
        let max_len = stem.len().max(key_len) as f64;
        if stem.len().abs_diff(key_len) as f64 > (max_len * 0.25).max(2.0) {
            continue;
        }
        let distance = character_distance(stem, &word_key.chars);
        let mut score = distance as f64 / max_len;
        let accepted = match key_len {
            0 | 1 => false,
            2..=4 => distance == 0,
            5..=7 => {
                distance <= 1 && stem.first() == word_key.chars.first()
                    // A final-letter substitution changes Maria into Mario.
                    && (stem.len() != key_len || stem.last() == word_key.chars.last())
            }
            _ => {
                if candidate_soundex.is_some() && candidate_soundex == word_key.soundex {
                    score *= 0.3;
                }
                stem.len() >= 6 && score < threshold
            }
        };
        if accepted && score < best_score {
            best_match = Some((&custom_words[word_key.word_index], plural));
            best_score = score;
        }
    }
    best_match.map(|(word, plural)| (word, best_score, plural))
}

#[cfg(test)]
fn levenshtein(left: &str, right: &str) -> usize {
    character_distance(
        &left.chars().collect::<Vec<_>>(),
        &right.chars().collect::<Vec<_>>(),
    )
}

// One row bounds memory by the entry's character count, never its UTF-8 bytes.
fn character_distance(left: &[char], right: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=right.len()).collect();
    for (i, a) in left.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, b) in right.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(a != b))
                .min(row[j] + 1)
                .min(above + 1);
            diagonal = above;
        }
    }
    row[right.len()]
}

fn soundex(word: &str) -> [u8; 4] {
    fn code(c: u8) -> u8 {
        match c.to_ascii_uppercase() {
            b'B' | b'F' | b'P' | b'V' => b'1',
            b'C' | b'G' | b'J' | b'K' | b'Q' | b'S' | b'X' | b'Z' => b'2',
            b'D' | b'T' => b'3',
            b'L' => b'4',
            b'M' | b'N' => b'5',
            b'R' => b'6',
            _ => b'0',
        }
    }
    let mut bytes = word.bytes();
    let Some(first) = bytes.next() else {
        return [b'0'; 4];
    };
    let mut result = [first.to_ascii_uppercase(), b'0', b'0', b'0'];
    let mut previous = code(first);
    let mut index = 1;
    for c in bytes {
        if matches!(c.to_ascii_uppercase(), b'H' | b'W') {
            continue;
        }
        let digit = code(c);
        if digit != b'0' && digit != previous {
            result[index] = digit;
            index += 1;
            if index == 4 {
                break;
            }
        }
        previous = digit;
    }
    result
}

fn preserve_case_pattern(original: &str, replacement: &str) -> String {
    let letters: Vec<char> = original.chars().filter(|c| c.is_alphabetic()).collect();
    if !letters.is_empty() && letters.iter().all(|c| c.is_uppercase()) {
        replacement.to_uppercase()
    } else if letters.first().is_some_and(|c| c.is_uppercase()) {
        match replacement.char_indices().find(|(_, c)| c.is_alphabetic()) {
            Some((index, first)) => format!(
                "{}{}{}",
                &replacement[..index],
                first.to_uppercase(),
                &replacement[index + first.len_utf8()..]
            ),
            None => replacement.to_string(),
        }
    } else {
        replacement.to_string()
    }
}

fn with_punctuation(prefix: &str, replacement: &str, suffix: &str) -> String {
    let (replacement_prefix, replacement_suffix) = extract_punctuation(replacement);
    // Edge punctuation can be part of the listed spelling (.NET, C++). Join
    // overlapping marks once while retaining surrounding quotes/ellipsis.
    let prefix_overlap = replacement_prefix
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .filter(|&end| prefix.ends_with(&replacement_prefix[..end]))
        .last()
        .unwrap_or(0);
    let suffix_overlap = suffix
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .take_while(|&end| end <= replacement_suffix.len())
        .filter(|&end| replacement_suffix.ends_with(&suffix[..end]))
        .last()
        .unwrap_or(0);
    format!(
        "{}{}{}",
        &prefix[..prefix.len() - prefix_overlap],
        replacement,
        &suffix[suffix_overlap..]
    )
}

fn extract_punctuation(word: &str) -> (&str, &str) {
    let start = word
        .char_indices()
        .find(|(_, c)| c.is_alphanumeric())
        .map(|(i, _)| i)
        .unwrap_or(word.len());
    let end = word
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(word.len());
    (&word[..start], &word[end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn correct(text: &str, words: &[&str]) -> String {
        apply_custom_words(
            text,
            &words.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            DEFAULT_THRESHOLD,
        )
    }

    #[test]
    fn exact_fuzzy_and_phonetic_matches_use_listed_spelling() {
        assert_eq!(correct("chargebee", &["ChargeBee"]), "ChargeBee");
        assert_eq!(correct("Kubernets", &["Kubernetes"]), "Kubernetes");
        assert_eq!(correct("Rupert", &["Robert"]), "Rupert");
    }

    #[test]
    fn shipped_rules_reject_reported_false_replacements() {
        for (candidate, entry) in [
            ("a", "AI"),
            ("as", "AWS"),
            ("not", ".NET"),
            ("need", ".NET"),
            ("read", "R&D"),
            ("red", "R&D"),
            ("some", "Sam"),
            ("same", "Sam"),
            ("time", "Tim"),
            ("team", "Tim"),
            ("and", "Andy"),
            ("us", "USA"),
            ("make", "Mike"),
            ("Maria", "Mario"),
            ("Mario", "Maria"),
            ("Julia", "Julio"),
            ("Julio", "Julia"),
            ("Rupert", "Robert"),
            ("plan C", "C#"),
        ] {
            assert_eq!(correct(candidate, &[entry]), candidate);
        }
    }

    #[test]
    fn listed_keys_are_protected_from_other_entries_in_any_order() {
        for entries in [["Maria", "Mario"], ["Mario", "Maria"]] {
            let vocabulary = Vocabulary::new(entries.iter().map(|s| s.to_string()).collect());
            let (text, edits) = vocabulary.correct("Maria Mario", DEFAULT_THRESHOLD);
            assert_eq!(text, "Maria Mario");
            assert!(edits.is_empty());
        }
        assert_eq!(correct("Robert", &["Roberts", "Robert"]), "Robert");
    }

    #[test]
    fn possessive_entries_do_not_double_their_ending() {
        let words = vec!["McDonald's".to_string(), "Denny’s".to_string()];
        for text in [
            "We met at McDonald's yesterday.",
            "We met at mcdonald's, then left.",
            "Lunch at Denny’s was fine.",
            "Lunch at Denny's was fine.",
        ] {
            let corrected = apply_custom_words(text, &words, DEFAULT_THRESHOLD);
            assert!(
                !corrected.contains("s's") && !corrected.contains("s’s"),
                "{corrected}"
            );
            assert!(
                !corrected.contains("’s's") && !corrected.contains("'s’s"),
                "{corrected}"
            );
        }
        assert_eq!(
            apply_custom_words("at mcdonald's today", &words, DEFAULT_THRESHOLD),
            "at McDonald's today"
        );
    }

    #[test]
    fn possessive_and_plural_suffixes_survive_corrections() {
        assert_eq!(
            correct(
                "robert's robert’s chargebee's chargebees",
                &["Robert", "ChargeBee"]
            ),
            "Robert's Robert’s ChargeBee's ChargeBees"
        );
        assert_eq!(
            correct("(ChargeBees’), chargebees's!", &["ChargeBee"]),
            "(ChargeBees’), ChargeBees's!"
        );
        assert_eq!(correct("Charge Bee’s,", &["ChargeBee"]), "ChargeBee’s,");
        assert_eq!(
            correct("ROBERT'S CHARGEBEE’S", &["Robert", "ChargeBee"]),
            "ROBERT'S CHARGEBEE’S"
        );
        let vocabulary = Vocabulary::new(vec!["Robert".into(), "ChargeBee".into()]);
        let (text, edits) = vocabulary.correct(
            "Robert's Robert’s ChargeBee's ChargeBees",
            DEFAULT_THRESHOLD,
        );
        assert_eq!(text, "Robert's Robert’s ChargeBee's ChargeBees");
        assert!(edits.is_empty());
    }

    #[test]
    fn unicode_keys_use_character_lengths_and_distances() {
        assert_eq!(correct("müller Muller", &["Müller"]), "Müller Müller");
        assert_eq!(correct("Mueller", &["Müller"]), "Mueller");
        assert_eq!(correct("größe Grose", &["Größe"]), "Größe Grose");
        assert_eq!(
            correct("große größe’s größes", &["Größe"]),
            "Größe Größe’s Größes"
        );
        assert_eq!(correct("zoë Zoe", &["Zoë"]), "Zoë Zoe");
        assert_eq!(correct("são paulo", &["São Paulo"]), "São Paulo");
        assert_eq!(levenshtein("Muller", "Müller"), 1);
        assert_eq!(levenshtein("größe", "grose"), 2);
    }

    #[test]
    fn medium_keys_allow_one_edit_except_first_and_final_substitutions() {
        assert_eq!(
            correct("robrt robbert robart", &["Robert"]),
            "Robert Robert Robert"
        );
        assert_eq!(correct("xobert robera", &["Robert"]), "xobert robera");
        assert_eq!(correct("maria mario", &["Maria", "Mario"]), "Maria Mario");
        // An exact listed key also wins over a longer entry's phonetic score.
        assert_eq!(correct("Ashcraft", &["Ashcroft", "Ashcraft"]), "Ashcraft");
    }

    #[test]
    fn joins_use_single_word_rules_and_enforce_long_key_candidate_floor() {
        assert_eq!(correct("Charge B", &["ChargeBee"]), "ChargeBee");
        assert_eq!(correct("Charge Bee", &["ChargeBee"]), "ChargeBee");
        assert_eq!(correct("Charge Be", &["ChargeBee"]), "ChargeBee");
        assert_eq!(correct("Ashrf", &["Ashcraft"]), "Ashrf");
        assert_eq!(correct("Ashcrf", &["Ashcraft"]), "Ashcraft");
        assert_eq!(correct("Muller", &["Müller"]), "Müller");
        assert_eq!(correct("Ruller", &["Müller"]), "Ruller");
    }

    #[test]
    fn medium_keys_allow_one_edit_except_a_final_character_substitution() {
        for (candidate, entry, expected) in [
            ("maria", "Maria", "Maria"),
            ("Mria", "Maria", "Maria"),
            ("Marria", "Maria", "Maria"),
            ("Marix", "Maria", "Marix"),
            ("Marixa", "Maria", "Maria"),
            ("Marja", "Maria", "Maria"),
            ("Xaria", "Maria", "Xaria"),
            ("Muler", "Müller", "Muler"),
            ("Muller", "Müller", "Müller"),
            ("Mülle", "Müller", "Müller"),
            ("Müllex", "Müller", "Müllex"),
            ("Müllers", "Müller", "Müllers"),
            ("Nemotro", "Nemotron", "Nemotron"),
            ("Mullers", "Müllers", "Müllers"),
            ("Müllert", "Müllers", "Müllert"),
        ] {
            assert_eq!(correct(candidate, &[entry]), expected);
        }
    }

    #[test]
    fn joined_and_single_keys_share_acceptance_rules() {
        for (candidate, entry, expected) in [
            ("A I", "AI", "AI"),
            ("t im", "Tim", "Tim"),
            ("Ti me", "Tim", "Ti me"),
            ("Ma ria", "Maria", "Maria"),
            ("Ma rio", "Maria", "Ma rio"),
            ("Mul ler", "Müller", "Müller"),
            ("Char ge B", "ChargeBee", "ChargeBee"),
            ("Ashcraffft", "Ashcraft", "Ashcraft"),
            ("Ash craffft", "Ashcraft", "Ashcraft"),
            ("Ash rf", "Ashcraft", "Ash rf"),
            ("Ash crf", "Ashcraft", "Ashcraft"),
            ("Charge (B)", "ChargeBee", "Charge (B)"),
            ("Charge [Bee]", "ChargeBee", "Charge [Bee]"),
            ("Charge. B", "ChargeBee", "Charge. B"),
        ] {
            assert_eq!(correct(candidate, &[entry]), expected);
        }
    }

    #[test]
    fn long_keys_use_the_strict_shipped_threshold_without_phonetic_boost() {
        assert_eq!(correct("xycdefghijk", &["Abcdefghijk"]), "xycdefghijk");
        assert_eq!(correct("xycdefghijkl", &["Abcdefghijkl"]), "Abcdefghijkl");
        let entry = "a".repeat(50);
        let at_threshold = format!("{}{}", "b".repeat(9), "a".repeat(41));
        let below_threshold = format!("{}{}", "b".repeat(8), "a".repeat(42));
        assert_eq!(correct(&at_threshold, &[&entry]), at_threshold);
        assert_eq!(correct(&below_threshold, &[&entry]), entry);
    }

    #[test]
    fn exact_spelling_is_not_counted_as_an_edit_and_single_keys_are_disabled() {
        let vocabulary = Vocabulary::new(vec!["ChargeBee".into(), "MacBook Pro".into()]);
        let (text, edits) = vocabulary.correct("ChargeBee MacBook Pro", DEFAULT_THRESHOLD);
        assert_eq!(text, "ChargeBee MacBook Pro");
        assert!(edits.is_empty());
        assert_eq!(
            correct("c c++ C# plan C", &["C++", "C#"]),
            "c c++ C# plan C"
        );
    }

    #[test]
    fn joins_words_and_preserves_punctuation_and_case() {
        assert_eq!(
            correct("Use (Charge Bee), please.", &["ChargeBee"]),
            "Use (ChargeBee), please."
        );
        assert_eq!(correct("CHARGE BEE,", &["ChargeBee"]), "CHARGEBEE,");
        assert_eq!(correct("「HANDEE。」", &["Handy"]), "「HANDEE。」");
    }

    #[test]
    fn punctuation_in_listed_terms_is_not_duplicated() {
        assert_eq!(correct("C++", &["C++"]), "C++");
        assert_eq!(correct("Use c++.", &["C++"]), "Use c++.");
        assert_eq!(correct("(.net),", &[".NET"]), "(.NET),");
        assert_eq!(correct("yahoo!", &["Yahoo!"]), "Yahoo!");
        assert_eq!(correct("\"C++\"!", &["C++"]), "\"C++\"!");
        assert_eq!(correct("...NET...", &[".NET"]), "...NET...");
        // Punctuation is restored only for an equal short key, never a near miss.
        assert_eq!(correct("Net", &[".Net"]), ".Net");
        assert_eq!(correct("need not", &[".Net"]), "need not");
    }

    #[test]
    fn empty_and_unicode_inputs_are_preserved_byte_for_byte() {
        let text = "  Grüße\tüber\nMüller und äußere Größe.  ";
        assert_eq!(correct(text, &[]), text);
        assert_eq!(correct(text, &["Kubernetes", "Müller", "你好"]), text);
        assert_eq!(correct("你好。", &["你号"]), "你好。");
        assert_eq!(correct("Munchen", &["München"]), "München");
    }

    #[test]
    fn unrelated_words_and_sentence_boundaries_are_preserved() {
        assert_eq!(
            correct("We met. Charge Bee works.", &["ChargeBee"]),
            "We met. ChargeBee works."
        );
        assert_eq!(correct("Charge. B", &["ChargeBee"]), "Charge. B");
        assert_eq!(
            correct("  hello\tworld\n", &["Kubernetes"]),
            "  hello\tworld\n"
        );
    }

    #[test]
    fn setting_normalizes_and_deduplicates_without_losing_first_spelling() {
        let normalized = normalize_custom_words(vec![
            " ChargeBee ".into(),
            "".into(),
            "  ".into(),
            "chargebee".into(),
            "Müller".into(),
            "MÜLLER".into(),
        ])
        .unwrap();
        assert_eq!(normalized, vec!["ChargeBee", "Müller"]);
        assert!(normalize_custom_words(vec!["a".repeat(64)]).is_ok());
        assert!(normalize_custom_words(vec!["ü".repeat(64)]).is_ok());
        assert!(normalize_custom_words(vec!["a".repeat(65)]).is_err());
        assert!(normalize_custom_words(vec!["two\nlines".into()]).is_err());
        assert!(normalize_custom_words(vec!["null\0byte".into()]).is_err());
        assert_eq!(
            normalize_custom_words(vec!["+++".into()]).unwrap_err(),
            "Vocabulary entries must contain at least one letter or number."
        );
    }

    #[test]
    fn setting_caps_unique_entries_without_truncating() {
        let mut words = (0..500).map(|i| format!("word{i}")).collect::<Vec<_>>();
        assert_eq!(normalize_custom_words(words.clone()).unwrap().len(), 500);
        words.push("WORD0".into());
        assert_eq!(normalize_custom_words(words.clone()).unwrap().len(), 500);
        words.push("overflow".into());
        assert!(normalize_custom_words(words).is_err());
    }

    #[test]
    fn edit_plan_uses_original_word_positions() {
        let vocabulary = Vocabulary::new(vec!["ChargeBee".into(), "MacBook Pro".into()]);
        let (text, edits) = vocabulary.correct("Use Charge Bee and Macbookpro.", DEFAULT_THRESHOLD);
        assert_eq!(text, "Use ChargeBee and MacBook Pro.");
        assert_eq!(edits.len(), 2);
        assert_eq!(
            (edits[0].start, edits[0].end, edits[0].text.as_str()),
            (1, 3, "ChargeBee")
        );
        assert_eq!(
            (edits[1].start, edits[1].end, edits[1].text.as_str()),
            (4, 5, "MacBook Pro.")
        );
    }

    #[test]
    fn local_distance_and_soundex_cover_edits_and_duplicate_consonants() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(soundex("Robert"), *b"R163");
        assert_eq!(soundex("Rupert"), *b"R163");
        assert_eq!(soundex("Ashcraft"), *b"A261");
        assert_eq!(soundex("Tymczak"), *b"T522");
        assert_eq!(soundex("Pfister"), *b"P236");
    }

    #[test]
    fn test_apply_custom_words_exact_match() {
        let text = "hello world";
        let custom_words = vec!["Hello".to_string(), "World".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_apply_custom_words_fuzzy_match() {
        let text = "helo wrold";
        let custom_words = vec!["hello".to_string(), "world".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert_eq!(result, "hello wrold");
    }

    #[test]
    fn test_preserve_case_pattern() {
        assert_eq!(preserve_case_pattern("HELLO", "world"), "WORLD");
        assert_eq!(preserve_case_pattern("Hello", "world"), "World");
        assert_eq!(preserve_case_pattern("hello", "WORLD"), "WORLD");
    }

    #[test]
    fn test_extract_punctuation() {
        assert_eq!(extract_punctuation("hello"), ("", ""));
        assert_eq!(extract_punctuation("!hello?"), ("!", "?"));
        assert_eq!(extract_punctuation("...hello..."), ("...", "..."));
    }

    #[test]
    fn test_extract_punctuation_uses_unicode_boundaries() {
        assert_eq!(extract_punctuation("你好。"), ("", "。"));
        assert_eq!(extract_punctuation("「你好」"), ("「", "」"));
        assert_eq!(extract_punctuation("你好！"), ("", "！"));
    }

    #[test]
    fn test_empty_custom_words() {
        let text = "hello world";
        let custom_words = vec![];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_apply_custom_words_ngram_two_words() {
        let text = "il cui nome è Charge Bee, che permette";
        let custom_words = vec!["ChargeBee".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert!(result.contains("ChargeBee,"), "unexpected result: {result}");
        assert!(!result.contains("Charge Bee"));
    }

    #[test]
    fn test_apply_custom_words_ngram_three_words() {
        let text = "use Chat G PT for this";
        let custom_words = vec!["ChatGPT".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert!(result.contains("ChatGPT"));
    }

    #[test]
    fn test_apply_custom_words_prefers_longer_ngram() {
        let text = "Open AI GPT model";
        let custom_words = vec!["OpenAI".to_string(), "GPT".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert_eq!(result, "OpenAI GPT model");
    }

    #[test]
    fn test_apply_custom_words_ngram_preserves_case() {
        let text = "CHARGE BEE is great";
        let custom_words = vec!["ChargeBee".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert!(result.contains("CHARGEBEE"));
    }

    #[test]
    fn test_apply_custom_words_ngram_with_spaces_in_custom() {
        // Custom word with space should also match against split words
        let text = "using Mac Book Pro";
        let custom_words = vec!["MacBook Pro".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert_eq!(result, "using MacBook Pro");
    }

    #[test]
    fn test_apply_custom_words_trailing_number_not_doubled() {
        // Verify that trailing non-alpha chars (like numbers) aren't double-counted
        // between build_ngram stripping them and extract_punctuation capturing them
        let text = "use GPT4 for this";
        let custom_words = vec!["GPT-4".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        // Should NOT produce "GPT-44" (double-counting the trailing 4)
        assert!(
            !result.contains("GPT-44"),
            "got double-counted result: {}",
            result
        );
    }

    #[test]
    fn test_apply_custom_words_matches_ampersand_word() {
        let text = "send it to RD for review";
        let custom_words = vec!["R&D".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert_eq!(result, "send it to R&D for review");
    }

    #[test]
    fn test_apply_custom_words_does_not_expand_short_ampersand_key() {
        let text = "send it to R and D for review";
        let custom_words = vec!["R&D".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert_eq!(result, text);
    }

    #[test]
    fn test_apply_custom_words_preserves_ampersand_word() {
        let text = "send it to R&D for review";
        let custom_words = vec!["R&D".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert_eq!(result, "send it to R&D for review");
    }

    #[test]
    fn test_apply_custom_words_handles_unicode_punctuation() {
        let text = "「Handee。」";
        let custom_words = vec!["Handy".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert_eq!(result, text);
    }

    #[test]
    fn test_apply_custom_words_skips_cjk_fuzzy_matching() {
        let text = "你好。";
        let custom_words = vec!["你号".to_string()];
        let result = apply_custom_words(text, &custom_words, DEFAULT_THRESHOLD);
        assert_eq!(result, text);
    }
}
