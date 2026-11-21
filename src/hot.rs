//! `stockwise hot`: what's hot in your investments and in the market.
//!
//! **Your investments** ranks holdings and watchlist names by *heat*:
//! the absolute excess move in σ, the volume surge, the importance of fresh
//! news or filings and a date within three days, multiplied up by the
//! portfolio weight, so a 5σ move in a 20% holding outranks the same move in
//! a 1% one.
//!
//! **Market** scans the universe's quotes for unusual volume (RVOL, paced
//! for the time of day while the session is open), gaps and 52-week
//! breakouts/breakdowns; adds NSE bulk and block deals, FII/DII flows, the
//! F&O ban list and OI build-up, sector leaders, theme baskets, the stocks
//! most mentioned in today's news and cross-market movers.
//!
//! The parsers and scoring here are pure; `commands::hot` fetches. All of
//! it describes data — none of it is a recommendation.

use crate::api::Quote;
use crate::pulse::Row;
use serde::Serialize;
use serde_json::Value;

// ── Heat ──

/// The parts of a row's heat, for `--json` and explanations.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Heat {
    pub moved: f64,
    pub volume: f64,
    pub news: f64,
    pub event: f64,
    /// Sum of the parts.
    pub raw: f64,
    /// `raw` scaled up by the portfolio weight.
    pub score: f64,
}

/// Heat of one row. Pure.
pub fn heat(row: &Row) -> Heat {
    // |excess move| in σ, capped so one wild print doesn't swamp the rest;
    // without bars, the plain day move per 2%.
    let moved = row
        .sigma
        .map(|z| z.abs())
        .or_else(|| row.change_pct.map(|c| c.abs() / 2.0))
        .unwrap_or(0.0)
        .min(6.0);
    let volume = row
        .volume_x
        .map(|v| (v - 1.0).clamp(0.0, 4.0))
        .unwrap_or(0.0);
    let news = row.news.iter().map(|n| n.importance).fold(0.0, f64::max) / 2.5;
    let news = news.min(4.0);
    let event = if row.events.is_empty() { 0.0 } else { 1.5 };
    let raw = moved + volume + news + event;
    Heat {
        moved,
        volume,
        news,
        event,
        raw,
        score: raw * (1.0 + 5.0 * row.weight.clamp(0.0, 1.0)),
    }
}

/// Rows hottest first, with their heat; rows with no heat at all are left
/// out.
pub fn rank(rows: &[Row]) -> Vec<(&Row, Heat)> {
    let mut v: Vec<(&Row, Heat)> = rows
        .iter()
        .map(|r| (r, heat(r)))
        .filter(|(_, h)| h.raw > 0.0)
        .collect();
    v.sort_by(|a, b| b.1.score.total_cmp(&a.1.score));
    v
}

// ── Market scanners on quotes ──

/// A stock in a market list.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Mover {
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub price: Option<f64>,
    pub change_pct: Option<f64>,
    /// Volume against the 3-month average, paced for the time of day.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rvol: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap_pct: Option<f64>,
    /// Why it is listed.
    pub note: String,
}

fn mover(q: &Quote, rvol: Option<f64>, note: String) -> Option<Mover> {
    Some(Mover {
        symbol: q.symbol.clone()?,
        name: q.short_name.clone().or(q.long_name.clone()),
        price: q.regular_market_price,
        change_pct: q.regular_market_change_percent,
        rvol,
        gap_pct: gap_pct(q),
        note,
    })
}

/// Today's volume against the 3-month daily average. `session_elapsed` is
/// the fraction of the regular session gone (1 after the close): early in
/// the day the volume is compared with the same share of an average day.
pub fn rvol(q: &Quote, session_elapsed: f64) -> Option<f64> {
    let v = q.regular_market_volume? as f64;
    let avg = q.average_daily_volume_3_month? as f64;
    // Volume through the day is front-loaded; never pace below 15%.
    let pace = session_elapsed.clamp(0.15, 1.0);
    (avg > 0.0 && v > 0.0).then(|| v / (avg * pace))
}

pub fn gap_pct(q: &Quote) -> Option<f64> {
    let open = q.regular_market_open.filter(|o| *o > 0.0)?;
    let prev = q.regular_market_previous_close.filter(|p| *p > 0.0)?;
    Some((open / prev - 1.0) * 100.0)
}

