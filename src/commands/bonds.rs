//! `bonds`: Indian G-sec and T-bill benchmark yields (CCIL, RBI), RBI's
//! list of outstanding securities, and a price ↔ yield calculator.

use super::*;
use crate::bonds::{self, Bond};
use crate::gsec::{self, Kind, Security, YieldRow};
use chrono::NaiveDate;
use std::time::Duration;

/// Network timeout for the CCIL and RBI pages.
pub(crate) const BONDS_TIMEOUT: Duration = Duration::from_secs(20);
const TIMEOUT: Duration = BONDS_TIMEOUT;

fn print_rows<T: Serialize + CsvRow>(fmt: Format, rows: &[T]) -> Result<()> {
    match fmt {
        Format::Csv => output::print_csv(rows),
        _ => output::print_json(rows),
    }
}

pub async fn cmd_bonds_yields(fmt: Format) -> Result<()> {
    let set = gsec::yields(TIMEOUT).await?;
    if !fmt.is_table() {
        return print_rows(fmt, &set.rows);
    }
    let source = set.rows.first().map_or("", |r| r.source);
    let date = set
        .rows
        .iter()
        .find_map(|r| r.date)
        .map(|d| d.format("%d %b %Y").to_string())
        .unwrap_or_default();
    print_header(&tf(Msg::BondsYieldsTitle, &[&source, &date]));
    println!(
        "  {} {} {}",
        pad_end(t(Msg::BondsTenor), 9).dimmed(),
        pad_end(t(Msg::BondsSecurity), 30).dimmed(),
        pad_start(t(Msg::BondsYtm), 9).dimmed()
    );
    let mut last_kind = None;
    for r in &set.rows {
        if last_kind.is_some_and(|k| k != r.kind) {
            println!();
        }
        last_kind = Some(r.kind);
        print_yield(r);
    }
    if let Some(ten) = gsec::ten_year(&set.rows) {
        println!(
            "\n  {} {}",
            t(Msg::BondsIndia10y).bold(),
            tf(
                Msg::BondsTenYearIs,
                &[
                    &ten.security,
                    &format!("{:.4}%", ten.ytm.unwrap_or_default())
                ]
            )
        );
    }
    println!(
        "\n  {}",
        t(if source == "CCIL" {
            Msg::BondsCcilNote
        } else {
            Msg::BondsRbiNote
        })
        .dimmed()
    );
    println!("  {}\n", tf(Msg::BondsSourceUrl, &[&set.url]).dimmed());
    Ok(())
}

fn print_yield(r: &YieldRow) {
    let ytm = r.ytm.map_or_else(
        || "N/A".dimmed().to_string(),
        |y| format!("{y:.4}%").bold().to_string(),
    );
    let tenor = match r.kind {
        Kind::Sdl => format!("SDL {}", r.tenor),
        _ => r.tenor.clone(),
    };
    println!(
        "  {} {} {}",
        pad_end(&tenor, 9),
        pad_end(&r.security, 30),
        pad_start(&ytm, 9)
    );
}

/// `bonds list [QUERY]`: a four-digit QUERY is a maturity year; anything
/// else matches the name, ISIN or NSE symbol.
pub async fn cmd_bonds_list(query: Option<&str>, fmt: Format) -> Result<()> {
    let (list, _) = gsec::securities(TIMEOUT).await?;
    let rows: Vec<Security> = list
        .into_iter()
        .filter(|s| query.is_none_or(|q| matches_query(s, q)))
        .collect();
    if !fmt.is_table() {
        return print_rows(fmt, &rows);
    }
    print_header(t(Msg::BondsListTitle));
    if rows.is_empty() {
        println!(
            "  {}\n",
            tf(Msg::BondsListEmpty, &[&query.unwrap_or_default()])
        );
        return Ok(());
    }
    println!(
        "  {} {} {} {} {}",
        pad_end("ISIN", 13).dimmed(),
        pad_end(t(Msg::BondsSecurity), 22).dimmed(),
        pad_end(t(Msg::SgbMatures), 12).dimmed(),
        pad_end("NSE", 11).dimmed(),
        pad_start(t(Msg::BondsOutstanding), 14).dimmed()
    );
    for s in &rows {
        println!(
            "  {} {} {} {} {}",
            pad_end(&s.isin, 13),
            pad_end(&s.name, 22).bold(),
            pad_end(&s.maturity.format("%d %b %Y").to_string(), 12),
            pad_end(&s.nse_symbol().unwrap_or_default(), 11).dimmed(),
            pad_start(
                &s.outstanding_cr.map_or("N/A".into(), |c| format!("{c:.0}")),
                14
            )
        );
    }
    println!("\n  {}\n", tf(Msg::BondsListNote, &[&rows.len()]).dimmed());
    Ok(())
}

