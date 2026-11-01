//! Capital-gains tax estimates: holding period per lot, tax years, and each
//! country's rules for setting losses off against gains. These are estimates
//! for listed shares only; `stockwise tax` labels them as such.

use chrono::{Datelike, Months, NaiveDate};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Country {
    India,
    Us,
    Uk,
}

impl Country {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "in" | "india" => Some(Self::India),
            "us" | "usa" => Some(Self::Us),
            "uk" => Some(Self::Uk),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::India => "India",
            Self::Us => "United States",
            Self::Uk => "United Kingdom",
        }
    }

    pub fn currency_symbol(self) -> &'static str {
        match self {
            Self::India => "₹",
            Self::Us => "$",
            Self::Uk => "£",
        }
    }

    /// Months a share must be held for a gain to be long-term; `None` where
    /// the holding period doesn't matter (UK).
    pub fn long_term_months(self) -> Option<u32> {
        match self {
            Self::India | Self::Us => Some(12),
            Self::Uk => None,
        }
    }

    /// Short-term rate in percent: India Sec. 111A; US top ordinary-income
    /// bracket; UK higher-rate CGT (18% for basic-rate taxpayers).
    pub fn st_rate(self) -> f64 {
        match self {
            Self::India => 20.0,
            Self::Us => 37.0,
            Self::Uk => 24.0,
        }
    }

    /// Long-term rate in percent: India Sec. 112A; US top LTCG bracket.
    pub fn lt_rate(self) -> f64 {
        match self {
            Self::India => 12.5,
            Self::Us => 20.0,
            Self::Uk => 24.0,
        }
    }

    /// Yearly tax-free gains: India ₹1.25 lakh of LTCG; UK £3,000 annual exempt amount.
    pub fn exemption(self) -> f64 {
        match self {
            Self::India => 125_000.0,
            Self::Us => 0.0,
            Self::Uk => 3_000.0,
        }
    }

    /// Scale from a quote currency to this country's tax currency, or `None`
    /// for holdings in another currency (taxed under other rules).
    pub fn currency_scale(self, currency: &str) -> Option<f64> {
        match (self, currency) {
            (Self::India, "INR") | (Self::Us, "USD") | (Self::Uk, "GBP") => Some(1.0),
            (Self::Uk, "GBp") => Some(0.01),
            _ => None,
        }
    }

    pub fn tax_currency(self) -> &'static str {
        match self {
            Self::India => "INR",
            Self::Us => "USD",
            Self::Uk => "GBP",
        }
    }

    /// First and last day of the tax year containing `today`. India runs
    /// April–March; the UK year starts on 6 April; the US uses the calendar year.
    pub fn tax_year(self, today: NaiveDate) -> (NaiveDate, NaiveDate) {
        let y = today.year();
        let ymd = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).expect("valid date");
        match self {
            Self::Us => (ymd(y, 1, 1), ymd(y, 12, 31)),
            Self::India => {
                let start = if today.month() >= 4 { y } else { y - 1 };
                (ymd(start, 4, 1), ymd(start + 1, 3, 31))
            }
            Self::Uk => {
                let start = if (today.month(), today.day()) >= (4, 6) {
                    y
                } else {
                    y - 1
                };
                (ymd(start, 4, 6), ymd(start + 1, 4, 5))
            }
        }
    }

    /// "FY 2026-27" (India), "Tax year 2026-27" (UK), "Tax year 2026" (US).
    pub fn tax_year_label(self, today: NaiveDate) -> String {
        let (start, _) = self.tax_year(today);
        let y = start.year();
        match self {
            Self::Us => format!("Tax year {}", y),
            Self::Uk => format!("Tax year {}-{:02}", y, (y + 1) % 100),
            Self::India => format!("FY {}-{:02}", y, (y + 1) % 100),
        }
    }

    /// Whether shares bought on `bought` and sold on `sold` are a long-term holding.
    pub fn is_long_term(self, bought: NaiveDate, sold: NaiveDate) -> bool {
        self.long_term_months()
            .is_some_and(|m| is_long_term(bought, sold, m))
    }
}

/// Long-term when held for more than `months`: sold after the same calendar
/// date `months` later.
pub fn is_long_term(bought: NaiveDate, sold: NaiveDate, months: u32) -> bool {
    bought
        .checked_add_months(Months::new(months))
        .is_some_and(|threshold| sold > threshold)
}

/// Gains and losses split by holding period. All amounts are positive.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Buckets {
    pub st_gain: f64,
    pub st_loss: f64,
    pub lt_gain: f64,
    pub lt_loss: f64,
}

impl Buckets {
    pub fn add(&mut self, gain: f64, long_term: bool) {
        match (gain >= 0.0, long_term) {
            (true, true) => self.lt_gain += gain,
            (true, false) => self.st_gain += gain,
            (false, true) => self.lt_loss -= gain,
            (false, false) => self.st_loss -= gain,
        }
    }