/// Minimum turnover for a scan entry, so a few hundred shares of an
/// illiquid name don't count as "unusual volume".
fn liquid(q: &Quote, min_value: f64) -> bool {
    let p = q.regular_market_price.unwrap_or(0.0);
    let v = q.average_daily_volume_3_month.unwrap_or(0) as f64;
    p * v >= min_value
}

/// Unusual volume (RVOL ≥ 2), highest first.
pub fn unusual_volume(
    quotes: &[Quote],
    session_elapsed: f64,
    min_value: f64,
    n: usize,
) -> Vec<Mover> {
    let mut v: Vec<(f64, &Quote)> = quotes
        .iter()
        .filter(|q| liquid(q, min_value))
        .filter_map(|q| Some((rvol(q, session_elapsed)?, q)))
        .filter(|(r, _)| *r >= 2.0)
        .collect();
    v.sort_by(|a, b| b.0.total_cmp(&a.0));
    v.into_iter()
        .take(n)
        .filter_map(|(r, q)| mover(q, Some(r), format!("{r:.1}× usual volume")))
        .collect()
}

/// Opening gaps of at least `min_pct`, largest first.
pub fn gaps(quotes: &[Quote], min_pct: f64, min_value: f64, n: usize) -> Vec<Mover> {
    let mut v: Vec<(f64, &Quote)> = quotes
        .iter()
        .filter(|q| liquid(q, min_value))
        .filter_map(|q| Some((gap_pct(q)?, q)))
        .filter(|(g, _)| g.abs() >= min_pct)
        .collect();
    v.sort_by(|a, b| b.0.abs().total_cmp(&a.0.abs()));
    v.into_iter()
        .take(n)
        .filter_map(|(g, q)| {
            mover(
                q,
                None,
                format!(
                    "gapped {} {:.1}%",
                    if g > 0.0 { "up" } else { "down" },
                    g.abs()
                ),
            )
        })
        .collect()
}

/// New 52-week highs (`up`) or lows, by day move.
pub fn range_breaks(quotes: &[Quote], up: bool, min_value: f64, n: usize) -> Vec<Mover> {
    let mut v: Vec<&Quote> = quotes
        .iter()
        .filter(|q| liquid(q, min_value))
        .filter(|q| {
            let Some(p) = q.regular_market_price else {
                return false;
            };
            if up {
                q.fifty_two_week_high
                    .is_some_and(|h| h > 0.0 && p >= h * 0.998)
            } else {
                q.fifty_two_week_low
                    .is_some_and(|l| l > 0.0 && p <= l * 1.002)
            }
        })
        .collect();
    v.sort_by(|a, b| {
        let c = |q: &Quote| q.regular_market_change_percent.unwrap_or(0.0);
        if up {
            c(b).total_cmp(&c(a))
        } else {
            c(a).total_cmp(&c(b))
        }
    });
    v.into_iter()
        .take(n)
        .filter_map(|q| {
            mover(
                q,
                None,
                if up {
                    "at a 52-week high"
                } else {
                    "at a 52-week low"
                }
                .into(),
            )
        })
        .collect()
}

/// Biggest movers by absolute day change, with their RVOL.
pub fn top_movers(quotes: &[Quote], session_elapsed: f64, min_value: f64, n: usize) -> Vec<Mover> {
    let mut v: Vec<&Quote> = quotes
        .iter()
        .filter(|q| liquid(q, min_value))
        .filter(|q| q.regular_market_change_percent.is_some_and(f64::is_finite))
        .collect();
    v.sort_by(|a, b| {
        b.regular_market_change_percent
            .unwrap_or(0.0)
            .abs()
            .total_cmp(&a.regular_market_change_percent.unwrap_or(0.0).abs())
    });
    v.into_iter()
        .take(n)
        .filter_map(|q| {
            let r = rvol(q, session_elapsed);
            let note = r.map(|r| format!("{r:.1}× volume")).unwrap_or_default();
            mover(q, r, note)
        })
        .collect()
}

