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
    /// India's 4% health and education cess on `st_tax + lt_tax`; zero elsewhere.
    pub cess: f64,
    /// India: the (short-term, long-term) taxable amounts that came from sales
    /// before 23 July 2024, taxed at 15% and 10%. Both are included in
    /// `st_taxable` and `lt_taxable`.
    pub taxable_before: (f64, f64),
    /// Short-term loss set off against long-term gains.
    pub st_loss_against_lt: f64,
    /// Long-term loss set off against short-term gains (US only).
    pub lt_loss_against_st: f64,
    /// Losses left over to carry forward.
    pub st_carry: f64,
    pub lt_carry: f64,
}

impl Estimate {
    /// Tax including cess.
    pub fn total(&self) -> f64 {
        self.st_tax + self.lt_tax + self.cess
    }
}

pub fn estimate(country: Country, b: Buckets) -> Estimate {
    match country {
        Country::India => india(b),
        Country::Us => us(b),
        Country::Uk => uk(b),
    }
}

/// Health and education cess on income tax in India (Finance Act 2018, Sec.
/// 2(11) of the Finance Act). It applies to every taxpayer; surcharge, which
/// depends on total income, is left out.
pub const INDIA_CESS_RATE: f64 = 0.04;

/// India's capital-gains rates changed for sales from 23 July 2024 (Finance
/// (No. 2) Act 2024): STCG under Sec. 111A 15% -> 20%, LTCG under Sec. 112A
/// 10% -> 12.5%.
pub fn india_rate_change_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2024, 7, 23).expect("valid date")
}

/// The Sec. 112A exemption for a financial year starting in `fy_start_year`:
/// ₹1 lakh up to FY 2023-24, ₹1.25 lakh from FY 2024-25 (for the whole year,
/// also for sales before 23 July 2024).
pub fn india_exemption_for_year(fy_start_year: i32) -> f64 {
    if fy_start_year >= 2024 {
        125_000.0
    } else {
        100_000.0
    }
}

/// India (listed shares with STT, from 23 July 2024): gains and losses of
/// the same kind net first. A net short-term loss is set off against
/// short-term gains and then long-term gains; a long-term loss only against
/// long-term gains — never short-term. Then ₹1.25 lakh of long-term gain is
/// exempt; STCG is taxed at 20%, the rest of LTCG at 12.5%. Unused losses
/// carry forward (8 years).
fn india(b: Buckets) -> Estimate {
    estimate_india_by_period(Buckets::default(), b, Country::India.exemption())
}

/// India with the rate change of 23 July 2024: `before` holds the gains and
/// losses of sales before that day (STCG 15%, LTCG 10%), `after` those from
/// it (20%, 12.5%). Set-off follows the rules of [`india`] on the pooled
/// amounts. Losses and the `exemption` are used against the higher-rate gains
/// first, which gives the lowest tax; the Act does not prescribe an order.
/// `Estimate::st_taxable` and the like are the totals of both periods;
/// [`Estimate::taxable_before`] gives the part taxed at the old rates.
pub fn estimate_india_by_period(before: Buckets, after: Buckets, exemption: f64) -> Estimate {
    // Net a loss against gains, `first` before `second`; returns the gains
    // left and the loss not used.
    fn net(loss: f64, first: f64, second: f64) -> (f64, f64, f64) {
        let first_left = (first - loss).max(0.0);
        let loss = (loss - first).max(0.0);
        let second_left = (second - loss).max(0.0);
        (first_left, second_left, (loss - second).max(0.0))
    }
    let (st_a, st_b, st_loss_left) = net(
        before.st_loss + after.st_loss,
        after.st_gain,
        before.st_gain,
    );
    let (lt_a, lt_b, lt_loss_left) = net(
        before.lt_loss + after.lt_loss,
        after.lt_gain,
        before.lt_gain,
    );
    let mut e = Estimate {
        lt_carry: lt_loss_left,
        ..Estimate::default()
    };
    // A short-term loss left over goes against long-term gains.
    let (lt_a, lt_b, st_carry) = net(st_loss_left, lt_a, lt_b);
    e.st_loss_against_lt = st_loss_left - st_carry;
    e.st_carry = st_carry;
    // The exemption, also against the higher-rate gains first.
    let exempt_a = lt_a.min(exemption);
    let exempt_b = lt_b.min(exemption - exempt_a);
    e.exemption_used = exempt_a + exempt_b;
    let (taxable_a, taxable_b) = (lt_a - exempt_a, lt_b - exempt_b);
    e.st_taxable = st_a + st_b;
    e.lt_net = lt_a + lt_b;
    e.lt_taxable = taxable_a + taxable_b;
    e.taxable_before = (st_b, taxable_b);
    let c = Country::India;
    e.st_tax = st_a * c.st_rate() / 100.0 + st_b * 15.0 / 100.0;
    e.lt_tax = taxable_a * c.lt_rate() / 100.0 + taxable_b * 10.0 / 100.0;
    e.cess = (e.st_tax + e.lt_tax) * INDIA_CESS_RATE;
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
    /// Payments dated on or before the due date cover the minimum.
    Paid,
    /// The due date is still ahead (or, for Q4, the year is still open).
    Pending,
    /// The quarter has not started.
    Upcoming,
    /// The due date has passed and the recorded payments fall short.
    Overdue,
    /// The minimum for this due date is zero.
    NothingDue,
}

impl std::fmt::Display for AdvanceTaxStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Paid => write!(f, "Paid"),
            Self::Pending => write!(f, "Pending"),
            Self::Upcoming => write!(f, "Upcoming"),
            Self::Overdue => write!(f, "Overdue"),
            Self::NothingDue => write!(f, "Nothing due"),
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

/// India's tax on `sales` of the financial year starting in `fy_start_year`:
/// sales before 23 July 2024 at the old rates, the rest at the new ones, and
/// the exemption of that year, as `tax export in` does by sale date.
fn india_estimate_of<'a>(
    sales: impl Iterator<Item = &'a AdvanceTaxSale>,
    fy_start_year: i32,
) -> Estimate {
    let (mut before, mut after) = (Buckets::default(), Buckets::default());
    for s in sales {
        if s.date < india_rate_change_date() {
            before.add(s.gain, s.is_long_term);
        } else {
            after.add(s.gain, s.is_long_term);
        }
    }
    estimate_india_by_period(before, after, india_exemption_for_year(fy_start_year))
}

/// An advance-tax payment. Section 234C looks at what was paid by each due
/// date, so the date matters; a payment without one is assumed to have been
/// made before every due date.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AdvancePayment {
    pub date: Option<NaiveDate>,
    pub amount: f64,
}

impl AdvancePayment {
    /// `3000`, `3,000` or `2025-06-14:3000`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (date, amount) = match text.split_once(':') {
            Some((d, a)) => {
                let date = NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d")
                    .map_err(|_| format!("'{d}' is not a date like 2025-06-14"))?;
                (Some(date), a)
            }
            None => (None, text),
        };
        let amount: f64 = amount
            .trim()
            .trim_start_matches('₹')
            .replace(',', "")
            .parse()
            .map_err(|_| format!("'{amount}' is not an amount"))?;
        if !amount.is_finite() || amount < 0.0 {
            return Err("an advance tax payment cannot be negative".into());
        }
        Ok(Self { date, amount })
    }

    /// Whether this payment is for the financial year starting `fy_start`:
    /// one dated before it was made for an earlier year.
    fn in_year(&self, fy_start: NaiveDate) -> bool {
        self.date.is_none_or(|d| d >= fy_start)
    }

    /// Whether this payment counts as made by `due`, in the year starting
    /// `fy_start`.
    fn counts_by(&self, fy_start: NaiveDate, due: NaiveDate) -> bool {
        self.in_year(fy_start) && self.date.is_none_or(|d| d <= due)
    }
}

/// Half a paisa: payments are compared with estimates computed in floating
/// point, so a payment equal to the tax must count as covering it.
const PAISA: f64 = 0.005;

/// Interest under Sec. 234C is charged on the shortfall rounded down to a
/// multiple of ₹100 (Rule 119A of the Income-tax Rules).
fn round_down_100(amount: f64) -> f64 {
    (amount / 100.0).floor() * 100.0
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
    /// Advance tax paid on or before this quarter's due date (cumulative).
    pub paid: f64,
    /// Shortfall below the required minimum cumulative tax that bears
    /// interest. For Q4 this leaves out tax on gains after 15 March that is
    /// paid by 31 March (Sec. 234C(1) proviso).
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
    /// Total advance tax paid, from all recorded payments.
    pub total_paid: f64,
    /// The payments the schedule was computed from.
    pub payments: Vec<AdvancePayment>,
    /// True when a payment had no date and was assumed to be on time.
    pub assumed_on_time: bool,
    /// True when a payment dated before the financial year began was left
    /// out (it was made for an earlier year); `total_paid` excludes it.
    pub ignored_before_year: bool,
    /// Total Section 234C deferment interest.
    pub total_sec_234c_interest: f64,
    /// True if evaluated after 31 March and paid < 90% of total tax.
    pub sec_234b_applicable: bool,
    /// Dividend and interest income included at the slab rate (zero when
    /// none was passed in).
    #[serde(default)]
    pub other_income: f64,
    /// The tax with cess on `other_income`, part of `total_tax_estimated`.
    #[serde(default)]
    pub other_income_tax: f64,
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
/// only on cumulative gains realized up to that quarter's cut-off date. For
/// Q4 (due 15 March) the requirement on 15 March is the tax on gains up to
/// 15 March, and payments up to 31 March count against gains after it.
pub fn compute_advance_tax(
    sales: &[AdvanceTaxSale],
    fy_start: NaiveDate,
    as_of: NaiveDate,
    payments: &[AdvancePayment],
) -> AdvanceTaxSchedule {
    compute_advance_tax_with_income(sales, &[], fy_start, as_of, payments)
}

/// Income other than capital gains that advance tax is due on, taxed at the
/// investor's slab `rate` (a fraction, before the 4% cess): dividends and
/// interest. Dividends share the capital-gains proviso to Sec. 234C(1): no
/// interest for a shortfall caused by a dividend received after a due date,
/// so each one counts from the quarter it was received.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AdvanceIncome {
    pub date: NaiveDate,
    pub amount: f64,
    pub rate: f64,
}

