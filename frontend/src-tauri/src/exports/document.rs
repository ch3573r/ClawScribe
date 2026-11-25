//! Minimal DOCX generation for local and OneDrive/SharePoint exports.
//!
//! The writer intentionally supports only the structure ClawScribe summaries
//! need: headings, styled text, lists, tables, and plain transcript paragraphs.
//! It writes the small Open XML ZIP package directly.

use std::{
    fmt::Write as _,
    io::{Cursor, Write},
};

use zip::write::SimpleFileOptions;

enum DocBlock {
    Heading {
        level: u8,
        text: String,
    },
    ListItem {
        id: usize,
        level: usize,
        kind: ListKind,
        start: u32,
        text: String,
    },
    Paragraph(String),
    TranscriptLine(String),
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

const WORD_LIST_LEVELS: usize = 9;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Bullet,
    DecimalDot,
    DecimalParen,
}

impl ListKind {
    fn abstract_id(self) -> u8 {
        match self {
            Self::Bullet => 0,
            Self::DecimalDot => 1,
            Self::DecimalParen => 2,
        }
    }
}

struct ActiveList {
    indent: usize,
    kind: ListKind,
    id: usize,
}

const CONTENT_TYPES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>
</Types>"#;

const ROOT_RELS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

const DOCUMENT_RELS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>
</Relationships>"#;

fn build_numbering_xml(blocks: &[DocBlock]) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
    );
    for kind in [
        ListKind::Bullet,
        ListKind::DecimalDot,
        ListKind::DecimalParen,
    ] {
        let abstract_id = kind.abstract_id();
        let _ = write!(
            xml,
            r#"<w:abstractNum w:abstractNumId="{abstract_id}"><w:multiLevelType w:val="multilevel"/>"#
        );
        for level in 0..WORD_LIST_LEVELS {
            let number = level + 1;
            let left = 360 * (level + 2);
            let (format, label) = match kind {
                ListKind::Bullet => ("bullet", "&#8226;".to_owned()),
                ListKind::DecimalDot => ("decimal", format!("%{number}.")),
                ListKind::DecimalParen => ("decimal", format!("%{number})")),
            };
            let _ = write!(
                xml,
                r#"<w:lvl w:ilvl="{level}"><w:start w:val="1"/><w:numFmt w:val="{format}"/><w:lvlText w:val="{label}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{left}" w:hanging="360"/></w:pPr></w:lvl>"#
            );
        }
        xml.push_str("</w:abstractNum>");
    }
    xml.push_str(r#"<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#);
    let mut last_id = 1;
    for block in blocks {
        if let DocBlock::ListItem {
            id,
            level,
            kind,
            start,
            ..
        } = block
        {
            if *id > last_id {
                last_id = *id;
                let abstract_id = kind.abstract_id();
                let _ = write!(
                    xml,
                    r#"<w:num w:numId="{id}"><w:abstractNumId w:val="{abstract_id}"/><w:lvlOverride w:ilvl="{level}"><w:startOverride w:val="{start}"/></w:lvlOverride></w:num>"#
                );
            }
        }
    }
    xml.push_str("</w:numbering>");
    xml
}

pub fn build_meeting_docx(
    meeting_title: &str,
    summary_markdown: &str,
    transcript: Option<&str>,
) -> Result<Vec<u8>, String> {
    let title = if meeting_title.trim().is_empty() {
        "Meeting notes"
    } else {
        meeting_title.trim()
    };

    let mut blocks = Vec::new();
    blocks.push(DocBlock::Heading {
        level: 1,
        text: super::metadata::escape_inline(title),
    });
    blocks.extend(parse_markdown_blocks(
        &crate::summary::sources::strip_source_links(summary_markdown),
    ));

    if let Some(transcript) = transcript {
        if !transcript.trim().is_empty() {
            blocks.push(DocBlock::Heading {
                level: 1,
                text: "Transcript".to_string(),
            });
            blocks.extend(transcript_blocks(transcript));
        }
    }

    let document_xml = build_document_xml(&blocks);

    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    add_zip_file(
        &mut zip,
        "[Content_Types].xml",
        CONTENT_TYPES_XML.as_bytes(),
    )?;
    add_zip_file(&mut zip, "_rels/.rels", ROOT_RELS_XML.as_bytes())?;
    add_zip_file(&mut zip, "word/document.xml", document_xml.as_bytes())?;
    add_zip_file(
        &mut zip,
        "word/_rels/document.xml.rels",
        DOCUMENT_RELS_XML.as_bytes(),
    )?;
    add_zip_file(
        &mut zip,
        "word/numbering.xml",
        build_numbering_xml(&blocks).as_bytes(),
    )?;

    let cursor = zip
        .finish()
        .map_err(|e| format!("Failed to finalize DOCX package: {e}"))?;
    Ok(cursor.into_inner())
}