/// Stocks named most often in `titles`, by company name or ticker, as
/// `(symbol, mentions)`. Names are matched as whole words.
pub fn most_mentioned(titles: &[String], quotes: &[Quote], n: usize) -> Vec<(String, usize)> {
    let lower: Vec<String> = titles
        .iter()
        .map(|t| format!(" {} ", normalize(t)))
        .collect();
    let mut counts: Vec<(String, usize)> = quotes
        .iter()
        .filter_map(|q| {
            let sym = q.symbol.clone()?;
            let base = sym.split('.').next().unwrap_or(&sym).to_lowercase();
            let name = q
                .short_name
                .as_deref()
                .or(q.long_name.as_deref())
                .map(crate::research::clean_company_name)
                .map(|n| normalize(&n))
                .filter(|n| n.len() >= 4);
            let needles: Vec<String> = std::iter::once(base)
                .filter(|b| b.len() >= 3 && !COMMON_WORDS.contains(&b.as_str()))
                .chain(name)
                .map(|w| format!(" {w} "))
                .collect();
            let c = lower
                .iter()
                .filter(|t| needles.iter().any(|w| t.contains(w.as_str())))
                .count();
            (c > 0).then_some((sym, c))
        })
        .collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    counts.truncate(n);
    counts
}

/// Tickers that are also English words would match everything.
const COMMON_WORDS: &[&str] = &[
    "all", "any", "are", "can", "for", "has", "now", "one", "the", "big", "new", "low", "key",
    "ups", "car", "fast", "gold", "life", "real", "well", "good", "safe", "bank", "info",
];

fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '&' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Sectors and themes ──

/// Sector indices (India) or sector ETFs (US).
pub fn sector_symbols(india: bool) -> &'static [(&'static str, &'static str)] {
    if india {
        &[
            ("^CNXIT", "IT"),
            ("^NSEBANK", "Bank"),
            ("^CNXAUTO", "Auto"),
            ("^CNXPHARMA", "Pharma"),
            ("^CNXFMCG", "FMCG"),
            ("^CNXMETAL", "Metal"),
            ("^CNXREALTY", "Realty"),
            ("^CNXENERGY", "Energy"),
            ("^CNXPSUBANK", "PSU Bank"),
            ("^CNXMEDIA", "Media"),
            ("^CNXINFRA", "Infra"),
        ]
    } else {
        &[
            ("XLK", "Technology"),
            ("XLF", "Financials"),
            ("XLE", "Energy"),
            ("XLV", "Health care"),
            ("XLY", "Consumer disc."),
            ("XLP", "Staples"),
            ("XLI", "Industrials"),
            ("XLB", "Materials"),
            ("XLU", "Utilities"),
            ("XLRE", "Real estate"),
            ("XLC", "Communication"),
        ]
    }
}

/// A theme basket (#359): an equal-weight list of stocks tied to a theme.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub india: &'static [&'static str],
    pub us: &'static [&'static str],
}

pub const THEMES: &[Theme] = &[
    Theme {
        name: "AI",
        india: &[
            "PERSISTENT.NS",
            "COFORGE.NS",
            "LTIM.NS",
            "TATAELXSI.NS",
            "KPITTECH.NS",
            "NETWEB.NS",
            "HAPPSTMNDS.NS",
        ],
        us: &["NVDA", "MSFT", "GOOGL", "META", "PLTR", "AMD", "SMCI"],
    },
    Theme {
        name: "Semiconductors",
        india: &[
            "DIXON.NS",
            "KAYNES.NS",
            "CGPOWER.NS",
            "SYRMA.NS",
            "AVALON.NS",
            "MOSCHIP.NS",
        ],
        us: &[
            "NVDA", "AVGO", "TSM", "ASML", "AMD", "MU", "QCOM", "AMAT", "LRCX", "INTC",
        ],
    },
    Theme {
        name: "Defence",
        india: &[
            "HAL.NS",
            "BEL.NS",
            "BDL.NS",
            "MAZDOCK.NS",
            "COCHINSHIP.NS",
            "BEML.NS",
            "DATAPATTNS.NS",
            "SOLARINDS.NS",
        ],
        us: &["LMT", "RTX", "NOC", "GD", "LHX", "HII"],
    },
    Theme {
        name: "Battery & EV",
        india: &[
            "EXIDEIND.NS",
            "ARE&M.NS",
            "OLAELEC.NS",
            "HBLENGINE.NS",
            "AMARAJABAT.NS",
        ],
        us: &["TSLA", "ALB", "QS", "ENVX", "EOSE", "RIVN"],
    },
    Theme {
        name: "Minerals & metals",
        india: &[
            "NMDC.NS",
            "HINDZINC.NS",
            "VEDL.NS",
            "COALINDIA.NS",
            "NATIONALUM.NS",
            "HINDCOPPER.NS",
            "GMDCLTD.NS",
        ],
        us: &["FCX", "MP", "NEM", "SCCO", "LAC", "ALB"],
    },
];

