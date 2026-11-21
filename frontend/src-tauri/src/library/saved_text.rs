use crate::summary::processor::validate_saved_summary;
use serde_json::Value;

pub(crate) const MAX_SAVED_TEXT_BYTES: usize = 512 * 1024;

// BlockNote occasionally saves the editable blocks without a Markdown representation.
// Read only visible text and children. Links, document IDs, styles and storage paths are private.
pub(crate) fn block_text(node: &Value, output: &mut String) {
    match node {
        Value::Array(nodes) => nodes.iter().for_each(|n| block_text(n, output)),
        Value::String(text) => output.push_str(text),
        Value::Object(fields) => {
            if let Some(text) = fields.get("text").and_then(Value::as_str) {
                output.push_str(text);
            } else if let Some(content) = fields.get("content") {
                block_text(content, output);
            } else if let Some(rows) = fields.get("rows").and_then(Value::as_array) {
                for row in rows {
                    if let Some(cells) = row.get("cells").and_then(Value::as_array) {
                        for (index, cell) in cells.iter().enumerate() {
                            if index != 0 {
                                output.push('\t');
                            }
                            block_text(cell, output);
                        }
                        output.push('\n');
                    }
                }
            }
            if fields.contains_key("children") || fields.contains_key("blocks") {
                output.push('\n');
            }
            if let Some(children) = fields.get("children") {
                block_text(children, output);
            }
        }
        _ => {}
    }
}

pub(crate) fn editor_text(blocks: &Value) -> Option<String> {
    let mut text = String::new();
    let nodes = blocks
        .as_array()
        .or_else(|| blocks.get("blocks")?.as_array())?;
    for node in nodes {
        block_text(node, &mut text);
    }
    Some(text.trim_end().to_owned())
}

/// Meeting memory already permits completed Markdown overviews. When both
/// representations exist, only use a passage that the current editor displays.
pub(crate) fn summary_overview(raw: &str, limit: usize) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    validate_saved_summary(&value).ok()?;
    let markdown = value.get("markdown")?.as_str()?;
    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = normalize(&markdown_text(markdown))
        .chars()
        .take(limit)
        .collect();
    if text.trim().is_empty() {
        return None;
    }
    if let Some(blocks) = value.get("summary_json").filter(|v| v.is_array()) {
        let visible = normalize(&editor_text(blocks)?);
        let (_, current) = visible.match_indices(text.trim()).next()?;
        return Some(current.to_owned());
    }
    Some(text)
}

/// Search the representation used by the editor. An old Markdown export
/// alongside edited BlockNote blocks must never introduce a second body.
pub(crate) fn summary_text(raw: &str) -> Option<String> {
    let mut value: Value = serde_json::from_str(raw).ok()?;
    if let Some(encoded) = value.as_str() {
        value = serde_json::from_str(encoded).ok()?;
    }
    validate_saved_summary(&value).ok()?;
    if let Some(blocks) = value.get("summary_json").filter(|blocks| blocks.is_array()) {
        return editor_text(blocks);
    }
    if let Some(markdown) = value
        .get("markdown")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Some(markdown_text(markdown));
    }
    let mut out = String::new();
    let mut section = |fallback: &str, node: &Value| {
        if let Some(blocks) = node.get("blocks").and_then(Value::as_array) {
            out.push_str(
                node.get("title")
                    .and_then(Value::as_str)
                    .unwrap_or(fallback),
            );
            out.push('\n');
            for block in blocks {
                if let Some(text) = block.get("content").and_then(Value::as_str) {
                    out.push_str(text);
                    out.push('\n');
                }
            }
        }
    };
    if let Some(sections) = value
        .pointer("/MeetingNotes/sections")
        .and_then(Value::as_array)
    {
        for (i, node) in sections.iter().enumerate() {
            section(&format!("section-{}", i + 1), node);
        }
    } else if let Some(order) = value.get("_section_order").and_then(Value::as_array) {
        for key in order.iter().filter_map(Value::as_str) {
            section(key, &value[key]);
        }
    } else {
        for (key, node) in value.as_object()? {
            section(key, node);
        }
    }
    Some(out.trim_end().to_owned())
}

