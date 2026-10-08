//! Independently authored public synthetic office references; no user data.
use std::io::{Cursor, Write};

pub fn pdf(pages: usize, with_text: bool, encrypted: bool) -> Vec<u8> {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        String::new(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut children = Vec::new();
    for page in 1..=pages {
        let page_id = objects.len() + 1;
        children.push(format!("{page_id} 0 R"));
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",page_id + 1));
        let content = if with_text {
            let mut lines =
                format!("BT /F1 10 Tf 12 TL 40 780 Td (Project Aster reference page {page}) Tj");
            for row in 1..=30 {
                lines.push_str(&format!(" T* (Section {row}: page {page} budget review, owner team {row}, decision_p{page}.) Tj"));
            }
            lines + " ET"
        } else {
            // A valid graphical page with no text layer, as in a scanned file.
            "40 40 515 720 re S".to_string()
        };
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ));
    }
    objects[1] = format!(
        "<< /Type /Pages /Count {pages} /Kids [{}] >>",
        children.join(" ")
    );
    let encryption = if encrypted {
        objects.push("<< /Filter /Standard /V 1 /R 2 /Length 40 /O <0000000000000000000000000000000000000000000000000000000000000000> /U <0000000000000000000000000000000000000000000000000000000000000000> /P -4 >>".into());
        format!(" /Encrypt {} 0 R /ID [<00000000000000000000000000000000> <00000000000000000000000000000000>]",objects.len())
    } else {
        String::new()
    };
    let mut output = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(output.len());
        write!(output, "{} 0 obj\n{object}\nendobj\n", index + 1).unwrap();
    }
    let xref = output.len();
    write!(output, "xref\n0 {}\n0000000000 65535 f \n", offsets.len()).unwrap();
    for offset in offsets.iter().skip(1) {
        writeln!(output, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        output,
        "trailer\n<< /Size {} /Root 1 0 R{encryption} >>\nstartxref\n{xref}\n%%EOF\n",
        offsets.len()
    )
    .unwrap();
    output
}

pub fn docx(document: &str, extra: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    archive.start_file("[Content_Types].xml", options).unwrap();
    archive.write_all(br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#).unwrap();
    archive.start_file("word/document.xml", options).unwrap();
    archive.write_all(document.as_bytes()).unwrap();
    for (name, bytes) in extra {
        archive.start_file(*name, options).unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

pub fn table_document() -> String {
    let mut xml = String::from(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Project Aster reference register</w:t></w:r></w:p><w:p/>"#,
    );
    for section in 1..=32 {
        xml.push_str("<w:tbl>");
        for row in 1..=12 {
            xml.push_str("<w:tr>");
            for column in 1..=4 {
                xml.push_str(&format!("<w:tc><w:p><w:r><w:t>Section {section}, row {row}, column {column}: review the supplier agreement and confirm the recorded budget decision.</w:t></w:r></w:p></w:tc>"));
            }
            xml.push_str("</w:tr>");
        }
        xml.push_str("</w:tbl>");
    }
    xml.push_str("</w:body></w:document>");
    xml
}
