//! `stockwise portfolio export --format beancount|hledger`: the portfolio as
//! a plain-text double-entry journal.
//!
//! The journal is rebuilt from the lots and realized sales StockWise keeps:
//! every lot that is still open and every lot behind a recorded sale becomes
//! a purchase, each sale becomes a disposal at its matched cost with a
//! short- or long-term capital-gains leg, and each recorded dividend becomes
//! income. Entries are in date order; on one day, purchases come before
//! dividends and dividends before sales.
//!
//! Accounts:
//! - `Assets:Investments:<Account>` holds the shares (`<Account>` is the
//!   demat account, such as `Zerodha`, or `Default`)
//! - `Assets:Cash:<CUR>` pays for purchases and receives sale proceeds and
//!   dividends
//! - `Income:CapitalGains:ShortTerm` / `Income:CapitalGains:LongTerm`
//! - `Income:Dividends`
//! - `Expenses:Tax:Withholding`, only when a dividend record carries tax
//!   withheld (current records hold the gross amount only)
//!
//! Commodity names: beancount allows `A-Z`, `0-9` and `'._-`, starting
//! with a letter, ending with a letter or digit, at most 24 characters. A
//! symbol is upper-cased and every other character becomes `_`:
//! `RELIANCE.NS` stays `RELIANCE.NS`, `M&M.NS` becomes `M_M.NS`,
//! `MF:120503` becomes `MF_120503`, `^NSEI` becomes `X_NSEI` (an `X` is put
//! in front of a name that does not start with a letter). A longer name is
//! cut and given a short hash. Each `commodity` directive keeps the original
//! symbol in its `name` metadata (beancount) or comment (hledger).
//!
//! Quantities and prices are in today's split-adjusted units, as StockWise
//! stores them. London prices quoted in pence (`GBp`) are converted to
//! pounds. All amounts are exact decimals, so every transaction balances to
//! zero without rounding.

use anyhow::Result;
use chrono::NaiveDate;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use crate::portfolio::{Portfolio, Sale};
use crate::tax::Country;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Beancount,
    Hledger,
}

/// An exact decimal: `m / 10^s`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dec {
    m: i128,
    s: u32,
}

/// Decimal places kept from the stored `f64` values.
const PLACES: u32 = 8;

impl Dec {
    fn from_f64(x: f64) -> Self {
        let text = format!("{:.*}", PLACES as usize, x);
        let (neg, digits) = match text.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, text.as_str()),
        };
        let m: i128 = digits.replace('.', "").parse().unwrap_or(0);
        Self {
            m: if neg { -m } else { m },
            s: PLACES,
        }
        .normalized()
    }

    fn rescale(self, s: u32) -> i128 {
        self.m * 10i128.pow(s - self.s)
    }

    fn add(self, o: Self) -> Self {
        let s = self.s.max(o.s);
        Self {
            m: self.rescale(s) + o.rescale(s),
            s,
        }
        .normalized()
    }

    fn neg(self) -> Self {
        Self {
            m: -self.m,
            s: self.s,
        }
    }

    fn mul(self, o: Self) -> Self {
        Self {
            m: self.m * o.m,
            s: self.s + o.s,
        }
        .normalized()
    }

    /// Divide by 100 (pence to pounds).
    fn centi(self) -> Self {
        Self {
            m: self.m,
            s: self.s + 2,
        }
        .normalized()
    }

    fn is_zero(self) -> bool {
        self.m == 0
    }

    /// Drop trailing zero decimals.
    fn normalized(mut self) -> Self {
        while self.s > 0 && self.m % 10 == 0 {
            self.m /= 10;
            self.s -= 1;
        }
        self
    }
}

impl std::fmt::Display for Dec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sign = if self.m < 0 { "-" } else { "" };
        let abs = self.m.unsigned_abs();
        if self.s == 0 {
            return write!(f, "{}{}", sign, abs);
        }
        let p = 10u128.pow(self.s);
        write!(
            f,
            "{}{}.{:0width$}",
            sign,
            abs / p,
            abs % p,
            width = self.s as usize
        )
    }
}

