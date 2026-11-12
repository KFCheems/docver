//! Office document -> plain text, in-process.
//!
//! The GUI must not shell out to pandoc for the common case: opening a folder
//! of contracts should not depend on a Haskell binary, and a diff that takes
//! 300 ms per redraw is a diff nobody scrubs. docx and odt are zip containers
//! of XML, so they are read directly. Everything else degrades honestly:
//!
//! * `.txt` / `.md` / `.org` are read as bytes.
//! * `.rtf` goes through a small reader that keeps text and drops formatting.
//! * `.doc` / `.wps` / `.pages` have no reader outside LibreOffice, so *only*
//!   these spawn `soffice`. That is the one external process left, and it sits
//!   exactly where it belongs: at the boundary of formats we cannot parse.

use std::io::Read;
use std::process::Command;

pub const DOC_EXTS: &[&str] = &["docx", "docm", "dotx", "doc", "odt", "rtf", "wps", "pages", "odp"];
pub const TEXT_EXTS: &[&str] = &["md", "markdown", "txt", "org"];

pub fn is_document(name: &str) -> bool {
    let ext = ext_of(name);
    DOC_EXTS.contains(&ext.as_str()) || TEXT_EXTS.contains(&ext.as_str())
}

pub fn ext_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Text of a document, given its bytes and extension.
pub fn to_text(bytes: &[u8], ext: &str) -> Result<String, String> {
    let ext = ext.to_ascii_lowercase();
    let raw = match ext.as_str() {
        "docx" | "docm" | "dotx" => docx(bytes)?,
        "odt" | "odp" => odt(bytes)?,
        "rtf" => rtf(&String::from_utf8_lossy(bytes)),
        "md" | "markdown" | "txt" | "org" => String::from_utf8_lossy(bytes).into_owned(),
        "doc" | "wps" | "pages" => via_soffice(bytes, &ext)?,
        other => return Err(format!("no reader for .{other}")),
    };
    Ok(normalize(&raw))
}

/// One paragraph per line, no filler.
///
/// Word emits empty paragraphs for spacing and pandoc draws tables with runs
/// of dashes; both are pure noise to a line-based aligner. The blank ones make
/// a single paragraph edit look like two, and the rule lines get aligned as if
/// they were prose. Drop them; leave the text itself untouched.
pub fn normalize(text: &str) -> String {
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let line = raw.trim_end();
        if line.trim().is_empty() || is_rule(line) {
            continue;
        }
        out.push(line);
    }
    out.join("\n")
}

fn is_rule(line: &str) -> bool {
    let mut total = 0;
    let mut any = false;
    for part in line.split_whitespace() {
        if part.len() < 3 || !part.chars().all(|c| matches!(c, '-' | '=' | '_' | '+' | '|')) {
            return false;
        }
        total += part.len();
        any = true;
    }
    // Six characters, not three: a single "---" is a table separator, but
    // "价格 - 100" must survive, and "-" between two words is prose.
    any && total >= 6
}

// ---------------------------------------------------------------------- docx

fn docx(bytes: &[u8]) -> Result<String, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("not a zip: {e}"))?;
    let xml = read_entry(&mut zip, "word/document.xml")?;
    Ok(parse_docx_xml(&xml))
}