    pub fn plus(self, o: Self) -> Self {
        Self {
            st_gain: self.st_gain + o.st_gain,
            st_loss: self.st_loss + o.st_loss,
            lt_gain: self.lt_gain + o.lt_gain,
            lt_loss: self.lt_loss + o.lt_loss,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.st_gain + self.st_loss + self.lt_gain + self.lt_loss == 0.0
    }
}

/// Tax on a set of gains after set-off. For the UK, which has no short/long
/// split, everything is reported in the short-term fields.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// Short-term gain left after set-off (taxable).
    pub st_taxable: f64,
    /// Long-term gain left after set-off, before the exemption.
    pub lt_net: f64,
    pub exemption_used: f64,
    pub lt_taxable: f64,
    pub st_tax: f64,
    pub lt_tax: f64,
    /// Short-term loss set off against long-term gains.
    pub st_loss_against_lt: f64,
    /// Long-term loss set off against short-term gains (US only).
    pub lt_loss_against_st: f64,
    /// Losses left over to carry forward.
    pub st_carry: f64,
    pub lt_carry: f64,
}

impl Estimate {
    pub fn total(&self) -> f64 {
        self.st_tax + self.lt_tax
    }
}

pub fn estimate(country: Country, b: Buckets) -> Estimate {
    match country {
        Country::India => india(b),
        Country::Us => us(b),
        Country::Uk => uk(b),
    }
}

/// India (listed shares with STT, from 23 July 2024): gains and losses of
/// the same kind net first. A net short-term loss is set off against
/// short-term gains and then long-term gains; a long-term loss only against
/// long-term gains — never short-term. Then ₹1.25 lakh of long-term gain is
/// exempt; STCG is taxed at 20%, the rest of LTCG at 12.5%. Unused losses
/// carry forward (8 years).
fn india(b: Buckets) -> Estimate {
    let c = Country::India;
    let mut st = b.st_gain - b.st_loss;
    let mut lt = b.lt_gain - b.lt_loss;
    let mut e = Estimate::default();
    if lt < 0.0 {
        e.lt_carry = -lt;
        lt = 0.0;
    }
    if st < 0.0 {
        let used = (-st).min(lt);
        e.st_loss_against_lt = used;
        e.st_carry = -st - used;
        lt -= used;
        st = 0.0;
    }
    e.st_taxable = st;
    e.lt_net = lt;
    e.exemption_used = lt.min(c.exemption());
    e.lt_taxable = lt - e.exemption_used;
    e.st_tax = e.st_taxable * c.st_rate() / 100.0;
    e.lt_tax = e.lt_taxable * c.lt_rate() / 100.0;
    e
}

/// US: short- and long-term results net within themselves, then a net loss
/// on one side offsets a net gain on the other. A loss left over is
/// deductible up to $3,000 a year against other income, the rest carries forward.
fn us(b: Buckets) -> Estimate {
    let c = Country::Us;
    let mut st = b.st_gain - b.st_loss;
    let mut lt = b.lt_gain - b.lt_loss;
    let mut e = Estimate::default();
    if st < 0.0 && lt > 0.0 {
        let used = (-st).min(lt);
        e.st_loss_against_lt = used;
        lt -= used;
        st += used;
    } else if lt < 0.0 && st > 0.0 {
        let used = (-lt).min(st);
        e.lt_loss_against_st = used;
        st -= used;
        lt += used;
    }
    e.st_carry = (-st).max(0.0);
    e.lt_carry = (-lt).max(0.0);
    e.st_taxable = st.max(0.0);
    e.lt_net = lt.max(0.0);
    e.lt_taxable = e.lt_net;
    e.st_tax = e.st_taxable * c.st_rate() / 100.0;
    e.lt_tax = e.lt_taxable * c.lt_rate() / 100.0;
    e
}

/// UK: no holding-period split. All gains and losses net, then £3,000 is
/// exempt and the rest is taxed at the CGT rate.
fn uk(b: Buckets) -> Estimate {
    let c = Country::Uk;
    let net = b.st_gain + b.lt_gain - b.st_loss - b.lt_loss;
    let mut e = Estimate::default();
    if net < 0.0 {
        e.st_carry = -net;
        return e;
    }
    e.exemption_used = net.min(c.exemption());
    e.st_taxable = net - e.exemption_used;
    e.st_tax = e.st_taxable * c.st_rate() / 100.0;
    e
}

// ── Advance Tax (India: Section 208, Section 211, Section 234B/C) ──

/// Statutory threshold under Section 208: advance tax applies if the total
/// estimated tax payable for the year is ₹10,000 or more.
pub const ADVANCE_TAX_SEC_208_THRESHOLD: f64 = 10_000.0;

/// Status of an advance tax quarterly instalment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdvanceTaxStatus {
    Paid,
    Pending,
    Upcoming,
}

impl std::fmt::Display for AdvanceTaxStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Paid => write!(f, "Paid"),
            Self::Pending => write!(f, "Pending"),
            Self::Upcoming => write!(f, "Upcoming"),
        }
    }
}

/// A realized capital gains/loss event used for advance tax bucketing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdvanceTaxSale {
    pub date: NaiveDate,
    pub gain: f64,
    pub is_long_term: bool,
}