/// A valid beancount commodity name for `symbol` (see the module docs).
pub fn commodity_name(symbol: &str) -> String {
    let mut s: String = symbol
        .to_uppercase()
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() || c.is_ascii_digit() || "'._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !s.starts_with(|c: char| c.is_ascii_uppercase()) {
        s.insert(0, 'X');
    }
    while s.ends_with(|c: char| !c.is_ascii_alphanumeric()) {
        s.pop();
    }
    if s.len() > 24 {
        // FNV-1a of the original symbol keeps cut names apart.
        let h = symbol.bytes().fold(0x811c9dc5u32, |h, b| {
            (h ^ b as u32).wrapping_mul(0x0100_0193)
        });
        s.truncate(19);
        while s.ends_with(|c: char| !c.is_ascii_alphanumeric()) {
            s.pop();
        }
        s = format!("{}_{:04X}", s, h & 0xFFFF);
    }
    s
}

/// One account-name component (`zerodha` → `Zerodha`).
fn account_component(name: &str) -> String {
    // ASCII letters, digits and dashes, starting with a capital or a digit.
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    match s.get(..1) {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), &s[1..]),
        None => "Default".into(),
    }
}

fn holdings_account(account: Option<&str>) -> String {
    format!(
        "Assets:Investments:{}",
        account
            .map(account_component)
            .unwrap_or_else(|| "Default".into())
    )
}

fn cash_account(currency: &str) -> String {
    format!("Assets:Cash:{}", currency)
}

const SHORT_TERM: &str = "Income:CapitalGains:ShortTerm";
const LONG_TERM: &str = "Income:CapitalGains:LongTerm";
const DIVIDENDS: &str = "Income:Dividends";

/// The currency a price is booked in, and whether it is in pence.
fn price_currency(symbol: &str, fallback: &str) -> (String, bool) {
    match crate::portfolio::symbol_currency(symbol) {
        Some("GBp") => ("GBP".into(), true),
        Some(c) => (c.to_ascii_uppercase(), false),
        None => (fallback.to_ascii_uppercase(), false),
    }
}

/// The currency a market's prices default to, for symbols without a
/// recognisable exchange suffix.
pub fn fallback_currency(m: crate::market::Market) -> String {
    use crate::market::Market;
    match m {
        Market::In | Market::Commodity => "INR".into(),
        Market::Us | Market::Fx => "USD".into(),
        Market::Crypto => crate::market::quote_currency().to_ascii_uppercase(),
        Market::Global(x) => {
            let info = x.info();
            match info.quote_unit {
                Some("GBp") => "GBP".into(),
                _ => info.currency.to_ascii_uppercase(),
            }
        }
    }
}

/// Long-term under the holding-period rule of the price currency's country
/// (12 months for other currencies). Intraday sales are short-term.
fn is_long_term(s: &Sale, currency: &str, bought: NaiveDate, sold: NaiveDate) -> bool {
    if s.is_intraday() {
        return false;
    }
    match currency {
        "INR" => Country::India.is_long_term(bought, sold),
        "USD" => Country::Us.is_long_term(bought, sold),
        "GBP" => Country::Uk.is_long_term(bought, sold),
        _ => crate::tax::is_long_term(bought, sold, 12),
    }
}

fn date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// A posting: account, amount, commodity, and an optional cost.
struct Posting {
    account: String,
    qty: Dec,
    commodity: String,
    /// Per-unit cost and the lot date (beancount `{cost, date}`).
    cost: Option<(Dec, String, Option<NaiveDate>)>,
    /// Per-unit sale price, shown as `@` in beancount (informational there).
    price: Option<(Dec, String)>,
}

struct Txn {
    date: NaiveDate,
    /// Order on one day: 0 purchase, 1 dividend, 2 sale.
    rank: u8,
    narration: String,
    meta: Vec<(String, String)>,
    postings: Vec<Posting>,
}

struct Journal {
    txns: Vec<Txn>,
    /// Commodity name → original symbol.
    commodities: BTreeMap<String, String>,
    currencies: BTreeSet<String>,
    skipped: Vec<String>,
}

