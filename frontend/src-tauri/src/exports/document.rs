//! Minimal DOCX generation for OneDrive/SharePoint file export.
//!
//! The writer intentionally supports only the structure ClawScribe summaries
//! need: headings, bullet list items, and plain paragraphs. It avoids adding a
//! document-generation dependency by creating the small Open XML ZIP package
//! directly.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;

enum DocBlock {
    Heading { level: u8, text: String },
    Bullet(String),
    Paragraph(String),
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

    for raw_line in markdown.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            flush_paragraph(&mut blocks, &mut paragraph_lines);
            continue;
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
        <w:t>{}</w:t>
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
      <w:r><w:t>{}</w:t></w:r>
    </w:p>"#,
        escape_xml(text)
    )
}

fn paragraph_xml(text: &str) -> String {
    format!(
        r#"<w:p>
      <w:pPr><w:spacing w:after="120"/></w:pPr>
      <w:r><w:t>{}</w:t></w:r>
    </w:p>"#,
        escape_xml(text)
    )
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
