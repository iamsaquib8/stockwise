//! Just enough of the `.xlsx` format to read one worksheet as rows of text:
//! a zip reader for stored and deflated entries (via `flate2`) and a scanner
//! for the sheet's `<c>` cells. Used for AMFI's expense-ratio export, which
//! is published only as a spreadsheet. No formulas, styles or dates — cells
//! come back as the text or number Excel stored.

use anyhow::{Context, Result, bail};
use std::io::Read;

fn u16_at(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as usize)
}

fn u32_at(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// One file inside the zip, as listed in the central directory (which has
/// the sizes even when the local header defers them to a data descriptor).
struct Entry {
    name: String,
    method: usize,
    compressed: usize,
    size: usize,
    local_header: usize,
}

fn central_directory(zip: &[u8]) -> Result<Vec<Entry>> {
    const EOCD: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const CDIR: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
    // The end record sits in the last 22 bytes plus an optional comment.
    let from = zip.len().saturating_sub(22 + 0xffff);
    let eocd = (from..zip.len().saturating_sub(21))
        .rev()
        .find(|&i| zip[i..i + 4] == EOCD)
        .context("not a zip file (no end of central directory)")?;
    let count = u16_at(zip, eocd + 10).context("truncated zip")?;
    let mut at = u32_at(zip, eocd + 16).context("truncated zip")?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if zip.get(at..at + 4) != Some(&CDIR[..]) {
            bail!("corrupt zip central directory");
        }
        let field = |off| u32_at(zip, at + off).context("truncated zip");
        let short = |off| u16_at(zip, at + off).context("truncated zip");
        let (name_len, extra_len, comment_len) = (short(28)?, short(30)?, short(32)?);
        let name = zip
            .get(at + 46..at + 46 + name_len)
            .context("truncated zip")?;
        out.push(Entry {
            name: String::from_utf8_lossy(name).into_owned(),
            method: short(10)?,
            compressed: field(20)?,
            size: field(24)?,
            local_header: field(42)?,
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(out)
}

fn extract(zip: &[u8], e: &Entry) -> Result<Vec<u8>> {
    let h = e.local_header;
    if zip.get(h..h + 4) != Some(&[0x50, 0x4b, 0x03, 0x04][..]) {
        bail!("corrupt zip entry {}", e.name);
    }
    let start = h
        + 30
        + u16_at(zip, h + 26).context("truncated zip")?
        + u16_at(zip, h + 28).context("truncated zip")?;
    let data = zip
        .get(start..start + e.compressed)
        .with_context(|| format!("truncated zip entry {}", e.name))?;
    match e.method {
        0 => Ok(data.to_vec()),
        8 => {
            let mut out = Vec::with_capacity(e.size);
            flate2::read::DeflateDecoder::new(data)
                .read_to_end(&mut out)
                .with_context(|| format!("cannot inflate {}", e.name))?;
            Ok(out)
        }
        m => bail!("zip entry {} uses unsupported compression {m}", e.name),
    }
}

/// `&amp;` and friends, plus numeric references.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Text between `<tag ...>` and `</tag>` inside `xml`, for every occurrence
/// of a `<t>` run (rich text in shared strings splits text across several).
fn text_runs(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<t") {
        let after = &rest[i + 2..];
        // `<t>` or `<t xml:space="preserve">`, not `<tc...>`-like tags.
        if !(after.starts_with('>') || after.starts_with(' ')) {
            rest = after;
            continue;
        }
        let Some(open_end) = after.find('>') else {
            break;
        };
        let body = &after[open_end + 1..];
        let Some(close) = body.find("</t>") else {
            break;
        };
        out.push_str(&unescape(&body[..close]));
        rest = &body[close + 4..];
    }
    out
}

fn shared_strings(xml: &str) -> Vec<String> {
    xml.split("<si>")
        .skip(1)
        .map(|si| text_runs(si.split("</si>").next().unwrap_or("")))
        .collect()
}

/// `B12` → column index 1.
fn column_index(cell_ref: &str) -> Option<usize> {
    let letters: String = cell_ref
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if letters.is_empty() {
        return None;
    }
    Some(
        letters.bytes().fold(0usize, |n, b| {
            n * 26 + (b.to_ascii_uppercase() - b'A' + 1) as usize
        }) - 1,
    )
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let len = tag[start..].find('"')?;
    Some(&tag[start..start + len])
}

/// Rows of the sheet XML as text cells. Missing cells inside a row are
/// empty strings, so column positions line up with the header row.
fn sheet_rows(xml: &str, shared: &[String]) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for row_xml in xml.split("<row").skip(1) {
        let row_xml = row_xml.split("</row>").next().unwrap_or("");
        let mut cells: Vec<String> = Vec::new();
        for cell in row_xml.split("<c ").skip(1) {
            let Some(tag_end) = cell.find('>') else {
                continue;
            };
            let tag = format!(" {}", &cell[..tag_end]);
            // `<c r="A1" s="2"/>` is an empty, styled cell.
            let body = if tag.ends_with('/') {
                ""
            } else {
                cell[tag_end + 1..].split("</c>").next().unwrap_or("")
            };
            let raw_value = || {
                body.split_once("<v>")
                    .and_then(|(_, v)| v.split_once("</v>"))
                    .map(|(v, _)| unescape(v))
                    .unwrap_or_default()
            };
            let value = match attr(&tag, "t") {
                Some("s") => raw_value()
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| shared.get(i).cloned())
                    .unwrap_or_default(),
                Some("inlineStr") => text_runs(body),
                _ => raw_value(),
            };
            let col = attr(&tag, "r")
                .and_then(column_index)
                .unwrap_or(cells.len());
            if col >= cells.len() {
                cells.resize(col + 1, String::new());
            }
            cells[col] = value;
        }
        rows.push(cells);
    }
    rows
}