/// One sector or theme and its day move.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Group {
    pub name: String,
    pub change_pct: Option<f64>,
    /// Members priced / members.
    pub priced: usize,
    pub members: usize,
    /// Best and worst member today.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best: Option<(String, f64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst: Option<(String, f64)>,
}

/// Equal-weight day move of each theme from `quotes`, strongest first.
pub fn theme_moves(quotes: &[Quote], india: bool) -> Vec<Group> {
    let mut out: Vec<Group> = THEMES
        .iter()
        .map(|t| {
            let members = if india { t.india } else { t.us };
            let moves: Vec<(String, f64)> = members
                .iter()
                .filter_map(|m| {
                    let q = quotes.iter().find(|q| q.symbol.as_deref() == Some(*m))?;
                    let c = q.regular_market_change_percent.filter(|c| c.is_finite())?;
                    Some((m.to_string(), c))
                })
                .collect();
            let avg = (!moves.is_empty())
                .then(|| moves.iter().map(|(_, c)| c).sum::<f64>() / moves.len() as f64);
            let best = moves.iter().max_by(|a, b| a.1.total_cmp(&b.1)).cloned();
            let worst = moves.iter().min_by(|a, b| a.1.total_cmp(&b.1)).cloned();
            Group {
                name: t.name.into(),
                change_pct: avg,
                priced: moves.len(),
                members: members.len(),
                best,
                worst,
            }
        })
        .collect();
    sort_groups(&mut out);
    out
}

/// Sector moves from index or ETF quotes, strongest first; missing last.
pub fn sector_moves(quotes: &[Quote], india: bool) -> Vec<Group> {
    let mut out: Vec<Group> = sector_symbols(india)
        .iter()
        .map(|(sym, name)| {
            let c = quotes
                .iter()
                .find(|q| q.symbol.as_deref() == Some(*sym))
                .and_then(|q| q.regular_market_change_percent)
                .filter(|c| c.is_finite());
            Group {
                name: name.to_string(),
                change_pct: c,
                priced: usize::from(c.is_some()),
                members: 1,
                best: None,
                worst: None,
            }
        })
        .collect();
    sort_groups(&mut out);
    out
}

fn sort_groups(v: &mut [Group]) {
    v.sort_by(|a, b| match (a.change_pct, b.change_pct) {
        (Some(x), Some(y)) => y.total_cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.name.cmp(&b.name),
    });
}

// ── NSE market data ──

/// A bulk or block deal.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Deal {
    pub kind: &'static str,
    pub date: String,
    pub symbol: String,
    pub name: String,
    pub client: String,
    pub buy: bool,
    pub qty: f64,
    pub price: f64,
}

impl Deal {
    pub fn value(&self) -> f64 {
        self.qty * self.price
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.replace(',', "").trim().parse().ok(),
        _ => None,
    }
}

fn text(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

/// NSE `snapshot-capital-market-largedeal`: bulk and block deals, largest
/// value first.
pub fn parse_large_deals(v: &Value) -> Vec<Deal> {
    let mut out = Vec::new();
    for (key, kind) in [("BULK_DEALS_DATA", "bulk"), ("BLOCK_DEALS_DATA", "block")] {
        for d in v.get(key).and_then(Value::as_array).into_iter().flatten() {
            let (Some(qty), Some(price)) =
                (d.get("qty").and_then(num), d.get("watp").and_then(num))
            else {
                continue;
            };
            let side = text(d, "buySell").to_ascii_uppercase();
            out.push(Deal {
                kind,
                date: text(d, "date"),
                symbol: text(d, "symbol"),
                name: text(d, "name"),
                client: text(d, "clientName"),
                buy: side.starts_with('B'),
                qty,
                price,
            });
        }
    }
    out.sort_by(|a, b| b.value().total_cmp(&a.value()));
    out
}

/// One day's net buying by foreign and domestic institutions, ₹ crore.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Flows {
    pub date: String,
    pub fii_net: Option<f64>,
    pub dii_net: Option<f64>,
    pub fii_buy: Option<f64>,
    pub fii_sell: Option<f64>,
    pub dii_buy: Option<f64>,
    pub dii_sell: Option<f64>,
}

