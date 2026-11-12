//! Machine-readable output for scripts: `--json` and `--csv`.
//!
//! A command builds its rows once — structs deriving `Serialize` with
//! snake_case fields, holding the same numbers the table shows — and then
//! picks a renderer with [`Format`]. Only data goes to stdout; errors still
//! go to stderr through `anyhow`, with a non-zero exit code.

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use std::borrow::Cow;

/// How a command prints its result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    /// Coloured tables and charts for people (the default).
    #[default]
    Table,
    /// Pretty-printed JSON on stdout.
    Json,
    /// RFC 4180 CSV with a header row on stdout.
    Csv,
}

impl Format {
    pub fn from_flags(json: bool, csv: bool) -> Self {
        if json {
            Format::Json
        } else if csv {
            Format::Csv
        } else {
            Format::Table
        }
    }

    pub fn is_table(self) -> bool {
        self == Format::Table
    }
}

/// Print `value` as pretty JSON on stdout.
pub fn print_json<T: Serialize + ?Sized>(value: &T) -> Result<()> {
    // println! (not a writer) so `| head` exits quietly via the broken-pipe hook.
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// A row type that can be written as CSV.
///
/// `COLUMNS` names the serialized fields to write, in order. Every column
/// must be a field of the row (checked by [`missing_columns`] in tests).
pub trait CsvRow: Serialize {
    const COLUMNS: &'static [&'static str];
}

/// Neutralize spreadsheet formulas before RFC 4180 quoting. Leading formula
/// characters are escaped unless the entire field is a numeric value.
pub fn csv_escape(field: &str) -> Cow<'_, str> {
    let starts_formula = field
        .chars()
        .next()
        .is_some_and(|ch| matches!(ch, '=' | '+' | '-' | '@' | '\t' | '\r'));
    let neutralize = starts_formula && field.parse::<f64>().is_err();
    if neutralize || field.contains([',', '"', '\n', '\r']) {
        let safe = if neutralize {
            format!("'{field}")
        } else {
            field.to_string()
        };
        if safe.contains([',', '"', '\n', '\r']) {
            Cow::Owned(format!("\"{}\"", safe.replace('"', "\"\"")))
        } else {
            Cow::Owned(safe)
        }
    } else {
        Cow::Borrowed(field)
    }
}

/// One CSV record, newline-terminated.
pub fn csv_line<S: AsRef<str>>(fields: &[S]) -> String {
    let mut line = fields
        .iter()
        .map(|f| csv_escape(f.as_ref()))
        .collect::<Vec<_>>()
        .join(",");
    line.push('\n');
    line
}

/// Text of one JSON value as a CSV cell: null is empty, strings are
/// unquoted, numbers and booleans as JSON writes them.
pub fn csv_cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Header plus one record per row.
pub fn to_csv<R: CsvRow>(rows: &[R]) -> Result<String> {
    let mut out = csv_line(R::COLUMNS);
    for row in rows {
        let value = serde_json::to_value(row)?;
        let cells: Vec<String> = R::COLUMNS
            .iter()
            .map(|c| value.get(c).map(csv_cell).unwrap_or_default())
            .collect();
        out.push_str(&csv_line(&cells));
    }
    Ok(out)
}

pub fn print_csv<R: CsvRow>(rows: &[R]) -> Result<()> {
    print!("{}", to_csv(rows)?);
    Ok(())
}

/// Columns of `R` that `row` does not serialize, and serialized fields that
/// are not columns. Both empty means CSV and JSON carry the same fields.
#[cfg(test)]
pub fn missing_columns<R: CsvRow>(row: &R) -> (Vec<String>, Vec<String>) {
    let value = serde_json::to_value(row).expect("row serializes");
    let obj = value.as_object().expect("row serializes to an object");
    let missing = R::COLUMNS
        .iter()
        .filter(|c| !obj.contains_key(**c))
        .map(|c| c.to_string())
        .collect();
    let extra = obj
        .keys()
        .filter(|k| !R::COLUMNS.contains(&k.as_str()))
        .cloned()
        .collect();
    (missing, extra)
}