fn build(p: &Portfolio, fallback: &str) -> Journal {
    let mut names = Names::default();
    let mut currencies = BTreeSet::new();
    let mut skipped = Vec::new();
    let mut txns = Vec::new();

    // Purchases: open lots plus the lots behind realized sales, merged by
    // (date, symbol, account, price) so each beancount lot is unique.
    let mut buys: BTreeMap<(NaiveDate, String, String, String), (Dec, Dec, String)> =
        BTreeMap::new();
    let mut add_buy = |d: &str, symbol: &str, account: Option<&str>, qty: f64, price: f64| {
        let Some(day) = date(d) else {
            return Err(format!("{} lot with date {:?}", symbol, d));
        };
        let (cur, pence) = price_currency(symbol, fallback);
        let mut unit = Dec::from_f64(price);
        if pence {
            unit = unit.centi();
        }
        let acct = holdings_account(account);
        let key = (day, symbol.to_string(), acct, unit.to_string());
        let e = buys.entry(key).or_insert((Dec { m: 0, s: 0 }, unit, cur));
        e.0 = e.0.add(Dec::from_f64(qty));
        Ok(())
    };
    for h in &p.holdings {
        for l in &h.lots {
            if let Err(e) = add_buy(&l.date, &h.symbol, l.account_key(), l.qty, l.price) {
                skipped.push(e);
            }
        }
    }
    for s in &p.realized {
        if let Err(e) = add_buy(&s.buy_date, &s.symbol, s.account_key(), s.qty, s.buy_price) {
            skipped.push(e);
        }
    }
    for ((day, symbol, acct, _), (qty, unit, cur)) in buys {
        if qty.is_zero() {
            continue;
        }
        let commodity = names.get(&symbol);
        currencies.insert(cur.clone());
        txns.push(Txn {
            date: day,
            rank: 0,
            narration: format!("Buy {} {}", qty, symbol),
            meta: vec![("symbol".into(), symbol.clone())],
            postings: vec![
                Posting {
                    account: acct,
                    qty,
                    commodity,
                    cost: Some((unit, cur.clone(), None)),
                    price: None,
                },
                Posting {
                    account: cash_account(&cur),
                    qty: qty.mul(unit).neg(),
                    commodity: cur,
                    cost: None,
                    price: None,
                },
            ],
        });
    }

    for d in &p.dividends {
        let Some(day) = date(&d.ex_date) else {
            skipped.push(format!("{} dividend with date {:?}", d.symbol, d.ex_date));
            continue;
        };
        let (cur, pence) = match d.currency.as_deref() {
            Some("GBp") => ("GBP".to_string(), true),
            Some(c) => (c.to_ascii_uppercase(), false),
            None => price_currency(&d.symbol, fallback),
        };
        let mut amount = Dec::from_f64(d.amount);
        if pence {
            amount = amount.centi();
        }
        if amount.is_zero() {
            continue;
        }
        currencies.insert(cur.clone());
        txns.push(Txn {
            date: day,
            rank: 1,
            narration: format!("Dividend {}", d.symbol),
            meta: vec![("symbol".into(), d.symbol.clone())],
            postings: vec![
                Posting {
                    account: cash_account(&cur),
                    qty: amount,
                    commodity: cur.clone(),
                    cost: None,
                    price: None,
                },
                Posting {
                    account: DIVIDENDS.into(),
                    qty: amount.neg(),
                    commodity: cur,
                    cost: None,
                    price: None,
                },
            ],
        });
    }

    for s in &p.realized {
        let (Some(bought), Some(sold)) = (date(&s.buy_date), date(&s.sell_date)) else {
            skipped.push(format!("{} sale on {:?}", s.symbol, s.sell_date));
            continue;
        };
        let (cur, pence) = price_currency(&s.symbol, fallback);
        let (mut cost, mut sell) = (Dec::from_f64(s.buy_price), Dec::from_f64(s.sell_price));
        if pence {
            cost = cost.centi();
            sell = sell.centi();
        }
        let qty = Dec::from_f64(s.qty);
        if qty.is_zero() {
            continue;
        }
        let basis = qty.mul(cost);
        let proceeds = qty.mul(sell);
        let gain = proceeds.add(basis.neg());
        let commodity = names.get(&s.symbol);
        currencies.insert(cur.clone());
        let gains = if is_long_term(s, &cur, bought, sold) {
            LONG_TERM
        } else {
            SHORT_TERM
        };
        let mut postings = vec![
            Posting {
                account: holdings_account(s.account_key()),
                qty: qty.neg(),
                commodity,
                cost: Some((cost, cur.clone(), Some(bought))),
                price: Some((sell, cur.clone())),
            },
            Posting {
                account: cash_account(&cur),
                qty: proceeds,
                commodity: cur.clone(),
                cost: None,
                price: None,
            },
        ];
        if !gain.is_zero() {
            postings.push(Posting {
                account: gains.into(),
                qty: gain.neg(),
                commodity: cur,
                cost: None,
                price: None,
            });
        }
        txns.push(Txn {
            date: sold,
            rank: 2,
            narration: format!("Sell {} {}", qty, s.symbol),
            meta: vec![("symbol".into(), s.symbol.clone())],
            postings,
        });
    }

    // Stable: on one day and rank, the order built above is kept.
    txns.sort_by_key(|t| (t.date, t.rank));
    Journal {
        txns,
        commodities: names.by_name,
        currencies,
        skipped,
    }
}

