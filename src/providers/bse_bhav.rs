//! BSE's end-of-day bhavcopy: one CSV per trading day with the open, high,
//! low, close, previous close and volume of every listed security
//! (`/download/BhavCopy/Equity/BhavCopy_BSE_CM_0_0_0_YYYYMMDD_F_0000.CSV`, a
//! public download). StockWise keeps only stocks (`FinInstrmTp` = `STK`)
//! and files each under its NSE-style symbol (`ABB` → `ABB.NS`), which is
//! the same ticker for almost every company listed on both exchanges.

use crate::api::Quote;
use chrono::NaiveDate;

/// Where the daily files live.
pub const BASE_URL: &str = "https://www.bseindia.com/download/BhavCopy/Equity";

/// The file for one trading day.
pub fn url(date: NaiveDate) -> String {
    format!(
        "{}/BhavCopy_BSE_CM_0_0_0_{}_F_0000.CSV",
        BASE_URL,
        date.format("%Y%m%d")
    )
}

/// One stock's day.
#[derive(Debug, Clone, PartialEq)]
pub struct EodRow {
    /// BSE ticker (`ABB`), the NSE symbol for dual-listed companies.
    pub ticker: String,
    pub name: String,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub close: f64,
    pub prev_close: Option<f64>,
    pub volume: Option<u64>,
}

impl EodRow {
    /// The day as a quote under `ticker.NS`, marked as end of day.
    pub fn to_quote(&self, trade_date: NaiveDate) -> Quote {
        let change = self.prev_close.map(|p| self.close - p);
        let change_pct = self
            .prev_close
            .filter(|p| *p > 0.0)
            .map(|p| (self.close - p) / p * 100.0);
        Quote {
            symbol: Some(format!("{}.NS", self.ticker)),
            short_name: Some(self.name.clone()),
            currency: Some("INR".into()),
            regular_market_price: Some(self.close),
            regular_market_change: change,
            regular_market_change_percent: change_pct,
            regular_market_previous_close: self.prev_close,
            regular_market_open: self.open,
            regular_market_day_high: self.high,
            regular_market_day_low: self.low,
            regular_market_volume: self.volume,
            regular_market_time: trade_date
                .and_hms_opt(10, 0, 0)
                .map(|t| t.and_utc().timestamp()),
            data_source: Some(format!("BSE bhavcopy {}", trade_date)),
            ..Default::default()
        }
    }
}

/// The trade date and stock rows of a bhavcopy; `None` when the body is
/// not one (an HTML page, an empty day).
pub fn parse(csv: &str) -> Option<(NaiveDate, Vec<EodRow>)> {
    let mut lines = csv.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<&str> = lines.next()?.split(',').map(str::trim).collect();
    let col = |name: &str| header.iter().position(|h| h.eq_ignore_ascii_case(name));
    let (date_c, kind_c, tick_c, name_c, close_c) = (
        col("TradDt")?,
        col("FinInstrmTp")?,
        col("TckrSymb")?,
        col("FinInstrmNm")?,
        col("ClsPric")?,
    );
    let (open_c, high_c, low_c, prev_c, vol_c) = (
        col("OpnPric"),
        col("HghPric"),
        col("LwPric"),
        col("PrvsClsgPric"),
        col("TtlTradgVol"),
    );
    let mut date = None;
    let mut rows = Vec::new();
    for line in lines {
        let cells: Vec<&str> = line.split(',').map(str::trim).collect();
        let get = |c: usize| cells.get(c).copied().unwrap_or("");
        let num = |c: Option<usize>| c.and_then(|c| get(c).parse::<f64>().ok());
        if !get(kind_c).eq_ignore_ascii_case("STK") {
            continue;
        }
        let ticker = get(tick_c).to_ascii_uppercase();
        let Some(close) = num(Some(close_c)).filter(|c| *c > 0.0) else {
            continue;
        };
        if ticker.is_empty()
            || !ticker
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '&' || c == '-')
        {
            continue;
        }
        if date.is_none() {
            date = NaiveDate::parse_from_str(get(date_c), "%Y-%m-%d").ok();
        }
        rows.push(EodRow {
            ticker,
            name: get(name_c).to_string(),
            open: num(open_c),
            high: num(high_c),
            low: num(low_c),
            close,
            prev_close: num(prev_c),
            volume: num(vol_c).map(|v| v as u64),
        });
    }
    Some((date?, rows)).filter(|(_, r)| !r.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "TradDt,BizDt,Sgmt,Src,FinInstrmTp,FinInstrmId,ISIN,TckrSymb,SctySrs,XpryDt,FininstrmActlXpryDt,StrkPric,OptnTp,FinInstrmNm,OpnPric,HghPric,LwPric,ClsPric,LastPric,PrvsClsgPric,UndrlygPric,SttlmPric,OpnIntrst,ChngInOpnIntrst,TtlTradgVol,TtlTrfVal,TtlNbOfTxsExctd,SsnId,NewBrdLotQty,Rmks,Rsvd1,Rsvd2,Rsvd3,Rsvd4\n\
2026-11-06,2026-11-06,CM,BSE,STK,500002,INE117A01022,ABB,A,,,,,ABB INDIA LIMITED,6510.00,6774.00,6431.00,6774.00,6774.00,6640.10,,6774.00,,,28168,185307125.00,4355,F1,1,,,,,\n\
2026-11-06,2026-11-06,CM,BSE,MF,590001,INF966L01EI5,SOMEETF,E,,,,,SOME ETF,10.00,10.00,10.00,10.00,10.00,10.00,,10.00,,,5,50.00,1,F1,1,,,,,\n\
2026-11-06,2026-11-06,CM,BSE,STK,500570,INE155A01022,TMPV,A,,,,,TATA MOTORS PV,400.00,410.00,395.00,405.00,405.00,400.00,,405.00,,,1000,405000.00,90,F1,1,,,,,\n";

    #[test]
    fn keeps_stocks_and_reads_the_day() {
        let (date, rows) = parse(CSV).unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 11, 6).unwrap());
        assert_eq!(rows.len(), 2, "the ETF row is dropped");
        assert_eq!(rows[0].ticker, "ABB");
        assert_eq!(rows[0].close, 6774.0);
        assert_eq!(rows[0].prev_close, Some(6640.1));
        assert_eq!(rows[0].volume, Some(28168));
        let q = rows[0].to_quote(date);
        assert_eq!(q.symbol.as_deref(), Some("ABB.NS"));
        let pct = q.regular_market_change_percent.unwrap();
        assert!((pct - 2.0165).abs() < 0.001, "{pct}");
    }

    #[test]
    fn a_page_that_is_not_a_bhavcopy_is_rejected() {
        assert!(parse("<html><body>Not found</body></html>").is_none());
        assert!(parse(CSV.lines().next().unwrap()).is_none());
    }

    #[test]
    fn the_url_names_the_day() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
        assert!(url(d).ends_with("BhavCopy_BSE_CM_0_0_0_20261106_F_0000.CSV"));
    }
}
