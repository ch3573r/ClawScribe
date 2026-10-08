use super::{extract::extract, fixtures, DocumentError, DocumentFormat};

#[test]
fn multipage_pdf_keeps_real_page_anchors() {
    let document = extract(DocumentFormat::Pdf, &fixtures::pdf(24, true, false)).unwrap();
    let pages: std::collections::BTreeSet<_> =
        document.blocks.iter().map(|b| b.page.unwrap()).collect();
    assert_eq!(pages, (1..=24).collect());
    for page in 1..=24 {
        let text = document
            .blocks
            .iter()
            .filter(|b| b.page == Some(page))
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains(&format!("decision_p{page}.")));
        assert!(!text.contains(&format!("decision_p{}.", page + 1)));
    }
}

#[test]
fn docx_tables_keep_document_paragraph_order_and_empty_paragraph_anchors() {
    let bytes = fixtures::docx(&fixtures::table_document(), &[]);
    let document = extract(DocumentFormat::Docx, &bytes).unwrap();
    assert_eq!(document.blocks.len(), 1537);
    assert_eq!(document.blocks[0].paragraph, 1);
    assert_eq!(document.blocks[1].paragraph, 3);
    assert!(document.blocks[1]
        .text
        .contains("Section 1, row 1, column 1"));
    assert!(document
        .blocks
        .last()
        .unwrap()
        .text
        .contains("Section 32, row 12, column 4"));
    assert!(document.blocks.iter().all(|b| b.page.is_none()));
}

#[test]
fn docx_text_box_paragraphs_keep_independent_stable_anchors() {
    let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Outer before. </w:t><w:drawing><w:txbxContent><w:p><w:r><w:t>Text box decision.</w:t></w:r></w:p><w:p/></w:txbxContent></w:drawing><w:t>Outer after.</w:t></w:r></w:p><w:p><w:r><w:t>Following paragraph.</w:t></w:r></w:p></w:body></w:document>"#;
    let document = extract(DocumentFormat::Docx, &fixtures::docx(xml, &[])).unwrap();
    let paragraphs: Vec<_> = document
        .blocks
        .iter()
        .map(|b| (b.paragraph, b.text.as_str()))
        .collect();
    assert_eq!(
        paragraphs,
        vec![
            (1, "Outer before. Outer after."),
            (2, "Text box decision."),
            (4, "Following paragraph.")
        ]
    );
}

#[test]
fn truncated_docx_xml_rejects_preceding_completed_paragraphs() {
    let xml = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Completed decision.</w:t></w:r></w:p><w:p><w:r><w:t>Unfinished decision."#;
    assert_eq!(
        extract(DocumentFormat::Docx, &fixtures::docx(xml, &[])),
        Err(DocumentError::Malformed)
    );
    let complete = fixtures::table_document();
    for invalid in [
        b"<Relationships><Relationship/>".as_slice(),
        b"<Relationships/><Relationships/>".as_slice(),
    ] {
        assert_eq!(
            extract(
                DocumentFormat::Docx,
                &fixtures::docx(
                    &complete,
                    &[("word/_rels/document.xml.rels", invalid.to_vec())]
                )
            ),
            Err(DocumentError::Malformed)
        );
    }
}

#[test]
fn format_mismatch_rejected() {
    assert_eq!(
        extract(DocumentFormat::Pdf, b"A plain reference"),
        Err(DocumentError::FormatMismatch)
    );
    assert_eq!(
        extract(DocumentFormat::Docx, &fixtures::pdf(2, true, false)),
        Err(DocumentError::FormatMismatch)
    );
    assert_eq!(
        extract(DocumentFormat::Text, &fixtures::pdf(2, true, false)),
        Err(DocumentError::FormatMismatch)
    );
}

#[test]
fn scanned_pdf_reports_no_text() {
    assert_eq!(
        extract(DocumentFormat::Pdf, &fixtures::pdf(12, false, false)),
        Err(DocumentError::NoText)
    );
}

#[test]
fn encrypted_pdf_reports_unsupported() {
    assert_eq!(
        extract(DocumentFormat::Pdf, &fixtures::pdf(3, true, true)),
        Err(DocumentError::EncryptedPdf)
    );
}

#[test]
fn invalid_utf8_rejected() {
    assert_eq!(
        extract(DocumentFormat::Text, &[0xff, 0xfe, 0]),
        Err(DocumentError::InvalidUtf8)
    );
    let document = extract(
        DocumentFormat::Markdown,
        "# Beschlüsse\n\nJa.\n\nPrüfung nächste Woche.".as_bytes(),
    )
    .unwrap();
    assert_eq!(document.blocks[1].text, "Ja.");
    assert!(document.blocks[2].text.contains("Prüfung"));
}

#[test]
fn zip_expansion_limit() {
    let bytes = fixtures::docx(
        &fixtures::table_document(),
        &[(
            "word/media/public-synthetic.bin",
            vec![b'A'; 32 * 1024 * 1024],
        )],
    );
    assert!(bytes.len() < 25 * 1024 * 1024);
    assert_eq!(
        extract(DocumentFormat::Docx, &bytes),
        Err(DocumentError::ZipLimit)
    );
}

#[test]
fn oversized_xml_entry_rejected_by_actual_decompressed_bytes() {
    let xml = format!("<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:body></w:document>","A".repeat(5*1024*1024));
    assert_eq!(
        extract(DocumentFormat::Docx, &fixtures::docx(&xml, &[])),
        Err(DocumentError::XmlLimit)
    );
}

#[test]
fn xml_external_relationship_ignored() {
    let relations = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="external" Target="https://example.com/not-fetched" TargetMode="External" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink"/></Relationships>"#.to_vec();
    let bytes = fixtures::docx(
        &fixtures::table_document(),
        &[("word/_rels/document.xml.rels", relations)],
    );
    let document = extract(DocumentFormat::Docx, &bytes).unwrap();
    assert_eq!(document.blocks.len(), 1537);
    let xml = r#"<!DOCTYPE x [<!ENTITY external SYSTEM "https://example.com/not-fetched">]><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>&external;</w:t></w:r></w:p></w:body></w:document>"#;
    assert_eq!(
        extract(DocumentFormat::Docx, &fixtures::docx(xml, &[])),
        Err(DocumentError::Malformed)
    );
}

#[test]
fn file_text_and_page_limits_are_enforced() {
    assert_eq!(
        extract(DocumentFormat::Text, &vec![b'A'; 25 * 1024 * 1024 + 1]),
        Err(DocumentError::InputLimit)
    );
    assert_eq!(
        extract(DocumentFormat::Text, &vec![b'A'; 2 * 1024 * 1024 + 1]),
        Err(DocumentError::TextLimit)
    );
    assert_eq!(
        extract(DocumentFormat::Pdf, &fixtures::pdf(501, false, false)),
        Err(DocumentError::PageLimit)
    );
    assert_eq!(super::INPUT_BYTES, 25 * 1024 * 1024);
    assert_eq!(super::TEXT_BYTES, 2 * 1024 * 1024);
    assert_eq!(super::PAGE_LIMIT, 500);
    assert_eq!(super::ZIP_BYTES, 32 * 1024 * 1024);
    assert_eq!(super::XML_BYTES, 5 * 1024 * 1024);
    assert_eq!(super::WORKER_BYTES, 512 * 1024 * 1024);
    assert_eq!(
        super::EXTRACTION_TIMEOUT,
        std::time::Duration::from_secs(10)
    );
}