pub(crate) fn matches_query(s: &Security, q: &str) -> bool {
    let q = q.trim();
    if q.len() == 4
        && let Ok(year) = q.parse::<i32>()
    {
        return chrono::Datelike::year(&s.maturity) == year;
    }
    let q = q.to_ascii_lowercase();
    s.name.to_ascii_lowercase().contains(&q)
        || s.isin.to_ascii_lowercase() == q
        || s.nse_symbol().is_some_and(|n| n.to_ascii_lowercase() == q)
}

/// What `bonds calc` was asked: a security (looked up in RBI's list) or an
/// explicit coupon and maturity, and either a price or a yield.
pub struct CalcInput<'a> {
    pub security: Option<&'a str>,
    pub coupon: Option<f64>,
    pub maturity: Option<NaiveDate>,
    pub price: Option<f64>,
    pub ytm: Option<f64>,
    pub settle: Option<NaiveDate>,
    pub tbill: bool,
    /// The price includes accrued interest (NSE trades G-secs at dirty prices).
    pub dirty: bool,
}

/// `bonds calc --json` (field order = key order).
#[derive(Debug, Serialize)]
pub struct CalcResult {
    pub security: Option<String>,
    pub isin: Option<String>,
    pub kind: &'static str,
    pub coupon: Option<f64>,
    pub maturity: NaiveDate,
    pub settlement: NaiveDate,
    pub days_to_maturity: i64,
    pub clean_price: f64,
    pub accrued_interest: f64,
    pub dirty_price: f64,
    pub ytm: f64,
    pub current_yield: Option<f64>,
    pub macaulay_duration: Option<f64>,
    pub modified_duration: Option<f64>,
    pub next_coupon: Option<NaiveDate>,
    pub coupons_left: Option<u32>,
}

pub async fn cmd_bonds_calc(input: CalcInput<'_>, fmt: Format) -> Result<()> {
    let settle = input
        .settle
        .unwrap_or_else(|| crate::clock::session_date(Market::In, chrono::Utc::now()));
    // Look the security up only when its terms weren't given.
    let found = match input.security {
        Some(name) if input.coupon.is_none() || input.maturity.is_none() => {
            let (list, _) = gsec::securities(TIMEOUT).await.with_context(|| {
                format!(
                    "Cannot look up '{name}' in RBI's list of G-secs — pass --coupon and --maturity instead"
                )
            })?;
            Some(gsec::lookup(&list, name)?.clone())
        }
        _ => None,
    };
    let result = calculate(&input, found.as_ref(), settle)?;
    if !fmt.is_table() {
        return output::print_json(&result);
    }
    print_calc(&result);
    Ok(())
}

