//! Retrieve relevant excerpts across the whole meeting, keeping source ordering.
use super::context_budget::{chunks_by_bytes, prefix_bytes};
use std::collections::{BTreeSet, HashSet};

fn terms(text: &str) -> HashSet<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|word| {
            word.len() > 2
                && !matches!(
                    word.as_str(),
                    "the"
                        | "and"
                        | "was"
                        | "what"
                        | "when"
                        | "who"
                        | "this"
                        | "that"
                        | "about"
                        | "with"
                        | "for"
                        | "how"
                        | "die"
                        | "der"
                        | "das"
                        | "und"
                        | "wer"
                        | "wann"
                        | "ist"
                        | "mit"
                        | "den"
                        | "des"
                        | "eine"
                        | "einer"
                        | "meeting"
                )
        })
        .collect()
}

pub(crate) fn retrieve(rows: &[String], question: &str, budget: usize) -> String {
    if budget < 256 {
        return String::new();
    }
    let full_size = rows.iter().map(|row| row.len() + 1).sum::<usize>();
    if full_size <= budget {
        return rows.join("\n");
    }
    // Break oversized rows too, so a match in the middle of a single long row is searchable.
    let rows: Vec<String> = rows
        .iter()
        .flat_map(|row| chunks_by_bytes(row, (budget / 3).max(256)))
        .collect();
    let query = terms(question);
    let documents: Vec<_> = rows.iter().map(|row| terms(row)).collect();
    let weights: Vec<_> = query
        .iter()
        .map(|term| {
            let frequency = documents
                .iter()
                .filter(|document| document.contains(term))
                .count();
            (
                term,
                (1.0 + rows.len() as f64 / frequency.max(1) as f64).ln(),
            )
        })
        .collect();
    let mut scores: Vec<(usize, f64)> = documents
        .iter()
        .enumerate()
        .map(|(index, words)| {
            let score = weights
                .iter()
                .filter(|(term, _)| words.contains(*term))
                .map(|(_, weight)| weight)
                .sum::<f64>();
            (index, score)
        })
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut selected = BTreeSet::new();
    let mut remaining = budget - 192;
    let mut add = |index: usize| {
        if index < rows.len() && !selected.contains(&index) && rows[index].len() < remaining {
            remaining -= rows[index].len() + 1;
            selected.insert(index);
        }
    };
    // Relevant evidence first, then immediate neighbors for context.
    for (index, score) in &scores {
        if *score > 0.0 {
            add(*index);
        }
    }
    for (index, score) in &scores {
        if *score > 0.0 {
            if *index > 0 {
                add(index - 1);
            }
            add(index + 1);
        }
    }
    // Overview/follow-up questions still sample the entire timeline.
    for slot in 0..16 {
        add(slot * rows.len().saturating_sub(1) / 15);
    }
    let mut output = format!("Selected excerpts: {} of {} windows. Other parts were not supplied. Do not claim exhaustive coverage; say when the excerpts cannot answer the question.\n", selected.len(), rows.len());
    for index in selected {
        output.push_str(&rows[index]);
        output.push('\n');
    }
    prefix_bytes(&output, budget).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_decision_in_middle_of_long_meeting() {
        let mut rows = vec!["[00:00] Routine agenda discussion. ".repeat(20); 300];
        rows[150] = "[45:00] The Nebula migration is postponed until October. Nora owns it.".into();
        let context = retrieve(&rows, "What happened to the Nebula migration?", 4096);
        assert!(context.contains("postponed until October"));
        assert!(context.contains("[45:00]"));
        assert!(context.len() <= 4096);
    }
    #[test]
    fn multilingual_context_respects_byte_budget() {
        let rows = vec!["日本語 😀 Grüße Entscheidungen. ".repeat(100); 20];
        assert!(retrieve(&rows, "Entscheidungen?", 2048).len() <= 2048);
    }
}