fn read_entry(zip: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, name: &str) -> Result<String, String> {
    let file = zip.by_name(name).map_err(|e| format!("{name}: {e}"))?;
    let mut buf = Vec::new();
    std::io::Read::take(file, 64 << 20)
        .read_to_end(&mut buf)
        .map_err(|e| format!("{name}: {e}"))?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn parse_docx_xml(xml: &str) -> String {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut out = String::new();
    let mut para = String::new();
    // Field codes (w:instrText) are instructions, not document text.
    let mut in_field_code = false;

    loop {
        let ev = match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(e) => e,
            Err(_) => break,
        };
        match ev {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.local_name();
                match name.as_ref() {
                    b"p" => para.clear(),
                    // A tab inside a paragraph: keep it, it separates cells.
                    b"tab" => para.push('\t'),
                    // An explicit line break is a line break in the source, and
                    // the aligner works on lines, so flush the paragraph.
                    b"br" | b"cr" => flush(&mut out, &mut para),
                    b"instrText" => in_field_code = true,
                    // w:delText is text Word has struck out. --track-changes=all
                    // keeps it visible rather than silently accepting the edit,
                    // so a redline cannot hide a deletion.
                    b"t" | b"delText" => {}
                    b"noBreakHyphen" => para.push('-'),
                    b"fldSimple" => in_field_code = false,
                    _ => {}
                }
            }
            Event::Text(t) => {
                if in_field_code {
                    continue;
                }
                let txt = quick_xml::escape::unescape(&t.decode().unwrap_or_default())
                    .unwrap_or_default()
                    .into_owned();
                para.push_str(&txt);
            }
            Event::CData(t) => {
                para.push_str(&String::from_utf8_lossy(&t));
            }
            Event::End(e) => {
                match e.local_name().as_ref() {
                    b"p" => flush(&mut out, &mut para),
                    b"instrText" => in_field_code = false,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    flush(&mut out, &mut para);
    out
}

fn flush(out: &mut String, para: &mut String) {
    let line = para.trim_end();
    if !line.is_empty() {
        out.push_str(line);
    }
    // Push the newline unconditionally: an empty paragraph still ends a block,
    // and dropping it here would merge two paragraphs into one line.
    out.push('\n');
    para.clear();
}

// ----------------------------------------------------------------------- odt

fn odt(bytes: &[u8]) -> Result<String, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("not a zip: {e}"))?;
    let xml = read_entry(&mut zip, "content.xml")?;
    Ok(parse_odt_xml(&xml))
}

fn parse_odt_xml(xml: &str) -> String {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut out = String::new();
    let mut para = String::new();
    loop {
        let ev = match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(e) => e,
            Err(_) => break,
        };
        match ev {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"p" | b"h" => para.clear(),
                b"tab" => para.push('\t'),
                b"line-break" => flush(&mut out, &mut para),
                b"s" => {
                    let n = e
                        .attributes()
                        .filter_map(|a| a.ok())
                        .find(|a| a.key.local_name().as_ref() == b"c")
                        .and_then(|a| String::from_utf8_lossy(&a.value).parse::<usize>().ok())
                        .unwrap_or(1);
                    for _ in 0..n {
                        para.push(' ');
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                para.push_str(&quick_xml::escape::unescape(&t.decode().unwrap_or_default()).unwrap_or_default());
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"p" | b"h" => flush(&mut out, &mut para),
                _ => {}
            },
            _ => {}
        }
    }
    flush(&mut out, &mut para);
    out
}

// ----------------------------------------------------------------------- rtf

/// Enough RTF to read a contract: text, paragraph breaks, tabs and \u escapes.
/// Formatting, font tables and embedded objects are skipped, not rendered.
fn rtf(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::new();
    let mut i = 0usize;
    // Depth at which an ignorable destination started ({\*\...}); 0 = none.
    let mut skip_until: Option<i32> = None;
    let mut depth: i32 = 0;
    let mut pending_space_skip = false;

    while i < b.len() {
        let c = b[i];
        match c {
            b'{' => {
                depth += 1;
                i += 1;
                // {\* ...} is a destination a naive reader must not print.
                if b.get(i) == Some(&b'\\') && b.get(i + 1) == Some(&b'*') {
                    skip_until = Some(depth);
                    i += 2;
                }
            }
            b'}' => {
                if skip_until == Some(depth) {
                    skip_until = None;
                }
                depth -= 1;
                i += 1;
            }
            b'\\' => {
                i += 1;
                if i >= b.len() {
                    break;
                }
                let ch = b[i];
                if ch.is_ascii_alphabetic() {
                    let start = i;
                    while i < b.len() && b[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = &src[start..i];
                    // Numeric parameter, if any.
                    let num_start = i;
                    if i < b.len() && (b[i] == b'-' || b[i].is_ascii_digit()) {
                        i += 1;
                        while i < b.len() && b[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    let num = src[num_start..i].parse::<i32>().ok();
                    if skip_until.is_none() {
                        match word {
                            "par" | "line" | "sect" | "page" => out.push('\n'),
                            "tab" => out.push('\t'),
                            "u" => {
                                if let Some(n) = num {
                                    let cp = if n < 0 { n + 65536 } else { n } as u32;
                                    if let Some(c) = char::from_u32(cp) {
                                        out.push(c);
                                    }
                                }
                                // \uN is followed by a fallback char for
                                // readers without Unicode; skip exactly one.
                                if i < b.len() && b[i] == b'?' {
                                    i += 1;
                                } else if i < b.len() && b[i] == b' ' {
                                    i += 1;
                                }
                            }
                            "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict" | "header"
                            | "footer" | "object" | "fldinst" => skip_until = Some(depth),
                            _ => {}
                        }
                    }
                    // A control word swallows one following space.
                    if i < b.len() && b[i] == b' ' {
                        i += 1;
                    }
                } else if ch == b'\'' {
                    // \'hh — a byte in the document's code page. Decode as
                    // cp1252 rather than dropping it; Chinese RTF uses \u.
                    if i + 2 < b.len() {
                        let hex = &src[i + 1..i + 3];
                        if let Ok(v) = u8::from_str_radix(hex, 16) {
                            if skip_until.is_none() {
                                out.push(cp1252(v));
                            }
                        }
                        i += 3;
                    } else {
                        i += 1;
                    }
                } else {
                    if skip_until.is_none() {
                        match ch {
                            b'\\' | b'{' | b'}' => out.push(ch as char),
                            b'~' => out.push('\u{a0}'),
                            b'-' => {}
                            _ => {}
                        }
                    }
                    i += 1;
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                if skip_until.is_none() {
                    // Copy one whole UTF-8 character, not one byte.
                    let len = utf8_len(c);
                    let end = (i + len).min(b.len());
                    out.push_str(&String::from_utf8_lossy(&b[i..end]));
                    i = end;
                } else {
                    i += 1;
                }
                pending_space_skip = false;
            }
        }
        let _ = pending_space_skip;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
        '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}', '\u{8f}',
        '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
        '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}', '\u{178}',
    ];
    match b {
        0x80..=0x9f => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

// ------------------------------------------------------------------- soffice

fn via_soffice(bytes: &[u8], ext: &str) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("docver-{}-{}", std::process::id(), ext));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let src = dir.join(format!("in.{ext}"));
    std::fs::write(&src, bytes).map_err(|e| e.to_string())?;
    let status = Command::new("soffice")
        .args(["--headless", "--convert-to", "docx", "--outdir"])
        .arg(&dir)
        .arg(&src)
        .status()
        .map_err(|e| format!("LibreOffice is not available: {e}"))?;
    let converted = dir.join("in.docx");
    let result = if status.success() && converted.exists() {
        std::fs::read(&converted).map_err(|e| e.to_string()).and_then(|b| docx(&b))
    } else {
        Err(format!("LibreOffice could not convert .{ext}"))
    };
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_drops_blank_lines_and_rules() {
        // The rule pandoc emits for a table is a run of dashes, one per column.
        let t = "a\n\n------ ------\nName   Value\n\nb\n   \n";
        assert_eq!(normalize(t), "a\nName   Value\nb");
    }

    #[test]
    fn short_dash_run_is_prose_not_a_rule() {
        // "---" is three characters: a horizontal rule in Markdown, but too
        // short to be a table separator, so it stays.
        assert_eq!(normalize("---\n"), "---");
    }

    #[test]
    fn price_dash_survives_normalize() {
        assert_eq!(normalize("价格 - 100\n"), "价格 - 100");
    }

    #[test]
    fn ext_is_lowercase() {
        assert_eq!(ext_of("合同.DOCX"), "docx");
        assert!(is_document("合同.docx"));
        assert!(!is_document("账单.pdf"));
    }

    #[test]
    fn rtf_keeps_text_and_breaks() {
        let src = r"{\rtf1\ansi{\fonttbl{\f0 Arial;}}\f0 Hello\par World\tab End}";
        assert_eq!(rtf(src), "Hello\nWorld\tEnd");
    }

    #[test]
    fn rtf_unicode_escape() {
        // 合 U+5408 = 21512, 约 U+7EA6 = 32422; the '?' is the ASCII fallback
        // every RTF reader must skip.
        assert_eq!(rtf(r"{\rtf1\u21512?\u32422?}"), "合约");
    }
}