/// Symbol → commodity name, keeping two symbols from sharing a name.
#[derive(Default)]
struct Names {
    by_symbol: BTreeMap<String, String>,
    by_name: BTreeMap<String, String>,
}

impl Names {
    fn get(&mut self, symbol: &str) -> String {
        if let Some(n) = self.by_symbol.get(symbol) {
            return n.clone();
        }
        let base = commodity_name(symbol);
        let mut name = base.clone();
        let mut i = 2;
        while self.by_name.contains_key(&name) {
            let suffix = format!("-{}", i);
            let mut b = base.clone();
            b.truncate(24 - suffix.len());
            name = format!("{}{}", b, suffix);
            i += 1;
        }
        self.by_symbol.insert(symbol.to_string(), name.clone());
        self.by_name.insert(name.clone(), symbol.to_string());
        name
    }
}

fn quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The portfolio as a journal in `fmt`. `fallback` is the currency for
/// symbols without a recognisable exchange suffix.
pub fn render(p: &Portfolio, fmt: Format, fallback: &str) -> String {
    let j = build(p, fallback);
    let mut out = String::new();
    let tool = match fmt {
        Format::Beancount => "beancount",
        Format::Hledger => "hledger",
    };
    let _ = writeln!(out, "; StockWise portfolio export ({}).", tool);
    let _ = writeln!(
        out,
        "; Rebuilt from recorded lots, sales and dividends; quantities are split-adjusted."
    );
    let _ = writeln!(
        out,
        "; For records and education only; check against your broker's statements."
    );
    for s in &j.skipped {
        let _ = writeln!(out, "; skipped (invalid date): {}", s);
    }
    out.push('\n');

    let accounts: BTreeSet<&str> = j
        .txns
        .iter()
        .flat_map(|t| t.postings.iter().map(|p| p.account.as_str()))
        .collect();
    let first = j
        .txns
        .first()
        .map(|t| t.date)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid date"));

    match fmt {
        Format::Beancount => {
            let main = Some(fallback.to_ascii_uppercase())
                .filter(|c| j.currencies.contains(c))
                .or_else(|| j.currencies.iter().next().cloned());
            if let Some(c) = main {
                let _ = writeln!(out, "option \"operating_currency\" \"{}\"\n", c);
            }
            for c in &j.currencies {
                let _ = writeln!(out, "{} commodity {}", first, c);
            }
            for (name, symbol) in &j.commodities {
                let _ = writeln!(out, "{} commodity {}", first, name);
                let _ = writeln!(out, "  name: \"{}\"", quote(symbol));
            }
            out.push('\n');
            for a in &accounts {
                let _ = writeln!(out, "{} open {}", first, a);
            }
        }
        Format::Hledger => {
            let _ = writeln!(out, "decimal-mark .\n");
            for c in &j.currencies {
                let _ = writeln!(out, "commodity {}", c);
            }
            for (name, symbol) in &j.commodities {
                let _ = writeln!(
                    out,
                    "commodity {}  ; symbol: {}",
                    hl_commodity(name),
                    symbol
                );
            }
            out.push('\n');
            for a in &accounts {
                let _ = writeln!(out, "account {}", a);
            }
        }
    }

    for t in &j.txns {
        out.push('\n');
        match fmt {
            Format::Beancount => {
                let _ = writeln!(out, "{} * \"{}\"", t.date, quote(&t.narration));
                for (k, v) in &t.meta {
                    let _ = writeln!(out, "  {}: \"{}\"", k, quote(v));
                }
                for p in &t.postings {
                    let mut line = format!("  {}  {} {}", p.account, p.qty, p.commodity);
                    if let Some((c, cur, d)) = &p.cost {
                        match d {
                            Some(d) => {
                                let _ = write!(line, " {{{} {}, {}}}", c, cur, d);
                            }
                            None => {
                                let _ = write!(line, " {{{} {}}}", c, cur);
                            }
                        }
                    }
                    if let Some((pr, cur)) = &p.price {
                        let _ = write!(line, " @ {} {}", pr, cur);
                    }
                    let _ = writeln!(out, "{}", line);
                }
            }
            Format::Hledger => {
                let tags: Vec<String> =
                    t.meta.iter().map(|(k, v)| format!("{}:{}", k, v)).collect();
                let _ = writeln!(out, "{} * {}  ; {}", t.date, t.narration, tags.join(", "));
                for p in &t.postings {
                    // hledger balances at the `@` price, so a disposal is
                    // priced at its cost basis; the sale price is a comment.
                    let mut line = format!("    {}  {}", p.account, hl_amount(p.qty, &p.commodity));
                    if let Some((c, cur, d)) = &p.cost {
                        let _ = write!(line, " @ {}", hl_amount(*c, cur));
                        let mut notes = Vec::new();
                        if let Some(d) = d {
                            notes.push(format!("lot:{}", d));
                        }
                        if let Some((pr, cur)) = &p.price {
                            notes.push(format!("sold_at:{} {}", pr, cur));
                        }
                        if !notes.is_empty() {
                            let _ = write!(line, "  ; {}", notes.join(", "));
                        }
                    }
                    let _ = writeln!(out, "{}", line);
                }
            }
        }
    }
    out
}