/// The calculation behind `bonds calc`, without any I/O.
pub fn calculate(
    input: &CalcInput<'_>,
    found: Option<&Security>,
    settle: NaiveDate,
) -> Result<CalcResult> {
    let maturity = input
        .maturity
        .or(found.map(|s| s.maturity))
        .context("Give a security (718GS2033, an ISIN) or --maturity YYYY-MM-DD")?;
    if input.dirty && input.price.is_none() {
        bail!("--dirty says the --price includes accrued interest; give a --price");
    }
    let days = (maturity - settle).num_days();
    if days <= 0 {
        bail!("{maturity} is not after the settlement date {settle}");
    }
    let name = found
        .map(|s| s.name.clone())
        .or(input.security.map(str::to_string));
    let isin = found.map(|s| s.isin.clone());
    if input.tbill {
        if days > 366 {
            bail!("T-bills mature within 364 days; {maturity} is {days} days away");
        }
        let (price, ytm) = match (input.price, input.ytm) {
            (Some(p), _) => (p, bonds::tbill_yield(p, days)?),
            (None, Some(y)) => (bonds::tbill_price(y, days)?, y),
            (None, None) => bail!("Give --price or --ytm"),
        };
        return Ok(CalcResult {
            security: name,
            isin,
            kind: "tbill",
            coupon: None,
            maturity,
            settlement: settle,
            days_to_maturity: days,
            clean_price: price,
            accrued_interest: 0.0,
            dirty_price: price,
            ytm,
            current_yield: None,
            macaulay_duration: Some(days as f64 / 365.0),
            modified_duration: Some(
                days as f64 / 365.0 / (1.0 + ytm / 100.0 * days as f64 / 365.0),
            ),
            next_coupon: None,
            coupons_left: None,
        });
    }
    let coupon = match (input.coupon, found) {
        (Some(c), _) => c,
        (None, Some(s)) => s.coupon.with_context(|| {
            format!(
                "{} has a floating coupon — the calculator handles fixed-coupon bonds only",
                s.name
            )
        })?,
        (None, None) => bail!("Give --coupon (percent a year) with --maturity"),
    };
    let bond = Bond::new(coupon, maturity)?;
    let a = match (input.price, input.ytm) {
        (Some(p), _) => {
            let clean = if input.dirty {
                p - bond.accrued(settle)?
            } else {
                p
            };
            bond.analytics_at_price(settle, clean)?
        }
        (None, Some(y)) => bond.analytics_at_yield(settle, y)?,
        (None, None) => bail!("Give --price or --ytm"),
    };
    Ok(CalcResult {
        security: name,
        isin,
        kind: "gsec",
        coupon: Some(coupon),
        maturity,
        settlement: settle,
        days_to_maturity: days,
        clean_price: a.clean_price,
        accrued_interest: a.accrued,
        dirty_price: a.dirty_price,
        ytm: a.ytm_pct,
        current_yield: Some(a.current_yield_pct),
        macaulay_duration: Some(a.macaulay_years),
        modified_duration: Some(a.modified_duration),
        next_coupon: Some(a.next_coupon),
        coupons_left: Some(a.coupons_left),
    })
}