/// Keep visible labels. Link destinations, reference definitions and HTML
/// comments can contain identifiers that never appear in the editor.
pub(crate) fn markdown_text(markdown: &str) -> String {
    static HIDDEN: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"(?ims)<!--.*?(?:-->|\z)|<script\b[^>]*>.*?(?:</script\s*>|\z)|<style\b[^>]*>.*?(?:</style\s*>|\z)|<template\b[^>]*>.*?(?:</template\s*>|\z)|^[ \t]{0,3}\[[^\]\n]+\]:[ \t]*(?:\r?\n[ \t]*)?[^\n]*(?:\n[ \t]+[^\n]*)*|</?[a-z][\w-]*(?:\s+[^>]*)?/?>").unwrap()
    });
    static FORMATTING: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"(?m)^[ ]{0,3}(?:#{1,6}|[-+*>])[ \t]+|\*\*([^*\n]+)\*\*|__([^_\n]+)__|~~([^~\n]+)~~|`([^`\n]+)`").unwrap()
    });
    let input = HIDDEN.replace_all(markdown, "");
    let input = FORMATTING.replace_all(&input, "$1$2$3$4");
    let mut chars = input.chars().peekable();
    let mut body = Vec::new();
    let mut brackets = Vec::new();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek().is_some_and(char::is_ascii_punctuation) {
            body.push(chars.next());
            continue;
        }
        if ch == '[' {
            brackets.push(body.len());
        }
        let opening = if ch == ']' { brackets.pop() } else { None };
        let Some(label_start) = opening.filter(|_| matches!(chars.peek(), Some('(' | '['))) else {
            body.push(Some(ch));
            continue;
        };
        body[label_start] = None;
        let open = chars.next().unwrap();
        let close = if open == '(' { ')' } else { ']' };
        let mut depth = 1usize;
        while let Some(c) = chars.next() {
            if c == '\\' {
                chars.next();
            } else if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
        }
    }
    body.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn matching_editor_paragraphs_supply_a_current_overview() {
        let stored = serde_json::json!({
            "markdown":"First paragraph.\n\nSecond paragraph.",
            "summary_json":[
                {"type":"paragraph","content":[{"type":"text","text":"First paragraph."}],"children":[]},
                {"type":"paragraph","content":[{"type":"text","text":"Second paragraph."}],"children":[]}
            ]
        });
        assert_eq!(
            super::summary_overview(&stored.to_string(), 100).as_deref(),
            Some("First paragraph. Second paragraph.")
        );
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn current_editor_and_legacy_sections_exclude_provider_metadata() {
        let saved = json!({"MeetingNotes":{"sections":[{"title":"Änderungen","blocks":[{"content":"Planung"}]}]},
            "codex":{"processing_log_path":"unshared"},"summary_sources":[{"text":"citation only"}]});
        assert_eq!(
            summary_text(&saved.to_string()).as_deref(),
            Some("Änderungen\nPlanung")
        );
        assert_eq!(
            summary_text(&json!(saved.to_string()).to_string()).as_deref(),
            Some("Änderungen\nPlanung")
        );
        assert!(summary_overview(&saved.to_string(), 100).is_none());
        let hidden = json!({"markdown":"Hidden alternate","summary_json":[{"content":[{"text":"Edited decision"}]}]});
        assert_eq!(
            summary_text(&hidden.to_string()).as_deref(),
            Some("Edited decision")
        );
        assert!(summary_overview(&hidden.to_string(), 100).is_none());
        let rejected = json!({"markdown":"Visible <think>private reasoning</think>"});
        assert!(summary_text(&rejected.to_string()).is_none());
        assert!(summary_overview(&rejected.to_string(), 100).is_none());
    }

    #[test]
    fn markdown_keeps_visible_labels_and_literal_punctuation() {
        let saved = json!({"markdown":"Öffnung 50%_ [Label](private(nested)) <!-- private comment -->\n[ref]: private-destination\n<style>private-style</style>"});
        let text = summary_text(&saved.to_string()).unwrap();
        assert!(text.contains("Öffnung 50%_"));
        assert!(text.contains("Label"));
        assert!(!text.contains("private"));
        assert_eq!(
            summary_overview(&saved.to_string(), 4).as_deref(),
            Some("Öffn")
        );
    }
}