/// hledger needs a commodity symbol with anything but letters in quotes.
fn hl_commodity(name: &str) -> String {
    if name.chars().all(|c| c.is_ascii_alphabetic()) {
        name.to_string()
    } else {
        format!("\"{}\"", name)
    }
}

fn hl_amount(q: Dec, commodity: &str) -> String {
    format!("{} {}", q, hl_commodity(commodity))
}

/// `stockwise portfolio export --format beancount|hledger [-o FILE]`.
pub fn cmd_export(fmt: Format, out: Option<&Path>, market: crate::market::Market) -> Result<()> {
    let p = Portfolio::load()?;
    let text = render(&p, fmt, &fallback_currency(market));
    match out {
        Some(path) => {
            std::fs::write(path, &text)
                .map_err(|e| anyhow::anyhow!("Cannot write {}: {}", path.display(), e))?;
            eprintln!("  Wrote {}", path.display());
        }
        None => {
            use std::io::Write;
            std::io::stdout().write_all(text.as_bytes())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portfolio::{DividendReceived, Holding, Lot};

    fn sale(symbol: &str, qty: f64, buy: (&str, f64), sell: (&str, f64)) -> Sale {
        serde_json::from_value(serde_json::json!({
            "symbol": symbol, "qty": qty,
            "buy_date": buy.0, "buy_price": buy.1,
            "sell_date": sell.0, "sell_price": sell.1,
            "source": "import:kite",
        }))
        .unwrap()
    }

    fn holding(symbol: &str, lots: Vec<Lot>) -> Holding {
        serde_json::from_value(serde_json::json!({
            "symbol": symbol, "shares": 0.0, "avg_cost": 0.0,
            "lots": lots,
        }))
        .unwrap()
    }

    fn fixture() -> Portfolio {
        let mut p = Portfolio::default();
        let mut l = Lot::new(10.0, 2400.5, "2023-01-02", "import:kite");
        l.account = Some("zerodha".into());
        p.holdings.push(holding("RELIANCE.NS", vec![l]));
        p.holdings.push(holding(
            "MF:120503",
            vec![Lot::new(12.345, 101.2345, "2022-06-01", "import:cas")],
        ));
        p.holdings.push(holding(
            "AAPL",
            vec![Lot::new(3.0, 150.25, "2024-02-01", "manual")],
        ));
        p.holdings.push(holding(
            "VOD.L",
            vec![Lot::new(100.0, 72.5, "2024-03-01", "manual")],
        ));
        p.holdings.push(holding(
            "M&M.NS",
            vec![Lot::new(1.0, 1500.0, "2024-04-01", "manual")],
        ));
        // Long-term (held > 12 months) and short-term sales of the same lot.
        p.realized.push(sale(
            "RELIANCE.NS",
            5.0,
            ("2023-01-02", 2400.5),
            ("2024-03-01", 2900.75),
        ));
        p.realized.push(sale(
            "RELIANCE.NS",
            2.0,
            ("2023-01-02", 2400.5),
            ("2023-06-01", 2300.0),
        ));
        // Intraday.
        p.realized.push(sale(
            "TCS.NS",
            1.0,
            ("2024-05-02", 3800.0),
            ("2024-05-02", 3810.0),
        ));
        p.realized.push(sale(
            "AAPL",
            1.5,
            ("2024-02-01", 150.25),
            ("2024-08-01", 210.1),
        ));
        p.dividends.push(DividendReceived {
            symbol: "RELIANCE.NS".into(),
            ex_date: "2023-08-21".into(),
            per_share: 9.0,
            qty: 17.0,
            amount: 153.0,
            currency: Some("INR".into()),
        });
        p
    }

    fn parse_num(s: &str) -> Dec {
        let (neg, d) = match s.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, s),
        };
        let (int, frac) = d.split_once('.').unwrap_or((d, ""));
        assert!(
            int.chars().all(|c| c.is_ascii_digit()) && !int.is_empty(),
            "{s}"
        );
        assert!(frac.chars().all(|c| c.is_ascii_digit()), "{s}");
        let m: i128 = format!("{}{}", int, frac).parse().unwrap();
        Dec {
            m: if neg { -m } else { m },
            s: frac.len() as u32,
        }
    }

    fn valid_account(a: &str) -> bool {
        let parts: Vec<_> = a.split(':').collect();
        ["Assets", "Liabilities", "Equity", "Income", "Expenses"].contains(&parts[0])
            && parts.len() >= 2
            && parts[1..].iter().all(|p| {
                p.starts_with(|c: char| c.is_ascii_uppercase() || c.is_ascii_digit())
                    && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
    }

    fn valid_commodity(c: &str) -> bool {
        let b = c.as_bytes();
        (2..=24).contains(&b.len())
            && b[0].is_ascii_uppercase()
            && b[b.len() - 1].is_ascii_alphanumeric()
            && c.chars()
                .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || "'._-".contains(ch))
    }

    /// Weight of a beancount posting: units × cost when a cost is given.
    fn bean_weight(rest: &str) -> (Dec, String) {
        let toks: Vec<&str> = rest.split_whitespace().collect();
        let qty = parse_num(toks[0]);
        assert!(valid_commodity(toks[1]), "commodity {}", toks[1]);
        if let Some(i) = rest.find('{') {
            let inner = &rest[i + 1..rest.find('}').unwrap()];
            let cost_part = inner.split(',').next().unwrap();
            let ct: Vec<&str> = cost_part.split_whitespace().collect();
            (qty.mul(parse_num(ct[0])), ct[1].to_string())
        } else {
            (qty, toks[1].to_string())
        }
    }

    #[test]
    fn beancount_output_balances_and_follows_the_syntax() {
        let text = render(&fixture(), Format::Beancount, "USD");
        let mut opened = BTreeSet::new();
        let mut declared = BTreeSet::new();
        let mut last_date = String::new();
        let mut current: Option<BTreeMap<String, Dec>> = None;
        let mut txns = 0;
        let finish = |cur: &mut Option<BTreeMap<String, Dec>>| {
            if let Some(sums) = cur.take() {
                for (c, v) in sums {
                    assert!(v.is_zero(), "unbalanced {c}: {v}\n{text}");
                }
            }
        };
        for line in text.lines() {
            if line.is_empty() || line.starts_with(';') || line.starts_with("option ") {
                finish(&mut current);
                continue;
            }
            if let Some(rest) = line.strip_prefix("  ") {
                let (k, v) = rest.split_once("  ").unwrap_or((rest, ""));
                if k.ends_with(':') || rest.contains(": \"") {
                    continue; // metadata
                }
                assert!(valid_account(k), "account {k}");
                assert!(opened.contains(k), "{k} not opened");
                let (w, c) = bean_weight(v);
                assert!(declared.contains(&c), "{c} not declared");
                let sums = current.as_mut().expect("posting outside a transaction");
                let e = sums.entry(c).or_insert(Dec { m: 0, s: 0 });
                *e = e.add(w);
                continue;
            }
            finish(&mut current);
            let (d, rest) = line.split_once(' ').unwrap();
            assert!(date(d).is_some(), "date {d}");
            assert!(d >= last_date.as_str(), "out of order: {line}");
            last_date = d.to_string();
            if let Some(c) = rest.strip_prefix("commodity ") {
                assert!(valid_commodity(c), "{c}");
                declared.insert(c.to_string());
            } else if let Some(a) = rest.strip_prefix("open ") {
                assert!(valid_account(a), "{a}");
                opened.insert(a.to_string());
            } else {
                assert!(rest.starts_with("* \""), "{line}");
                txns += 1;
                current = Some(BTreeMap::new());
            }
        }
        finish(&mut current);
        // 7 purchases (the sold AAPL lot is in another account than the
        // open one; TCS was bought and sold intraday), 1 dividend, 4 sales.
        assert_eq!(txns, 12, "{text}");
        assert!(
            text.contains("-5 RELIANCE.NS {2400.5 INR, 2023-01-02} @ 2900.75 INR"),
            "{text}"
        );
        assert!(
            text.contains("Income:CapitalGains:LongTerm  -2501.25 INR"),
            "{text}"
        );
        assert!(
            text.contains("Income:CapitalGains:ShortTerm  201 INR"),
            "{text}"
        );
        assert!(
            text.contains("commodity MF_120503\n  name: \"MF:120503\""),
            "{text}"
        );
        assert!(text.contains("commodity M_M.NS"), "{text}");
        // Pence converted to pounds.
        assert!(text.contains("100 VOD.L {0.725 GBP}"), "{text}");
        assert!(text.contains("Income:Dividends  -153 INR"), "{text}");
    }

    #[test]
    fn hledger_output_balances_and_follows_the_syntax() {
        let text = render(&fixture(), Format::Hledger, "USD");
        let mut accounts = BTreeSet::new();
        let mut current: Option<BTreeMap<String, Dec>> = None;
        let mut last_date = String::new();
        let mut txns = 0;
        let finish = |cur: &mut Option<BTreeMap<String, Dec>>| {
            if let Some(sums) = cur.take() {
                for (c, v) in sums {
                    assert!(v.is_zero(), "unbalanced {c}: {v}");
                }
            }
        };
        for line in text.lines() {
            if let Some(a) = line.strip_prefix("account ") {
                assert!(valid_account(a));
                accounts.insert(a.to_string());
                continue;
            }
            if let Some(rest) = line.strip_prefix("    ") {
                let body = rest.split("  ;").next().unwrap();
                let (acct, amt) = body.split_once("  ").expect("two spaces before the amount");
                assert!(accounts.contains(acct), "{acct} not declared");
                let (main, at) = match amt.split_once(" @ ") {
                    Some((m, a)) => (m, Some(a)),
                    None => (amt, None),
                };
                let (q, c) = main.split_once(' ').unwrap();
                let c = c.trim_matches('"');
                assert!(valid_commodity(c), "{c}");
                let qty = parse_num(q);
                let (w, wc) = match at {
                    Some(a) => {
                        let (p, pc) = a.split_once(' ').unwrap();
                        (qty.mul(parse_num(p)), pc.to_string())
                    }
                    None => (qty, c.to_string()),
                };
                let sums = current.as_mut().expect("posting outside a transaction");
                let e = sums.entry(wc).or_insert(Dec { m: 0, s: 0 });
                *e = e.add(w);
                continue;
            }
            finish(&mut current);
            if line.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                let d = &line[..10];
                assert!(date(d).is_some());
                assert!(d >= last_date.as_str());
                last_date = d.to_string();
                txns += 1;
                current = Some(BTreeMap::new());
            }
        }
        finish(&mut current);
        assert_eq!(txns, 12);
        assert!(
            text.contains("commodity \"RELIANCE.NS\"  ; symbol: RELIANCE.NS"),
            "{text}"
        );
        assert!(text.contains("    Assets:Investments:Zerodha  -5 \"RELIANCE.NS\" @ 2400.5 INR  ; lot:2023-01-02, sold_at:2900.75 INR"), "{text}");
        assert!(text.contains("commodity INR\n"));
    }

    #[test]
    fn commodity_names_are_valid() {
        assert_eq!(commodity_name("RELIANCE.NS"), "RELIANCE.NS");
        assert_eq!(commodity_name("MF:120503"), "MF_120503");
        assert_eq!(commodity_name("M&M.NS"), "M_M.NS");
        assert_eq!(commodity_name("^NSEI"), "X_NSEI");
        assert_eq!(commodity_name("9988.HK"), "X9988.HK");
        assert_eq!(commodity_name("btc-usd"), "BTC-USD");
        assert_eq!(commodity_name("USDINR=X"), "USDINR_X");
        let long = commodity_name("VERYLONGSYMBOLNAMEFORTESTING.NS");
        assert!(long.len() <= 24, "{long}");
        for s in [
            "RELIANCE.NS",
            "MF:1",
            "^NSEI",
            "A.",
            "VERYLONGSYMBOLNAMEFORTESTING.NS",
            "X",
        ] {
            let n = commodity_name(s);
            assert!(n.len() == 1 || valid_commodity(&n), "{s} -> {n}");
        }
    }

    #[test]
    fn colliding_names_get_a_suffix() {
        let mut n = Names::default();
        assert_eq!(n.get("M&M.NS"), "M_M.NS");
        assert_eq!(n.get("M_M.NS"), "M_M.NS-2");
        assert_eq!(n.get("M&M.NS"), "M_M.NS");
    }

    #[test]
    fn decimals_are_exact() {
        assert_eq!(Dec::from_f64(2400.5).to_string(), "2400.5");
        assert_eq!(
            Dec::from_f64(0.1).add(Dec::from_f64(0.2)).to_string(),
            "0.3"
        );
        assert_eq!(
            Dec::from_f64(-1.25).mul(Dec::from_f64(4.0)).to_string(),
            "-5"
        );
        assert_eq!(Dec::from_f64(72.5).centi().to_string(), "0.725");
        assert_eq!(Dec::from_f64(-0.05).to_string(), "-0.05");
    }

    #[test]
    fn account_components_are_valid() {
        assert_eq!(account_component("zerodha"), "Zerodha");
        assert_eq!(account_component("ind money"), "Ind-money");
        assert_eq!(account_component("_"), "Default");
        assert!(valid_account(&holdings_account(Some("1st-demat"))));
    }

    #[test]
    fn empty_portfolio_gives_an_empty_journal() {
        let text = render(&Portfolio::default(), Format::Beancount, "INR");
        assert!(text.lines().all(|l| l.is_empty() || l.starts_with(';')));
    }
}