fn zip_options() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

fn add_zip_file(
    zip: &mut zip::ZipWriter<Cursor<Vec<u8>>>,
    path: &str,
    content: &[u8],
) -> Result<(), String> {
    zip.start_file(path, zip_options())
        .map_err(|e| format!("Failed to add DOCX part {path}: {e}"))?;
    zip.write_all(content)
        .map_err(|e| format!("Failed to write DOCX part {path}: {e}"))
}

fn parse_markdown_blocks(markdown: &str) -> Vec<DocBlock> {
    let mut blocks = Vec::new();
    let mut paragraph_lines = Vec::new();
    let mut next_list_id = 2;
    let mut lists: Vec<ActiveList> = Vec::new();

    let mut lines = markdown.lines().peekable();
    while let Some(raw_line) = lines.next() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            flush_paragraph(&mut blocks, &mut paragraph_lines);
            continue;
        }

        if trimmed.starts_with('|') {
            let header = split_table_row(trimmed);
            if lines
                .peek()
                .is_some_and(|line| is_table_delimiter(line, header.len()))
            {
                flush_paragraph(&mut blocks, &mut paragraph_lines);
                lines.next(); // Delimiter line.
                let mut rows = Vec::new();
                while lines
                    .peek()
                    .is_some_and(|line| line.trim().starts_with('|'))
                {
                    let mut row = split_table_row(lines.next().unwrap());
                    row.resize(header.len(), String::new());
                    rows.push(row);
                }
                lists.clear();
                blocks.push(DocBlock::Table { header, rows });
                continue;
            }
        }

        if let Some((level, text)) = parse_heading(trimmed) {
            flush_paragraph(&mut blocks, &mut paragraph_lines);
            lists.clear();
            blocks.push(DocBlock::Heading {
                level,
                text: text.to_owned(),
            });
            continue;
        }

        if let Some((kind, start, text)) = list_marker(trimmed) {
            flush_paragraph(&mut blocks, &mut paragraph_lines);
            let indent = raw_line
                .bytes()
                .take_while(u8::is_ascii_whitespace)
                .map(|byte| if byte == b'\t' { 4 } else { 1 })
                .sum();
            while lists.last().is_some_and(|list| {
                list.indent > indent || (list.indent == indent && list.kind != kind)
            }) {
                lists.pop();
            }
            if lists.last().is_none_or(|list| list.indent < indent) {
                let id = if kind == ListKind::Bullet {
                    1
                } else {
                    let id = next_list_id;
                    next_list_id += 1;
                    id
                };
                lists.push(ActiveList { indent, kind, id });
            }
            blocks.push(DocBlock::ListItem {
                id: lists.last().unwrap().id,
                level: (lists.len() - 1).min(WORD_LIST_LEVELS - 1),
                start,
                kind,
                text: text.to_owned(),
            });
            continue;
        }

        lists.clear();
        paragraph_lines.push(trimmed);
    }

    flush_paragraph(&mut blocks, &mut paragraph_lines);
    blocks
}

/// Split on unescaped pipes, retaining empty cells and unescaping literal pipes.
pub(super) fn split_table_row(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut backslashes = 0;
    let mut trailing_pipe = false;
    for ch in line.chars() {
        trailing_pipe = ch == '|' && backslashes % 2 == 0;
        if trailing_pipe {
            cells.push(cell.trim().to_string());
            cell.clear();
        } else {
            if ch == '|' {
                cell.pop(); // Remove the escape backslash.
            }
            cell.push(ch);
        }
        backslashes = if ch == '\\' { backslashes + 1 } else { 0 };
    }
    if !trailing_pipe || cells.is_empty() {
        cells.push(cell.trim().to_string());
    }
    cells
}