/// NSE `fiidiiTradeReact`.
pub fn parse_fii_dii(v: &Value) -> Option<Flows> {
    let rows = v.as_array()?;
    let find = |pfx: &str| {
        rows.iter()
            .find(|r| text(r, "category").to_ascii_uppercase().starts_with(pfx))
    };
    let fii = find("FII");
    let dii = find("DII");
    if fii.is_none() && dii.is_none() {
        return None;
    }
    let get = |r: Option<&Value>, k: &str| r.and_then(|r| r.get(k)).and_then(num);
    Some(Flows {
        date: fii.or(dii).map(|r| text(r, "date")).unwrap_or_default(),
        fii_net: get(fii, "netValue"),
        dii_net: get(dii, "netValue"),
        fii_buy: get(fii, "buyValue"),
        fii_sell: get(fii, "sellValue"),
        dii_buy: get(dii, "buyValue"),
        dii_sell: get(dii, "sellValue"),
    })
}

/// NSE `fo_secban.csv`: the date line, then `n,SYMBOL` rows.
pub fn parse_fo_ban(csv: &str) -> Vec<String> {
    csv.lines()
        .skip(1)
        .filter_map(|l| {
            let sym = l.split(',').nth(1)?.trim();
            (!sym.is_empty()
                && sym
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "&-_".contains(c)))
            .then(|| sym.to_string())
        })
        .collect()
}

/// An open-interest build-up: OI change combined with the price move.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OiMove {
    pub symbol: String,
    /// Open-interest change, percent.
    pub oi_change_pct: f64,
    pub price_change_pct: Option<f64>,
    /// `long build-up`, `short build-up`, `short covering`, `long unwinding`.
    pub reading: &'static str,
}

/// The textbook reading of price and OI moving together.
pub fn oi_reading(price_up: bool, oi_up: bool) -> &'static str {
    match (price_up, oi_up) {
        (true, true) => "long build-up",
        (false, true) => "short build-up",
        (true, false) => "short covering",
        (false, false) => "long unwinding",
    }
}

/// NSE `live-analysis-oi-spurts-underlyings`, with day changes from
/// `quotes` (`SYMBOL.NS`), largest OI change first.
pub fn parse_oi_spurts(v: &Value, quotes: &[Quote], n: usize) -> Vec<OiMove> {
    let rows = v
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<OiMove> = rows
        .iter()
        .filter_map(|r| {
            let symbol = text(r, "symbol");
            let latest = r.get("latestOI").and_then(num)?;
            let prev = r.get("prevOI").and_then(num).filter(|p| *p > 0.0)?;
            let oi = (latest / prev - 1.0) * 100.0;
            let yahoo = format!("{symbol}.NS");
            let pc = quotes
                .iter()
                .find(|q| q.symbol.as_deref() == Some(yahoo.as_str()))
                .and_then(|q| q.regular_market_change_percent);
            Some(OiMove {
                reading: oi_reading(pc.unwrap_or(0.0) >= 0.0, oi >= 0.0),
                symbol,
                oi_change_pct: oi,
                price_change_pct: pc,
            })
        })
        .collect();
    out.sort_by(|a, b| b.oi_change_pct.abs().total_cmp(&a.oi_change_pct.abs()));
    out.truncate(n);
    out
}