/// Tax with cess on the `income` received from `from` to `to`.
fn income_tax_between(income: &[AdvanceIncome], from: NaiveDate, to: NaiveDate) -> f64 {
    income
        .iter()
        .filter(|i| i.date >= from && i.date <= to)
        .map(|i| i.amount * i.rate * (1.0 + INDIA_CESS_RATE))
        .sum()
}

/// [`compute_advance_tax`] with dividend and interest income added at the
/// slab rate, in the quarter it was received.
pub fn compute_advance_tax_with_income(
    sales: &[AdvanceTaxSale],
    income: &[AdvanceIncome],
    fy_start: NaiveDate,
    as_of: NaiveDate,
    payments: &[AdvancePayment],
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

    // A payment dated before the year began was made for an earlier year.
    let ignored_before_year = payments.iter().any(|p| !p.in_year(fy_start));
    let total_paid: f64 = payments
        .iter()
        .filter(|p| p.in_year(fy_start))
        .map(|p| p.amount)
        .sum();

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
        let cumulative_tax_estimated = india_estimate_of(
            sales
                .iter()
                .filter(|s| s.date >= fy_start && s.date <= m.cutoff_date),
            y,
        )
        .total()
            + income_tax_between(income, fy_start, m.cutoff_date);

        // What is due on the due date itself: the tax on gains up to that
        // day. It is the same as the cut-off figure except for Q4, whose
        // cut-off (31 Mar) is after its due date (15 Mar).
        let tax_at_due = india_estimate_of(
            sales
                .iter()
                .filter(|s| s.date >= fy_start && s.date <= m.due_date),
            y,
        )
        .total()
            + income_tax_between(income, fy_start, m.due_date);
        let required_at_due = tax_at_due * (m.req_pct / 100.0);

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
            tax_at_due,
            required_at_due,
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
            tax_at_due,
            required_at_due,
        ) = quarters_data[i];

        let paid_by_due: f64 = payments
            .iter()
            .filter(|p| p.counts_by(fy_start, m.due_date))
            .map(|p| p.amount)
            .sum();
        let paid_by_cutoff: f64 = payments
            .iter()
            .filter(|p| p.counts_by(fy_start, m.cutoff_date))
            .map(|p| p.amount)
            .sum();
        // Sec. 234C(1) proviso (second): for the last instalment, a shortfall
        // caused by capital gains arising after 15 March carries no interest
        // if the tax on them is paid by 31 March. So the 15 March requirement
        // is the tax on gains up to 15 March, and payments up to 31 March
        // count against the later gains. When those are not paid in full by
        // 31 March, the whole shortfall at 15 March bears interest.
        let late_gains_unpaid = m.cutoff_date > m.due_date
            && as_of > m.cutoff_date
            && paid_by_cutoff + PAISA < min_cumulative_tax_required;
        let (basis_required, basis_tax) = if late_gains_unpaid {
            (min_cumulative_tax_required, cumulative_tax_estimated)
        } else {
            (required_at_due, tax_at_due)
        };
        let shortfall = (basis_required - paid_by_due).max(0.0);
        // Interest runs from the due date, so it is known only once that day has passed.
        let sec_234c_interest = if sec_208_applicable
            && as_of > m.due_date
            && paid_by_due < (m.safe_harbour_pct / 100.0) * basis_tax
        {
            round_down_100(shortfall) * (m.interest_rate_pct / 100.0)
        } else {
            0.0
        };
        let covered = paid_by_due + PAISA >= required_at_due
            && paid_by_cutoff + PAISA >= min_cumulative_tax_required;
        let status = if as_of < m.period_start {
            if covered && min_cumulative_tax_required > 0.0 {
                AdvanceTaxStatus::Paid
            } else {
                AdvanceTaxStatus::Upcoming
            }
        } else if min_cumulative_tax_required < 0.005 {
            AdvanceTaxStatus::NothingDue
        } else if covered {
            AdvanceTaxStatus::Paid
        } else if as_of > m.due_date && !(m.quarter == "Q4" && as_of <= fy_end) {
            AdvanceTaxStatus::Overdue
        } else {
            AdvanceTaxStatus::Pending
        };
        let (quarter_paid, shortfall, sec_234c_interest, status) =
            (paid_by_due, shortfall, sec_234c_interest, status);

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
    let paid_by_year_end: f64 = payments
        .iter()
        .filter(|p| p.counts_by(fy_start, fy_end))
        .map(|p| p.amount)
        .sum();
    let sec_234b_applicable = sec_208_applicable
        && as_of > fy_end
        && !payments.is_empty()
        && paid_by_year_end < 0.90 * total_tax_estimated;

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
        payments: payments.to_vec(),
        assumed_on_time: payments.iter().any(|p| p.date.is_none()),
        ignored_before_year,
        total_sec_234c_interest,
        sec_234b_applicable,
        other_income: income
            .iter()
            .filter(|i| i.date >= fy_start && i.date <= fy_end)
            .map(|i| i.amount)
            .sum(),
        other_income_tax: income_tax_between(income, fy_start, fy_end),
        quarters,
    }
}

// ── Next instalment and reminders ──

/// The next advance-tax instalment still to pay: what is due by its date
/// and the Sec. 234C interest if it is missed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NextInstalment {
    pub quarter: String,
    pub due_date: NaiveDate,
    /// Days from `as_of` to the due date (0 on the day itself).
    pub days_left: i64,
    /// Cumulative minimum required by the due date (15/45/75/100% of the
    /// tax on gains and income so far).
    pub required_by_due: f64,
    /// Paid so far that counts toward it.
    pub paid: f64,
    /// Still to pay by the due date.
    pub amount_due: f64,
    /// Sec. 234C interest if nothing more is paid by the due date: 1% a month
    /// for three months (Q1–Q3) or one month (Q4) on the shortfall rounded
    /// down to ₹100. Zero when Sec. 208 does not apply (tax under ₹10,000).
    pub interest_if_missed: f64,
}

/// The first instalment due on or after `as_of` in `s`, with what is left
/// to pay on it. `None` once the year's last due date has passed.
pub fn next_instalment(s: &AdvanceTaxSchedule, as_of: NaiveDate) -> Option<NextInstalment> {
    let q = s.quarters.iter().find(|q| q.due_date >= as_of)?;
    let (rate, safe_harbour) = match q.quarter.as_str() {
        "Q1" => (0.03, 0.12),
        "Q2" => (0.03, 0.36),
        "Q3" => (0.03, 0.75),
        _ => (0.01, 1.0),
    };
    let required = q.min_cumulative_tax_required;
    let paid = s
        .payments
        .iter()
        .filter(|p| p.counts_by(s.fy_start, q.due_date))
        .map(|p| p.amount)
        .sum::<f64>();
    let amount_due = (required - paid).max(0.0);
    let pct = q.required_cumulative_pct / 100.0;
    let tax = if pct > 0.0 { required / pct } else { 0.0 };
    let interest_if_missed =
        if s.sec_208_applicable && amount_due > PAISA && paid + PAISA < safe_harbour * tax {
            round_down_100(amount_due) * rate
        } else {
            0.0
        };
    Some(NextInstalment {
        quarter: q.quarter.clone(),
        due_date: q.due_date,
        days_left: (q.due_date - as_of).num_days(),
        required_by_due: required,
        paid,
        amount_due,
        interest_if_missed,
    })
}

/// How many days before a due date the advance-tax reminder starts.
pub const ADVANCE_REMINDER_DAYS: i64 = 7;

/// An advance-tax reminder for a notifier: set when an instalment with
/// something left to pay falls due within `days_ahead` days of `as_of`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdvanceReminder {
    pub instalment: NextInstalment,
    /// One line for a notification.
    pub message: String,
}

/// The reminder due on `as_of` for schedule `s`, if any: the next instalment
/// is at most `days_ahead` days away and something is still due on it.
pub fn advance_tax_reminder(
    s: &AdvanceTaxSchedule,
    as_of: NaiveDate,
    days_ahead: i64,
) -> Option<AdvanceReminder> {
    let next = next_instalment(s, as_of)?;
    if next.days_left > days_ahead || next.amount_due < 1.0 || !s.sec_208_applicable {
        return None;
    }
    let inr = |x: f64| format!("₹{}", crate::goals::indian_grouping(x.round()));
    let when = match next.days_left {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        n => format!("in {n} days"),
    };
    let mut message = format!(
        "Advance tax {} ({}) is due {} on {}: about {} to pay",
        next.quarter,
        s.fy_label,
        when,
        next.due_date.format("%d %b %Y"),
        inr(next.amount_due),
    );
    if next.interest_if_missed > 0.0 {
        message.push_str(&format!(
            "; missing it adds about {} of Sec. 234C interest",
            inr(next.interest_if_missed)
        ));
    }
    message.push_str(". Estimate — verify with your CA.");
    Some(AdvanceReminder {
        instalment: next,
        message,
    })
}

// ── Indian capital gains outside Sec. 111A/112A (Sec. 50AA, 112, 115BBH) ──

/// Assets whose gains Indian tax does not treat like listed shares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndiaAsset {
    /// A debt-oriented fund or ETF (debt, liquid, gilt, money market): a
    /// "specified mutual fund" under Sec. 50AA under both definitions.
    DebtFund { listed: bool },
    /// A fund that is neither equity- nor debt-oriented: gold, silver,
    /// international, fund of funds, conservative hybrid.
    OtherFund { listed: bool },
    /// A market-linked debenture (Sec. 50AA, whenever bought).
    MarketLinkedDebenture,
    /// A Sovereign Gold Bond sold on the exchange.
    GoldBond,
    /// Crypto and other virtual digital assets (Sec. 115BBH).
    Vda,
}

/// The rule a gain falls under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndiaRule {
    /// Sec. 50AA: always short-term, taxed at the slab rate.
    Sec50AA,
    /// Short-term (held too briefly), taxed at the slab rate.
    ShortTermSlab,
    /// Long-term under Sec. 112: 12.5% without indexation (sales from
    /// 23 Jul 2024; 20% with indexation before), no ₹1.25 lakh exemption.
    LongTerm112,
    /// Sec. 115BBH: 30% flat on each gain; losses are never set off.
    Vda115BBH,
}

