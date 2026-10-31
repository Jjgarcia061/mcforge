//! Plain text from the files a library indexes, the links between them, and
//! the passages the text is split into. Read-only; runs on a blocking thread.

use std::io::{Cursor, Read};
use std::path::Path;

use quick_xml::events::Event;

use crate::error::{Error, Result};

pub const EXTENSIONS: [&str; 7] = ["md", "markdown", "txt", "html", "htm", "docx", "pdf"];
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// Passages are about this long, so a few of them fit any model's context.
const CHUNK_BYTES: usize = 1200;
/// Each passage repeats the end of the previous one, so no idea is cut in half.
const OVERLAP_BYTES: usize = 180;

pub struct Document {
    pub text: String,
    /// What Markdown links point at: `[[Note]]` names and relative file paths.
    pub links: Vec<String>,
}

pub fn supported(path: &Path) -> bool {
    extension(path).is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

pub fn read(path: &Path, bytes: &[u8]) -> Result<Document> {
    let ext = extension(path).unwrap_or_default();
    let (text, links) = match ext.as_str() {
        "md" | "markdown" => {
            let text = crate::sources::decode(bytes).0.into_owned();
            let links = markdown_links(&text);
            (text, links)
        }
        "txt" => (crate::sources::decode(bytes).0.into_owned(), Vec::new()),
        "html" | "htm" => (html_text(&crate::sources::decode(bytes).0), Vec::new()),
        "docx" => (docx_text(bytes)?, Vec::new()),
        "pdf" => (pdf_text(bytes)?, Vec::new()),
        _ => return Err(Error::Io(format!("unsupported file type .{ext}"))),
    };
    let text = tidy(&text);
    if text.is_empty() {
        return Err(Error::Io(
            "no text found (a scanned document needs OCR)".into(),
        ));
    }
    Ok(Document { text, links })
}

/// The PDF parser can panic on documents it does not support; that must cost
/// one file, not the app.
fn pdf_text(bytes: &[u8]) -> Result<String> {
    std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes))
        .map_err(|_| Error::Io("this PDF uses features the reader does not support".into()))?
        .map_err(|e| Error::Io(format!("could not read the PDF: {e}")))
}

/// The paragraphs of `word/document.xml`, the body of a .docx file.
fn docx_text(bytes: &[u8]) -> Result<String> {
    let mut xml = String::new();
    zip::ZipArchive::new(Cursor::new(bytes))
        .and_then(|mut zip| {
            zip.by_name("word/document.xml")?
                .read_to_string(&mut xml)
                .map_err(Into::into)
        })
        .map_err(|e| Error::Io(format!("could not read the Word document: {e}")))?;

    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut text = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == "t" => in_text = true,
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "p" => text.push_str("\n\n"),
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                "tab" => text.push('\t'),
                "br" => text.push('\n'),
                _ => {}
            },
            Ok(Event::Text(t)) if in_text => text.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(r)) if in_text => {
                if let Some(c) = r.resolve_char_ref().ok().flatten().or_else(|| entity(&r)) {
                    text.push(c);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Error::Io(format!("could not read the Word document: {e}"))),
            _ => {}
        }
    }
    Ok(text)
}

fn entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" | "#39" => '\'',
        "nbsp" => ' ',
        "aacute" => 'á',
        "eacute" => 'é',
        "iacute" => 'í',
        "oacute" => 'ó',
        "uacute" => 'ú',
        "ntilde" => 'ñ',
        "Aacute" => 'Á',
        "Eacute" => 'É',
        "Iacute" => 'Í',
        "Oacute" => 'Ó',
        "Uacute" => 'Ú',
        "Ntilde" => 'Ñ',
        "uuml" => 'ü',
        "iexcl" => '¡',
        "iquest" => '¿',
        _ => return None,
    })
}