/// Fraction of the regular session elapsed at `now` (1 when closed).
pub fn session_elapsed(market: crate::market::Market, now: chrono::DateTime<chrono::Utc>) -> f64 {
    if !crate::clock::is_open(market, now) {
        return 1.0;
    }
    let date = crate::clock::session_date(market, now);
    let (Some(close), Some(open)) = (
        crate::clock::close_at(market, date),
        crate::clock::next_open(market, now - chrono::Duration::hours(12)),
    ) else {
        return 1.0;
    };
    let total = (close - open).num_seconds() as f64;
    if total <= 0.0 {
        return 1.0;
    }
    ((now - open).num_seconds() as f64 / total).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulse::Why;

    fn row(symbol: &str, weight: f64, sigma: Option<f64>, vol: Option<f64>) -> Row {
        Row {
            symbol: symbol.into(),
            name: None,
            held: weight > 0.0,
            weight,
            price: Some(100.0),
            currency: None,
            change_pct: Some(-3.0),
            day_pnl: None,
            bench_pct: None,
            sigma,
            volume_x: vol,
            gap_pct: None,
            range_break: None,
            why: Why::Quiet,
            news: vec![],
            events: vec![],
            action: None,
        }
    }

    fn quote(sym: &str, price: f64, chg: f64, vol: u64, avg: u64) -> Quote {
        Quote {
            symbol: Some(sym.into()),
            regular_market_price: Some(price),
            regular_market_change_percent: Some(chg),
            regular_market_volume: Some(vol),
            average_daily_volume_3_month: Some(avg),
            ..Default::default()
        }
    }

    #[test]
    fn heat_ranks_a_big_holding_above_the_same_move_in_a_small_one() {
        let rows = vec![
            row("SMALL.NS", 0.01, Some(-4.0), Some(2.0)),
            row("BIG.NS", 0.25, Some(-4.0), Some(2.0)),
            row("WATCH.NS", 0.0, Some(-5.0), Some(3.0)),
            row("QUIET.NS", 0.10, Some(0.0), Some(1.0)),
        ];
        let ranked = rank(&rows);
        let order: Vec<&str> = ranked.iter().map(|(r, _)| r.symbol.as_str()).collect();
        // BIG: 5 × 2.25 = 11.25; WATCH: 7 × 1 = 7; SMALL: 5 × 1.05.
        assert_eq!(order[..3], ["BIG.NS", "WATCH.NS", "SMALL.NS"]);
        // A holding with nothing going on is left out, not ranked last.
        assert!(!order.contains(&"QUIET.NS"));
        let h = ranked[0].1;
        assert_eq!(h.moved, 4.0);
        assert_eq!(h.volume, 1.0);
    }

    #[test]
    fn news_and_events_add_heat_and_sigma_is_capped() {
        let mut r = row("X.NS", 0.0, Some(-20.0), None);
        assert_eq!(heat(&r).moved, 6.0);
        r.events.push(crate::calendar::Event {
            symbol: "X.NS".into(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 11, 23).unwrap(),
            kind: crate::calendar::EventKind::Results,
            detail: String::new(),
            source: crate::calendar::Source::Nse,
            estimate: false,
        });
        assert_eq!(heat(&r).event, 1.5);
    }

    #[test]
    fn rvol_is_paced_for_the_time_of_day() {
        let q = quote("A.NS", 100.0, 1.0, 1_000_000, 1_000_000);
        assert_eq!(rvol(&q, 1.0), Some(1.0));
        // Halfway through the session with a full day's volume: 2×.
        assert_eq!(rvol(&q, 0.5), Some(2.0));
        // Never paced below 15% (the first minutes would explode).
        assert!((rvol(&q, 0.01).unwrap() - 1.0 / 0.15).abs() < 1e-9);
    }

    #[test]
    fn scanners_find_volume_gaps_and_range_breaks() {
        let mut hot = quote("HOT.NS", 500.0, 6.0, 5_000_000, 1_000_000);
        hot.regular_market_open = Some(530.0);
        hot.regular_market_previous_close = Some(500.0 / 1.06);
        hot.fifty_two_week_high = Some(500.5);
        let illiquid = quote("TINY.NS", 2.0, 15.0, 90_000, 10_000);
        let mut low = quote("LOW.NS", 50.0, -2.0, 1_000_000, 900_000);
        low.fifty_two_week_low = Some(50.0);
        let qs = vec![hot, illiquid, low];
        let uv = unusual_volume(&qs, 1.0, 1e6, 5);
        assert_eq!(uv.len(), 1);
        assert_eq!(uv[0].symbol, "HOT.NS");
        let g = gaps(&qs, 3.0, 1e6, 5);
        assert_eq!(g[0].symbol, "HOT.NS");
        assert!(g[0].note.contains("gapped up"));
        assert_eq!(range_breaks(&qs, true, 1e6, 5)[0].symbol, "HOT.NS");
        assert_eq!(range_breaks(&qs, false, 1e6, 5)[0].symbol, "LOW.NS");
    }

    #[test]
    fn mentions_count_whole_names_and_skip_common_words() {
        let mut tcs = quote("TCS.NS", 1.0, 0.0, 0, 0);
        tcs.short_name = Some("Tata Consultancy Services Limited".into());
        let mut one = quote("ONE.NS", 1.0, 0.0, 0, 0);
        one.short_name = Some("One97".into());
        let titles = vec![
            "TCS wins $1bn deal".to_string(),
            "Tata Consultancy Services shares rise".to_string(),
            "One more rate cut?".to_string(),
            "TCSL unrelated".to_string(),
        ];
        let m = most_mentioned(&titles, &[tcs, one], 5);
        assert_eq!(m, vec![("TCS.NS".to_string(), 2)]);
    }

    #[test]
    fn themes_and_sectors_sort_strongest_first_with_missing_last() {
        let qs = vec![
            quote("HAL.NS", 1.0, 3.0, 0, 0),
            quote("BEL.NS", 1.0, 1.0, 0, 0),
            quote("NMDC.NS", 1.0, -1.0, 0, 0),
            quote("^CNXIT", 1.0, 0.5, 0, 0),
            quote("^CNXAUTO", 1.0, f64::NAN, 0, 0),
        ];
        let t = theme_moves(&qs, true);
        assert_eq!(t[0].name, "Defence");
        assert_eq!(t[0].change_pct, Some(2.0));
        assert_eq!(t[0].best, Some(("HAL.NS".into(), 3.0)));
        assert!(t.last().unwrap().change_pct.is_none());
        let s = sector_moves(&qs, true);
        assert_eq!(s[0].name, "IT");
        assert!(
            s.iter()
                .find(|g| g.name == "Auto")
                .unwrap()
                .change_pct
                .is_none()
        );
    }

    #[test]
    fn nse_deals_flows_ban_and_oi_parse() {
        let deals = serde_json::json!({
            "BULK_DEALS_DATA": [
                {"date": "21-Nov-2026", "symbol": "ABC", "name": "ABC Ltd", "clientName": "Big Fund", "buySell": "BUY", "qty": "1,00,000", "watp": "250.5"},
                {"date": "21-Nov-2026", "symbol": "XYZ", "name": "XYZ Ltd", "clientName": "Seller", "buySell": "SELL", "qty": 10, "watp": 10}
            ],
            "BLOCK_DEALS_DATA": [
                {"date": "21-Nov-2026", "symbol": "BIG", "name": "Big Ltd", "clientName": "Promoter", "buySell": "SELL", "qty": 5000000, "watp": 1000}
            ]
        });
        let d = parse_large_deals(&deals);
        assert_eq!(d.len(), 3);
        assert_eq!(
            (d[0].symbol.as_str(), d[0].kind, d[0].buy),
            ("BIG", "block", false)
        );
        assert_eq!(d[1].qty, 100_000.0);

        let flows = serde_json::json!([
            {"category": "DII **", "date": "20-Nov-2026", "buyValue": "12000.5", "sellValue": "10000", "netValue": "2000.5"},
            {"category": "FII/FPI **", "date": "20-Nov-2026", "buyValue": "9000", "sellValue": "11500", "netValue": "-2500"}
        ]);
        let f = parse_fii_dii(&flows).unwrap();
        assert_eq!(f.fii_net, Some(-2500.0));
        assert_eq!(f.dii_net, Some(2000.5));
        assert_eq!(f.date, "20-Nov-2026");

        let ban = "Securities in Ban For Trade Date 21-NOV-2026:\n1,RBLBANK\n2,M&MFIN\n";
        assert_eq!(parse_fo_ban(ban), vec!["RBLBANK", "M&MFIN"]);

        let oi = serde_json::json!({"data": [
            {"symbol": "AAA", "latestOI": 1300, "prevOI": 1000},
            {"symbol": "BBB", "latestOI": 900, "prevOI": 1000}
        ]});
        let qs = vec![
            quote("AAA.NS", 1.0, -2.0, 0, 0),
            quote("BBB.NS", 1.0, 1.0, 0, 0),
        ];
        let o = parse_oi_spurts(&oi, &qs, 5);
        assert_eq!(o[0].symbol, "AAA");
        assert_eq!(o[0].reading, "short build-up");
        assert_eq!(o[1].reading, "short covering");
    }
}
