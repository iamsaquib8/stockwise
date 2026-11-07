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

/// Excel's last column, `XFD`: a worksheet has at most this many columns.
const MAX_COLUMNS: usize = 16_384;

/// `B12` → column index 1. `None` when there are no column letters or the
/// column is past `XFD`, so a corrupt reference cannot ask for a huge row.
fn column_index(cell_ref: &str) -> Option<usize> {
    let mut n = 0usize;
    let mut any = false;
    for b in cell_ref.bytes().take_while(u8::is_ascii_alphabetic) {
        any = true;
        n = n * 26 + (b.to_ascii_uppercase() - b'A' + 1) as usize;
        // Stop before the value can overflow: it only grows with each letter.
        if n > MAX_COLUMNS {
            return None;
        }
    }
    any.then(|| n - 1)
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
            // A cell without a reference follows the previous one; a cell
            // whose reference is not a valid column is dropped.
            let col = match attr(&tag, "r") {
                Some(r) => match column_index(r) {
                    Some(col) => col,
                    None => continue,
                },
                None => cells.len(),
            };
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

/// One worksheet of a workbook: its tab name and its rows of text cells.
#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

impl Sheet {
    /// The sheet as RFC-4180 CSV lines, so every broker CSV parser (`groww`,
    /// `upstox`, `kite`, `indmoney`, `csv`, `cas`) can import it. Blank rows
    /// are dropped.
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        for row in &self.rows {
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
        out
    }
}

/// The `(name, zip path)` of each sheet in tab order: `xl/workbook.xml` lists
/// the sheets, and `xl/_rels/workbook.xml.rels` says which file each one is.
fn sheet_order(entries: &[Entry], bytes: &[u8]) -> Vec<(String, String)> {
    let read = |name: &str| -> Option<String> {
        let e = entries.iter().find(|e| e.name == name)?;
        Some(String::from_utf8_lossy(&extract(bytes, e).ok()?).into_owned())
    };
    let (Some(book), Some(rels)) = (read("xl/workbook.xml"), read("xl/_rels/workbook.xml.rels"))
    else {
        return Vec::new();
    };
    let targets: Vec<(String, String)> = rels
        .split("<Relationship ")
        .skip(1)
        .filter_map(|r| {
            let tag = format!(" {}", r.split('>').next()?);
            Some((
                attr(&tag, "Id")?.to_string(),
                attr(&tag, "Target")?.to_string(),
            ))
        })
        .collect();
    book.split("<sheet ")
        .skip(1)
        .filter_map(|t| {
            let tag = format!(" {}", t.split('>').next()?);
            let name = unescape(attr(&tag, "name")?);
            // `r:id`, or another prefix bound to the relationships namespace.
            let id = tag
                .split_whitespace()
                .find_map(|kv| kv.strip_prefix("r:id=\""))?
                .trim_end_matches(['"', '/']);
            let target = &targets.iter().find(|(i, _)| i == id)?.1;
            let path = match target.strip_prefix('/') {
                Some(abs) => abs.to_string(),
                None => format!("xl/{target}"),
            };
            Some((name, path))
        })
        .collect()
}

/// Every non-empty worksheet of the `.xlsx` workbook, in tab order. When the
/// workbook's sheet list cannot be read, the sheets come in zip order named
/// `Sheet1`, `Sheet2`, ...
pub fn read_workbook(bytes: &[u8]) -> Result<Vec<Sheet>> {
    let entries = central_directory(bytes)?;
    let shared = match entries.iter().find(|e| e.name == "xl/sharedStrings.xml") {
        Some(e) => shared_strings(&String::from_utf8_lossy(&extract(bytes, e)?)),
        None => Vec::new(),
    };
    let mut order = sheet_order(&entries, bytes);
    order.retain(|(_, path)| entries.iter().any(|e| &e.name == path));
    if order.is_empty() {
        order = entries
            .iter()
            .filter(|e| e.name.starts_with("xl/worksheets/") && e.name.ends_with(".xml"))
            .enumerate()
            .map(|(i, e)| (format!("Sheet{}", i + 1), e.name.clone()))
            .collect();
    }
    let mut sheets = Vec::new();
    for (name, path) in order {
        let e = entries.iter().find(|e| e.name == path).expect("retained");
        let xml = String::from_utf8_lossy(&extract(bytes, e)?).into_owned();
        let rows = sheet_rows(&xml, &shared);
        if !rows.is_empty() {
            sheets.push(Sheet { name, rows });
        }
    }
    if sheets.is_empty() {
        bail!("spreadsheet has no worksheet");
    }
    Ok(sheets)
}

/// The sheet to import: the one called `wanted` (any case), or else the first
/// whose CSV `matches` (its header has the columns the importer needs). When
/// none matches, the first sheet, so the importer reports what it found.
pub fn pick_sheet<'a>(
    sheets: &'a [Sheet],
    wanted: Option<&str>,
    matches: impl Fn(&str) -> bool,
) -> Result<&'a Sheet> {
    if let Some(wanted) = wanted {
        return sheets
            .iter()
            .find(|s| s.name.trim().eq_ignore_ascii_case(wanted.trim()))
            .with_context(|| {
                let names: Vec<String> = sheets.iter().map(|s| format!("\"{}\"", s.name)).collect();
                format!(
                    "No sheet named \"{wanted}\" in the workbook. Sheets: {}",
                    names.join(", ")
                )
            });
    }
    sheets
        .iter()
        .find(|s| matches(&s.to_csv()))
        .or(sheets.first())
        .context("spreadsheet has no worksheet")
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
    fn column_index_is_bounded_to_excels_last_column() {
        assert_eq!(column_index("A1"), Some(0));
        assert_eq!(column_index("b12"), Some(1));
        assert_eq!(column_index("Z1"), Some(25));
        assert_eq!(column_index("AA1"), Some(26));
        // XFD is column 16,384, index 16,383: the last one allowed.
        // X=24, F=6, D=4: ((24 * 26) + 6) * 26 + 4 = 16,384.
        assert_eq!(column_index("XFD1"), Some(16_383));
        assert_eq!(column_index("XFE1"), None);
        assert_eq!(column_index("ZZZZZ1"), None);
        // Enough letters to overflow a usize if folded without a bound.
        assert_eq!(column_index(&"Z".repeat(64)), None);
        assert_eq!(column_index("1"), None);
        assert_eq!(column_index(""), None);
    }

    #[test]
    fn hostile_cell_references_are_dropped_not_allocated() {
        // `ZZZZZZZZ1` would have resized the row to about 2e11 strings; a huge
        // row number is ignored because rows are read in document order.
        let xml = r#"<sheetData><row r="99999999999999999999"><c r="A99999999999999999999"><v>1</v></c><c r="ZZZZZZZZ1"><v>evil</v></c><c r="C1"><v>3</v></c></row></sheetData>"#;
        let rows = sheet_rows(xml, &[]);
        assert_eq!(rows, vec![vec!["1", "", "3"]]);
        // A cell with no reference at all still follows the previous one.
        let rows = sheet_rows(
            r#"<row><c s="1"><v>x</v></c><c s="1"><v>y</v></c></row>"#,
            &[],
        );
        assert_eq!(rows, vec![vec!["x", "y"]]);
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
        let sheets = read_workbook(include_bytes!("fixtures/amfi_ter_360one.xlsx")).unwrap();
        let csv = sheets[0].to_csv();
        let first_line = csv.lines().next().unwrap();
        assert!(first_line.starts_with("NSDL Scheme Code,"));
        assert!(csv.contains("360 ONE Balanced Hybrid Fund"));
    }

    #[test]
    fn sheets_come_in_tab_order_with_their_names() {
        // The workbook lists "Mutual Funds" (sheet2.xml) before "Equity" (sheet1.xml).
        let sheets = read_workbook(include_bytes!("fixtures/holdings_two_sheets.xlsx")).unwrap();
        let names: Vec<&str> = sheets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Mutual Funds", "Equity"]);
        assert_eq!(sheets[0].rows[0], ["Fund", "Units", "NAV", "ISIN"]);
        assert_eq!(sheets[1].rows[1], ["INFY", "10", "1500"]);
    }

    #[test]
    fn pick_sheet_takes_the_first_matching_header_or_the_named_sheet() {
        let sheets = read_workbook(include_bytes!("fixtures/holdings_two_sheets.xlsx")).unwrap();
        let has_symbol = |csv: &str| csv.to_lowercase().contains("symbol");
        assert_eq!(
            pick_sheet(&sheets, None, has_symbol).unwrap().name,
            "Equity"
        );
        // Nothing matches: the first sheet, so the importer can say what it found.
        assert_eq!(
            pick_sheet(&sheets, None, |_| false).unwrap().name,
            "Mutual Funds"
        );
        let named = pick_sheet(&sheets, Some(" mutual funds "), has_symbol).unwrap();
        assert_eq!(named.name, "Mutual Funds");
        let err = pick_sheet(&sheets, Some("Summary"), has_symbol)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("\"Equity\"") && err.contains("Summary"),
            "{err}"
        );
    }
}