impl AdvanceTaxSale {
    pub fn new(date: NaiveDate, gain: f64, is_long_term: bool) -> Self {
        Self {
            date,
            gain,
            is_long_term,
        }
    }
}

/// One quarterly instalment under Indian Income Tax Act (Section 211 / Section 234C).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdvanceTaxQuarter {
    /// Quarter identifier: "Q1", "Q2", "Q3", "Q4".
    pub quarter: String,
    /// Statutory due date: 15 Jun, 15 Sep, 15 Dec, 15 Mar.
    pub due_date: NaiveDate,
    /// Cut-off date for transactions contributing to this instalment.
    pub cutoff_date: NaiveDate,
    /// Net STCG realized in this quarter alone.
    pub stcg_quarter: f64,
    /// Net LTCG realized in this quarter alone.
    pub ltcg_quarter: f64,
    /// Cumulative STCG realized up to this quarter's cut-off.
    pub stcg_cumulative: f64,
    /// Cumulative LTCG realized up to this quarter's cut-off.
    pub ltcg_cumulative: f64,
    /// Cumulative tax estimated on realized gains up to this cut-off.
    pub cumulative_tax_estimated: f64,
    /// Minimum required cumulative percentage under Section 211 (15%, 45%, 75%, 100%).
    pub required_cumulative_pct: f64,
    /// Minimum cumulative tax required by this due date.
    pub min_cumulative_tax_required: f64,
    /// Estimated tax payable in this specific quarter.
    pub quarter_tax_payable: f64,
    /// Cumulative advance tax paid up to this quarter (if tracked).
    pub paid: f64,
    /// Shortfall below the required minimum cumulative tax.
    pub shortfall: f64,
    /// Estimated Section 234C interest on shortfall (3% for Q1/Q2/Q3, 1% for Q4).
    pub sec_234c_interest: f64,
    /// Status: Paid, Pending, or Upcoming.
    pub status: AdvanceTaxStatus,
}

/// Advance tax schedule for an Indian financial year.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdvanceTaxSchedule {
    /// Financial year label, e.g. "FY 2025-26 (AY 2026-27)".
    pub fy_label: String,
    /// Start date of the financial year (1 April).
    pub fy_start: NaiveDate,
    /// End date of the financial year (31 March).
    pub fy_end: NaiveDate,
    /// Evaluation date.
    pub as_of: NaiveDate,
    /// Total net STCG realized across the financial year.
    pub total_stcg: f64,
    /// Total net LTCG realized across the financial year.
    pub total_ltcg: f64,
    /// Total estimated tax on all realized gains for the FY.
    pub total_tax_estimated: f64,
    /// Section 208 applicability threshold (₹10,000).
    pub sec_208_threshold: f64,
    /// True if total_tax_estimated >= 10,000.
    pub sec_208_applicable: bool,
    /// Total advance tax paid so far.
    pub total_paid: f64,
    /// Total Section 234C deferment interest.
    pub total_sec_234c_interest: f64,
    /// True if evaluated after 31 March and paid < 90% of total tax.
    pub sec_234b_applicable: bool,
    /// Four quarterly instalments.
    pub quarters: Vec<AdvanceTaxQuarter>,
}