/// Visible text of a web page: tags dropped, scripts and styles skipped,
/// blocks on their own lines.
fn html_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    while let Some(start) = rest.find(['<', '&']) {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.starts_with('&') {
            let end = rest.find(';').filter(|&e| e <= 10);
            let decoded = end.and_then(|e| {
                let name = &rest[1..e];
                name.strip_prefix('#')
                    .and_then(|n| {
                        n.strip_prefix(['x', 'X'])
                            .map_or_else(|| n.parse().ok(), |h| u32::from_str_radix(h, 16).ok())
                    })
                    .and_then(char::from_u32)
                    .or_else(|| entity(name))
            });
            match (end, decoded) {
                (Some(e), Some(c)) => {
                    out.push(c);
                    rest = &rest[e + 1..];
                }
                _ => {
                    out.push('&');
                    rest = &rest[1..];
                }
            }
            continue;
        }
        let Some(end) = rest.find('>') else { break };
        let tag = rest[1..end].trim_start_matches('/').to_ascii_lowercase();
        let name = tag.split([' ', '\t', '\n', '/']).next().unwrap_or("");
        rest = &rest[end + 1..];
        if matches!(name, "script" | "style" | "head") && !rest.is_empty() {
            let close = format!("</{name}");
            rest = rest
                .to_ascii_lowercase()
                .find(&close)
                .and_then(|i| rest[i..].find('>').map(|j| &rest[i + j + 1..]))
                .unwrap_or("");
        } else if matches!(
            name,
            "p" | "div"
                | "br"
                | "li"
                | "tr"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "section"
                | "article"
                | "table"
        ) {
            out.push_str("\n\n");
        } else if matches!(name, "td" | "th") {
            out.push('\t');
        }
    }
    out.push_str(rest);
    out
}

/// `[[Note]]`, `[[Note|alias]]` and `[text](path.md)` targets, in order, once each.
fn markdown_links(text: &str) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    let mut push = |target: &str| {
        let target = target.trim();
        if !target.is_empty() && !links.iter().any(|l| l == target) {
            links.push(target.to_owned());
        }
    };
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find("]]") else { break };
        push(rest[..end].split(['|', '#']).next().unwrap_or(""));
        rest = &rest[end + 2..];
    }
    let mut rest = text;
    while let Some(start) = rest.find("](") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find(')') else { break };
        let target = rest[..end].split([' ', '#']).next().unwrap_or("");
        let external = target.contains("://") || target.starts_with("mailto:");
        if !external {
            push(&target.replace("%20", " "));
        }
        rest = &rest[end + 1..];
    }
    links
}

/// Trims lines, collapses runs of spaces and of blank lines.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = false;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank { "\n\n" } else { "\n" });
        }
        out.push_str(&line);
        blank = false;
    }
    out
}

