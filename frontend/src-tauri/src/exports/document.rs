//! Minimal DOCX generation for OneDrive/SharePoint file export.
//!
//! The writer intentionally supports only the structure ClawScribe summaries
//! need: headings, bullet list items, tables, and plain paragraphs. It avoids adding a
//! document-generation dependency by creating the small Open XML ZIP package
//! directly.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;

enum DocBlock {
    Heading {
        level: u8,
        text: String,
    },
    Bullet(String),
    Paragraph(String),
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
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

const NUMBERING_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:abstractNum w:abstractNumId="0">
    <w:lvl w:ilvl="0">
      <w:start w:val="1"/>
      <w:numFmt w:val="bullet"/>
      <w:lvlText w:val="&#8226;"/>
      <w:lvlJc w:val="left"/>
      <w:pPr>
        <w:ind w:left="720" w:hanging="360"/>
      </w:pPr>
    </w:lvl>
  </w:abstractNum>
  <w:num w:numId="1">
    <w:abstractNumId w:val="0"/>
  </w:num>
</w:numbering>"#;

pub fn build_meeting_docx(
    meeting_title: &str,
    summary_markdown: &str,
    transcript: Option<&str>,
) -> Result<Vec<u8>, String> {
    let title = clean_inline_markdown(meeting_title);
    let title = if title.is_empty() {
        "Meeting notes".to_string()
    } else {
        title
    };

    let mut blocks = Vec::new();
    blocks.push(DocBlock::Heading {
        level: 1,
        text: title,
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
    add_zip_file(&mut zip, "word/numbering.xml", NUMBERING_XML.as_bytes())?;

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
    let mut paragraph_lines: Vec<String> = Vec::new();

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
                    rows.push(row.iter().map(|cell| clean_inline_markdown(cell)).collect());
                }
                blocks.push(DocBlock::Table {
                    header: header
                        .iter()
                        .map(|cell| clean_inline_markdown(cell))
                        .collect(),
                    rows,
                });
                continue;
            }
        }

        if let Some((level, text)) = parse_heading(trimmed) {
            flush_paragraph(&mut blocks, &mut paragraph_lines);
            blocks.push(DocBlock::Heading {
                level,
                text: clean_inline_markdown(text),
            });
            continue;
        }

        if let Some(text) = strip_bullet_marker(trimmed) {
            flush_paragraph(&mut blocks, &mut paragraph_lines);
            blocks.push(DocBlock::Bullet(clean_inline_markdown(text)));
            continue;
        }

        paragraph_lines.push(clean_inline_markdown(trimmed));
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
        .map(|line| DocBlock::Paragraph(line.to_string()))
        .collect()
}

fn flush_paragraph(blocks: &mut Vec<DocBlock>, paragraph_lines: &mut Vec<String>) {
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

fn strip_bullet_marker(line: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest.trim());
        }
    }

    let bytes = line.as_bytes();
    let digit_count = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if digit_count == 0 || digit_count + 1 >= bytes.len() {
        return None;
    }
    let marker = bytes[digit_count];
    if (marker == b'.' || marker == b')') && bytes[digit_count + 1].is_ascii_whitespace() {
        return Some(line[digit_count + 2..].trim());
    }
    None
}

fn clean_inline_markdown(text: &str) -> String {
    text.trim()
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
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
            DocBlock::Bullet(text) => xml.push_str(&bullet_xml(text)),
            DocBlock::Paragraph(text) => xml.push_str(&paragraph_xml(text)),
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
    format!(
        r#"<w:p>
      <w:pPr>
        <w:outlineLvl w:val="{outline_level}"/>
        <w:spacing w:before="240" w:after="120"/>
      </w:pPr>
      <w:r>
        <w:rPr><w:b/><w:sz w:val="{size}"/></w:rPr>
        <w:t xml:space="preserve">{}</w:t>
      </w:r>
    </w:p>"#,
        escape_xml(text)
    )
}

fn bullet_xml(text: &str) -> String {
    format!(
        r#"<w:p>
      <w:pPr>
        <w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr>
        <w:spacing w:after="80"/>
      </w:pPr>
      <w:r><w:t xml:space="preserve">{}</w:t></w:r>
    </w:p>"#,
        escape_xml(text)
    )
}

fn paragraph_xml(text: &str) -> String {
    format!(
        r#"<w:p>
      <w:pPr><w:spacing w:after="120"/></w:pPr>
      <w:r><w:t xml:space="preserve">{}</w:t></w:r>
    </w:p>"#,
        escape_xml(text)
    )
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
            let bold = if index == 0 {
                "<w:rPr><w:b/></w:rPr>"
            } else {
                ""
            };
            xml.push_str(&format!(
                r#"<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/></w:tcPr><w:p><w:r>{bold}<w:t xml:space="preserve">{}</w:t></w:r></w:p></w:tc>"#,
                escape_xml(cell)
            ));
        }
        xml.push_str("</w:tr>");
    }
    xml.push_str(r#"</w:tbl><w:p><w:pPr><w:spacing w:after="120"/></w:pPr></w:p>"#);
    xml
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
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

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
            bullet_xml(" bullet "),
            paragraph_xml(" paragraph "),
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
        assert!(document_xml.contains("Plain summary text."));
        assert!(document_xml.contains("Transcript"));
        assert!(document_xml.contains("&lt;approved&gt;"));
    }

    #[test]
    fn markdown_blocks_support_headings_bullets_and_paragraphs() {
        let blocks =
            parse_markdown_blocks("## Next steps\n1. First\n- Second\nA paragraph\ncontinues");
        assert_eq!(blocks.len(), 4);
        assert!(matches!(blocks[0], DocBlock::Heading { level: 2, .. }));
        assert!(matches!(blocks[1], DocBlock::Bullet(_)));
        assert!(matches!(blocks[2], DocBlock::Bullet(_)));
        assert!(matches!(blocks[3], DocBlock::Paragraph(_)));
    }
}
