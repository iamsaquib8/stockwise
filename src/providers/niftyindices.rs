//! NSE index members from niftyindices.com, the site of NSE Indices Ltd.
//! It publishes each index's members as a CSV meant for download
//! (`/IndexConstituent/ind_nifty500list.csv`). StockWise reads it when
//! nseindia.com refuses the request: members only, no prices, listed
//! alphabetically (not by market cap).

/// Where the member CSVs live.
pub const BASE_URL: &str = "https://www.niftyindices.com/IndexConstituent";

/// The CSV file for an NSE index name (`"NIFTY 500"`), `None` for an index
/// niftyindices.com has no list for here.
pub fn csv_file(index: &str) -> Option<&'static str> {
    Some(match index {
        "NIFTY 50" => "ind_nifty50list.csv",
        "NIFTY NEXT 50" => "ind_niftynext50list.csv",
        "NIFTY 100" => "ind_nifty100list.csv",
        "NIFTY BANK" => "ind_niftybanklist.csv",
        "NIFTY IT" => "ind_niftyitlist.csv",
        "NIFTY 500" => "ind_nifty500list.csv",
        "NIFTY MIDCAP 150" => "ind_niftymidcap150list.csv",
        "NIFTY SMALLCAP 250" => "ind_niftysmallcap250list.csv",
        _ => return None,
    })
}

/// The full URL of an index's member CSV.
pub fn csv_url(index: &str) -> Option<String> {
    csv_file(index).map(|f| format!("{}/{}", BASE_URL, f))
}

/// Yahoo symbols (`RELIANCE.NS`) from a member CSV, in file order. The
/// header names the columns (`Company Name,Industry,Symbol,Series,ISIN
/// Code`); `None` when there is no `Symbol` column or no member.
pub fn parse_members(csv: &str) -> Option<Vec<String>> {
    let mut lines = csv.lines().map(str::trim).filter(|l| !l.is_empty());
    let header = lines.next()?;
    let col = header.split(',').position(|h| {
        h.trim()
            .trim_start_matches('\u{feff}')
            .eq_ignore_ascii_case("symbol")
    })?;
    let out: Vec<String> = lines
        .filter_map(|l| {
            // Company names may be quoted and hold commas: read the symbol
            // column counting from the right when the row is longer.
            let cells = split_csv(l);
            let extra = cells.len().checked_sub(header.split(',').count())?;
            let sym = cells.get(col + extra)?.trim();
            let ok = !sym.is_empty()
                && sym
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '&' || c == '-');
            ok.then(|| format!("{}.NS", sym.to_ascii_uppercase()))
        })
        .collect();
    (!out.is_empty()).then_some(out)
}

/// Split one CSV row, honouring double quotes.
fn split_csv(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => cells.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    cells.push(cur);
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_symbols_as_yahoo_tickers() {
        let csv = "Company Name,Industry,Symbol,Series,ISIN Code\n\
                   360 ONE WAM Ltd.,Financial Services,360ONE,EQ,INE466L01038\n\
                   \"Mahindra & Mahindra, Ltd.\",Automobile,M&M,EQ,INE101A01026\n\
                   Bajaj Auto Ltd.,Automobile,BAJAJ-AUTO,EQ,INE917I01010\n";
        assert_eq!(
            parse_members(csv).unwrap(),
            ["360ONE.NS", "M&M.NS", "BAJAJ-AUTO.NS"]
        );
    }

    #[test]
    fn a_page_that_is_not_the_csv_is_rejected() {
        assert!(parse_members("<html><body>Access denied</body></html>").is_none());
        assert!(parse_members("Company Name,Industry,Symbol\n").is_none());
    }

    #[test]
    fn every_universe_has_a_file() {
        for index in [
            "NIFTY 50",
            "NIFTY NEXT 50",
            "NIFTY 100",
            "NIFTY BANK",
            "NIFTY IT",
            "NIFTY 500",
            "NIFTY MIDCAP 150",
            "NIFTY SMALLCAP 250",
        ] {
            assert!(csv_url(index).unwrap().ends_with(".csv"), "{index}");
        }
        assert!(csv_file("SENSEX").is_none());
    }
}