/// Splits text into passages of about `CHUNK_BYTES`, along paragraphs and
/// sentences, each starting with the end of the previous one.
pub fn chunks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for piece in text.split("\n\n").flat_map(pieces) {
        if !current.is_empty() && current.len() + piece.len() + 2 > CHUNK_BYTES {
            let tail = overlap(&current).to_owned();
            out.push(std::mem::replace(&mut current, tail));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(piece);
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

/// A paragraph, cut at sentence ends (or spaces) when it is longer than a passage.
fn pieces(paragraph: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = paragraph.trim();
    while rest.len() > CHUNK_BYTES {
        let limit = rest.floor_char_boundary(CHUNK_BYTES);
        let cut = rest[..limit]
            .rfind(". ")
            .map(|i| i + 1)
            .filter(|&i| i > CHUNK_BYTES / 2)
            .or_else(|| rest[..limit].rfind(' '))
            .unwrap_or(limit);
        out.push(rest[..cut].trim());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// The last `OVERLAP_BYTES` of a passage, starting at a word.
fn overlap(passage: &str) -> &str {
    let from = passage.ceil_char_boundary(passage.len().saturating_sub(OVERLAP_BYTES));
    let tail = &passage[from..];
    tail.find(' ').map_or(tail, |i| &tail[i + 1..])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A one-page PDF with `lines` of text in a standard font.
    pub(crate) fn pdf(lines: &[&str]) -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.5");
        let pages = doc.new_object_id();
        let font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
        });
        let resources = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
        let mut ops = vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 12.into()]),
            Operation::new("Td", vec![72.into(), 720.into()]),
        ];
        for line in lines {
            let latin1: Vec<u8> = line.chars().map(|c| c as u8).collect();
            ops.push(Operation::new(
                "Tj",
                vec![Object::String(latin1, lopdf::StringFormat::Literal)],
            ));
            ops.push(Operation::new("Td", vec![0.into(), (-16).into()]));
        }
        ops.push(Operation::new("ET", vec![]));
        let content = doc.add_object(Stream::new(
            dictionary! {},
            Content { operations: ops }.encode().unwrap(),
        ));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => content,
        });
        doc.objects.insert(
            pages,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1,
                "Resources" => resources, "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            }),
        );
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    /// A minimal .docx with one paragraph per entry.
    pub(crate) fn docx(paragraphs: &[&str]) -> Vec<u8> {
        use std::io::Write;
        let body: String = paragraphs
            .iter()
            .map(|p| format!("<w:p><w:r><w:t xml:space=\"preserve\">{p}</w:t></w:r></w:p>"))
            .collect();
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}</w:body></w:document>"
        );
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_every_supported_format() {
        let md = read(
            Path::new("Metas.md"),
            "# Metas 2026\n\nVer [[Política de descuentos|descuentos]] y [precios](precios/lista%20general.md), \
             no [web](https://x.com).\n\nOtra vez [[Política de descuentos]]."
                .as_bytes(),
        )
        .unwrap();
        assert!(md.text.starts_with("# Metas 2026\n\nVer"));
        assert_eq!(
            md.links,
            ["Política de descuentos", "precios/lista general.md"]
        );

        let html = read(
            Path::new("a.html"),
            b"<html><head><title>x</title><style>p{}</style></head><body><h1>Almac&eacute;n</h1><p>Rotaci&#243;n &amp; stock</p><script>var a=1<2;</script></body></html>",
        )
        .unwrap();
        assert_eq!(html.text, "Almacén\n\nRotación & stock");

        let docx = read(
            Path::new("a.docx"),
            &docx(&["Proceso de contratación", "Paso 1 &amp; 2"]),
        )
        .unwrap();
        assert_eq!(docx.text, "Proceso de contratación\n\nPaso 1 & 2");

        let pdf = read(
            Path::new("a.PDF"),
            &pdf(&["Viaticos por dia: 1,800 pesos", "Reembolso en cinco dias"]),
        )
        .unwrap();
        assert!(pdf.text.contains("1,800 pesos"), "{}", pdf.text);
        assert!(pdf.text.contains("Reembolso"), "{}", pdf.text);

        assert!(read(Path::new("a.pdf"), b"%PDF-1.4 garbage").is_err());
        assert!(read(Path::new("vacío.txt"), b"  \n\n ").is_err());
        assert!(!supported(Path::new("a.xlsx")) && supported(Path::new("NOTAS.MD")));
    }

    #[test]
    fn chunks_follow_paragraphs_and_overlap() {
        let paragraph = |n: usize| format!("Párrafo {n}. {}", "palabra ".repeat(40).trim());
        let text: Vec<String> = (0..12).map(paragraph).collect();
        let passages = chunks(&text.join("\n\n"));
        assert!(passages.len() > 2);
        for c in &passages {
            assert!(c.len() <= CHUNK_BYTES + OVERLAP_BYTES + 2, "{}", c.len());
        }
        let first_end: String = passages[0].chars().rev().take(20).collect();
        let first_end: String = first_end.chars().rev().collect();
        assert!(
            passages[1].contains(&first_end),
            "the next passage repeats the end of the previous one"
        );

        let long = "Una frase larga. ".repeat(200);
        assert!(
            chunks(&long)
                .iter()
                .all(|c| c.len() <= CHUNK_BYTES + OVERLAP_BYTES + 2)
        );
        assert_eq!(chunks("corto"), ["corto"]);
    }
}