pub(super) fn is_table_delimiter(line: &str, columns: usize) -> bool {
    let cells = split_table_row(line);
    cells.len() == columns
        && cells.iter().all(|cell| {
            let dashes = cell.strip_prefix(':').unwrap_or(cell);
            let dashes = dashes.strip_suffix(':').unwrap_or(dashes);
            dashes.len() >= 3 && dashes.chars().all(|ch| ch == '-')
        })
}

fn transcript_blocks(transcript: &str) -> Vec<DocBlock> {
    transcript
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| DocBlock::TranscriptLine(line.to_string()))
        .collect()
}

fn flush_paragraph(blocks: &mut Vec<DocBlock>, paragraph_lines: &mut Vec<&str>) {
    if paragraph_lines.is_empty() {
        return;
    }
    let paragraph = paragraph_lines.join(" ");
    paragraph_lines.clear();
    if !paragraph.trim().is_empty() {
        blocks.push(DocBlock::Paragraph(paragraph));
    }
}

fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &line[level..];
    if !rest
        .chars()
        .next()
        .map(char::is_whitespace)
        .unwrap_or(false)
    {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim();
    if text.is_empty() {
        None
    } else {
        Some((level as u8, text))
    }
}

fn list_marker(line: &str) -> Option<(ListKind, u32, &str)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(text) = line.strip_prefix(marker) {
            return Some((ListKind::Bullet, 1, text.trim()));
        }
    }

    let bytes = line.as_bytes();
    let digit_count = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if !(1..=9).contains(&digit_count) || digit_count + 1 >= bytes.len() {
        return None;
    }
    let kind = match bytes[digit_count] {
        b'.' => ListKind::DecimalDot,
        b')' => ListKind::DecimalParen,
        _ => return None,
    };
    if !bytes[digit_count + 1].is_ascii_whitespace() {
        return None;
    }
    let start = line[..digit_count].parse().ok()?;
    Some((kind, start, line[digit_count + 2..].trim()))
}

fn build_document_xml(blocks: &[DocBlock]) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>"#,
    );

    for block in blocks {
        match block {
            DocBlock::Heading { level, text } => xml.push_str(&heading_xml(*level, text)),
            DocBlock::ListItem {
                id, level, text, ..
            } => xml.push_str(&list_xml(*id, *level, text)),
            DocBlock::Paragraph(text) => {
                xml.push_str(&paragraph_xml(&inline_xml(text, RunStyle::default())))
            }
            DocBlock::TranscriptLine(text) => {
                xml.push_str(&paragraph_xml(&run_xml(text, RunStyle::default())))
            }
            DocBlock::Table { header, rows } => xml.push_str(&table_xml(header, rows)),
        }
    }

    xml.push_str(
        r#"<w:sectPr>
      <w:pgSz w:w="12240" w:h="15840"/>
      <w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>
    </w:sectPr>
  </w:body>
</w:document>"#,
    );
    xml
}

fn heading_xml(level: u8, text: &str) -> String {
    let size = match level {
        1 => 32,
        2 => 28,
        3 => 24,
        _ => 22,
    };
    let outline_level = level.saturating_sub(1);
    let runs = inline_xml(
        text,
        RunStyle {
            bold: true,
            size: Some(size),
            ..RunStyle::default()
        },
    );
    format!(
        r#"<w:p><w:pPr><w:spacing w:before="240" w:after="120"/><w:outlineLvl w:val="{outline_level}"/></w:pPr>{runs}</w:p>"#,
    )
}

fn list_xml(id: usize, level: usize, text: &str) -> String {
    let runs = inline_xml(text, RunStyle::default());
    format!(
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="{id}"/></w:numPr><w:spacing w:after="80"/></w:pPr>{runs}</w:p>"#,
    )
}