impl IndiaRule {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sec50AA => "Sec. 50AA — short-term at slab rate",
            Self::ShortTermSlab => "short-term — slab rate",
            Self::LongTerm112 => "Sec. 112 — long-term 12.5%, no exemption",
            Self::Vda115BBH => "Sec. 115BBH — 30% flat, no set-off",
        }
    }
}

/// Units bought from this day are in a specified mutual fund (Sec. 50AA,
/// Finance Act 2023).
pub fn sec_50aa_start() -> NaiveDate {
    NaiveDate::from_ymd_opt(2023, 4, 1).expect("valid date")
}

/// From this day a specified mutual fund is one with more than 65% in debt
/// and money-market instruments (Finance (No. 2) Act 2024). Before it, any
/// fund with 35% or less in domestic equity was one, so gold, silver and
/// international funds bought from 1 Apr 2023 and sold before it were too.
pub fn sec_50aa_redefined() -> NaiveDate {
    NaiveDate::from_ymd_opt(2025, 4, 1).expect("valid date")
}

/// Sec. 115BBH rate on virtual digital assets.
pub const VDA_RATE: f64 = 0.30;
/// Sec. 194S TDS on the sale consideration of a virtual digital asset.
pub const VDA_TDS_RATE: f64 = 0.01;

/// Which rule a gain on `asset` bought on `bought` and sold on `sold`
/// falls under.
pub fn india_rule(asset: IndiaAsset, bought: NaiveDate, sold: NaiveDate) -> IndiaRule {
    let new_rates = sold >= india_rate_change_date();
    // Listed units: 12 months from 23 Jul 2024; unlisted units 24 months
    // (36 before that day, for everything but listed shares and units).
    let lt_months = |listed: bool| match (listed, new_rates) {
        (true, _) => 12,
        (false, true) => 24,
        (false, false) => 36,
    };
    let term = |listed: bool| {
        if is_long_term(bought, sold, lt_months(listed)) {
            IndiaRule::LongTerm112
        } else {
            IndiaRule::ShortTermSlab
        }
    };
    match asset {
        IndiaAsset::Vda => IndiaRule::Vda115BBH,
        IndiaAsset::MarketLinkedDebenture => IndiaRule::Sec50AA,
        IndiaAsset::DebtFund { .. } if bought >= sec_50aa_start() => IndiaRule::Sec50AA,
        IndiaAsset::OtherFund { .. }
            if bought >= sec_50aa_start() && sold < sec_50aa_redefined() =>
        {
            IndiaRule::Sec50AA
        }
        IndiaAsset::DebtFund { listed } | IndiaAsset::OtherFund { listed } => term(listed),
        IndiaAsset::GoldBond => term(true),
    }
}

/// One gain outside Sec. 111A/112A.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OtherGain {
    pub rule: IndiaRule,
    pub gain: f64,
    /// What the asset was sold for (Sec. 194S TDS on crypto is 1% of it).
    pub proceeds: f64,
    /// Sold before 23 Jul 2024 (Sec. 112 at 20% with indexation).
    pub before_rate_change: bool,
}

/// Gains outside Sec. 111A/112A, totalled by rule. Slab-rate income is not
/// taxed here (the slab is unknown); Sec. 112 and 115BBH are.
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OtherGains {
    /// Net Sec. 50AA gain (may be negative: a short-term loss).
    pub sec_50aa: f64,
    /// Net short-term gain at the slab rate on other assets.
    pub short_term_slab: f64,
    /// Net long-term gain under Sec. 112, after its own losses.
    pub long_term_112: f64,
    /// Tax at 12.5% (20% for sales before 23 Jul 2024) on a positive
    /// `long_term_112`, before cess. Indexation is not applied.
    pub long_term_112_tax: f64,
    /// Sum of crypto gains (losses are not netted).
    pub vda_gains: f64,
    /// Crypto losses: ignored, never set off or carried forward.
    pub vda_losses_ignored: f64,
    /// 30% of `vda_gains`, before cess.
    pub vda_tax: f64,
    /// 1% TDS (Sec. 194S) on crypto sale proceeds, credited against the tax.
    pub vda_tds: f64,
    /// Cess on the Sec. 112 and 115BBH tax.
    pub cess: f64,
}

impl OtherGains {
    /// Sec. 112 and 115BBH tax with cess (the slab-rate parts are left out).
    pub fn flat_tax(&self) -> f64 {
        self.long_term_112_tax + self.vda_tax + self.cess
    }

    /// Income taxed at the slab rate, net (Sec. 50AA plus other short-term).
    pub fn slab_income(&self) -> f64 {
        self.sec_50aa + self.short_term_slab
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Total `gains` by rule. Each crypto gain is taxed on its own and no crypto
/// loss is set off against anything, even another crypto gain (Sec.
/// 115BBH(2)). Sec. 112 losses are netted with Sec. 112 gains only.
pub fn india_other_gains(gains: &[OtherGain]) -> OtherGains {
    let mut o = OtherGains::default();
    // Sec. 112 gains before and from 23 Jul 2024, and all its losses.
    let (mut lt_new, mut lt_old, mut lt_loss) = (0.0, 0.0, 0.0);
    for g in gains {
        match g.rule {
            IndiaRule::Sec50AA => o.sec_50aa += g.gain,
            IndiaRule::ShortTermSlab => o.short_term_slab += g.gain,
            IndiaRule::LongTerm112 if g.gain < 0.0 => lt_loss -= g.gain,
            IndiaRule::LongTerm112 if g.before_rate_change => lt_old += g.gain,
            IndiaRule::LongTerm112 => lt_new += g.gain,
            IndiaRule::Vda115BBH => {
                if g.gain >= 0.0 {
                    o.vda_gains += g.gain;
                } else {
                    o.vda_losses_ignored -= g.gain;
                }
                o.vda_tds += g.proceeds.max(0.0) * VDA_TDS_RATE;
            }
        }
    }
    o.long_term_112 = lt_new + lt_old - lt_loss;
    if o.long_term_112 > 0.0 {
        // Losses go against the higher-rate (older, 20%) gains first.
        let from_old = lt_loss.min(lt_old);
        let old_g = lt_old - from_old;
        let new_g = lt_new - (lt_loss - from_old).min(lt_new);
        o.long_term_112_tax = old_g * 0.20 + new_g * 0.125;
    }
    o.vda_tax = o.vda_gains * VDA_RATE;
    o.cess = (o.long_term_112_tax + o.vda_tax) * INDIA_CESS_RATE;
    o
}

// ── US wash sales (IRS Publication 550) ──

/// Days either side of a sale in which buying substantially identical shares
/// makes a loss on it a wash sale.
pub const WASH_SALE_DAYS: i64 = 30;

/// Shares bought in a wash sale's window that take on its disallowed loss.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WashReplacement {
    /// Purchase date, `YYYY-MM-DD`.
    pub date: String,
    /// True when `date` is an estimate (see `Lot::date_estimated`), so the
    /// purchase may not really fall inside the window.
    pub date_estimated: bool,
    /// Shares of this purchase used as replacement shares.
    pub qty: f64,
    /// Disallowed loss added to the basis of these shares.
    pub basis_added: f64,
    /// Days the washed shares were held, added to the holding period of
    /// these shares.
    pub days_added: i64,
    /// When these shares were sold since: the index of that sale in the
    /// `sales` passed to [`wash_sales`]. `None` for shares still held.
    pub sale: Option<usize>,
}

/// A loss on shares of a sale with replacement shares bought within 30 days
/// before or after it. The disallowed part of the loss is added to the basis
/// of the replacement shares.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Wash {
    /// Index of the loss sale in the `sales` passed to [`wash_sales`].
    pub sale: usize,
    /// Index of the part of that sale (see [`WashSales::parts`]).
    pub part: usize,
    pub symbol: String,
    pub sell_date: String,
    /// Shares of the part sold at a loss.
    pub qty: f64,
    /// The loss on those shares (positive), including basis added to them
    /// by an earlier wash sale.
    pub loss: f64,
    /// Replacement shares found, at most `qty`.
    pub replacement_qty: f64,
    /// The part of the loss that is disallowed (positive): the loss times
    /// replacement shares over shares sold.
    pub disallowed: f64,
    pub replacements: Vec<WashReplacement>,
}

/// Shares of one sale that share a basis and a holding period. A sale is
/// split into parts when only some of its shares replaced shares sold in an
/// earlier wash sale.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SalePart {
    pub qty: f64,
    /// Loss from an earlier wash sale added to the basis of these shares.
    pub basis_added: f64,
    /// Days added to the holding period of these shares by that wash sale.
    pub days_added: i64,
    /// Loss on these shares disallowed by a wash sale (positive).
    pub disallowed: f64,
}

/// Wash sales found among a list of sales.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct WashSales {
    pub washes: Vec<Wash>,
    /// Per sale, in the order passed to [`wash_sales`]: its parts. A sale
    /// with no wash sale has one part holding all its shares, and a non-USD
    /// sale has none.
    pub parts: Vec<Vec<SalePart>>,
}

/// One purchase that can be a replacement: an open lot, or the buy side of
/// a sale.
struct Buy {
    date: NaiveDate,
    date_estimated: bool,
    price: f64,
    account: Option<String>,
    left: f64,
    /// The sale these shares went out in, with its date.
    sold: Option<(usize, NaiveDate)>,
}

/// A part of a sale while matching; amounts in cents so that the amounts
/// split between replacements add up to the disallowed loss exactly.
#[derive(Clone)]
struct Part {
    qty: f64,
    basis_cents: i64,
    days_added: i64,
    disallowed_cents: i64,
    /// Shares that replaced earlier washed shares. They cannot be matched
    /// as replacements again.
    replaced: bool,
}

fn ymd_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

fn cents(x: f64) -> i64 {
    (x * 100.0).round() as i64
}

fn dollars(c: i64) -> f64 {
    c as f64 / 100.0
}

