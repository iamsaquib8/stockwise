//! Regular-vs-Direct mutual-fund commission bleed: the yearly rupees a held
//! Regular plan pays over its Direct twin, what that compounds to, and which
//! units could move to Direct now without tax. Pure arithmetic over a fund's
//! lots and AMFI's disclosed expense ratios (TER); no advice, no forecast:
//! the 10-year figure uses a return the caller states.

use crate::funds::{Scheme, TerTable, base_scheme_name, ter_key};
use crate::offmarket::{find_scheme, fund_id, is_equity_oriented};
use crate::portfolio::{Lot, Portfolio};
use crate::sellpreview::{ExitLoadTerms, long_term_from};
use crate::tax::{self, Buckets, Country};
use chrono::NaiveDate;
use serde::Serialize;

/// Gross annual return (fraction) used for the compounded drag when none is given.
pub const DEFAULT_ASSUMED_RETURN: f64 = 0.10;
pub const DRAG_YEARS: i32 = 10;

#[derive(Debug, Clone)]
pub struct BleedInput<'a> {
    pub symbol: &'a str,
    pub lots: &'a [Lot],
    /// Latest NAV per unit.
    pub nav: f64,
    /// Total TER as a fraction (0.0219 = 2.19%).
    pub ter_regular: f64,
    pub ter_direct: f64,
    /// Gross annual return as a fraction, before TER.
    pub assumed_return: f64,
    pub today: NaiveDate,
    /// Gains and losses already realized this India tax year.
    pub realized: Buckets,
    /// False for debt and other funds: tax on a switch follows other rules
    /// and is not computed.
    pub equity_oriented: bool,
    pub exit_load: ExitLoadTerms,
}

