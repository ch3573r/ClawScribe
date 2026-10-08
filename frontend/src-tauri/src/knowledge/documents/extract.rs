//! Parser contract; extraction runs only in the bounded child in the app.
//! Pure local extraction. Run only inside the bounded document child in production.
use super::{
    DocumentBlock, DocumentError, DocumentFormat, ExtractedDocument, INPUT_BYTES, PAGE_LIMIT,
    TEXT_BYTES, XML_BYTES, ZIP_BYTES,
};
use quick_xml::{events::Event, name::ResolveResult, NsReader};
use std::{collections::HashSet, io::Read};

pub fn extract(format: DocumentFormat, bytes: &[u8]) -> Result<ExtractedDocument, DocumentError> {
    if bytes.len() > INPUT_BYTES {
        return Err(DocumentError::InputLimit);
    }
    let mut blocks = Blocks::default();
    match format {
        DocumentFormat::Pdf => pdf(bytes, &mut blocks)?,
        DocumentFormat::Docx => docx(bytes, &mut blocks)?,
        DocumentFormat::Text | DocumentFormat::Markdown => {
            if bytes.starts_with(b"%PDF-") || bytes.starts_with(b"PK\x03\x04") {
                return Err(DocumentError::FormatMismatch);
            }
            let text = std::str::from_utf8(bytes).map_err(|_| DocumentError::InvalidUtf8)?;
            if text.contains('\0') {
                return Err(DocumentError::InvalidUtf8);
            }
            if text.len() > TEXT_BYTES {
                return Err(DocumentError::TextLimit);
            }
            paragraphs(text.trim_start_matches('\u{feff}'), None, &mut blocks)?;
        }
    }
    if blocks.items.is_empty() {
        return Err(DocumentError::NoText);
    }
    Ok(ExtractedDocument {
        format,
        blocks: blocks.items,
    })
}

#[derive(Default)]
struct Blocks {
    bytes: usize,
    items: Vec<DocumentBlock>,
}
impl Blocks {
    fn push(&mut self, page: Option<u32>, paragraph: u32, text: &str) -> Result<(), DocumentError> {
        let text = text.trim();
        self.bytes = self
            .bytes
            .checked_add(text.len())
            .filter(|n| *n <= TEXT_BYTES)
            .ok_or(DocumentError::TextLimit)?;
        if !text.is_empty() {
            self.items.push(DocumentBlock {
                page,
                paragraph,
                text: text.to_owned(),
            });
        }
        Ok(())
    }
}

fn paragraphs(text: &str, page: Option<u32>, blocks: &mut Blocks) -> Result<(), DocumentError> {
    let mut paragraph = 1;
    let mut current = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                blocks.push(page, paragraph, &current)?;
                paragraph += 1;
                current.clear();
            }
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
            if current.len() > TEXT_BYTES {
                return Err(DocumentError::TextLimit);
            }
        }
    }
    blocks.push(page, paragraph, &current)
}

fn pdf(bytes: &[u8], blocks: &mut Blocks) -> Result<(), DocumentError> {
    if !bytes.starts_with(b"%PDF-") {
        return Err(DocumentError::FormatMismatch);
    }
    let document = lopdf::Document::load_mem(bytes).map_err(|_| DocumentError::Malformed)?;
    if document.is_encrypted() {
        return Err(DocumentError::EncryptedPdf);
    }
    let pages = document.get_pages();
    if pages.len() > PAGE_LIMIT {
        return Err(DocumentError::PageLimit);
    }
    for page in pages.keys() {
        let text = document
            .extract_text(&[*page])
            .map_err(|_| DocumentError::Malformed)?;
        if text.len() > TEXT_BYTES {
            return Err(DocumentError::TextLimit);
        }
        paragraphs(&text, Some(*page), blocks)?;
    }
    Ok(())
}

fn docx(bytes: &[u8], blocks: &mut Blocks) -> Result<(), DocumentError> {
    if !bytes.starts_with(b"PK\x03\x04") {
        return Err(DocumentError::FormatMismatch);
    }
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| DocumentError::Malformed)?;
    let mut total = 0usize;
    let mut names = HashSet::new();
    let mut document = None;
    let mut content_types = None;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| DocumentError::Malformed)?;
        let name = entry.name().to_owned();
        if entry.enclosed_name().is_none() || !names.insert(name.clone()) {
            return Err(DocumentError::Malformed);
        }
        let xml = name.ends_with(".xml") || name.ends_with(".rels");
        let mut count = 0usize;
        let mut captured = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let read = entry
                .read(&mut buffer)
                .map_err(|_| DocumentError::Malformed)?;
            if read == 0 {
                break;
            }
            count += read;
            total += read;
            if total > ZIP_BYTES {
                return Err(DocumentError::ZipLimit);
            }
            if xml && count > XML_BYTES {
                return Err(DocumentError::XmlLimit);
            }
            if xml {
                captured.extend_from_slice(&buffer[..read]);
            }
        }
        if xml {
            reject_dtd(&captured)?;
        }
        match name.as_str() {
            "word/document.xml" => document = Some(captured),
            "[Content_Types].xml" => content_types = Some(captured),
            _ => {} // Never follow relationships or unpack files to disk.
        }
    }
    let types = content_types.ok_or(DocumentError::FormatMismatch)?;
    let types = std::str::from_utf8(&types).map_err(|_| DocumentError::Malformed)?;
    if !types.contains(
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
    ) {
        return Err(DocumentError::FormatMismatch);
    }
    docx_paragraphs(&document.ok_or(DocumentError::FormatMismatch)?, blocks)
}

fn reject_dtd(bytes: &[u8]) -> Result<(), DocumentError> {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    loop {
        match reader.read_event().map_err(|_| DocumentError::Malformed)? {
            Event::DocType(_) => return Err(DocumentError::Malformed),
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

fn docx_paragraphs(bytes: &[u8], blocks: &mut Blocks) -> Result<(), DocumentError> {
    const WORD: &[u8] = b"http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    const STRICT: &[u8] = b"http://purl.oclc.org/ooxml/wordprocessingml/main";
    let mut reader = NsReader::from_reader(bytes);
    let mut ordinal = 0u32;
    let mut in_paragraph = false;
    let mut in_text = false;
    let mut text = String::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|_| DocumentError::Malformed)?;
        let word = matches!(namespace, ResolveResult::Bound(ns) if ns.as_ref() == WORD || ns.as_ref() == STRICT);
        match event {
            Event::Start(ref tag) if word => match tag.local_name().as_ref() {
                b"p" => {
                    if in_paragraph {
                        return Err(DocumentError::Malformed);
                    }
                    ordinal += 1;
                    in_paragraph = true;
                    text.clear();
                }
                b"t" if in_paragraph => in_text = true,
                _ => {}
            },
            Event::Empty(ref tag) if word => match tag.local_name().as_ref() {
                b"p" => ordinal += 1,
                b"tab" if in_paragraph => text.push('\t'),
                b"br" | b"cr" if in_paragraph => text.push('\n'),
                _ => {}
            },
            Event::Text(ref value) if in_text => {
                text.push_str(&value.unescape().map_err(|_| DocumentError::Malformed)?);
                if text.len() > TEXT_BYTES {
                    return Err(DocumentError::TextLimit);
                }
            }
            Event::End(ref tag) if word => match tag.local_name().as_ref() {
                b"t" => in_text = false,
                b"p" => {
                    blocks.push(None, ordinal, &text)?;
                    in_paragraph = false;
                    in_text = false;
                }
                _ => {}
            },
            Event::DocType(_) => return Err(DocumentError::Malformed),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}