/// Wash sales among `sales` (IRS Publication 550): a sale at a loss is a wash
/// sale when shares of the same symbol were bought within 30 days before or
/// after it. Purchases counted are the open lots in `holdings` and the buy
/// side of the other sales; shares of the same purchase as the sold lot
/// (same date, account and price), and shares sold on the same day as the
/// loss, are not replacements. The disallowed loss is the loss times
/// replacement shares over shares sold. Each purchase is used as a
/// replacement once, earliest purchase first, and losses are taken in
/// sale-date order. Replacement shares take on the disallowed loss and the
/// holding period of the washed shares, share by share: a later sale of them
/// is split into parts, each matched on its own. Only USD (US-taxed) stocks
/// are checked: crypto such as `BTC-USD` is not stock or securities.
pub fn wash_sales(
    sales: &[crate::portfolio::Sale],
    holdings: &[crate::portfolio::Holding],
) -> WashSales {
    use crate::portfolio::symbol_currency;
    let is_usd = |symbol: &str| {
        symbol_currency(symbol) == Some("USD")
            && crate::market::AssetClass::of(symbol) == crate::market::AssetClass::Stock
    };

    // Every purchase of each USD symbol, earliest first.
    let mut buys: std::collections::BTreeMap<&str, Vec<Buy>> = Default::default();
    // Bonus shares are allotted, not bought, so they are never replacements.
    for h in holdings.iter().filter(|h| is_usd(&h.symbol)) {
        for l in h
            .lots
            .iter()
            .filter(|l| l.source != crate::portfolio::BONUS_SOURCE)
        {
            if let Some(date) = ymd_date(&l.date) {
                buys.entry(&h.symbol).or_default().push(Buy {
                    date,
                    date_estimated: l.date_estimated,
                    price: l.price,
                    account: l.account_key().map(str::to_string),
                    left: l.qty,
                    sold: None,
                });
            }
        }
    }
    for (i, s) in sales
        .iter()
        .enumerate()
        .filter(|(_, s)| is_usd(&s.symbol) && !s.bonus_shares)
    {
        if let (Some(date), Some(sold)) = (ymd_date(&s.buy_date), ymd_date(&s.sell_date)) {
            buys.entry(&s.symbol).or_default().push(Buy {
                date,
                date_estimated: s.buy_date_estimated,
                price: s.buy_price,
                account: s.account_key().map(str::to_string),
                left: s.qty,
                sold: Some((i, sold)),
            });
        }
    }
    for list in buys.values_mut() {
        list.sort_by_key(|b| b.date);
    }

    let mut parts: Vec<Vec<Part>> = sales
        .iter()
        .map(|s| {
            if is_usd(&s.symbol) {
                vec![Part {
                    qty: s.qty,
                    basis_cents: 0,
                    days_added: 0,
                    disallowed_cents: 0,
                    replaced: false,
                }]
            } else {
                Vec::new()
            }
        })
        .collect();
    let mut order: Vec<usize> = (0..sales.len())
        .filter(|&i| is_usd(&sales[i].symbol))
        .collect();
    order.sort_by(|&a, &b| {
        (sales[a].sell_date.as_str(), sales[a].buy_date.as_str())
            .cmp(&(sales[b].sell_date.as_str(), sales[b].buy_date.as_str()))
    });
    let mut washes = Vec::new();
    for i in order {
        let s = &sales[i];
        let (Some(bought), Some(sold)) = (ymd_date(&s.buy_date), ymd_date(&s.sell_date)) else {
            continue;
        };
        let Some(list) = buys.get_mut(s.symbol.as_str()) else {
            continue;
        };
        let same_purchase = |b: &Buy| {
            b.date == bought
                && b.account.as_deref() == s.account_key()
                && (b.price - s.buy_price).abs() <= 1e-9 * s.buy_price.abs().max(1.0)
        };
        // Parts only split on other sales (a sale's own buy side is the
        // same purchase), so this sale's parts stay put while it is matched.
        for p in 0..parts[i].len() {
            let part = parts[i][p].clone();
            let loss_cents = cents(part.qty * (s.buy_price - s.sell_price)) + part.basis_cents;
            if loss_cents <= 0 || part.qty <= 0.0 {
                continue;
            }
            let days_added = (sold - bought).num_days() + part.days_added;
            let mut need = part.qty;
            let mut matched: Vec<(usize, f64)> = Vec::new();
            for (k, b) in list.iter_mut().enumerate() {
                if need <= 1e-9 {
                    break;
                }
                if b.left <= 1e-9
                    || (b.date - sold).num_days().abs() > WASH_SALE_DAYS
                    || same_purchase(b)
                    // Shares sold on or before the loss date are not held
                    // to take on the disallowed loss.
                    || b.sold.is_some_and(|(_, d)| d <= sold)
                {
                    continue;
                }
                let qty = b.left.min(need);
                b.left -= qty;
                need -= qty;
                matched.push((k, qty));
            }
            if matched.is_empty() {
                continue;
            }
            let replacement_qty = part.qty - need.max(0.0);
            let disallowed_cents = (loss_cents as f64 * replacement_qty / part.qty).round() as i64;
            // Split the disallowed loss between the replacements by their
            // running share count: each gets a non-negative amount and the
            // amounts add up to the whole.
            let mut replacements = Vec::new();
            let mut done_qty = 0.0;
            let mut done_cents = 0;
            for (n, (k, qty)) in matched.iter().enumerate() {
                let b = &list[*k];
                done_qty += qty;
                let upto = if n + 1 == matched.len() {
                    disallowed_cents
                } else {
                    (disallowed_cents as f64 * done_qty / replacement_qty).round() as i64
                };
                let basis_cents = upto - done_cents;
                done_cents = upto;
                if let Some((j, _)) = b.sold {
                    split_off_replaced(&mut parts[j], *qty, basis_cents, days_added);
                }
                replacements.push(WashReplacement {
                    date: b.date.to_string(),
                    date_estimated: b.date_estimated,
                    qty: *qty,
                    basis_added: dollars(basis_cents),
                    days_added,
                    sale: b.sold.map(|(j, _)| j),
                });
            }
            parts[i][p].disallowed_cents += disallowed_cents;
            washes.push(Wash {
                sale: i,
                part: p,
                symbol: s.symbol.clone(),
                sell_date: s.sell_date.clone(),
                qty: part.qty,
                loss: dollars(loss_cents),
                replacement_qty,
                disallowed: dollars(disallowed_cents),
                replacements,
            });
        }
    }
    washes.sort_by_key(|w| (w.sale, w.part));
    WashSales {
        washes,
        parts: parts
            .into_iter()
            .map(|ps| {
                ps.into_iter()
                    .map(|p| SalePart {
                        qty: p.qty,
                        basis_added: dollars(p.basis_cents),
                        days_added: p.days_added,
                        disallowed: dollars(p.disallowed_cents),
                    })
                    .collect()
            })
            .collect(),
    }
}

/// Move `qty` shares of a sale that were not yet replacement shares into a
/// part of their own, with the basis and holding period they take on.
fn split_off_replaced(parts: &mut Vec<Part>, mut qty: f64, basis_cents: i64, days_added: i64) {
    let mut taken = 0.0;
    for p in parts.iter_mut().filter(|p| !p.replaced) {
        if qty <= 1e-9 {
            break;
        }
        let q = p.qty.min(qty);
        p.qty -= q;
        qty -= q;
        taken += q;
    }
    parts.retain(|p| p.qty > 1e-9);
    if taken > 1e-9 {
        parts.push(Part {
            qty: taken,
            basis_cents,
            days_added,
            disallowed_cents: 0,
            replaced: true,
        });
    }
}