/// Round to `places` decimals so derived values (P&L, percentages) don't
/// print float noise such as `12.300000000000002` in JSON or CSV.
pub fn round(x: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (x * f).round() / f
}

pub fn round_opt(x: Option<f64>, places: i32) -> Option<f64> {
    x.map(|v| round(v, places))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Row {
        symbol: String,
        price: Option<f64>,
        held: bool,
    }

    impl CsvRow for Row {
        const COLUMNS: &'static [&'static str] = &["symbol", "price", "held"];
    }

    #[test]
    fn plain_fields_are_not_quoted() {
        assert_eq!(csv_escape("TCS.NS"), "TCS.NS");
        assert_eq!(csv_escape(""), "");
        assert_eq!(csv_escape("-12.5"), "-12.5");
    }

    #[test]
    fn special_characters_are_quoted_per_rfc_4180() {
        assert_eq!(csv_escape("M&M, Ltd"), "\"M&M, Ltd\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_escape("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_escape("cr\rhere"), "\"cr\rhere\"");
        assert_eq!(csv_escape("\""), "\"\"\"\"");
    }

    #[test]
    fn spreadsheet_formulas_are_neutralized_without_changing_numeric_cells() {
        assert_eq!(csv_escape("=1+1"), "'=1+1");
        assert_eq!(csv_escape("+SUM(A1:A2)"), "'+SUM(A1:A2)");
        assert_eq!(csv_escape("-HYPERLINK(A1)"), "'-HYPERLINK(A1)");
        assert_eq!(csv_escape("@SUM(A1:A2)"), "'@SUM(A1:A2)");
        assert_eq!(csv_escape("\t=1+1"), "'\t=1+1");
        assert_eq!(csv_escape("\r=1+1"), "\"'\r=1+1\"");
        assert_eq!(csv_escape("=HYPERLINK(\"https://example.test\",\"go\")"),
                   "\"'=HYPERLINK(\"\"https://example.test\"\",\"\"go\"\")\"");
        assert_eq!(csv_escape("-200.00"), "-200.00");
        assert_eq!(csv_escape("+200.00"), "+200.00");
        assert_eq!(csv_escape("RELIANCE.NS"), "RELIANCE.NS");
    }

    #[test]
    fn csv_line_joins_and_terminates() {
        assert_eq!(csv_line(&["a", "b,c", ""]), "a,\"b,c\",\n");
    }

    #[test]
    fn rows_render_with_header_nulls_empty() {
        let rows = vec![
            Row {
                symbol: "TCS.NS".into(),
                price: Some(3512.4),
                held: true,
            },
            Row {
                symbol: "Odd, \"Co\"".into(),
                price: None,
                held: false,
            },
        ];
        assert_eq!(
            to_csv(&rows).unwrap(),
            "symbol,price,held\nTCS.NS,3512.4,true\n\"Odd, \"\"Co\"\"\",,false\n"
        );
        assert_eq!(to_csv::<Row>(&[]).unwrap(), "symbol,price,held\n");
    }

    #[test]
    fn missing_columns_reports_both_directions() {
        let row = Row {
            symbol: "X".into(),
            price: None,
            held: false,
        };
        assert_eq!(missing_columns(&row), (vec![], vec![]));
    }

    #[test]
    fn format_from_flags() {
        assert_eq!(Format::from_flags(false, false), Format::Table);
        assert_eq!(Format::from_flags(true, false), Format::Json);
        assert_eq!(Format::from_flags(false, true), Format::Csv);
    }

    #[test]
    fn rounding_removes_float_noise() {
        assert_eq!(round(0.1 + 0.2, 2), 0.3);
        assert_eq!(round(-12.346, 2), -12.35);
        assert_eq!(round_opt(None, 2), None);
    }
}