/// Every row of the workbook's first worksheet.
pub fn read_first_sheet(bytes: &[u8]) -> Result<Vec<Vec<String>>> {
    let entries = central_directory(bytes)?;
    let sheet = entries
        .iter()
        .find(|e| e.name == "xl/worksheets/sheet1.xml")
        .or_else(|| {
            entries
                .iter()
                .find(|e| e.name.starts_with("xl/worksheets/") && e.name.ends_with(".xml"))
        })
        .context("spreadsheet has no worksheet")?;
    let shared = match entries.iter().find(|e| e.name == "xl/sharedStrings.xml") {
        Some(e) => shared_strings(&String::from_utf8_lossy(&extract(bytes, e)?)),
        None => Vec::new(),
    };
    let xml = String::from_utf8_lossy(&extract(bytes, sheet)?).into_owned();
    Ok(sheet_rows(&xml, &shared))
}

/// Every worksheet in the `.xlsx` workbook, in central-directory order.
pub fn read_all_sheets(bytes: &[u8]) -> Result<Vec<Vec<Vec<String>>>> {
    let entries = central_directory(bytes)?;
    let shared = match entries.iter().find(|e| e.name == "xl/sharedStrings.xml") {
        Some(e) => shared_strings(&String::from_utf8_lossy(&extract(bytes, e)?)),
        None => Vec::new(),
    };
    let mut sheets = Vec::new();
    for e in entries
        .iter()
        .filter(|e| e.name.starts_with("xl/worksheets/") && e.name.ends_with(".xml"))
    {
        let xml = String::from_utf8_lossy(&extract(bytes, e)?).into_owned();
        let rows = sheet_rows(&xml, &shared);
        if !rows.is_empty() {
            sheets.push(rows);
        }
    }
    if sheets.is_empty() {
        bail!("spreadsheet has no worksheet");
    }
    Ok(sheets)
}

/// Convert an `.xlsx` workbook's worksheets into RFC-4180 CSV lines so that
/// every broker CSV parser (`groww`, `upstox`, `kite`, `indmoney`, `csv`)
/// can import `.xlsx` files directly without manual conversion.
pub fn workbook_to_csv(bytes: &[u8]) -> Result<String> {
    let sheets = read_all_sheets(bytes)?;
    let mut out = String::new();
    for sheet in sheets {
        for row in sheet {
            if row.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            for (i, cell) in row.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let clean = cell.replace(['\r', '\n'], " ");
                if clean.contains(',') || clean.contains('"') {
                    out.push('"');
                    out.push_str(&clean.replace('"', "\"\""));
                    out.push('"');
                } else {
                    out.push_str(&clean);
                }
            }
            out.push('\n');
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_amfi_ter_export() {
        // A real AMFI TER export (one fund house, October 2026): deflated
        // entries with data descriptors, inline `t="str"` text cells.
        let rows = read_first_sheet(include_bytes!("fixtures/amfi_ter_360one.xlsx")).unwrap();
        assert_eq!(rows.len(), 289);
        assert_eq!(rows[0][0], "NSDL Scheme Code");
        assert_eq!(rows[0][14], "Direct Plan - Total TER (%)");
        assert_eq!(rows[1][1], "360 ONE Balanced Hybrid Fund");
        assert_eq!(rows[1][4], "46296", "dates are Excel serial numbers");
        assert_eq!(rows[1][9], "2.19");
        assert_eq!(rows[1][14], "0.62");
        // Footer notes: text in the first cell, empty styled cells after it.
        let footer = rows.last().unwrap();
        assert!(
            footer[0].starts_with("For months May to March"),
            "{footer:?}"
        );
        assert!(footer[1..].iter().all(String::is_empty));
    }

    #[test]
    fn rejects_non_zip_input() {
        assert!(read_first_sheet(b"Scheme Code;Name").is_err());
        assert!(read_first_sheet(&[]).is_err());
    }

    #[test]
    fn parses_cells_shared_strings_and_gaps() {
        let shared = shared_strings(
            "<sst><si><t>Fund &amp; Co</t></si><si><r><t>Rich</t></r><r><t xml:space=\"preserve\"> text</t></r></si></sst>",
        );
        assert_eq!(shared, vec!["Fund & Co", "Rich text"]);
        let xml = r#"<sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1" s="1"><v>1.25</v></c></row><row r="2"><c r="B2" t="inlineStr"><is><t>A&lt;B &#8377;</t></is></c><c r="D2" s="3"/></row><row r="3"/></sheetData>"#;
        let rows = sheet_rows(xml, &shared);
        assert_eq!(rows[0], vec!["Fund & Co", "", "1.25"]);
        assert_eq!(rows[1], vec!["", "A<B ₹", "", ""]);
        assert!(rows[2].is_empty());
        assert_eq!(column_index("AA10"), Some(26));
        assert_eq!(unescape("a & b &unknown; &#x41;"), "a & b &unknown; A");
    }

    #[test]
    fn converts_workbook_to_rfc4180_csv() {
        let csv = workbook_to_csv(include_bytes!("fixtures/amfi_ter_360one.xlsx")).unwrap();
        let first_line = csv.lines().next().unwrap();
        assert!(first_line.starts_with("NSDL Scheme Code,"));
        assert!(csv.contains("360 ONE Balanced Hybrid Fund"));
    }
}