/// The last day a purchase of the same shares makes a loss on a sale on
/// `sold` a wash sale.
pub fn wash_sale_window_end(sold: NaiveDate) -> NaiveDate {
    sold + chrono::Duration::days(WASH_SALE_DAYS)
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
        // Cess 4% of 29,375 = 1,175.
        assert!(close(e.cess, 1_175.0));
        assert!(close(e.total(), 30_550.0));
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
        assert!(close(e.total(), 20_800.0)); // with 4% cess
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
        // Sales across 4 quarters in FY 2025-26, tax including 4% cess:
        // Q1 (before 15 Jun): STCG ₹1,00,000 -> 20,000 + 800 cess = ₹20,800
        // Q2 (16 Jun - 15 Sep): LTCG ₹3,00,000 -> cumulative LTCG tax (3L - 1.25L = 1.75L @ 12.5% = 21,875)
        // Q3 (16 Sep - 15 Dec): STCG ₹50,000
        // Q4 (16 Dec - 31 Mar): LTCG ₹1,00,000
        let sales = vec![
            AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false),
            AdvanceTaxSale::new(d("2025-08-20"), 300_000.0, true),
            AdvanceTaxSale::new(d("2025-11-15"), 50_000.0, false),
            AdvanceTaxSale::new(d("2026-01-20"), 100_000.0, true),
        ];

        let fy_start = d("2025-04-01");
        let schedule = compute_advance_tax(&sales, fy_start, d("2025-11-01"), &[]);

        assert_eq!(schedule.fy_label, "FY 2025-26 (AY 2026-27)");
        assert!(schedule.sec_208_applicable);

        // Q1: 20,000 + 4% = 20,800; 15% of it = 3,120.
        let q1 = &schedule.quarters[0];
        assert_eq!(q1.quarter, "Q1");
        assert_eq!(q1.due_date, d("2025-06-15"));
        assert!(close(q1.stcg_cumulative, 100_000.0));
        assert!(close(q1.ltcg_cumulative, 0.0));
        assert!(close(q1.cumulative_tax_estimated, 20_800.0));
        assert!(close(q1.required_cumulative_pct, 15.0));
        assert!(close(q1.min_cumulative_tax_required, 3_120.0));
        assert!(close(q1.quarter_tax_payable, 3_120.0));
        // No payment recorded: past due and unpaid, not "Paid".
        assert_eq!(q1.status, AdvanceTaxStatus::Overdue);

        // Q2: STCG 20k + LTCG (1.75L * 12.5% = 21,875) = 41,875; +4% = 43,550.
        let q2 = &schedule.quarters[1];
        assert_eq!(q2.quarter, "Q2");
        assert_eq!(q2.due_date, d("2025-09-15"));
        assert!(close(q2.stcg_cumulative, 100_000.0));
        assert!(close(q2.ltcg_cumulative, 300_000.0));
        assert!(close(q2.cumulative_tax_estimated, 43_550.0));
        assert!(close(q2.required_cumulative_pct, 45.0));
        assert!(close(q2.min_cumulative_tax_required, 19_597.5));
        assert!(close(q2.quarter_tax_payable, 16_477.5));
        assert_eq!(q2.status, AdvanceTaxStatus::Overdue);

        // Q3: STCG 30k + LTCG 21,875 = 51,875; +4% = 53,950.
        let q3 = &schedule.quarters[2];
        assert_eq!(q3.quarter, "Q3");
        assert_eq!(q3.due_date, d("2025-12-15"));
        assert!(close(q3.stcg_cumulative, 150_000.0));
        assert!(close(q3.ltcg_cumulative, 300_000.0));
        assert!(close(q3.cumulative_tax_estimated, 53_950.0));
        assert!(close(q3.required_cumulative_pct, 75.0));
        assert!(close(q3.min_cumulative_tax_required, 40_462.5));
        assert!(close(q3.quarter_tax_payable, 20_865.0));
        assert_eq!(q3.status, AdvanceTaxStatus::Pending); // as_of is 2025-11-01 (in Q3)

        // Q4: STCG 30k + LTCG 2.75L * 12.5% (34,375) = 64,375; +4% = 66,950.
        let q4 = &schedule.quarters[3];
        assert_eq!(q4.quarter, "Q4");
        assert_eq!(q4.due_date, d("2026-03-15"));
        assert!(close(q4.stcg_cumulative, 150_000.0));
        assert!(close(q4.ltcg_cumulative, 400_000.0));
        assert!(close(q4.cumulative_tax_estimated, 66_950.0));
        assert!(close(q4.required_cumulative_pct, 100.0));
        assert!(close(q4.min_cumulative_tax_required, 66_950.0));
        assert!(close(q4.quarter_tax_payable, 26_487.5));
        assert_eq!(q4.status, AdvanceTaxStatus::Upcoming); // Q4 is future relative to 2025-11-01

        // Verify total sum of quarter payables matches total tax
        let sum_payable: f64 = schedule
            .quarters
            .iter()
            .map(|q| q.quarter_tax_payable)
            .sum();
        assert!(close(sum_payable, 66_950.0));
        assert!(close(schedule.total_tax_estimated, 66_950.0));
    }

    #[test]
    fn advance_tax_losses_in_subsequent_quarters() {
        // Gain in Q1: STCG ₹1,00,000 -> tax 20,000 + 4% cess = ₹20,800
        // Loss in Q2: STCL ₹70,000 -> net cumulative STCG ₹30,000 -> 6,000 + 240 = ₹6,240
        // Min req Q1 = 15% of 20,800 = 3,120 (payable = 3,120)
        // Min req Q2 = 45% of 6,240 = 2,808, below the prior 3,120, so Q2 payable is 0.
        let sales = vec![
            AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false),
            AdvanceTaxSale::new(d("2025-07-15"), -70_000.0, false),
        ];

        let schedule = compute_advance_tax(&sales, d("2025-04-01"), d("2025-10-01"), &[]);
        assert!(close(schedule.quarters[0].quarter_tax_payable, 3_120.0));
        assert!(close(schedule.quarters[1].quarter_tax_payable, 0.0));
        assert!(close(
            schedule.quarters[1].min_cumulative_tax_required,
            2_808.0
        ));
    }

    #[test]
    fn advance_tax_sec_208_threshold_exemption() {
        // Gain: STCG ₹40,000 -> 8,000 + 4% cess = ₹8,320 (< ₹10,000 threshold)
        let sales = vec![AdvanceTaxSale::new(d("2025-05-10"), 40_000.0, false)];
        let schedule = compute_advance_tax(&sales, d("2025-04-01"), d("2025-06-01"), &[]);

        assert!(!schedule.sec_208_applicable);
        assert!(close(schedule.total_tax_estimated, 8_320.0));
        assert_eq!(schedule.sec_208_threshold, 10_000.0);
    }

    #[test]
    fn advance_tax_status_progression_across_quarters() {
        let sales = vec![AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false)];

        // Before Q1 (e.g. 2025-03-15) -> Q1 upcoming
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-03-15"), &[]);
        assert_eq!(s.quarters[0].status, AdvanceTaxStatus::Upcoming);

        // During Q1 (e.g. 2025-05-01) -> Q1 pending
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-05-01"), &[]);
        assert_eq!(s.quarters[0].status, AdvanceTaxStatus::Pending);
        assert_eq!(s.quarters[1].status, AdvanceTaxStatus::Upcoming);

        // During Q2 (e.g. 2025-07-01) with no recorded payment: Q1 overdue, Q2 pending
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-07-01"), &[]);
        assert_eq!(s.quarters[0].status, AdvanceTaxStatus::Overdue);
        assert_eq!(s.quarters[1].status, AdvanceTaxStatus::Pending);

        // After the year, nothing recorded: every instalment with an amount due is overdue.
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2026-04-10"), &[]);
        for q in &s.quarters {
            assert_eq!(q.status, AdvanceTaxStatus::Overdue, "{}", q.quarter);
        }
        // No interest is computed without payments, because none were recorded.
        assert!(s.quarters.iter().all(|q| q.paid == 0.0));
        assert!(!s.sec_234b_applicable);
    }

    #[test]
    fn nothing_is_paid_until_a_payment_covers_it() {
        // Tax ₹20,800: Q1 needs 3,120 by 15 Jun, Q2 9,360 by 15 Sep.
        let sales = vec![AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false)];
        let late = [AdvancePayment {
            date: Some(d("2025-07-01")),
            amount: 20_800.0,
        }];
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-10-01"), &late);
        // Paid after the Q1 date: Q1 is still overdue, Q2 is covered.
        assert_eq!(s.quarters[0].status, AdvanceTaxStatus::Overdue);
        assert!(close(s.quarters[0].paid, 0.0));
        assert_eq!(s.quarters[1].status, AdvanceTaxStatus::Paid);
        assert!(close(s.quarters[1].paid, 20_800.0));
        assert!(!s.assumed_on_time);
    }

    #[test]
    fn advance_tax_paid_shortfall_and_234c_interest() {
        // STCG ₹1,00,000 in Q1 -> tax ₹20,800 with cess (Min Q1 req ₹3,120, Min Q2 req ₹9,360)
        let sales = vec![AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false)];

        // Evaluation on 2025-10-01 (post Q2). The user paid only ₹2,000, with no date
        // (assumed paid on time):
        let two_k = [AdvancePayment {
            date: None,
            amount: 2_000.0,
        }];
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-10-01"), &two_k);
        assert!(s.assumed_on_time);

        // Q1 shortfall: 3,120 - 2,000 = 1,120, rounded down to 1,100 (Rule 119A).
        // Safe harbour 12% = 2,496. Paid (2,000) < 2,496 -> interest 3% on 1,100 = 33.
        let q1 = &s.quarters[0];
        assert_eq!(q1.status, AdvanceTaxStatus::Overdue);
        assert!(close(q1.shortfall, 1_120.0));
        assert!(close(q1.sec_234c_interest, 33.0));

        // Q2 shortfall: 9,360 - 2,000 = 7,360 -> 7,300. Safe harbour 36% = 7,488.
        // Interest 3% on 7,300 = 219.
        let q2 = &s.quarters[1];
        assert_eq!(q2.status, AdvanceTaxStatus::Overdue);
        assert!(close(q2.shortfall, 7_360.0));
        assert!(close(q2.sec_234c_interest, 219.0));

        assert!(close(s.total_sec_234c_interest, 252.0));

        // A bare total of ₹10,000 covers Q1 and Q2:
        let ten_k = [AdvancePayment {
            date: None,
            amount: 10_000.0,
        }];
        let s_paid = compute_advance_tax(&sales, d("2025-04-01"), d("2025-10-01"), &ten_k);
        assert_eq!(s_paid.quarters[0].status, AdvanceTaxStatus::Paid);
        assert_eq!(s_paid.quarters[1].status, AdvanceTaxStatus::Paid);
        assert!(close(s_paid.total_sec_234c_interest, 0.0));
    }

    #[test]
    fn late_payment_is_charged_interest_for_each_missed_due_date() {
        // STCG ₹1,00,000 on 10 May 2025: tax 20,000 + 800 cess = ₹20,800.
        // Everything is paid on 10 Jan 2026, after the Q1, Q2 and Q3 due dates.
        let sales = vec![AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false)];
        let late = [AdvancePayment {
            date: Some(d("2026-01-10")),
            amount: 20_800.0,
        }];
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2026-01-20"), &late);
        // Q1: 15% = 3,120 -> 3,100 x 3% = 93. Q2: 45% = 9,360 -> 9,300 x 3% = 279.
        // Q3: 75% = 15,600 x 3% = 468. Q4 is not due yet.
        assert!(close(s.quarters[0].sec_234c_interest, 93.0));
        assert!(close(s.quarters[1].sec_234c_interest, 279.0));
        assert!(close(s.quarters[2].sec_234c_interest, 468.0));
        assert!(close(s.quarters[3].sec_234c_interest, 0.0));
        assert!(close(s.total_sec_234c_interest, 840.0));
        assert!(close(s.total_paid, 20_800.0));
        // The same total paid before 15 June has no interest at all.
        let on_time = [AdvancePayment {
            date: Some(d("2025-06-14")),
            amount: 20_800.0,
        }];
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2026-01-20"), &on_time);
        assert!(close(s.total_sec_234c_interest, 0.0));
        assert!(
            s.quarters
                .iter()
                .take(3)
                .all(|q| q.status == AdvanceTaxStatus::Paid)
        );
    }

    #[test]
    fn q4_gain_after_15_march_paid_by_31_march_has_no_234c_interest() {
        // Sec. 234C(1) proviso. STCG ₹10,00,000 sold on 20 Mar 2026:
        // tax 2,00,000 + 4% cess 8,000 = ₹2,08,000, paid on 25 Mar. Nothing
        // had arisen by 15 Mar, so nothing was due then.
        let sales = vec![AdvanceTaxSale::new(d("2026-03-20"), 1_000_000.0, false)];
        let paid = [AdvancePayment {
            date: Some(d("2026-03-25")),
            amount: 208_000.0,
        }];
        for as_of in ["2026-03-26", "2026-04-10"] {
            let s = compute_advance_tax(&sales, d("2025-04-01"), d(as_of), &paid);
            let q4 = &s.quarters[3];
            assert!(close(s.total_tax_estimated, 208_000.0), "{as_of}");
            assert!(close(q4.sec_234c_interest, 0.0), "{as_of}");
            assert!(close(q4.shortfall, 0.0), "{as_of}");
            assert_eq!(q4.status, AdvanceTaxStatus::Paid, "{as_of}");
            assert!(close(s.total_sec_234c_interest, 0.0));
        }
        // Paid on 1 Apr instead: too late for the proviso, so the whole
        // ₹2,08,000 was short on 15 Mar: 1% = ₹2,080, and Q4 is overdue.
        let late = [AdvancePayment {
            date: Some(d("2026-04-01")),
            amount: 208_000.0,
        }];
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2026-04-10"), &late);
        assert!(close(s.quarters[3].sec_234c_interest, 2_080.0));
        assert_eq!(s.quarters[3].status, AdvanceTaxStatus::Overdue);
        // Before 31 Mar is over, the late gains can still be paid: no interest yet.
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2026-03-26"), &[]);
        assert!(close(s.quarters[3].sec_234c_interest, 0.0));
        assert_eq!(s.quarters[3].status, AdvanceTaxStatus::Pending);
    }

    #[test]
    fn q4_proviso_covers_only_gains_after_15_march() {
        // ₹5,00,000 STCG on 10 Mar (tax 1,00,000 + 4,000 cess = ₹1,04,000, due
        // by 15 Mar) and ₹5,00,000 on 20 Mar (another ₹1,04,000). Everything is
        // paid on 25 Mar: the second half is protected, the first is not.
        let sales = vec![
            AdvanceTaxSale::new(d("2026-03-10"), 500_000.0, false),
            AdvanceTaxSale::new(d("2026-03-20"), 500_000.0, false),
        ];
        let paid = [AdvancePayment {
            date: Some(d("2026-03-25")),
            amount: 208_000.0,
        }];
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2026-04-10"), &paid);
        let q4 = &s.quarters[3];
        assert!(close(q4.shortfall, 104_000.0));
        // 1% of ₹1,04,000.
        assert!(close(q4.sec_234c_interest, 1_040.0));
        assert_eq!(q4.status, AdvanceTaxStatus::Overdue);
    }

    #[test]
    fn advance_tax_uses_the_rates_and_exemption_of_the_sale_date() {
        // FY 2023-24: STCG ₹1,00,000 at 15% = 15,000 + 4% cess = ₹15,600.
        let sales = vec![AdvanceTaxSale::new(d("2023-08-10"), 100_000.0, false)];
        let s = compute_advance_tax(&sales, d("2023-04-01"), d("2024-04-10"), &[]);
        assert!(close(s.total_tax_estimated, 15_600.0));
        // LTCG ₹1,50,000 in FY 2023-24: ₹1 lakh exempt, 10% on 50,000 = 5,000
        // + cess = ₹5,200.
        let sales = vec![AdvanceTaxSale::new(d("2023-08-10"), 150_000.0, true)];
        let s = compute_advance_tax(&sales, d("2023-04-01"), d("2024-04-10"), &[]);
        assert!(close(s.total_tax_estimated, 5_200.0));
        // FY 2024-25 has both: 1 Jun 2024 at 15% (15,000) and 1 Aug 2024 at
        // 20% (20,000), cess 4% on 35,000 = 1,400: ₹36,400.
        let sales = vec![
            AdvanceTaxSale::new(d("2024-06-01"), 100_000.0, false),
            AdvanceTaxSale::new(d("2024-08-01"), 100_000.0, false),
        ];
        let s = compute_advance_tax(&sales, d("2024-04-01"), d("2025-04-10"), &[]);
        assert!(close(s.total_tax_estimated, 36_400.0));
        // Q2 (to 15 Sep) already includes both sales; Q1 (to 15 Jun) only the first.
        assert!(close(s.quarters[0].cumulative_tax_estimated, 15_600.0));
        // FY 2025-26 is unchanged: STCG ₹1,00,000 at 20% + cess = ₹20,800.
        let sales = vec![AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false)];
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2026-04-10"), &[]);
        assert!(close(s.total_tax_estimated, 20_800.0));
    }

    #[test]
    fn a_payment_dated_before_the_year_does_not_count() {
        // Tax ₹20,800 for FY 2025-26 (Q1 needs ₹3,120 by 15 Jun 2025). A
        // payment of 20,800 dated 14 Jun 2024 was made for FY 2024-25.
        let sales = vec![AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false)];
        let old = [AdvancePayment {
            date: Some(d("2024-06-14")),
            amount: 20_800.0,
        }];
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-10-01"), &old);
        assert!(s.ignored_before_year);
        assert!(close(s.total_paid, 0.0));
        assert!(close(s.quarters[0].paid, 0.0));
        assert_eq!(s.quarters[0].status, AdvanceTaxStatus::Overdue);
        // Q1 shortfall 3,120 -> 3,100 at 3% = 93 (no payment counted).
        assert!(close(s.quarters[0].sec_234c_interest, 93.0));
        // The same payment dated inside the year counts.
        let ok = [AdvancePayment {
            date: Some(d("2025-04-01")),
            amount: 20_800.0,
        }];
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-10-01"), &ok);
        assert!(!s.ignored_before_year);
        assert!(close(s.total_paid, 20_800.0));
        assert_eq!(s.quarters[0].status, AdvanceTaxStatus::Paid);
    }

    #[test]
    fn advance_payments_parse_with_and_without_a_date() {
        let p = AdvancePayment::parse("2025-06-14:3000").unwrap();
        assert_eq!(p.date, Some(d("2025-06-14")));
        assert!(close(p.amount, 3_000.0));
        let p = AdvancePayment::parse("₹25,000").unwrap();
        assert_eq!(p.date, None);
        assert!(close(p.amount, 25_000.0));
        assert!(AdvancePayment::parse("-5").is_err());
        assert!(AdvancePayment::parse("soon:3000").is_err());
        assert!(AdvancePayment::parse("abc").is_err());
    }

    #[test]
    fn india_estimate_adds_four_percent_cess() {
        // STCG ₹1,00,000 at 20% = 20,000; cess 4% = 800.
        let b = Buckets {
            st_gain: 100_000.0,
            ..Buckets::default()
        };
        let e = estimate(Country::India, b);
        assert!(close(e.st_tax, 20_000.0));
        assert!(close(e.cess, 800.0));
        assert!(close(e.total(), 20_800.0));
        // LTCG ₹3,00,000: (3L - 1.25L) x 12.5% = 21,875; cess 875.
        let b = Buckets {
            lt_gain: 300_000.0,
            ..Buckets::default()
        };
        let e = estimate(Country::India, b);
        assert!(close(e.lt_tax, 21_875.0));
        assert!(close(e.cess, 875.0));
        // US and UK have no cess.
        assert_eq!(estimate(Country::Us, b).cess, 0.0);
        assert_eq!(estimate(Country::Uk, b).cess, 0.0);
    }

    #[test]
    fn india_rates_follow_the_sale_date() {
        let none = Buckets::default();
        // STCG ₹1,00,000 sold before 23 Jul 2024: 15% = 15,000 (not 20,000).
        let b = b(100_000.0, 0.0, 0.0, 0.0);
        let e = estimate_india_by_period(b, none, india_exemption_for_year(2024));
        assert!(close(e.st_tax, 15_000.0));
        assert!(close(e.cess, 600.0));
        assert_eq!(e.taxable_before, (100_000.0, 0.0));
        // The same sale from 23 Jul 2024 is taxed at 20%.
        let e = estimate_india_by_period(none, b, india_exemption_for_year(2024));
        assert!(close(e.st_tax, 20_000.0));
        assert_eq!(e.taxable_before, (0.0, 0.0));
        // FY 2025-26 results are those of `estimate`.
        assert_eq!(
            estimate_india_by_period(none, b, 125_000.0),
            estimate(Country::India, b)
        );
    }

    #[test]
    fn mixed_year_uses_old_and_new_rates() {
        // FY 2024-25: STCG ₹1,00,000 before 23 Jul (15% = 15,000) and ₹1,00,000
        // after (20% = 20,000).
        let before = b(100_000.0, 0.0, 100_000.0, 0.0);
        let after = b(100_000.0, 0.0, 100_000.0, 0.0);
        let e = estimate_india_by_period(before, after, india_exemption_for_year(2024));
        assert!(close(e.st_tax, 35_000.0));
        // LTCG ₹1,00,000 each side, exemption ₹1.25 lakh used against the
        // 12.5% gains first (all 1,00,000), then 25,000 of the 10% gains:
        // taxable 75,000 at 10% = 7,500.
        assert!(close(e.exemption_used, 125_000.0));
        assert!(close(e.lt_taxable, 75_000.0));
        assert!(close(e.lt_tax, 7_500.0));
        assert_eq!(e.taxable_before, (100_000.0, 75_000.0));
        assert!(close(e.cess, (35_000.0 + 7_500.0) * 0.04));
    }

    #[test]
    fn mixed_year_losses_go_against_the_higher_rate_gains_first() {
        // STCG 1,00,000 before and 1,00,000 after, STCL 30,000 (after):
        // after 70,000 at 20% = 14,000, before 1,00,000 at 15% = 15,000.
        let before = b(100_000.0, 0.0, 0.0, 0.0);
        let after = b(100_000.0, 30_000.0, 0.0, 0.0);
        let e = estimate_india_by_period(before, after, 125_000.0);
        assert!(close(e.st_tax, 29_000.0));
        // A loss larger than the new-rate gains spills into the old ones:
        // STCL 1,30,000 vs after 1,00,000: 30,000 off the before gains,
        // leaving 70,000 at 15% = 10,500.
        let after = b(100_000.0, 130_000.0, 0.0, 0.0);
        let e = estimate_india_by_period(before, after, 125_000.0);
        assert!(close(e.st_tax, 10_500.0));
        assert!(close(e.st_taxable, 70_000.0));
    }

    #[test]
    fn exemption_was_one_lakh_before_fy_2024_25() {
        assert_eq!(india_exemption_for_year(2023), 100_000.0);
        assert_eq!(india_exemption_for_year(2024), 125_000.0);
        // FY 2023-24: LTCG 3,00,000 - 1,00,000 = 2,00,000 at 10% = 20,000.
        let e =
            estimate_india_by_period(b(0.0, 0.0, 300_000.0, 0.0), Buckets::default(), 100_000.0);
        assert!(close(e.exemption_used, 100_000.0));
        assert!(close(e.lt_tax, 20_000.0));
    }

    #[test]
    fn advance_tax_serialization_roundtrip() {
        let sales = vec![AdvanceTaxSale::new(d("2025-05-10"), 100_000.0, false)];
        let paid = [AdvancePayment {
            date: Some(d("2025-06-01")),
            amount: 5_000.0,
        }];
        let s = compute_advance_tax(&sales, d("2025-04-01"), d("2025-10-01"), &paid);
        let json_str = serde_json::to_string(&s).unwrap();
        let deserialized: AdvanceTaxSchedule = serde_json::from_str(&json_str).unwrap();
        assert_eq!(s, deserialized);
    }

    mod wash {
        use super::super::*;
        use crate::portfolio::{Holding, Lot, Sale};

        fn sale(symbol: &str, qty: f64, buy: (&str, f64), sell: (&str, f64)) -> Sale {
            Sale {
                symbol: symbol.into(),
                qty,
                buy_date: buy.0.into(),
                buy_price: buy.1,
                sell_date: sell.0.into(),
                sell_price: sell.1,
                buy_date_estimated: false,
                source: "manual".into(),
                order_id: None,
                intraday: false,
                account: None,
                bonuses: Vec::new(),
                bonus_shares: false,
                charges: None,
                fx_currency: None,
                buy_fx_rate: None,
                origin: None,
            }
        }

        fn held(symbol: &str, lots: &[(f64, f64, &str)]) -> Holding {
            let lots: Vec<Lot> = lots
                .iter()
                .map(|(q, p, d)| Lot::new(*q, *p, *d, "manual"))
                .collect();
            Holding {
                symbol: symbol.into(),
                shares: lots.iter().map(|l| l.qty).sum(),
                avg_cost: 0.0,
                added_at: String::new(),
                lots,
            }
        }

        #[test]
        fn a_buy_ten_days_after_the_loss_is_a_wash_sale() {
            // 10 AAPL bought at 200, sold at 150 on 2025-03-01: loss 500.
            // 10 more bought on 2025-03-11 and still held.
            let sales = [sale(
                "AAPL",
                10.0,
                ("2024-06-03", 200.0),
                ("2025-03-01", 150.0),
            )];
            let holdings = [held("AAPL", &[(10.0, 155.0, "2025-03-11")])];
            let w = wash_sales(&sales, &holdings).washes;
            assert_eq!(w.len(), 1);
            assert_eq!((w[0].sale, w[0].loss, w[0].disallowed), (0, 500.0, 500.0));
            assert_eq!(
                w[0].replacements,
                [WashReplacement {
                    date: "2025-03-11".into(),
                    date_estimated: false,
                    qty: 10.0,
                    basis_added: 500.0,
                    days_added: 271,
                    sale: None,
                }]
            );
        }

        #[test]
        fn a_buy_before_the_sale_counts_and_its_later_sale_carries_the_basis() {
            // 10 bought 2025-01-02 at 100, 10 more 2025-02-10 at 90; the first
            // 10 sold 2025-02-20 at 80 (loss 200). The second lot, bought 10
            // days before, is the replacement and is sold on 2025-06-02.
            let sales = [
                sale("MSFT", 10.0, ("2025-01-02", 100.0), ("2025-02-20", 80.0)),
                sale("MSFT", 10.0, ("2025-02-10", 90.0), ("2025-06-02", 120.0)),
            ];
            let w = wash_sales(&sales, &[]).washes;
            assert_eq!(w.len(), 1);
            assert_eq!((w[0].sale, w[0].disallowed), (0, 200.0));
            assert_eq!(w[0].replacements[0].sale, Some(1));
            assert_eq!(w[0].replacements[0].date, "2025-02-10");
        }

        #[test]
        fn a_partial_replacement_disallows_its_share_of_the_loss() {
            // 100 sold at a 1,000 loss; 25 bought back 5 days later.
            let sales = [sale(
                "NVDA",
                100.0,
                ("2024-01-02", 50.0),
                ("2025-04-01", 40.0),
            )];
            let holdings = [held("NVDA", &[(25.0, 41.0, "2025-04-06")])];
            let w = wash_sales(&sales, &holdings).washes;
            assert_eq!((w[0].replacement_qty, w[0].disallowed), (25.0, 250.0));
        }

        #[test]
        fn a_replacement_is_used_once() {
            // Two losses of 100 each, one 10-share buy in both windows: only
            // the first loss is a wash sale.
            let sales = [
                sale("X", 10.0, ("2024-01-02", 20.0), ("2025-05-01", 10.0)),
                sale("X", 10.0, ("2024-02-02", 20.0), ("2025-05-05", 10.0)),
            ];
            let holdings = [held("X", &[(10.0, 11.0, "2025-05-10")])];
            let w = wash_sales(&sales, &holdings).washes;
            assert_eq!(w.len(), 1);
            assert_eq!(w[0].sale, 0);
        }

        #[test]
        fn a_buy_more_than_30_days_away_is_not_a_wash_sale() {
            let sales = [sale(
                "AAPL",
                10.0,
                ("2024-06-03", 200.0),
                ("2025-03-01", 150.0),
            )];
            let holdings = [held(
                "AAPL",
                &[(10.0, 155.0, "2025-04-01"), (5.0, 190.0, "2025-01-29")],
            )];
            assert!(wash_sales(&sales, &holdings).washes.is_empty());
            // Day 30 is still inside the window.
            let holdings = [held("AAPL", &[(10.0, 155.0, "2025-03-31")])];
            assert_eq!(wash_sales(&sales, &holdings).washes.len(), 1);
        }

        #[test]
        fn a_sale_at_a_gain_is_not_a_wash_sale() {
            let sales = [sale(
                "AAPL",
                10.0,
                ("2024-06-03", 150.0),
                ("2025-03-01", 200.0),
            )];
            let holdings = [held("AAPL", &[(10.0, 195.0, "2025-03-11")])];
            assert!(wash_sales(&sales, &holdings).washes.is_empty());
        }

        #[test]
        fn the_rest_of_the_sold_lot_is_not_a_replacement() {
            // 20 bought on 2025-01-02, 10 of them sold at a loss 15 days later.
            let sales = [sale(
                "AAPL",
                10.0,
                ("2025-01-02", 200.0),
                ("2025-01-17", 150.0),
            )];
            let holdings = [held("AAPL", &[(10.0, 200.0, "2025-01-02")])];
            assert!(wash_sales(&sales, &holdings).washes.is_empty());
        }

        #[test]
        fn shares_sold_on_the_same_day_are_not_replacements() {
            // Two lots, both sold at a loss on the same day.
            let sales = [
                sale("AAPL", 10.0, ("2025-01-02", 200.0), ("2025-01-20", 150.0)),
                sale("AAPL", 10.0, ("2025-01-10", 190.0), ("2025-01-20", 150.0)),
            ];
            assert!(wash_sales(&sales, &[]).washes.is_empty());
        }

        #[test]
        fn non_usd_symbols_are_ignored() {
            let sales = [sale(
                "TCS.NS",
                10.0,
                ("2024-06-03", 4000.0),
                ("2025-03-01", 3500.0),
            )];
            let holdings = [held("TCS.NS", &[(10.0, 3550.0, "2025-03-11")])];
            assert!(wash_sales(&sales, &holdings).washes.is_empty());
        }

        #[test]
        fn replacement_shares_keep_their_own_basis_in_a_later_sale() {
            // A 100 loss on 10 shares moves to 10 of a 20-share purchase at
            // 10. All 20 are sold at 12, then 10 bought back within 30 days.
            // The replaced 10 lose 80 (cost 100 + 100, proceeds 120); the
            // other 10 gain 20. The 80 loss is disallowed, not an average.
            let sales = [
                sale("X", 10.0, ("2025-01-02", 30.0), ("2025-03-01", 20.0)),
                sale("X", 20.0, ("2025-03-05", 10.0), ("2025-05-01", 12.0)),
            ];
            let holdings = [held("X", &[(10.0, 12.5, "2025-05-10")])];
            let r = wash_sales(&sales, &holdings);
            assert_eq!(r.washes.len(), 2);
            let later = &r.washes[1];
            assert_eq!((later.sale, later.qty, later.loss), (1, 10.0, 80.0));
            assert_eq!(later.disallowed, 80.0);
            // The second sale is split: 10 plain shares, 10 replacement
            // shares with the 100 basis and the 58 days the first lot was held.
            assert_eq!(r.parts[1].len(), 2);
            assert_eq!((r.parts[1][0].qty, r.parts[1][0].basis_added), (10.0, 0.0));
            let replaced = &r.parts[1][1];
            assert_eq!((replaced.qty, replaced.basis_added), (10.0, 100.0));
            assert_eq!((replaced.days_added, replaced.disallowed), (58, 80.0));
            // The open lot takes the 80 and both holding periods (58 + 57).
            assert_eq!(later.replacements[0].basis_added, 80.0);
            assert_eq!(later.replacements[0].days_added, 115);
        }

        #[test]
        fn an_estimated_purchase_date_is_flagged() {
            let sales = [sale(
                "AAPL",
                10.0,
                ("2024-06-03", 200.0),
                ("2025-03-01", 150.0),
            )];
            let mut h = held("AAPL", &[(10.0, 155.0, "2025-03-11")]);
            h.lots[0].date_estimated = true;
            let w = wash_sales(&sales, &[h]).washes;
            assert!(w[0].replacements[0].date_estimated);
        }

        #[test]
        fn split_amounts_add_up_to_the_disallowed_loss() {
            // A 0.01 loss on 1 share, replaced by two half-share purchases.
            let sales = [sale(
                "AAPL",
                1.0,
                ("2024-06-03", 10.01),
                ("2025-03-01", 10.0),
            )];
            let holdings = [held(
                "AAPL",
                &[(0.5, 10.0, "2025-03-05"), (0.5, 10.0, "2025-03-06")],
            )];
            let w = &wash_sales(&sales, &holdings).washes[0];
            assert_eq!(w.disallowed, 0.01);
            let split: f64 = w.replacements.iter().map(|r| r.basis_added).sum();
            assert_eq!(split, 0.01);
        }

        #[test]
        fn the_window_ends_30_days_after_the_sale() {
            assert_eq!(
                wash_sale_window_end(NaiveDate::from_ymd_opt(2025, 3, 1).unwrap()),
                NaiveDate::from_ymd_opt(2025, 3, 31).unwrap()
            );
        }

        #[test]
        fn every_replacement_gets_a_non_negative_share_of_the_loss() {
            // A 2-cent loss split over four quarter-share replacements.
            let sales = [sale("AAPL", 1.0, ("2024-06-03", 1.02), ("2025-03-01", 1.0))];
            let holdings = [held(
                "AAPL",
                &[
                    (0.25, 1.0, "2025-03-02"),
                    (0.25, 1.0, "2025-03-03"),
                    (0.25, 1.0, "2025-03-04"),
                    (0.25, 1.0, "2025-03-05"),
                ],
            )];
            let w = &wash_sales(&sales, &holdings).washes[0];
            assert_eq!(w.disallowed, 0.02);
            assert!(w.replacements.iter().all(|r| r.basis_added >= 0.0));
            let split: i64 = w.replacements.iter().map(|r| cents(r.basis_added)).sum();
            assert_eq!(split, 2);
        }

        #[test]
        fn crypto_quoted_in_usd_is_not_checked() {
            let sales = [sale(
                "BTC-USD",
                1.0,
                ("2024-06-03", 60000.0),
                ("2025-03-01", 50000.0),
            )];
            let holdings = [held("BTC-USD", &[(1.0, 51000.0, "2025-03-11")])];
            let w = wash_sales(&sales, &holdings);
            assert!(w.washes.is_empty());
            assert!(w.parts[0].is_empty());
        }

        #[test]
        fn shares_sold_before_the_loss_are_not_replacements() {
            // A: bought 2025-01-02 at 20, sold 2025-01-20 at 10 (loss 100).
            // B: bought 2025-01-10 at 19, sold 2025-01-25 at 9. B's shares
            // replace A's; A's shares, already sold, cannot replace B's.
            let sales = [
                sale("XYZ", 10.0, ("2025-01-02", 20.0), ("2025-01-20", 10.0)),
                sale("XYZ", 10.0, ("2025-01-10", 19.0), ("2025-01-25", 9.0)),
            ];
            let w = wash_sales(&sales, &[]);
            assert_eq!(w.washes.len(), 1);
            assert_eq!((w.washes[0].sale, w.washes[0].disallowed), (0, 100.0));
            assert_eq!(w.parts[0][0].disallowed, 100.0);
            assert_eq!(w.parts[1][0].basis_added, 100.0);
        }
    }

    #[test]
    fn sec_50aa_makes_new_debt_fund_units_short_term_whatever_the_holding() {
        let debt = IndiaAsset::DebtFund { listed: false };
        // Bought from 1 Apr 2023: Sec. 50AA even after three years.
        assert_eq!(
            india_rule(debt, d("2023-04-01"), d("2026-06-01")),
            IndiaRule::Sec50AA
        );
        // Bought before: long-term after 24 months (unlisted, sold from 23 Jul 2024).
        assert_eq!(
            india_rule(debt, d("2023-03-31"), d("2026-06-01")),
            IndiaRule::LongTerm112
        );
        // Bought before 1 Apr 2023, sold before 23 Jul 2024 within 36 months.
        assert_eq!(
            india_rule(debt, d("2022-09-01"), d("2023-12-01")),
            IndiaRule::ShortTermSlab
        );
        // A listed debt ETF bought before 1 Apr 2023: 12 months.
        let etf = IndiaAsset::DebtFund { listed: true };
        assert_eq!(
            india_rule(etf, d("2023-01-02"), d("2024-08-01")),
            IndiaRule::LongTerm112
        );
        // Gold funds were specified MFs only until 31 Mar 2025.
        let gold = IndiaAsset::OtherFund { listed: false };
        assert_eq!(
            india_rule(gold, d("2023-06-01"), d("2025-03-31")),
            IndiaRule::Sec50AA
        );
        assert_eq!(
            india_rule(gold, d("2023-06-01"), d("2025-07-01")),
            IndiaRule::LongTerm112
        );
        // Market-linked debentures: always Sec. 50AA.
        assert_eq!(
            india_rule(
                IndiaAsset::MarketLinkedDebenture,
                d("2019-01-01"),
                d("2026-01-01")
            ),
            IndiaRule::Sec50AA
        );
        assert_eq!(
            india_rule(IndiaAsset::Vda, d("2020-01-01"), d("2026-01-01")),
            IndiaRule::Vda115BBH
        );
    }

    #[test]
    fn sec_115bbh_taxes_each_crypto_gain_and_sets_off_no_loss() {
        let g = |rule, gain: f64, proceeds: f64| OtherGain {
            rule,
            gain,
            proceeds,
            before_rate_change: false,
        };
        let o = india_other_gains(&[
            g(IndiaRule::Vda115BBH, 100_000.0, 300_000.0),
            g(IndiaRule::Vda115BBH, -50_000.0, 100_000.0),
            g(IndiaRule::Sec50AA, -10_000.0, 90_000.0),
        ]);
        assert_eq!(o.vda_gains, 100_000.0);
        assert_eq!(o.vda_losses_ignored, 50_000.0);
        // 30% of the whole ₹1 lakh: neither loss reduces it.
        assert!(close(o.vda_tax, 30_000.0));
        assert!(close(o.cess, 1_200.0));
        assert!(close(o.flat_tax(), 31_200.0));
        // 1% TDS (Sec. 194S) on both sales' proceeds.
        assert!(close(o.vda_tds, 4_000.0));
        assert!(close(o.slab_income(), -10_000.0));
    }

    #[test]
    fn sec_112_losses_net_against_its_own_gains_at_the_higher_rate_first() {
        let g = |gain: f64, before: bool| OtherGain {
            rule: IndiaRule::LongTerm112,
            gain,
            proceeds: 0.0,
            before_rate_change: before,
        };
        let o = india_other_gains(&[g(10_000.0, true), g(20_000.0, false), g(-5_000.0, false)]);
        assert!(close(o.long_term_112, 25_000.0));
        // The ₹5,000 loss goes against the 20% gain: 5,000 × 20% + 20,000 × 12.5%.
        assert!(close(o.long_term_112_tax, 1_000.0 + 2_500.0));
    }

    fn schedule_with(
        sales: &[AdvanceTaxSale],
        income: &[AdvanceIncome],
        paid: &[AdvancePayment],
        as_of: &str,
    ) -> AdvanceTaxSchedule {
        compute_advance_tax_with_income(sales, income, d("2026-04-01"), d(as_of), paid)
    }

    #[test]
    fn next_instalment_says_what_is_due_and_the_234c_interest_if_missed() {
        // ₹4 lakh STCG in May: tax 80,000 + 4% cess = 83,200.
        let sales = [AdvanceTaxSale::new(d("2026-05-10"), 400_000.0, false)];
        let s = schedule_with(&sales, &[], &[], "2026-11-21");
        let n = next_instalment(&s, d("2026-11-21")).unwrap();
        assert_eq!(n.quarter, "Q3");
        assert_eq!(n.due_date, d("2026-12-15"));
        assert_eq!(n.days_left, 24);
        assert!(close(n.required_by_due, 83_200.0 * 0.75));
        assert!(close(n.amount_due, 62_400.0));
        // 3% of the shortfall rounded down to ₹100.
        assert!(close(n.interest_if_missed, 62_400.0 * 0.03));
        // With ₹40,000 already paid: 22,400 due, interest on 22,400.
        let paid = [AdvancePayment {
            date: Some(d("2026-09-10")),
            amount: 40_000.0,
        }];
        let s = schedule_with(&sales, &[], &paid, "2026-11-21");
        let n = next_instalment(&s, d("2026-11-21")).unwrap();
        assert!(close(n.amount_due, 22_400.0));
        assert!(close(n.interest_if_missed, 672.0));
        // After 15 March nothing is left to remind about.
        let s = schedule_with(&sales, &[], &[], "2027-03-20");
        assert!(next_instalment(&s, d("2027-03-20")).is_none());
    }

    #[test]
    fn the_reminder_fires_within_seven_days_of_a_due_date_with_tax_left() {
        let sales = [AdvanceTaxSale::new(d("2026-05-10"), 400_000.0, false)];
        let s = schedule_with(&sales, &[], &[], "2026-12-08");
        let r = advance_tax_reminder(&s, d("2026-12-08"), ADVANCE_REMINDER_DAYS).unwrap();
        assert_eq!(r.instalment.days_left, 7);
        assert!(r.message.contains("Q3") && r.message.contains("15 Dec 2026"));
        assert!(r.message.contains("₹62,400"), "{}", r.message);
        assert!(r.message.contains("234C"));
        // Eight days ahead: not yet.
        let s = schedule_with(&sales, &[], &[], "2026-12-07");
        assert!(advance_tax_reminder(&s, d("2026-12-07"), ADVANCE_REMINDER_DAYS).is_none());
        // Paid in full: nothing to remind.
        let paid = [AdvancePayment {
            date: Some(d("2026-12-01")),
            amount: 62_400.0,
        }];
        let s = schedule_with(&sales, &[], &paid, "2026-12-08");
        assert!(advance_tax_reminder(&s, d("2026-12-08"), ADVANCE_REMINDER_DAYS).is_none());
        // Under ₹10,000 of tax a year: no advance tax, no reminder.
        let small = [AdvanceTaxSale::new(d("2026-05-10"), 20_000.0, false)];
        let s = schedule_with(&small, &[], &[], "2026-12-08");
        assert!(advance_tax_reminder(&s, d("2026-12-08"), ADVANCE_REMINDER_DAYS).is_none());
    }

    #[test]
    fn dividend_income_counts_from_the_quarter_it_was_received() {
        let income = [
            AdvanceIncome {
                date: d("2026-05-20"),
                amount: 100_000.0,
                rate: 0.30,
            },
            AdvanceIncome {
                date: d("2027-01-10"),
                amount: 100_000.0,
                rate: 0.30,
            },
        ];
        let s = schedule_with(&[], &income, &[], "2026-11-21");
        // ₹31,200 of tax a quarter-1 dividend brings; the January one only
        // counts for Q4.
        assert!(close(s.quarters[0].cumulative_tax_estimated, 31_200.0));
        assert!(close(s.quarters[2].cumulative_tax_estimated, 31_200.0));
        assert!(close(s.quarters[3].cumulative_tax_estimated, 62_400.0));
        assert!(close(s.other_income, 200_000.0));
        assert!(close(s.other_income_tax, 62_400.0));
        assert!(s.sec_208_applicable);
    }
}