/// The four advance tax quarterly milestones (due date, cut-off date, quarter name, cumulative rate)
/// for a financial year starting in `fy_start_year`.
pub fn advance_tax_cutoffs(fy_start_year: i32) -> [(NaiveDate, NaiveDate, &'static str, f64); 4] {
    let y = fy_start_year;
    [
        (
            NaiveDate::from_ymd_opt(y, 6, 15).expect("valid date"),
            NaiveDate::from_ymd_opt(y, 6, 15).expect("valid date"),
            "Q1",
            15.0,
        ),
        (
            NaiveDate::from_ymd_opt(y, 9, 15).expect("valid date"),
            NaiveDate::from_ymd_opt(y, 9, 15).expect("valid date"),
            "Q2",
            45.0,
        ),
        (
            NaiveDate::from_ymd_opt(y, 12, 15).expect("valid date"),
            NaiveDate::from_ymd_opt(y, 12, 15).expect("valid date"),
            "Q3",
            75.0,
        ),
        (
            NaiveDate::from_ymd_opt(y + 1, 3, 15).expect("valid date"),
            NaiveDate::from_ymd_opt(y + 1, 3, 31).expect("valid date"),
            "Q4",
            100.0,
        ),
    ]
}

/// Compute quarterly advance tax instalments under Section 208 and 211 of
/// the Indian Income-tax Act, with Section 234C(1) proviso protections for
/// capital gains.
///
/// The proviso to Section 234C(1) provides that no interest for deferment
/// of advance tax is charged for shortfalls in earlier quarters if the
/// shortfall is on account of capital gains realized in subsequent quarters.
/// Accordingly, each quarter's required cumulative tax is estimated based
/// only on cumulative gains realized up to that quarter's cut-off date.
pub fn compute_advance_tax(
    sales: &[AdvanceTaxSale],
    fy_start: NaiveDate,
    as_of: NaiveDate,
    paid: Option<f64>,
) -> AdvanceTaxSchedule {
    let y = if fy_start.month() >= 4 {
        fy_start.year()
    } else {
        fy_start.year() - 1
    };
    let fy_start = NaiveDate::from_ymd_opt(y, 4, 1).expect("valid date");
    let fy_end = NaiveDate::from_ymd_opt(y + 1, 3, 31).expect("valid date");
    let fy_label = format!(
        "FY {}-{:02} (AY {}-{:02})",
        y,
        (y + 1) % 100,
        y + 1,
        (y + 2) % 100
    );

    struct QuarterMeta {
        quarter: &'static str,
        due_date: NaiveDate,
        period_start: NaiveDate,
        period_end: NaiveDate,
        cutoff_date: NaiveDate,
        req_pct: f64,
        safe_harbour_pct: f64,
        interest_rate_pct: f64,
    }

    let meta = [
        QuarterMeta {
            quarter: "Q1",
            due_date: NaiveDate::from_ymd_opt(y, 6, 15).expect("valid date"),
            period_start: fy_start,
            period_end: NaiveDate::from_ymd_opt(y, 6, 15).expect("valid date"),
            cutoff_date: NaiveDate::from_ymd_opt(y, 6, 15).expect("valid date"),
            req_pct: 15.0,
            safe_harbour_pct: 12.0,
            interest_rate_pct: 3.0,
        },
        QuarterMeta {
            quarter: "Q2",
            due_date: NaiveDate::from_ymd_opt(y, 9, 15).expect("valid date"),
            period_start: NaiveDate::from_ymd_opt(y, 6, 16).expect("valid date"),
            period_end: NaiveDate::from_ymd_opt(y, 9, 15).expect("valid date"),
            cutoff_date: NaiveDate::from_ymd_opt(y, 9, 15).expect("valid date"),
            req_pct: 45.0,
            safe_harbour_pct: 36.0,
            interest_rate_pct: 3.0,
        },
        QuarterMeta {
            quarter: "Q3",
            due_date: NaiveDate::from_ymd_opt(y, 12, 15).expect("valid date"),
            period_start: NaiveDate::from_ymd_opt(y, 9, 16).expect("valid date"),
            period_end: NaiveDate::from_ymd_opt(y, 12, 15).expect("valid date"),
            cutoff_date: NaiveDate::from_ymd_opt(y, 12, 15).expect("valid date"),
            req_pct: 75.0,
            safe_harbour_pct: 75.0,
            interest_rate_pct: 3.0,
        },
        QuarterMeta {
            quarter: "Q4",
            due_date: NaiveDate::from_ymd_opt(y + 1, 3, 15).expect("valid date"),
            period_start: NaiveDate::from_ymd_opt(y, 12, 16).expect("valid date"),
            period_end: fy_end,
            cutoff_date: fy_end,
            req_pct: 100.0,
            safe_harbour_pct: 100.0,
            interest_rate_pct: 1.0,
        },
    ];

    let total_paid = paid.unwrap_or(0.0).max(0.0);

    // Compute cumulative taxes and quarter gains
    let mut quarters_data = Vec::with_capacity(4);
    let mut prior_max_required = 0.0;

    for m in &meta {
        let mut q_buckets = Buckets::default();
        for s in sales
            .iter()
            .filter(|s| s.date >= m.period_start && s.date <= m.period_end)
        {
            q_buckets.add(s.gain, s.is_long_term);
        }
        let stcg_quarter = q_buckets.st_gain - q_buckets.st_loss;
        let ltcg_quarter = q_buckets.lt_gain - q_buckets.lt_loss;

        let mut cum_buckets = Buckets::default();
        for s in sales
            .iter()
            .filter(|s| s.date >= fy_start && s.date <= m.cutoff_date)
        {
            cum_buckets.add(s.gain, s.is_long_term);
        }
        let stcg_cumulative = cum_buckets.st_gain - cum_buckets.st_loss;
        let ltcg_cumulative = cum_buckets.lt_gain - cum_buckets.lt_loss;
        let est = estimate(Country::India, cum_buckets);
        let cumulative_tax_estimated = est.total();

        let min_cumulative_tax_required = cumulative_tax_estimated * (m.req_pct / 100.0);
        let quarter_tax_payable = (min_cumulative_tax_required - prior_max_required).max(0.0);
        prior_max_required = prior_max_required.max(min_cumulative_tax_required);

        quarters_data.push((
            stcg_quarter,
            ltcg_quarter,
            stcg_cumulative,
            ltcg_cumulative,
            cumulative_tax_estimated,
            min_cumulative_tax_required,
            quarter_tax_payable,
        ));
    }

    let total_tax_estimated = quarters_data[3].4;
    let sec_208_applicable = total_tax_estimated >= ADVANCE_TAX_SEC_208_THRESHOLD;

    let mut quarters = Vec::with_capacity(4);

    for (i, m) in meta.iter().enumerate() {
        let (
            stcg_quarter,
            ltcg_quarter,
            stcg_cumulative,
            ltcg_cumulative,
            cumulative_tax_estimated,
            min_cumulative_tax_required,
            quarter_tax_payable,
        ) = quarters_data[i];

        let (quarter_paid, shortfall, sec_234c_interest, status) = if let Some(p) = paid {
            let p = p.max(0.0);
            let paid_for_q = p.min(min_cumulative_tax_required);
            let shortfall = (min_cumulative_tax_required - p).max(0.0);
            let interest = if sec_208_applicable
                && as_of > m.due_date
                && p < (m.safe_harbour_pct / 100.0) * cumulative_tax_estimated
            {
                shortfall * (m.interest_rate_pct / 100.0)
            } else {
                0.0
            };

            let status = if as_of < m.period_start {
                if p >= min_cumulative_tax_required && min_cumulative_tax_required > 0.0 {
                    AdvanceTaxStatus::Paid
                } else {
                    AdvanceTaxStatus::Upcoming
                }
            } else if as_of > m.due_date {
                if p >= min_cumulative_tax_required {
                    AdvanceTaxStatus::Paid
                } else {
                    AdvanceTaxStatus::Pending
                }
            } else if p >= min_cumulative_tax_required && min_cumulative_tax_required > 0.0 {
                AdvanceTaxStatus::Paid
            } else {
                AdvanceTaxStatus::Pending
            };
            (paid_for_q, shortfall, interest, status)
        } else {
            let status = if as_of < m.period_start {
                AdvanceTaxStatus::Upcoming
            } else if as_of > m.due_date {
                if m.quarter == "Q4" && as_of <= fy_end {
                    AdvanceTaxStatus::Pending
                } else {
                    AdvanceTaxStatus::Paid
                }
            } else {
                AdvanceTaxStatus::Pending
            };
            (0.0, 0.0, 0.0, status)
        };

        quarters.push(AdvanceTaxQuarter {
            quarter: m.quarter.to_string(),
            due_date: m.due_date,
            cutoff_date: m.cutoff_date,
            stcg_quarter,
            ltcg_quarter,
            stcg_cumulative,
            ltcg_cumulative,
            cumulative_tax_estimated,
            required_cumulative_pct: m.req_pct,
            min_cumulative_tax_required,
            quarter_tax_payable,
            paid: quarter_paid,
            shortfall,
            sec_234c_interest,
            status,
        });
    }

    let total_sec_234c_interest = quarters.iter().map(|q| q.sec_234c_interest).sum();
    let sec_234b_applicable = sec_208_applicable
        && as_of > fy_end
        && paid.is_some_and(|p| p < 0.90 * total_tax_estimated);

    AdvanceTaxSchedule {
        fy_label,
        fy_start,
        fy_end,
        as_of,
        total_stcg: quarters[3].stcg_cumulative,
        total_ltcg: quarters[3].ltcg_cumulative,
        total_tax_estimated,
        sec_208_threshold: ADVANCE_TAX_SEC_208_THRESHOLD,
        sec_208_applicable,
        total_paid,
        total_sec_234c_interest,
        sec_234b_applicable,
        quarters,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn b(st_gain: f64, st_loss: f64, lt_gain: f64, lt_loss: f64) -> Buckets {
        Buckets {
            st_gain,
            st_loss,
            lt_gain,
            lt_loss,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn long_term_means_held_more_than_twelve_months() {
        let bought = d("2024-03-15");
        assert!(!is_long_term(bought, d("2025-03-15"), 12));
        assert!(is_long_term(bought, d("2025-03-16"), 12));
        // Month ends clamp: 29 Feb + 12 months = 28 Feb.
        assert!(is_long_term(d("2024-02-29"), d("2025-03-01"), 12));
        assert!(!Country::Uk.is_long_term(bought, d("2030-01-01")));
    }

    #[test]
    fn tax_years() {
        let fy = Country::India.tax_year(d("2026-10-21"));
        assert_eq!(fy, (d("2026-04-01"), d("2027-03-31")));
        assert_eq!(Country::India.tax_year(d("2027-03-31")).0, d("2026-04-01"));
        assert_eq!(Country::Uk.tax_year(d("2026-04-05")).0, d("2025-04-06"));
        assert_eq!(Country::Uk.tax_year(d("2026-04-06")).0, d("2026-04-06"));
        assert_eq!(Country::Us.tax_year(d("2026-10-21")).1, d("2026-12-31"));
        assert_eq!(Country::India.tax_year_label(d("2026-10-21")), "FY 2026-27");
        assert_eq!(Country::India.tax_year_label(d("2027-01-05")), "FY 2026-27");
        assert_eq!(
            Country::Uk.tax_year_label(d("2026-04-05")),
            "Tax year 2025-26"
        );
        assert_eq!(Country::Us.tax_year_label(d("2026-10-21")), "Tax year 2026");
    }

    #[test]
    fn buckets_split_gains_and_losses() {
        let mut x = Buckets::default();
        x.add(100.0, false);
        x.add(-40.0, false);
        x.add(300.0, true);
        x.add(-10.0, true);
        assert_eq!(x, b(100.0, 40.0, 300.0, 10.0));
        assert_eq!(x.plus(x), b(200.0, 80.0, 600.0, 20.0));
    }

    // India worked examples (amounts in ₹).

    #[test]
    fn india_gains_only_exemption_applies_to_ltcg() {
        // STCG 1,00,000 → 20% = 20,000.
        // LTCG 2,00,000 − 1,25,000 exempt = 75,000 → 12.5% = 9,375.
        let e = estimate(Country::India, b(100_000.0, 0.0, 200_000.0, 0.0));
        assert!(close(e.st_tax, 20_000.0));
        assert!(close(e.exemption_used, 125_000.0));
        assert!(close(e.lt_taxable, 75_000.0));
        assert!(close(e.lt_tax, 9_375.0));
        assert!(close(e.total(), 29_375.0));
    }

    #[test]
    fn india_short_term_loss_nets_against_short_term_gain_first() {
        // STCG 1,00,000 − STCL 30,000 = 70,000 → 14,000. LTCG untouched.
        let e = estimate(Country::India, b(100_000.0, 30_000.0, 150_000.0, 0.0));
        assert!(close(e.st_taxable, 70_000.0));
        assert!(close(e.st_tax, 14_000.0));
        assert!(close(e.st_loss_against_lt, 0.0));
        assert!(close(e.lt_taxable, 25_000.0));
    }

    #[test]
    fn india_excess_short_term_loss_is_set_off_against_ltcg() {
        // STCL 50,000 with no STCG reduces LTCG 3,00,000 to 2,50,000;
        // 1,25,000 exempt, 1,25,000 taxed at 12.5% = 15,625.
        let e = estimate(Country::India, b(0.0, 50_000.0, 300_000.0, 0.0));
        assert!(close(e.st_loss_against_lt, 50_000.0));
        assert!(close(e.lt_net, 250_000.0));
        assert!(close(e.lt_tax, 15_625.0));
        assert!(close(e.st_carry, 0.0));
    }

    #[test]
    fn india_long_term_loss_never_reduces_stcg() {
        // LTCL 1,00,000 can't touch STCG 1,00,000: 20,000 tax, LTCL carried.
        let e = estimate(Country::India, b(100_000.0, 0.0, 0.0, 100_000.0));
        assert!(close(e.st_tax, 20_000.0));
        assert!(close(e.lt_carry, 100_000.0));
        assert!(close(e.total(), 20_000.0));
    }

    #[test]
    fn india_mixed_losses_with_carry_forward() {
        // STCG 40k − STCL 1L = −60k short-term. LTCG 50k − LTCL 20k = 30k.
        // 30k of the short-term loss wipes out LTCG; 30k carries forward.
        let e = estimate(Country::India, b(40_000.0, 100_000.0, 50_000.0, 20_000.0));
        assert!(close(e.st_loss_against_lt, 30_000.0));
        assert!(close(e.st_carry, 30_000.0));
        assert!(close(e.lt_carry, 0.0));
        assert!(close(e.total(), 0.0));
    }

    #[test]
    fn india_ltcg_within_the_exemption_is_tax_free() {
        let e = estimate(Country::India, b(0.0, 0.0, 90_000.0, 0.0));
        assert!(close(e.exemption_used, 90_000.0));
        assert!(close(e.total(), 0.0));
    }

    #[test]
    fn us_net_loss_on_one_side_offsets_the_other() {
        // ST −5,000, LT +20,000 → LT 15,000 at 20% = 3,000.
        let e = estimate(Country::Us, b(1_000.0, 6_000.0, 20_000.0, 0.0));
        assert!(close(e.st_loss_against_lt, 5_000.0));
        assert!(close(e.lt_tax, 3_000.0));
        // LT −4,000 against ST +10,000 → ST 6,000 at 37%.
        let e = estimate(Country::Us, b(10_000.0, 0.0, 0.0, 4_000.0));
        assert!(close(e.lt_loss_against_st, 4_000.0));
        assert!(close(e.st_tax, 6_000.0 * 0.37));
        // Both negative: everything carries.
        let e = estimate(Country::Us, b(0.0, 1.0, 0.0, 2.0));
        assert!(close(e.st_carry, 1.0) && close(e.lt_carry, 2.0));
    }

    #[test]
    fn uk_nets_everything_then_applies_the_annual_exemption() {
        // Gains 10,000 − losses 2,000 = 8,000; 3,000 exempt; 5,000 at 24%.
        let e = estimate(Country::Uk, b(6_000.0, 2_000.0, 4_000.0, 0.0));
        assert!(close(e.exemption_used, 3_000.0));
        assert!(close(e.total(), 1_200.0));
        let e = estimate(Country::Uk, b(0.0, 500.0, 0.0, 0.0));
        assert!(close(e.st_carry, 500.0));
    }

    #[test]
    fn currency_scale_picks_the_tax_currency() {
        assert_eq!(Country::India.currency_scale("INR"), Some(1.0));
        assert_eq!(Country::India.currency_scale("USD"), None);
        assert_eq!(Country::Uk.currency_scale("GBp"), Some(0.01));
        assert_eq!(Country::parse("India"), Some(Country::India));
        assert_eq!(Country::parse("fr"), None);
    }

    #[test]
    fn advance_tax_cutoffs_are_correct() {
        let cutoffs = advance_tax_cutoffs(2025);
        assert_eq!(cutoffs[0], (d("2025-06-15"), d("2025-06-15"), "Q1", 15.0));
        assert_eq!(cutoffs[1], (d("2025-09-15"), d("2025-09-15"), "Q2", 45.0));
        assert_eq!(cutoffs[2], (d("2025-12-15"), d("2025-12-15"), "Q3", 75.0));
        assert_eq!(cutoffs[3], (d("2026-03-15"), d("2026-03-31"), "Q4", 100.0));
    }

    #[test]
    fn advance_tax_quarterly_schedule_bucketing_and_proviso() {
        // Sales across 4 quarters in FY 2025-26:
        // Q1 (before 15 Jun): STCG ₹1,00,000 -> Tax @ 20% = ₹20,000
        // Q2 (16 Jun - 15 Sep): LTCG ₹3,00,000 -> Tax on cumulative LTCG (3L - 1.25L = 1.75L @ 12.5% = ₹21,875)
        // Q3 (16 Sep - 15 Dec): STCG ₹50,000
        // Q4 (16 Dec - 31 Mar): LTCG ₹1,00,000
        let sales = vec![
            AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false),
            AdvanceTaxSale::new(d("2025-08-20"), 300_000.0, true),
            AdvanceTaxSale::new(d("2025-11-15"), 50_000.0, false),
            AdvanceTaxSale::new(d("2026-01-20"), 100_000.0, true),
        ];

        let fy_start = d("2025-04-01");
        let schedule = compute_advance_tax(&sales, fy_start, d("2025-11-01"), None);

        assert_eq!(schedule.fy_label, "FY 2025-26 (AY 2026-27)");
        assert!(schedule.sec_208_applicable);

        // Q1 checks:
        let q1 = &schedule.quarters[0];
        assert_eq!(q1.quarter, "Q1");
        assert_eq!(q1.due_date, d("2025-06-15"));
        assert!(close(q1.stcg_cumulative, 100_000.0));
        assert!(close(q1.ltcg_cumulative, 0.0));
        assert!(close(q1.cumulative_tax_estimated, 20_000.0));
        assert!(close(q1.required_cumulative_pct, 15.0));
        assert!(close(q1.min_cumulative_tax_required, 3_000.0));
        assert!(close(q1.quarter_tax_payable, 3_000.0));
        assert_eq!(q1.status, AdvanceTaxStatus::Paid); // Past due date

        // Q2 checks:
        let q2 = &schedule.quarters[1];
        assert_eq!(q2.quarter, "Q2");
        assert_eq!(q2.due_date, d("2025-09-15"));
        assert!(close(q2.stcg_cumulative, 100_000.0));
        assert!(close(q2.ltcg_cumulative, 300_000.0));
        // STCG 20k + LTCG (1.75L * 12.5% = 21,875) = 41,875
        assert!(close(q2.cumulative_tax_estimated, 41_875.0));
        assert!(close(q2.required_cumulative_pct, 45.0));
        assert!(close(q2.min_cumulative_tax_required, 18_843.75));
        assert!(close(q2.quarter_tax_payable, 15_843.75));
        assert_eq!(q2.status, AdvanceTaxStatus::Paid); // Past due date

        // Q3 checks:
        let q3 = &schedule.quarters[2];
        assert_eq!(q3.quarter, "Q3");
        assert_eq!(q3.due_date, d("2025-12-15"));
        assert!(close(q3.stcg_cumulative, 150_000.0));
        assert!(close(q3.ltcg_cumulative, 300_000.0));
        // STCG 30k + LTCG 21,875 = 51,875
        assert!(close(q3.cumulative_tax_estimated, 51_875.0));
        assert!(close(q3.required_cumulative_pct, 75.0));
        assert!(close(q3.min_cumulative_tax_required, 38_906.25));
        assert!(close(q3.quarter_tax_payable, 20_062.50));
        assert_eq!(q3.status, AdvanceTaxStatus::Pending); // as_of is 2025-11-01 (in Q3)

        // Q4 checks:
        let q4 = &schedule.quarters[3];
        assert_eq!(q4.quarter, "Q4");
        assert_eq!(q4.due_date, d("2026-03-15"));
        assert!(close(q4.stcg_cumulative, 150_000.0));
        assert!(close(q4.ltcg_cumulative, 400_000.0));
        // STCG 30k + LTCG 2.75L * 12.5% (34,375) = 64,375
        assert!(close(q4.cumulative_tax_estimated, 64_375.0));
        assert!(close(q4.required_cumulative_pct, 100.0));
        assert!(close(q4.min_cumulative_tax_required, 64_375.0));
        assert!(close(q4.quarter_tax_payable, 25_468.75));
        assert_eq!(q4.status, AdvanceTaxStatus::Upcoming); // Q4 is future relative to 2025-11-01

        // Verify total sum of quarter payables matches total tax
        let sum_payable: f64 = schedule
            .quarters
            .iter()
            .map(|q| q.quarter_tax_payable)
            .sum();
        assert!(close(sum_payable, 64_375.0));
        assert!(close(schedule.total_tax_estimated, 64_375.0));
    }

    #[test]
    fn advance_tax_losses_in_subsequent_quarters() {
        // Gain in Q1: STCG ₹1,00,000 -> Tax ₹20,000
        // Loss in Q2: STCL ₹70,000 -> Net cumulative STCG ₹30,000 -> Tax ₹6,000
        // Min req Q1 = 3,000 (payable = 3,000)
        // Min req Q2 = 45% * 6,000 = 2,700. Since prior req was 3,000, Q2 payable is 0.0
        let sales = vec![
            AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false),
            AdvanceTaxSale::new(d("2025-07-15"), -70_000.0, false),
        ];

        let schedule = compute_advance_tax(&sales, d("2025-04-01"), d("2025-10-01"), None);
        assert!(close(schedule.quarters[0].quarter_tax_payable, 3_000.0));
        assert!(close(schedule.quarters[1].quarter_tax_payable, 0.0));
        assert!(close(
            schedule.quarters[1].min_cumulative_tax_required,
            2_700.0
        ));
    }

    #[test]
    fn advance_tax_sec_208_threshold_exemption() {
        // Gain: STCG ₹40,000 -> Tax @ 20% = ₹8,000 (< ₹10,000 threshold)
        let sales = vec![AdvanceTaxSale::new(d("2025-05-10"), 40_000.0, false)];
        let schedule = compute_advance_tax(&sales, d("2025-04-01"), d("2025-06-01"), None);

        assert!(!schedule.sec_208_applicable);
        assert!(close(schedule.total_tax_estimated, 8_000.0));
        assert_eq!(schedule.sec_208_threshold, 10_000.0);
    }

    #[test]
    fn advance_tax_status_progression_across_quarters() {
        let sales = vec![AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false)];

        // Before Q1 (e.g. 2025-03-15) -> Q1 upcoming
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-03-15"), None);
        assert_eq!(s.quarters[0].status, AdvanceTaxStatus::Upcoming);

        // During Q1 (e.g. 2025-05-01) -> Q1 pending
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-05-01"), None);
        assert_eq!(s.quarters[0].status, AdvanceTaxStatus::Pending);
        assert_eq!(s.quarters[1].status, AdvanceTaxStatus::Upcoming);

        // During Q2 (e.g. 2025-07-01) -> Q1 paid, Q2 pending
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-07-01"), None);
        assert_eq!(s.quarters[0].status, AdvanceTaxStatus::Paid);
        assert_eq!(s.quarters[1].status, AdvanceTaxStatus::Pending);

        // Post FY (e.g. 2026-04-10) -> All paid
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2026-04-10"), None);
        assert_eq!(s.quarters[0].status, AdvanceTaxStatus::Paid);
        assert_eq!(s.quarters[1].status, AdvanceTaxStatus::Paid);
        assert_eq!(s.quarters[2].status, AdvanceTaxStatus::Paid);
        assert_eq!(s.quarters[3].status, AdvanceTaxStatus::Paid);
    }

    #[test]
    fn advance_tax_paid_shortfall_and_234c_interest() {
        // STCG ₹1,00,000 in Q1 -> Tax ₹20,000 (Min Q1 req ₹3,000, Min Q2 req ₹9,000)
        let sales = vec![AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false)];

        // Evaluation on 2025-10-01 (post Q2). Suppose user paid only ₹2,000 total advance tax:
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-10-01"), Some(2_000.0));

        // Q1 shortfall: 3,000 - 2,000 = 1,000.
        // Safe harbour 12% = 2,400. Paid (2,000) < 2,400 -> interest 3% on 1,000 = 30.0
        let q1 = &s.quarters[0];
        assert_eq!(q1.status, AdvanceTaxStatus::Pending);
        assert!(close(q1.shortfall, 1_000.0));
        assert!(close(q1.sec_234c_interest, 30.0));

        // Q2 shortfall: 9,000 - 2,000 = 7,000.
        // Safe harbour 36% = 7,200. Paid (2,000) < 7,200 -> interest 3% on 7,000 = 210.0
        let q2 = &s.quarters[1];
        assert_eq!(q2.status, AdvanceTaxStatus::Pending);
        assert!(close(q2.shortfall, 7_000.0));
        assert!(close(q2.sec_234c_interest, 210.0));

        assert!(close(s.total_sec_234c_interest, 240.0));

        // If user paid ₹10,000 total:
        let s_paid = compute_advance_tax(&sales, d("2025-04-01"), d("2025-10-01"), Some(10_000.0));
        assert_eq!(s_paid.quarters[0].status, AdvanceTaxStatus::Paid);
        assert_eq!(s_paid.quarters[1].status, AdvanceTaxStatus::Paid);
        assert!(close(s_paid.total_sec_234c_interest, 0.0));
    }

    #[test]
    fn advance_tax_serialization_roundtrip() {
        let sales = vec![AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false)];
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-10-01"), Some(5_000.0));
        let json_str = serde_json::to_string(&s).unwrap();
        let deserialized: AdvanceTaxSchedule = serde_json::from_str(&json_str).unwrap();
        assert_eq!(s, deserialized);
    }
}
