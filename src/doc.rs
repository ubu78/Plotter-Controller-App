//! DOCX / TXT / MD -> plain text (PDF is not supported in the Rust build yet).
use std::io::{Cursor, Read};

fn cp1251(b: &[u8]) -> String {
    b.iter().map(|&c| match c { 0xA8 => 'Ё', 0xB8 => 'ё', 0xC0..=0xFF => char::from_u32(0x410 + (c as u32 - 0xC0)).unwrap(), _ => c as char }).collect()
}

pub fn doc_to_text(name: &str, buf: &[u8]) -> Result<String, String> {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "docx" => {
            let mut z = zip::ZipArchive::new(Cursor::new(buf)).map_err(|e| format!("Not a valid DOCX: {e}"))?;
            let mut xml = String::new();
            z.by_name("word/document.xml").map_err(|_| "Not a valid DOCX (word/document.xml missing)".to_string())?.read_to_string(&mut xml).map_err(|e| e.to_string())?;
            let doc = roxmltree::Document::parse(&xml).map_err(|e| e.to_string())?;
            let mut out = vec![];
            for p in doc.descendants().filter(|n| n.tag_name().name() == "p") {
                let mut s = String::new();
                for n in p.descendants() { match n.tag_name().name() { "t" => s.push_str(n.text().unwrap_or("")), "tab" => s.push('\t'), "br" => s.push('\n'), _ => {} } }
                out.push(s);
            }
            Ok(out.join("\n").trim().to_string())
        }
        "txt" | "md" | "markdown" | "" => Ok(String::from_utf8(buf.to_vec()).unwrap_or_else(|_| cp1251(buf))),
        "pdf" => {
            // pdf-extract can panic on odd files: never let that take the server down
            let b = buf.to_vec();
            match std::panic::catch_unwind(move || pdf_extract::extract_text_from_mem(&b)) {
                Ok(Ok(t)) => Ok(t.trim().to_string()),
                Ok(Err(e)) => Err(format!("Cannot read PDF text: {e}")),
                Err(_) => Err("Cannot read PDF text (unsupported file)".into()),
            }
        }
        _ => Err(format!("Unsupported file type: .{ext}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn txt_utf8_then_cp1251() {
        assert_eq!(doc_to_text("a.txt", "привет".as_bytes()).unwrap(), "привет");
        // "привет" in cp1251: п=EF р=F0 и=E8 в=E2 е=E5=F2=E2... build byte-wise
        let cp: Vec<u8> = "привет".chars().map(|c| match c {
            'ё' => 0xB8u8, 'Ё' => 0xA8,
            c if (c as u32) >= 0x410 && (c as u32) <= 0x44F => (c as u32 - 0x410 + 0xC0) as u8,
            _ => c as u8 }).collect();
        assert_eq!(doc_to_text("b.txt", &cp).unwrap(), "привет");
    }

    #[test]
    fn md_extension_is_text() {
        assert_eq!(doc_to_text("note.md", b"# hi").unwrap(), "# hi");
    }

    #[test]
    fn docx_paragraphs_with_breaks() {
        let mut cur = Cursor::new(Vec::new());
        { let mut z = zip::ZipWriter::new(&mut cur);
          z.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
          z.write_all(r#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>Первый</w:t><w:tab/></w:r></w:p><w:p><w:r><w:t>Второй</w:t></w:r></w:p></w:body></w:document>"#.as_bytes()).unwrap();
          z.finish().unwrap(); }
        assert_eq!(doc_to_text("x.docx", cur.get_ref()).unwrap(), "Первый\t\nВторой");
    }

    #[test]
    fn bad_docx_is_clean_error() {
        assert!(doc_to_text("x.docx", b"not a zip").unwrap_err().contains("DOCX"));
    }

    #[test]
    fn unsupported_extension() {
        assert!(doc_to_text("x.exe", b"").is_err());
    }
}