fn print_calc(r: &CalcResult) {
    let title = match (&r.security, &r.coupon) {
        (Some(name), _) => name.clone(),
        (None, Some(c)) => format!("{c:.2}% bond maturing {}", r.maturity.format("%d %b %Y")),
        (None, None) => format!("T-bill maturing {}", r.maturity.format("%d %b %Y")),
    };
    print_header(&title);
    let date = |d: NaiveDate| d.format("%d %b %Y").to_string();
    if let Some(isin) = &r.isin {
        print_kv("ISIN", isin);
    }
    print_kv(t(Msg::BondsMaturity), &date(r.maturity));
    print_kv(
        t(Msg::BondsSettlement),
        &format!(
            "{} ({})",
            date(r.settlement),
            tf(Msg::BondsDaysLeft, &[&r.days_to_maturity])
        ),
    );
    print_kv(
        t(Msg::BondsCleanPrice),
        &format!("{:.4}", r.clean_price).bold().to_string(),
    );
    if r.kind == "gsec" {
        print_kv(t(Msg::BondsAccrued), &format!("{:.4}", r.accrued_interest));
        print_kv(t(Msg::BondsDirtyPrice), &format!("{:.4}", r.dirty_price));
    }
    print_kv(
        t(Msg::BondsYtm),
        &format!(
            "{:.4}% {}",
            r.ytm,
            t(if r.kind == "tbill" {
                Msg::BondsTbillBasis
            } else {
                Msg::BondsSemiAnnual
            })
        )
        .bold()
        .to_string(),
    );
    if let Some(cy) = r.current_yield {
        print_kv(t(Msg::BondsCurrentYield), &format!("{cy:.4}%"));
    }
    if let Some(d) = r.macaulay_duration {
        print_kv(t(Msg::BondsMacaulay), &format!("{d:.2}"));
    }
    if let Some(d) = r.modified_duration {
        print_kv(t(Msg::BondsModified), &format!("{d:.2}"));
    }
    if let (Some(next), Some(left)) = (r.next_coupon, r.coupons_left) {
        print_kv(
            t(Msg::BondsNextCoupon),
            &tf(Msg::BondsCouponsLeft, &[&date(next), &left]),
        );
    }
    println!(
        "\n  {}\n",
        t(if r.kind == "tbill" {
            Msg::BondsTbillNote
        } else {
            Msg::BondsCalcNote
        })
        .dimmed()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn input() -> CalcInput<'static> {
        CalcInput {
            security: None,
            coupon: None,
            maturity: None,
            price: None,
            ytm: None,
            settle: None,
            tbill: false,
            dirty: false,
        }
    }

    fn security() -> Security {
        Security {
            isin: "IN0020230085".into(),
            name: "7.18% GS 2033".into(),
            coupon: Some(7.18),
            issue_date: date(2023, 8, 14),
            maturity: date(2033, 8, 14),
            outstanding_cr: Some(201_000.0),
        }
    }

    #[test]
    fn calc_from_a_looked_up_security_and_price() {
        let found = security();
        let r = calculate(
            &CalcInput {
                security: Some("718GS2033"),
                price: Some(100.0),
                ..input()
            },
            Some(&found),
            date(2026, 8, 14),
        )
        .unwrap();
        assert_eq!(r.security.as_deref(), Some("7.18% GS 2033"));
        assert_eq!(r.isin.as_deref(), Some("IN0020230085"));
        assert!((r.ytm - 7.18).abs() < 1e-6, "{}", r.ytm);
        assert_eq!(r.accrued_interest, 0.0);
        assert_eq!(r.coupons_left, Some(14));
    }

    #[test]
    fn dirty_prices_lose_their_accrued_interest() {
        let found = security();
        let settle = date(2026, 11, 14); // 90 days into the period
        let clean = calculate(
            &CalcInput {
                price: Some(100.0),
                ..input()
            },
            Some(&found),
            settle,
        )
        .unwrap();
        let dirty = calculate(
            &CalcInput {
                price: Some(100.0 + clean.accrued_interest),
                dirty: true,
                ..input()
            },
            Some(&found),
            settle,
        )
        .unwrap();
        assert!((clean.accrued_interest - 1.795).abs() < 1e-9);
        assert!((dirty.ytm - clean.ytm).abs() < 1e-7);
    }

    #[test]
    fn calc_from_explicit_terms_and_tbills() {
        let r = calculate(
            &CalcInput {
                coupon: Some(7.0),
                maturity: Some(date(2031, 4, 1)),
                ytm: Some(7.0),
                ..input()
            },
            None,
            date(2026, 4, 1),
        )
        .unwrap();
        assert!((r.clean_price - 100.0).abs() < 1e-9);
        let tb = calculate(
            &CalcInput {
                tbill: true,
                maturity: Some(date(2027, 1, 14)),
                price: Some(98.6),
                ..input()
            },
            None,
            date(2026, 10, 26),
        )
        .unwrap();
        assert_eq!(tb.days_to_maturity, 80);
        assert!((tb.ytm - 1.4 / 98.6 * 365.0 / 80.0 * 100.0).abs() < 1e-9);
        assert_eq!(tb.kind, "tbill");
    }

    #[test]
    fn calc_errors_are_clear() {
        let err = |i: CalcInput<'_>, f: Option<&Security>| {
            calculate(&i, f, date(2026, 10, 26))
                .unwrap_err()
                .to_string()
        };
        assert!(err(input(), None).contains("--maturity"));
        assert!(
            err(
                CalcInput {
                    maturity: Some(date(2030, 1, 1)),
                    coupon: Some(7.0),
                    ytm: Some(7.0),
                    dirty: true,
                    ..input()
                },
                None
            )
            .contains("--dirty")
        );
        assert!(
            err(
                CalcInput {
                    maturity: Some(date(2030, 1, 1)),
                    price: Some(99.0),
                    ..input()
                },
                None
            )
            .contains("--coupon")
        );
        let mut frb = security();
        frb.name = "FRB 2033".into();
        frb.coupon = None;
        assert!(
            err(
                CalcInput {
                    price: Some(99.0),
                    ..input()
                },
                Some(&frb)
            )
            .contains("floating")
        );
        assert!(
            err(
                CalcInput {
                    tbill: true,
                    maturity: Some(date(2028, 1, 1)),
                    price: Some(95.0),
                    ..input()
                },
                None
            )
            .contains("364 days")
        );
        assert!(
            err(
                CalcInput {
                    maturity: Some(date(2020, 1, 1)),
                    coupon: Some(7.0),
                    price: Some(99.0),
                    ..input()
                },
                None
            )
            .contains("not after")
        );
    }

    #[test]
    fn list_queries() {
        let s = security();
        assert!(matches_query(&s, "2033"));
        assert!(!matches_query(&s, "2034"));
        assert!(matches_query(&s, "7.18%"));
        assert!(matches_query(&s, "718gs2033"));
        assert!(matches_query(&s, "IN0020230085"));
        assert!(!matches_query(&s, "SGrB"));
    }
}