/// Units held under a year, with what switching them now would cost.
#[derive(Debug, Clone, Serialize)]
pub struct EarlyLot {
    pub lot_no: usize,
    pub units: f64,
    pub bought: String,
    pub days_held: i64,
    /// The first day a redemption is long-term.
    pub long_term_on: String,
    pub gain: f64,
    /// Short-term tax (with cess) if redeemed today.
    pub tax_today: f64,
    pub exit_load: f64,
    pub annual_ter_saved: f64,
    /// (tax + exit load) / monthly TER saved; None when nothing is saved.
    pub breakeven_months: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwitchPlan {
    pub units_total: f64,
    /// Units held more than 12 months.
    pub long_term_units: f64,
    /// How many of them redeem inside the unused Sec. 112A exemption (FIFO).
    pub tax_free_units: f64,
    /// Gain realized by those units.
    pub tax_free_gain: f64,
    /// Sec. 112A exemption unused before any switch.
    pub exemption_left: f64,
    /// Tax (with cess) if every long-term unit were switched today.
    pub tax_all_long_term: f64,
    pub early: Vec<EarlyLot>,
    /// False when the fund is not equity-oriented: tax not computed.
    pub tax_modeled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bleed {
    pub symbol: String,
    pub value: f64,
    pub cost: f64,
    pub ter_regular: f64,
    pub ter_direct: f64,
    /// `ter_regular - ter_direct`, as a fraction.
    pub ter_gap: f64,
    /// `value × ter_gap` a year.
    pub annual_bleed: f64,
    pub assumed_return: f64,
    /// Extra value after 10 years in Direct at the same gross return, no new money.
    pub drag_10y: f64,
    pub switch: SwitchPlan,
}

/// Value of `amount` after `years` at a gross `ret` less `ter`, compounded yearly.
fn grow(amount: f64, ret: f64, ter: f64, years: i32) -> f64 {
    amount * (1.0 + ret - ter).powi(years)
}

pub fn analyze(i: &BleedInput) -> Bleed {
    let units_total: f64 = i.lots.iter().map(|l| l.qty).sum();
    let value = units_total * i.nav;
    let cost: f64 = i.lots.iter().map(Lot::cost).sum();
    let gap = (i.ter_regular - i.ter_direct).max(0.0);
    let drag = grow(value, i.assumed_return, i.ter_direct, DRAG_YEARS)
        - grow(value, i.assumed_return, i.ter_regular, DRAG_YEARS);

    let country = Country::India;
    let before = tax::estimate(country, i.realized);
    let exemption_left = (country.exemption() - before.exemption_used).max(0.0);
    let parse = |d: &str| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok();

    let mut long_units = 0.0;
    let mut tax_free_units = 0.0;
    let mut tax_free_gain = 0.0;
    let mut room = exemption_left;
    let mut lt_buckets = Buckets::default();
    let mut early = Vec::new();
    let mut stopped = false;
    for (idx, l) in i.lots.iter().enumerate() {
        let Some(b) = parse(&l.date) else { continue };
        let unit_gain = i.nav - l.price;
        let gain = l.qty * unit_gain;
        if country.is_long_term(b, i.today) {
            long_units += l.qty;
            lt_buckets.add(gain, true);
            // FIFO: units go oldest first, so a lot that no longer fits
            // inside the exemption stops the tax-free prefix.
            if !stopped {
                if gain <= room + 1e-9 {
                    tax_free_units += l.qty;
                    tax_free_gain += gain;
                    room -= gain;
                } else {
                    let part = if unit_gain > 0.0 {
                        (room / unit_gain).clamp(0.0, l.qty)
                    } else {
                        l.qty
                    };
                    tax_free_units += part;
                    tax_free_gain += part * unit_gain;
                    stopped = true;
                }
            }
        } else {
            let days_held = (i.today - b).num_days().max(0);
            let mut st = Buckets::default();
            st.add(gain, false);
            let tax_today = if i.equity_oriented {
                tax::estimate(country, i.realized.plus(st)).total() - before.total()
            } else {
                0.0
            };
            let exit_load = if days_held < i.exit_load.days {
                l.qty * i.nav * i.exit_load.pct / 100.0
            } else {
                0.0
            };
            let annual = l.qty * i.nav * gap;
            let cost_now = tax_today + exit_load;
            early.push(EarlyLot {
                lot_no: idx + 1,
                units: l.qty,
                bought: l.date.clone(),
                days_held,
                long_term_on: long_term_from(b, 12)
                    .map(|d| d.to_string())
                    .unwrap_or_default(),
                gain,
                tax_today,
                exit_load,
                annual_ter_saved: annual,
                breakeven_months: (annual > 0.0).then(|| cost_now / (annual / 12.0)),
            });
        }
    }
    let tax_all_long_term = if i.equity_oriented {
        tax::estimate(country, i.realized.plus(lt_buckets)).total() - before.total()
    } else {
        0.0
    };
    Bleed {
        symbol: i.symbol.to_string(),
        value,
        cost,
        ter_regular: i.ter_regular,
        ter_direct: i.ter_direct,
        ter_gap: gap,
        annual_bleed: value * gap,
        assumed_return: i.assumed_return,
        drag_10y: drag,
        switch: SwitchPlan {
            units_total,
            long_term_units: long_units,
            tax_free_units: if i.equity_oriented {
                tax_free_units
            } else {
                0.0
            },
            tax_free_gain: if i.equity_oriented {
                tax_free_gain
            } else {
                0.0
            },
            exemption_left,
            tax_all_long_term,
            early,
            tax_modeled: i.equity_oriented,
        },
    }
}

/// The Direct growth plan of the same scheme.
#[derive(Debug, Clone, Serialize)]
pub struct Twin {
    pub code: u32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FundBleed {
    pub name: String,
    pub category: Option<String>,
    pub twin: Option<Twin>,
    #[serde(flatten)]
    pub bleed: Bleed,
}

/// A held fund the report leaves out, and why.
#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub symbol: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BleedReport {
    pub funds: Vec<FundBleed>,
    pub skipped: Vec<Skipped>,
    pub annual_bleed: f64,
    pub drag_10y: f64,
    pub assumed_return: f64,
    /// Month of AMFI's TER disclosure, `MM-YYYY`.
    pub ter_month: String,
}

/// The Direct growth scheme of the same AMC and fund as `regular`.
pub fn direct_twin<'a>(schemes: &'a [Scheme], regular: &Scheme) -> Option<&'a Scheme> {
    let key = ter_key(base_scheme_name(&regular.name));
    schemes.iter().find(|s| {
        s.is_direct()
            && s.is_growth()
            && s.amc == regular.amc
            && ter_key(base_scheme_name(&s.name)) == key
    })
}