fn paragraph_xml(runs: &str) -> String {
    format!(r#"<w:p><w:pPr><w:spacing w:after="120"/></w:pPr>{runs}</w:p>"#)
}

fn table_xml(header: &[String], rows: &[Vec<String>]) -> String {
    let mut xml =
        String::from(r#"<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="pct"/><w:tblBorders>"#);
    for edge in ["top", "left", "bottom", "right", "insideH", "insideV"] {
        xml.push_str(&format!(
            r#"<w:{edge} w:val="single" w:sz="4" w:color="auto"/>"#
        ));
    }
    xml.push_str("</w:tblBorders></w:tblPr><w:tblGrid>");
    let widths: Vec<usize> = (0..header.len())
        .map(|i| 9360 / header.len() + usize::from(i < 9360 % header.len()))
        .collect();
    for width in &widths {
        xml.push_str(&format!(r#"<w:gridCol w:w="{width}"/>"#));
    }
    xml.push_str("</w:tblGrid>");
    for (index, row) in std::iter::once(header)
        .chain(rows.iter().map(Vec::as_slice))
        .enumerate()
    {
        xml.push_str("<w:tr>");
        if index == 0 {
            xml.push_str("<w:trPr><w:tblHeader/></w:trPr>");
        }
        for (cell, width) in row.iter().zip(&widths) {
            let runs = inline_xml(
                cell,
                RunStyle {
                    bold: index == 0,
                    ..RunStyle::default()
                },
            );
            xml.push_str(&format!(
                r#"<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/></w:tcPr><w:p>{runs}</w:p></w:tc>"#,
            ));
        }
        xml.push_str("</w:tr>");
    }
    xml.push_str(r#"</w:tbl><w:p><w:pPr><w:spacing w:after="120"/></w:pPr></w:p>"#);
    xml
}

#[derive(Clone, Copy, Default)]
struct RunStyle {
    bold: bool,
    italic: bool,
    code: bool,
    size: Option<u8>,
}

fn run_xml(text: &str, style: RunStyle) -> String {
    let mut xml = String::from("<w:r>");
    if style.bold || style.italic || style.code || style.size.is_some() {
        xml.push_str("<w:rPr>");
        if style.code {
            xml.push_str(r#"<w:rFonts w:ascii="Consolas" w:hAnsi="Consolas"/>"#);
        }
        if style.bold {
            xml.push_str("<w:b/>");
        }
        if style.italic {
            xml.push_str("<w:i/>");
        }
        if let Some(size) = style.size {
            xml.push_str(&format!(r#"<w:sz w:val="{size}"/>"#));
        }
        xml.push_str("</w:rPr>");
    }
    xml.push_str(r#"<w:t xml:space="preserve">"#);
    xml.push_str(&escape_xml(text));
    xml.push_str("</w:t></w:r>");
    xml
}

fn inline_xml(text: &str, style: RunStyle) -> String {
    let mut xml = String::new();
    write_inline_runs(&mut xml, text, style, 0);
    if xml.is_empty() {
        xml = run_xml("", style);
    }
    xml
}

fn write_inline_runs(xml: &mut String, text: &str, style: RunStyle, depth: u8) {
    let mut plain = String::new();
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        if let Some(escaped) = escaped_punctuation(rest) {
            plain.push(escaped);
            index += 2;
            continue;
        }
        let marker = rest.chars().next().unwrap();
        if matches!(marker, '*' | '_' | CODE_MARKER) {
            let width = rest
                .bytes()
                .take_while(|byte| *byte == marker as u8)
                .count();
            let after = &rest[width..];
            let intraword = marker == '_'
                && text[..index].ends_with(char::is_alphanumeric)
                && after.starts_with(char::is_alphanumeric);
            let code = marker == CODE_MARKER;
            if depth < 12
                && (code || (width <= 3 && !intraword && !after.starts_with(char::is_whitespace)))
            {
                if let Some((content, remaining)) = closing_delimiter(after, marker, width) {
                    if !plain.is_empty() {
                        xml.push_str(&run_xml(&plain, style));
                        plain.clear();
                    }
                    if code {
                        let content = content
                            .strip_prefix(' ')
                            .and_then(|value| value.strip_suffix(' '))
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or(content);
                        xml.push_str(&run_xml(
                            content,
                            RunStyle {
                                code: true,
                                ..style
                            },
                        ));
                    } else {
                        let style = RunStyle {
                            bold: style.bold || width >= 2,
                            italic: style.italic || width % 2 == 1,
                            ..style
                        };
                        write_inline_runs(xml, content, style, depth + 1);
                    }
                    index = text.len() - remaining.len();
                    continue;
                }
            }
            plain.push_str(&rest[..width]);
            index += width;
        } else {
            plain.push(marker);
            index += marker.len_utf8();
        }
    }
    if !plain.is_empty() {
        xml.push_str(&run_xml(&plain, style));
    }
}

const CODE_MARKER: char = '\u{60}';

fn escaped_punctuation(text: &str) -> Option<char> {
    text.strip_prefix('\\')?
        .chars()
        .next()
        .filter(char::is_ascii_punctuation)
}

fn closing_delimiter(text: &str, marker: char, width: usize) -> Option<(&str, &str)> {
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        if marker != CODE_MARKER {
            if escaped_punctuation(rest).is_some() {
                index += 2;
                continue;
            }
            if rest.starts_with(CODE_MARKER) {
                let code_width = rest
                    .bytes()
                    .take_while(|byte| *byte == CODE_MARKER as u8)
                    .count();
                if let Some((_, remaining)) =
                    closing_delimiter(&rest[code_width..], CODE_MARKER, code_width)
                {
                    index = text.len() - remaining.len();
                    continue;
                }
                index += code_width;
                continue;
            }
        }
        let Some(ch) = rest.chars().next() else { break };
        if ch != marker {
            index += ch.len_utf8();
            continue;
        }
        let count = rest
            .bytes()
            .take_while(|byte| *byte == marker as u8)
            .count();
        let before = &text[..index];
        let remaining = &rest[count..];
        let closes = if marker == CODE_MARKER {
            count == width
        } else {
            (count == width || (count == 3 && width < count))
                && !before.ends_with(char::is_whitespace)
                && !(marker == '_'
                    && before.ends_with(char::is_alphanumeric)
                    && remaining.starts_with(char::is_alphanumeric))
        };
        if index > 0 && closes {
            return Some((&text[..index + count - width], remaining));
        }
        index += count;
    }
    None
}

fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            '\u{9}'
            | '\u{a}'
            | '\u{d}'
            | '\u{20}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'..='\u{10ffff}' => escaped.push(ch),
            _ => {}
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn docx_uses_formatted_runs_and_restarts_numbering() {
        let bytes = build_meeting_docx(
            "Notes",
            "## Heading\n\nNormal **bold &** *italic* `<code>` \\*literal\\*.\n\n3. **First**\n4. Second\n\n- Bullet\n\n1. Restart\n\n| Value |\n| --- |\n| *cell* |",
            Some("[00:01] **Literal** \u{1}"),
        )
        .unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut part = |name: &str| {
            let mut xml = String::new();
            archive
                .by_name(name)
                .unwrap()
                .read_to_string(&mut xml)
                .unwrap();
            xml
        };
        let document = part("word/document.xml");
        let numbering = part("word/numbering.xml");
        assert!(document.contains(r#"<w:b/></w:rPr><w:t xml:space="preserve">bold &amp;"#));
        assert!(document.contains(r#"<w:i/></w:rPr><w:t xml:space="preserve">italic"#));
        assert!(document.contains(r#"<w:rFonts w:ascii="Consolas" w:hAnsi="Consolas"/>"#));
        assert!(document.contains("&lt;code&gt;"));
        assert!(document.contains("*literal*"));
        assert!(document.contains("**Literal**"));
        assert!(!document.contains('\u{1}'));
        assert_eq!(document.matches(r#"<w:numId w:val="1"/>"#).count(), 1);
        assert_eq!(document.matches(r#"<w:numId w:val="2"/>"#).count(), 2);
        assert_eq!(document.matches(r#"<w:numId w:val="3"/>"#).count(), 1);
        assert!(numbering.contains(r#"<w:numFmt w:val="decimal"/>"#));
        assert!(numbering.contains(r#"<w:startOverride w:val="3"/>"#));
        assert!(numbering.contains(r#"<w:startOverride w:val="1"/>"#));
        assert!(document.contains(r#"<w:i/></w:rPr><w:t xml:space="preserve">cell"#));
        for xml in [document, numbering] {
            let mut reader = quick_xml::Reader::from_str(&xml);
            while reader.read_event().unwrap() != quick_xml::events::Event::Eof {}
        }
    }

    #[test]
    fn nested_lists_keep_ordered_starts_levels_and_parent_continuation() {
        let blocks = parse_markdown_blocks(
            "3. Parent\n   - Bullet detail\n   7) Ordered detail\n   8) More detail\n4. Next parent",
        );
        let document = build_document_xml(&blocks);
        let numbering = build_numbering_xml(&blocks);
        assert_eq!(
            document
                .matches(r#"<w:ilvl w:val="0"/><w:numId w:val="2"/>"#)
                .count(),
            2
        );
        assert_eq!(
            document
                .matches(r#"<w:ilvl w:val="1"/><w:numId w:val="1"/>"#)
                .count(),
            1
        );
        assert_eq!(
            document
                .matches(r#"<w:ilvl w:val="1"/><w:numId w:val="3"/>"#)
                .count(),
            2
        );
        assert!(numbering.contains(r#"<w:num w:numId="3"><w:abstractNumId w:val="2"/><w:lvlOverride w:ilvl="1"><w:startOverride w:val="7"/>"#));
        assert!(numbering.contains(r#"<w:lvlText w:val="%2)"/>"#));
    }

    #[test]
    fn adjacent_docx_tables_are_separated_by_an_empty_paragraph() {
        let blocks =
            parse_markdown_blocks("| First |\n| --- |\n| One |\n\n| Second |\n| --- |\n| Two |");
        let xml = build_document_xml(&blocks);
        assert_eq!(xml.matches("<w:tbl>").count(), 2);
        assert!(
            xml.contains(r#"</w:tbl><w:p><w:pPr><w:spacing w:after="120"/></w:pPr></w:p><w:tbl>"#)
        );
    }

    #[test]
    fn final_docx_table_has_an_empty_paragraph_before_section_properties() {
        let blocks = parse_markdown_blocks("# Notes\n\n| Task |\n| --- |\n| Review |");
        let xml = build_document_xml(&blocks);
        let after_table = xml.rsplit_once("</w:tbl>").unwrap().1;
        assert!(after_table
            .starts_with(r#"<w:p><w:pPr><w:spacing w:after="120"/></w:pPr></w:p><w:sectPr>"#));
    }

    #[test]
    fn docx_renders_a_table_with_escaped_cells_and_repeatable_header() {
        let bytes = build_meeting_docx(
            "Notes",
            "| **Owner** | Task |\n| :--- | ---: |\n| Ana | Ship & learn |\n| Ben | <review> |",
            None,
        )
        .unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert_eq!(xml.matches("<w:tbl>").count(), 1);
        assert_eq!(xml.matches("<w:tr>").count(), 3);
        assert_eq!(xml.matches("<w:tblHeader/>").count(), 1);
        assert_eq!(xml.matches("<w:tc>").count(), 6);
        assert!(xml.contains("Ship &amp; learn"));
        assert!(xml.contains("&lt;review&gt;"));
        assert!(!xml.contains('|'));
        assert!(!xml.contains("**"));
        assert_eq!(xml.matches(r#"<w:gridCol w:w="4680"/>"#).count(), 2);
        assert!(xml.contains(r#"<w:tblW w:w="5000" w:type="pct"/>"#));
        let table = xml
            .split("<w:tbl>")
            .nth(1)
            .unwrap()
            .split("</w:tbl>")
            .next()
            .unwrap();
        assert_eq!(table.matches("<w:p>").count(), 6);
        assert_eq!(table.matches("<w:b/>").count(), 2);
        assert!(!xml.contains("<w:t>"));
    }

    #[test]
    fn table_rows_pad_truncate_and_keep_escaped_pipes_in_one_cell() {
        let blocks = parse_markdown_blocks(
            "| Name | Note |\n| --- | :---: |\n| Ana |\n| Ben | left \\| right | extra |",
        );
        let DocBlock::Table { header, rows } = &blocks[0] else {
            panic!("expected table")
        };
        assert_eq!(header, &["Name", "Note"]);
        assert_eq!(rows[0], ["Ana", ""]);
        assert_eq!(rows[1], ["Ben", "left | right"]);
        assert!(table_xml(header, rows).contains("left | right"));
        assert_eq!(split_table_row(r"| a \\| b |"), [r"a \\", "b"]);
        assert_eq!(split_table_row(r"| a \|"), ["a |"]);
    }

    #[test]
    fn table_grid_widths_sum_to_text_width_and_text_preserves_spaces() {
        let header = vec![" heading ".to_string(); 7];
        let xml = table_xml(&header, &[vec![" cell ".to_string(); 7]]);
        assert_eq!(xml.matches(r#"<w:gridCol w:w="1338"/>"#).count(), 1);
        assert_eq!(xml.matches(r#"<w:gridCol w:w="1337"/>"#).count(), 6);
        for xml in [
            xml,
            heading_xml(1, " heading "),
            list_xml(1, 0, " bullet "),
            paragraph_xml(&run_xml(" paragraph ", RunStyle::default())),
        ] {
            assert!(xml.contains("xml:space=\"preserve\"> "));
            assert!(!xml.contains("<w:t>"));
        }
    }

    #[test]
    fn pipe_lines_without_a_valid_delimiter_stay_paragraphs() {
        for markdown in [
            "| not a table |",
            "| Name |\n| -- |",
            "| Name |\n| --- | --- |",
        ] {
            let blocks = parse_markdown_blocks(markdown);
            assert!(matches!(&blocks[0], DocBlock::Paragraph(text) if text.starts_with("|")));
            assert!(!build_document_xml(&blocks).contains("<w:tbl>"));
        }
    }

    #[test]
    fn docx_keeps_citation_time_without_internal_link() {
        let bytes = build_meeting_docx(
            "Notes",
            "Send draft [00:12:34](#clawscribe-source-abc123)",
            None,
        )
        .unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(xml.contains("(00:12:34)"));
        assert!(!xml.contains("clawscribe-source"));
    }

    #[test]
    fn docx_contains_expected_text_and_files() {
        let bytes = build_meeting_docx(
            "Weekly Sync",
            "# Decisions\n- Ship & learn\n\nPlain **summary** text.",
            Some("[00:01] Alice: <approved>"),
        )
        .unwrap();

        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert!(archive.by_name("[Content_Types].xml").is_ok());
        assert!(archive.by_name("_rels/.rels").is_ok());
        assert!(archive.by_name("word/_rels/document.xml.rels").is_ok());
        assert!(archive.by_name("word/numbering.xml").is_ok());

        let mut document_xml = String::new();
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut document_xml)
            .unwrap();

        assert!(document_xml.contains("Weekly Sync"));
        assert!(document_xml.contains("Decisions"));
        assert!(document_xml.contains("Ship &amp; learn"));
        assert!(document_xml.contains(r#"preserve">Plain </w:t>"#));
        assert!(document_xml.contains(r#"<w:b/></w:rPr><w:t xml:space="preserve">summary</w:t>"#));
        assert!(document_xml.contains("Transcript"));
        assert!(document_xml.contains("&lt;approved&gt;"));
    }

    #[test]
    fn markdown_blocks_support_headings_bullets_and_paragraphs() {
        let blocks =
            parse_markdown_blocks("## Next steps\n1. First\n- Second\nA paragraph\ncontinues");
        assert_eq!(blocks.len(), 4);
        assert!(matches!(blocks[0], DocBlock::Heading { level: 2, .. }));
        assert!(matches!(
            blocks[1],
            DocBlock::ListItem {
                kind: ListKind::DecimalDot,
                ..
            }
        ));
        assert!(matches!(
            blocks[2],
            DocBlock::ListItem {
                kind: ListKind::Bullet,
                ..
            }
        ));
        assert!(matches!(blocks[3], DocBlock::Paragraph(_)));
    }
}