/// Match every held `MF:` Regular growth plan to its Direct twin and work out
/// the bleed. `nav_of` gives a symbol's NAV (the AMFI NAV in the live command).
pub fn report(
    portfolio: &Portfolio,
    schemes: &[Scheme],
    ter: &TerTable,
    assumed_return: f64,
    today: NaiveDate,
    realized: Buckets,
) -> BleedReport {
    let mut r = BleedReport {
        funds: Vec::new(),
        skipped: Vec::new(),
        annual_bleed: 0.0,
        drag_10y: 0.0,
        assumed_return,
        ter_month: ter.month.clone(),
    };
    for h in &portfolio.holdings {
        let Some(id) = fund_id(&h.symbol) else {
            continue;
        };
        let skip = |reason: &str| Skipped {
            symbol: h.symbol.clone(),
            reason: reason.to_string(),
        };
        let Some(scheme) = find_scheme(schemes, id) else {
            r.skipped.push(skip("not in AMFI's NAV file"));
            continue;
        };
        if scheme.is_direct() {
            continue;
        }
        if !scheme.is_growth() {
            r.skipped.push(skip(
                "IDCW/dividend option: only growth plans are compared with their Direct twin",
            ));
            continue;
        }
        let Some(nav) = scheme.nav else {
            r.skipped.push(skip("AMFI publishes no NAV"));
            continue;
        };
        let Some(rec) = ter.find(&scheme.name) else {
            r.skipped.push(skip("not in AMFI's TER disclosure"));
            continue;
        };
        let (Some(reg), Some(dir)) = (&rec.regular, &rec.direct) else {
            r.skipped.push(skip(
                "AMFI discloses no TER for both Regular and Direct plans",
            ));
            continue;
        };
        let category = scheme.category.clone();
        let input = BleedInput {
            symbol: &h.symbol,
            lots: &h.lots,
            nav,
            ter_regular: reg.total,
            ter_direct: dir.total,
            assumed_return,
            today,
            realized,
            equity_oriented: is_equity_oriented(category.as_deref().unwrap_or(""), &scheme.name),
            exit_load: ExitLoadTerms::equity_default(),
        };
        let bleed = analyze(&input);
        r.annual_bleed += bleed.annual_bleed;
        r.drag_10y += bleed.drag_10y;
        r.funds.push(FundBleed {
            name: scheme.full_name(),
            category,
            twin: direct_twin(schemes, scheme).map(|t| Twin {
                code: t.code,
                name: t.full_name(),
            }),
            bleed,
        });
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn input<'a>(lots: &'a [Lot], realized: Buckets) -> BleedInput<'a> {
        BleedInput {
            symbol: "MF:100001",
            lots,
            nav: 200.0,
            ter_regular: 0.0210,
            ter_direct: 0.0080,
            assumed_return: 0.10,
            today: d("2026-11-08"),
            realized,
            equity_oriented: true,
            exit_load: ExitLoadTerms::equity_default(),
        }
    }

    #[test]
    fn annual_bleed_and_ten_year_drag() {
        // 1,000 units × ₹200 = ₹2,00,000; gap 1.30% → ₹2,600 a year.
        let lots = [Lot::new(1000.0, 100.0, "2024-01-01", "import:cas")];
        let b = analyze(&input(&lots, Buckets::default()));
        assert!((b.value - 200_000.0).abs() < 1e-6);
        assert!((b.ter_gap - 0.013).abs() < 1e-12);
        assert!((b.annual_bleed - 2_600.0).abs() < 1e-6);
        let expect = 200_000.0 * (1.092_f64.powi(10) - 1.079_f64.powi(10));
        assert!((b.drag_10y - expect).abs() < 1e-6);
        assert!(b.drag_10y > 10.0 * b.annual_bleed * 0.9);
    }

    #[test]
    fn tax_free_units_stop_at_exemption() {
        // Lot 1: 500 units, gain ₹100 each = ₹50,000. Lot 2: 1,000 units,
        // gain ₹100 each = ₹1,00,000. Exemption ₹1,25,000 covers lot 1 and
        // 750 units of lot 2.
        let lots = [
            Lot::new(500.0, 100.0, "2023-01-01", "import:cas"),
            Lot::new(1000.0, 100.0, "2024-01-01", "import:cas"),
        ];
        let b = analyze(&input(&lots, Buckets::default()));
        assert!((b.switch.exemption_left - 125_000.0).abs() < 1e-6);
        assert!((b.switch.tax_free_units - 1250.0).abs() < 1e-6);
        assert!((b.switch.tax_free_gain - 125_000.0).abs() < 1e-6);
        // Switching all 1,500: ₹25,000 over the exemption at 12.5% + 4% cess.
        assert!((b.switch.tax_all_long_term - 25_000.0 * 0.125 * 1.04).abs() < 1e-6);
    }

    #[test]
    fn used_exemption_shrinks_headroom() {
        let lots = [Lot::new(1000.0, 100.0, "2024-01-01", "import:cas")];
        let realized = Buckets {
            lt_gain: 100_000.0,
            ..Buckets::default()
        };
        let b = analyze(&input(&lots, realized));
        assert!((b.switch.exemption_left - 25_000.0).abs() < 1e-6);
        assert!((b.switch.tax_free_units - 250.0).abs() < 1e-6);
    }

    #[test]
    fn early_lot_breakeven() {
        // 100 units bought 2026-06-01 at ₹180: gain ₹2,000. STCG 20% = 400
        // + 4% cess = 416. Exit load 1% of ₹20,000 = 200. Annual TER saved
        // = 20,000 × 1.30% = 260 → (416 + 200) / (260 / 12) months.
        let lots = [Lot::new(100.0, 180.0, "2026-06-01", "import:cas")];
        let b = analyze(&input(&lots, Buckets::default()));
        assert_eq!(b.switch.long_term_units, 0.0);
        let e = &b.switch.early[0];
        assert_eq!(e.days_held, 160);
        assert_eq!(e.long_term_on, "2027-06-02");
        assert!((e.tax_today - 416.0).abs() < 1e-6);
        assert!((e.exit_load - 200.0).abs() < 1e-6);
        assert!((e.annual_ter_saved - 260.0).abs() < 1e-6);
        assert!((e.breakeven_months.unwrap() - 616.0 / (260.0 / 12.0)).abs() < 1e-9);
    }

    #[test]
    fn debt_fund_skips_tax_model() {
        let lots = [Lot::new(100.0, 100.0, "2024-01-01", "import:cas")];
        let mut i = input(&lots, Buckets::default());
        i.equity_oriented = false;
        let b = analyze(&i);
        assert!(!b.switch.tax_modeled);
        assert_eq!(b.switch.tax_free_units, 0.0);
        assert_eq!(b.switch.tax_all_long_term, 0.0);
        assert!((b.switch.long_term_units - 100.0).abs() < 1e-9);
    }

    #[test]
    fn no_gap_no_bleed() {
        let lots = [Lot::new(10.0, 100.0, "2024-01-01", "import:cas")];
        let mut i = input(&lots, Buckets::default());
        i.ter_direct = i.ter_regular;
        let b = analyze(&i);
        assert_eq!(b.annual_bleed, 0.0);
        assert!(b.drag_10y.abs() < 1e-9);
    }

    fn scheme(code: u32, name: &str, plan: &str) -> Scheme {
        Scheme {
            code,
            name: name.into(),
            plan: Some(plan.into()),
            option: Some("Growth".into()),
            nav: Some(200.0),
            date: Some(d("2026-11-06")),
            isin_growth: Some(format!("INF000K{code:05}")),
            isin_reinvest: None,
            amc: Some("Example Mutual Fund".into()),
            category: Some("Equity Scheme - Flexi Cap Fund".into()),
            scheme_type: Some("Open Ended Schemes".into()),
        }
    }

    #[test]
    fn report_matches_regular_to_direct_twin() {
        use crate::funds::{TerCosts, TerRecord};
        let costs = |t: f64| TerCosts {
            total: t,
            base: None,
            brokerage: None,
            transaction: None,
            statutory_levies: None,
        };
        let schemes = vec![
            scheme(
                100001,
                "Example Flexi Cap Fund - Regular Plan",
                "Regular Plan",
            ),
            scheme(
                100002,
                "Example Flexi Cap Fund - Direct Plan",
                "Direct Plan",
            ),
            scheme(100003, "Example Bluechip Fund - Direct Plan", "Direct Plan"),
        ];
        let ter = TerTable::new(
            "10-2026".into(),
            vec![TerRecord {
                name: "Example Flexi Cap Fund".into(),
                nsdl_code: None,
                category: None,
                date: d("2026-10-31"),
                regular: Some(costs(0.0210)),
                direct: Some(costs(0.0080)),
            }],
        );
        let mut p = Portfolio::default();
        p.add_lot(
            "MF:100001",
            Lot::new(1000.0, 100.0, "2024-01-01", "import:cas"),
        );
        // A Direct holding and a non-fund are ignored; an unknown fund is skipped.
        p.add_lot(
            "MF:100003",
            Lot::new(10.0, 100.0, "2024-01-01", "import:cas"),
        );
        p.add_lot(
            "MF:999999",
            Lot::new(10.0, 100.0, "2024-01-01", "import:cas"),
        );
        p.add_lot("TCS.NS", Lot::new(1.0, 100.0, "2024-01-01", "manual"));
        let r = report(
            &p,
            &schemes,
            &ter,
            0.10,
            d("2026-11-08"),
            Buckets::default(),
        );
        assert_eq!(r.funds.len(), 1);
        let f = &r.funds[0];
        assert_eq!(f.twin.as_ref().unwrap().code, 100002);
        assert!((f.bleed.annual_bleed - 2_600.0).abs() < 1e-6);
        assert!((r.annual_bleed - 2_600.0).abs() < 1e-6);
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].symbol, "MF:999999");
        assert!(f.bleed.switch.tax_modeled);
    }
}
