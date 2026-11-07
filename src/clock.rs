//! Exchange clock: trading days, session hours and bot phases for NSE (IST)
//! and NYSE (US Eastern), independent of the machine's own time zone.
//!
//! Everything here is a pure function of a UTC instant, so callers pass
//! `Utc::now()` and tests pass fixed times.
//!
//! * IST is a fixed +05:30 offset (India has no daylight saving).
//! * US Eastern applies the US daylight-saving rule (second Sunday of March
//!   to the first Sunday of November, switching at 02:00 local). That rule is
//!   small enough to compute here instead of pulling in a time-zone database.
//! * The global exchanges (`-m uk`, `-m jp`, ...) take their time zone,
//!   hours and holiday table from their row in `market::GLOBAL_MARKETS`; the
//!   [`TzRule`]s cover European and Australian summer time the same way.
//!
//! # Updating the holiday tables
//!
//! NSE publishes next year's trading holidays in a circular every December
//! ("Holidays for the year …", equity segment). NYSE publishes three years at
//! a time. To update:
//!
//! 1. Add the new rows to [`NSE_HOLIDAYS`] / [`NYSE_HOLIDAYS`] (weekday
//!    closures only — weekend holidays don't change anything) and any 1:00 pm
//!    early closes to [`NYSE_EARLY_CLOSES`].
//! 2. Move the year from [`NSE_PROVISIONAL_YEARS`] (if it was there) into
//!    [`NSE_OFFICIAL_YEARS`], or extend [`NYSE_OFFICIAL_YEARS`].
//! 3. Run `cargo test clock` — the tests check that every row is a weekday
//!    inside a covered year.
//!
//! The global exchanges' tables ([`LSE`], [`XETRA`], [`JPX`], [`HKEX`],
//! [`SGX`], [`TSX`], [`ASX`]) work the same way: add rows, then list the year
//! in the table's `official_years` (or `provisional_years`).
//!
//! Years outside the tables fall back to "weekdays are trading days", and
//! [`calendar_coverage`] reports that so commands can warn about it.

use crate::market::Market;
use chrono::{
    DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveTime, TimeZone, Utc, Weekday,
};

/// Bot session phases, derived from the exchange's open and close times.
///
/// For NSE (09:15–15:30 IST) the boundaries are 09:00, 09:15, 09:30, 14:00,
/// 15:15, 15:20 and 16:00. NYSE uses the same offsets from 09:30 and 16:00 ET
/// (or 13:00 on early-close days).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Phase {
    PreMarket,  // open − 15 min
    Opening,    // open → open + 15 min: the entry window
    Active,     // until close − 90 min
    WindDown,   // no new entries, stops tighten
    SquareOff,  // close − 15 min: close everything
    PostMarket, // close − 10 min → close + 30 min: settle
    Closed,     // anything else, weekends and holidays
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Phase::PreMarket => write!(f, "Pre-Market"),
            Phase::Opening => write!(f, "Opening"),
            Phase::Active => write!(f, "Active Trading"),
            Phase::WindDown => write!(f, "Wind-Down"),
            Phase::SquareOff => write!(f, "Square-Off"),
            Phase::PostMarket => write!(f, "Post-Market"),
            Phase::Closed => write!(f, "Closed"),
        }
    }
}

/// How an exchange's UTC offset changes through the year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TzRule {
    /// A fixed offset in minutes east of UTC (Japan, Hong Kong, Singapore).
    Fixed(i32),
    /// European summer time on a standard offset in hours (UK 0, Germany
    /// 1): one hour more from the last Sunday of March to the last Sunday of
    /// October, switching at 01:00 UTC.
    Eu(i32),
    /// US Eastern (NYSE, Toronto).
    UsEastern,
    /// Sydney: +10, and +11 from the first Sunday of October to the first
    /// Sunday of April (switching at 16:00 UTC the day before).
    AuEastern,
}

impl TzRule {
    /// The UTC offset at an instant.
    pub fn offset_at(self, utc: DateTime<Utc>) -> FixedOffset {
        let year = utc.year();
        let hours = match self {
            TzRule::Fixed(minutes) => return minutes_offset(minutes),
            TzRule::UsEastern => return us_eastern_offset(utc),
            TzRule::Eu(std) => {
                let start = last_sunday(year, 3).and_time(hm(1, 0)).and_utc();
                let end = last_sunday(year, 10).and_time(hm(1, 0)).and_utc();
                std + i32::from(utc >= start && utc < end)
            }
            TzRule::AuEastern => {
                // Summer time ends in April and starts again in October.
                let end = (nth_weekday(year, 4, Weekday::Sun, 1) - Duration::days(1))
                    .and_time(hm(16, 0))
                    .and_utc();
                let start = (nth_weekday(year, 10, Weekday::Sun, 1) - Duration::days(1))
                    .and_time(hm(16, 0))
                    .and_utc();
                if utc < end || utc >= start { 11 } else { 10 }
            }
        };
        minutes_offset(hours * 60)
    }

    /// The offset in force on a local calendar date. Trading hours never
    /// fall in a changeover hour, so the date alone decides.
    fn offset_on_local_date(self, date: NaiveDate) -> FixedOffset {
        let year = date.year();
        let hours = match self {
            TzRule::Fixed(minutes) => return minutes_offset(minutes),
            TzRule::UsEastern => return us_offset_for_local_date(date),
            TzRule::Eu(std) => {
                std + i32::from(date >= last_sunday(year, 3) && date < last_sunday(year, 10))
            }
            TzRule::AuEastern => {
                let summer = date < nth_weekday(year, 4, Weekday::Sun, 1)
                    || date >= nth_weekday(year, 10, Weekday::Sun, 1);
                if summer { 11 } else { 10 }
            }
        };
        minutes_offset(hours * 60)
    }
}

/// An exchange's regular trading hours, in its local time.
#[derive(Debug, Clone, Copy)]
pub struct Session {
    pub tz: TzRule,
    /// Label after local times in messages ("09:00 JST").
    pub tz_label: &'static str,
    pub open: (u32, u32),
    pub close: (u32, u32),
    /// Midday break (Tokyo, Hong Kong, Singapore).
    pub lunch: Option<((u32, u32), (u32, u32))>,
    /// Close on the half days in the holiday table.
    pub half_day_close: Option<(u32, u32)>,
}

/// A bundled holiday calendar for one exchange.
#[derive(Debug, Clone, Copy)]
pub struct HolidayTable {
    /// Weekday closures: (year, month, day, name).
    pub closures: &'static [(i32, u32, u32, &'static str)],
    /// Trading days that close early, at the session's `half_day_close`.
    pub half_days: &'static [(i32, u32, u32)],
    /// Years whose rows are the exchange's published list.
    pub official_years: &'static [i32],
    /// Years with only fixed-date holidays filled in.
    pub provisional_years: &'static [i32],
}

/// NSE equity-segment trading holidays that fall on weekdays.
///
/// 2026: NSE circular of December 2025 (15 weekday holidays).
/// 2027: provisional — only the fixed-date national holidays that fall on a
/// weekday. NSE had not published the 2027 list when this table was written;
/// replace these rows with the official list when it is out.
pub const NSE_HOLIDAYS: &[(i32, u32, u32, &str)] = &[
    (2026, 1, 26, "Republic Day"),
    (2026, 3, 3, "Holi"),
    (2026, 3, 26, "Shri Ram Navami"),
    (2026, 3, 31, "Shri Mahavir Jayanti"),
    (2026, 4, 3, "Good Friday"),
    (2026, 4, 14, "Dr. Baba Saheb Ambedkar Jayanti"),
    (2026, 5, 1, "Maharashtra Day"),
    (2026, 5, 28, "Bakri Id"),
    (2026, 6, 26, "Muharram"),
    (2026, 9, 14, "Ganesh Chaturthi"),
    (2026, 10, 2, "Mahatma Gandhi Jayanti"),
    (2026, 10, 20, "Dussehra"),
    (2026, 11, 10, "Diwali Balipratipada"),
    (2026, 11, 24, "Prakash Gurpurb Sri Guru Nanak Dev"),
    (2026, 12, 25, "Christmas"),
    (2027, 1, 26, "Republic Day"),
    // Good Friday follows Easter (28 Mar 2027), not a lunar date, and NSE has
    // closed on it every year; NSE's own 2027 circular is not out yet (it
    // comes in December), so 2027 stays provisional.
    (2027, 3, 26, "Good Friday"),
    (2027, 4, 14, "Dr. Baba Saheb Ambedkar Jayanti"),
];

/// Years whose NSE list above is the official circular.
pub const NSE_OFFICIAL_YEARS: &[i32] = &[2026];
/// Years with only the fixed-date holidays filled in.
pub const NSE_PROVISIONAL_YEARS: &[i32] = &[2027];

/// NYSE full-day closures (NYSE Group calendar of December 2025).
pub const NYSE_HOLIDAYS: &[(i32, u32, u32, &str)] = &[
    (2026, 1, 1, "New Year's Day"),
    (2026, 1, 19, "Martin Luther King, Jr. Day"),
    (2026, 2, 16, "Washington's Birthday"),
    (2026, 4, 3, "Good Friday"),
    (2026, 5, 25, "Memorial Day"),
    (2026, 6, 19, "Juneteenth"),
    (2026, 7, 3, "Independence Day (observed)"),
    (2026, 9, 7, "Labor Day"),
    (2026, 11, 26, "Thanksgiving Day"),
    (2026, 12, 25, "Christmas Day"),
    (2027, 1, 1, "New Year's Day"),
    (2027, 1, 18, "Martin Luther King, Jr. Day"),
    (2027, 2, 15, "Washington's Birthday"),
    (2027, 3, 26, "Good Friday"),
    (2027, 5, 31, "Memorial Day"),
    (2027, 6, 18, "Juneteenth (observed)"),
    (2027, 7, 5, "Independence Day (observed)"),
    (2027, 9, 6, "Labor Day"),
    (2027, 11, 25, "Thanksgiving Day"),
    (2027, 12, 24, "Christmas Day (observed)"),
    (2028, 1, 17, "Martin Luther King, Jr. Day"),
    (2028, 2, 21, "Washington's Birthday"),
    (2028, 4, 14, "Good Friday"),
    (2028, 5, 29, "Memorial Day"),
    (2028, 6, 19, "Juneteenth"),
    (2028, 7, 4, "Independence Day"),
    (2028, 9, 4, "Labor Day"),
    (2028, 11, 23, "Thanksgiving Day"),
    (2028, 12, 25, "Christmas Day"),
];

/// NYSE days that close at 1:00 pm ET.
pub const NYSE_EARLY_CLOSES: &[(i32, u32, u32)] = &[
    (2026, 11, 27),
    (2026, 12, 24),
    (2027, 11, 26),
    (2028, 7, 3),
    (2028, 11, 24),
];

pub const NYSE_OFFICIAL_YEARS: &[i32] = &[2026, 2027, 2028];

// ── Global exchanges ──
//
// Weekday closures and half days, filled in by the market table's
// `holidays` field. Years not listed in `official_years` fall back to
// weekdays-only with a coverage warning.

/// London Stock Exchange: England & Wales bank holidays; 24 and 31 December
/// close at 12:30. LSE business-days page and settlement calendar.
pub const LSE: HolidayTable = HolidayTable {
    closures: &[
        (2026, 1, 1, "New Year's Day"),
        (2026, 4, 3, "Good Friday"),
        (2026, 4, 6, "Easter Monday"),
        (2026, 5, 4, "Early May Bank Holiday"),
        (2026, 5, 25, "Spring Bank Holiday"),
        (2026, 8, 31, "Summer Bank Holiday"),
        (2026, 12, 25, "Christmas Day"),
        (2026, 12, 28, "Boxing Day (substitute)"),
        (2027, 1, 1, "New Year's Day"),
        (2027, 3, 26, "Good Friday"),
        (2027, 3, 29, "Easter Monday"),
        (2027, 5, 3, "Early May Bank Holiday"),
        (2027, 5, 31, "Spring Bank Holiday"),
        (2027, 8, 30, "Summer Bank Holiday"),
        (2027, 12, 27, "Christmas Day (substitute)"),
        (2027, 12, 28, "Boxing Day (substitute)"),
    ],
    half_days: &[
        (2026, 12, 24),
        (2026, 12, 31),
        (2027, 12, 24),
        (2027, 12, 31),
    ],
    official_years: &[2026, 2027],
    provisional_years: &[],
};

/// Deutsche Börse Xetra trading calendar. Christmas Eve and New Year's Eve
/// are full closures; German regional holidays (Whit Monday, ...) trade.
pub const XETRA: HolidayTable = HolidayTable {
    closures: &[
        (2026, 1, 1, "New Year's Day"),
        (2026, 4, 3, "Good Friday"),
        (2026, 4, 6, "Easter Monday"),
        (2026, 5, 1, "Labour Day"),
        (2026, 12, 24, "Christmas Eve"),
        (2026, 12, 25, "Christmas Day"),
        (2026, 12, 31, "New Year's Eve"),
        (2027, 1, 1, "New Year's Day"),
        (2027, 3, 26, "Good Friday"),
        (2027, 3, 29, "Easter Monday"),
        (2027, 12, 24, "Christmas Eve"),
        (2027, 12, 31, "New Year's Eve"),
    ],
    half_days: &[],
    official_years: &[2026, 2027],
    provisional_years: &[],
};

/// Japan Exchange Group holiday calendar (national holidays, 2–3 January and
/// 31 December). No half days.
pub const JPX: HolidayTable = HolidayTable {
    closures: &[
        (2026, 1, 1, "New Year's Day"),
        (2026, 1, 2, "Market Holiday"),
        (2026, 1, 12, "Coming of Age Day"),
        (2026, 2, 11, "National Foundation Day"),
        (2026, 2, 23, "Emperor's Birthday"),
        (2026, 3, 20, "Vernal Equinox Day"),
        (2026, 4, 29, "Showa Day"),
        (2026, 5, 4, "Greenery Day"),
        (2026, 5, 5, "Children's Day"),
        (2026, 5, 6, "Constitution Memorial Day (observed)"),
        (2026, 7, 20, "Marine Day"),
        (2026, 8, 11, "Mountain Day"),
        (2026, 9, 21, "Respect for the Aged Day"),
        (2026, 9, 22, "Citizens' Holiday"),
        (2026, 9, 23, "Autumnal Equinox Day"),
        (2026, 10, 12, "Sports Day"),
        (2026, 11, 3, "Culture Day"),
        (2026, 11, 23, "Labour Thanksgiving Day"),
        (2026, 12, 31, "Market Holiday"),
        (2027, 1, 1, "New Year's Day"),
        (2027, 1, 11, "Coming of Age Day"),
        (2027, 2, 11, "National Foundation Day"),
        (2027, 2, 23, "Emperor's Birthday"),
        (2027, 3, 22, "Vernal Equinox Day (observed)"),
        (2027, 4, 29, "Showa Day"),
        (2027, 5, 3, "Constitution Memorial Day"),
        (2027, 5, 4, "Greenery Day"),
        (2027, 5, 5, "Children's Day"),
        (2027, 7, 19, "Marine Day"),
        (2027, 8, 11, "Mountain Day"),
        (2027, 9, 20, "Respect for the Aged Day"),
        (2027, 9, 23, "Autumnal Equinox Day"),
        (2027, 10, 11, "Sports Day"),
        (2027, 11, 3, "Culture Day"),
        (2027, 11, 23, "Labour Thanksgiving Day"),
        (2027, 12, 31, "Market Holiday"),
    ],
    half_days: &[],
    official_years: &[2026, 2027],
    provisional_years: &[],
};

/// Hong Kong Exchanges: SEHK circulars CT/075/25 (2026) and CT/077/26
/// (2027). The eves of Lunar New Year, Christmas and New Year are morning-only.
pub const HKEX: HolidayTable = HolidayTable {
    closures: &[
        (2026, 1, 1, "New Year's Day"),
        (2026, 2, 17, "Lunar New Year's Day"),
        (2026, 2, 18, "Second day of Lunar New Year"),
        (2026, 2, 19, "Third day of Lunar New Year"),
        (2026, 4, 3, "Good Friday"),
        (2026, 4, 6, "Day following Ching Ming Festival"),
        (2026, 4, 7, "Day following Easter Monday"),
        (2026, 5, 1, "Labour Day"),
        (2026, 5, 25, "Day following the Birthday of the Buddha"),
        (2026, 6, 19, "Tuen Ng Festival"),
        (2026, 7, 1, "HKSAR Establishment Day"),
        (2026, 10, 1, "National Day"),
        (2026, 10, 19, "Day following Chung Yeung Festival"),
        (2026, 12, 25, "Christmas Day"),
        (2027, 1, 1, "New Year's Day"),
        (2027, 2, 8, "Third day of Lunar New Year"),
        (2027, 2, 9, "Fourth day of Lunar New Year"),
        (2027, 3, 26, "Good Friday"),
        (2027, 3, 29, "Easter Monday"),
        (2027, 4, 5, "Ching Ming Festival"),
        (2027, 5, 13, "Birthday of the Buddha"),
        (2027, 6, 9, "Tuen Ng Festival"),
        (2027, 7, 1, "HKSAR Establishment Day"),
        (2027, 9, 16, "Day following Mid-Autumn Festival"),
        (2027, 10, 1, "National Day"),
        (2027, 10, 8, "Chung Yeung Festival"),
        (2027, 12, 27, "First weekday after Christmas Day"),
    ],
    half_days: &[
        (2026, 2, 16),
        (2026, 12, 24),
        (2026, 12, 31),
        (2027, 2, 5),
        (2027, 12, 24),
        (2027, 12, 31),
    ],
    official_years: &[2026, 2027],
    provisional_years: &[],
};

/// Singapore Exchange: it follows the Ministry of Manpower's public
/// holidays (2026 cross-checked against SGX's monthly trading-day counts).
/// 2027 is MOM's list; SGX had not published its own or the 2027 half days.
pub const SGX: HolidayTable = HolidayTable {
    closures: &[
        (2026, 1, 1, "New Year's Day"),
        (2026, 2, 17, "Chinese New Year"),
        (2026, 2, 18, "Chinese New Year"),
        (2026, 4, 3, "Good Friday"),
        (2026, 5, 1, "Labour Day"),
        (2026, 5, 27, "Hari Raya Haji"),
        (2026, 6, 1, "Vesak Day (observed)"),
        (2026, 8, 10, "National Day (observed)"),
        (2026, 11, 9, "Deepavali (observed)"),
        (2026, 12, 25, "Christmas Day"),
        (2027, 1, 1, "New Year's Day"),
        (2027, 2, 8, "Chinese New Year (observed)"),
        (2027, 3, 10, "Hari Raya Puasa"),
        (2027, 3, 26, "Good Friday"),
        (2027, 5, 17, "Hari Raya Haji"),
        (2027, 5, 20, "Vesak Day"),
        (2027, 8, 9, "National Day"),
        (2027, 10, 28, "Deepavali"),
    ],
    half_days: &[(2026, 2, 16), (2026, 12, 24), (2026, 12, 31)],
    official_years: &[2026],
    provisional_years: &[2027],
};

/// Toronto Stock Exchange (TMX holiday notices). Christmas Eve closes at
/// 13:00 ET.
///
/// 2026 is TMX's published list. 2027 is provisional: TMX had not published
/// it (no 2027 "TMX Group Holiday Operating Schedule" notice existed in Nov
/// 2026; TMX issues it each December, https://www.tmx.com/en/newsroom), so
/// the rows follow the holidays TMX observes every year: New Year's Day,
/// Family Day (3rd Monday of February), Good Friday (Easter is 28 Mar 2027),
/// Victoria Day (Monday before 25 May), Canada Day, Civic Holiday (1st Monday
/// of August), Labour Day, Thanksgiving (2nd Monday of October), and Christmas
/// and Boxing Day moved to the next weekdays because 25 and 26 Dec 2027 are a
/// Saturday and a Sunday. Check against TMX's notice and move 2027 to
/// `official_years` (see the module docs).
pub const TSX: HolidayTable = HolidayTable {
    closures: &[
        (2026, 1, 1, "New Year's Day"),
        (2026, 2, 16, "Family Day"),
        (2026, 4, 3, "Good Friday"),
        (2026, 5, 18, "Victoria Day"),
        (2026, 7, 1, "Canada Day"),
        (2026, 8, 3, "Civic Holiday"),
        (2026, 9, 7, "Labour Day"),
        (2026, 10, 12, "Thanksgiving Day"),
        (2026, 12, 25, "Christmas Day"),
        (2026, 12, 28, "Boxing Day (observed)"),
        (2027, 1, 1, "New Year's Day"),
        (2027, 2, 15, "Family Day"),
        (2027, 3, 26, "Good Friday"),
        (2027, 5, 24, "Victoria Day"),
        (2027, 7, 1, "Canada Day"),
        (2027, 8, 2, "Civic Holiday"),
        (2027, 9, 6, "Labour Day"),
        (2027, 10, 11, "Thanksgiving Day"),
        (2027, 12, 27, "Christmas Day (observed)"),
        (2027, 12, 28, "Boxing Day (observed)"),
    ],
    half_days: &[(2026, 12, 24), (2027, 12, 24)],
    official_years: &[2026],
    provisional_years: &[2027],
};

/// Australian Securities Exchange trading calendar. 24 and 31 December
/// stop normal trading at 14:10 Sydney time.
pub const ASX: HolidayTable = HolidayTable {
    closures: &[
        (2026, 1, 1, "New Year's Day"),
        (2026, 1, 26, "Australia Day"),
        (2026, 4, 3, "Good Friday"),
        (2026, 4, 6, "Easter Monday"),
        (2026, 6, 8, "King's Birthday"),
        (2026, 12, 25, "Christmas Day"),
        (2026, 12, 28, "Boxing Day (substitute)"),
        (2027, 1, 1, "New Year's Day"),
        (2027, 1, 26, "Australia Day"),
        (2027, 3, 26, "Good Friday"),
        (2027, 3, 29, "Easter Monday"),
        (2027, 6, 14, "King's Birthday"),
        (2027, 12, 27, "Christmas Day (substitute)"),
        (2027, 12, 28, "Boxing Day (substitute)"),
    ],
    half_days: &[
        (2026, 12, 24),
        (2026, 12, 31),
        (2027, 12, 24),
        (2027, 12, 31),
    ],
    official_years: &[2026, 2027],
    provisional_years: &[],
};

/// How much of a year's holiday calendar is bundled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarCoverage {
    /// The exchange's published list.
    Official,
    /// Not yet the exchange's published list: only fixed-date holidays
    /// (NSE), or the public holidays it follows (SGX).
    Provisional,
    /// No list: every weekday is treated as a trading day.
    Missing,
}

pub fn calendar_coverage(market: Market, year: i32) -> CalendarCoverage {
    let (official, provisional): (&[i32], &[i32]) = match market {
        Market::In => (NSE_OFFICIAL_YEARS, NSE_PROVISIONAL_YEARS),
        Market::Us => (NYSE_OFFICIAL_YEARS, &[]),
        Market::Global(x) => {
            let table = x.info().holidays;
            (table.official_years, table.provisional_years)
        }
        // No exchange holidays to track (COMEX gold and silver trade
        // nearly around the clock on weekdays).
        Market::Crypto | Market::Fx | Market::Commodity => return CalendarCoverage::Official,
    };
    if official.contains(&year) {
        CalendarCoverage::Official
    } else if provisional.contains(&year) {
        CalendarCoverage::Provisional
    } else {
        CalendarCoverage::Missing
    }
}

/// A one-line warning when `year`'s holiday list is incomplete, else None.
pub fn coverage_warning(market: Market, year: i32) -> Option<String> {
    match calendar_coverage(market, year) {
        CalendarCoverage::Official => None,
        CalendarCoverage::Provisional => Some(format!(
            "The {} holiday list for {} is provisional (not yet the exchange's own list) — \
             upgrade StockWise once the exchange publishes it.",
            exchange(market),
            year
        )),
        CalendarCoverage::Missing => Some(format!(
            "No {} holiday list for {} is bundled — every weekday is treated as a \
             trading day. Upgrade StockWise for the current calendar.",
            exchange(market),
            year
        )),
    }
}

/// Exchange name used in messages.
pub fn exchange(market: Market) -> &'static str {
    match market {
        Market::In => "NSE",
        Market::Us => "NYSE",
        Market::Crypto => "crypto (24/7)",
        Market::Fx => "FX (24x5)",
        Market::Commodity => "commodities (24x5)",
        Market::Global(x) => x.info().short,
    }
}

/// Time-zone label used in messages.
pub fn tz_label(market: Market) -> &'static str {
    match market {
        Market::In => "IST",
        Market::Us => "ET",
        Market::Crypto | Market::Fx | Market::Commodity => "UTC",
        Market::Global(x) => x.info().session.tz_label,
    }
}

fn utc_offset() -> FixedOffset {
    FixedOffset::east_opt(0).expect("valid offset")
}

fn minutes_offset(minutes: i32) -> FixedOffset {
    FixedOffset::east_opt(minutes * 60).expect("valid offset")
}

pub fn ist() -> FixedOffset {
    FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid offset")
}

fn hm(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).expect("valid time")
}

/// The `n`th (1-based) `weekday` of `month`.
fn nth_weekday(year: i32, month: u32, weekday: Weekday, n: u32) -> NaiveDate {
    let first = NaiveDate::from_ymd_opt(year, month, 1).expect("valid date");
    let shift = (7 + weekday.num_days_from_monday() - first.weekday().num_days_from_monday()) % 7;
    first + Duration::days((shift + 7 * (n - 1)) as i64)
}

/// The last Sunday of `month`.
fn last_sunday(year: i32, month: u32) -> NaiveDate {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .expect("valid date");
    let last = next - Duration::days(1);
    last - Duration::days(last.weekday().num_days_from_sunday() as i64)
}

/// Whether US daylight saving time is in effect on `date` (local), using the
/// rule in force since 2007. Market hours never fall in the 02:00 changeover
/// hour, so the date alone decides.
fn us_dst_on(date: NaiveDate) -> bool {
    let start = nth_weekday(date.year(), 3, Weekday::Sun, 2);
    let end = nth_weekday(date.year(), 11, Weekday::Sun, 1);
    date >= start && date < end
}

fn us_offset_for_local_date(date: NaiveDate) -> FixedOffset {
    let hours = if us_dst_on(date) { -4 } else { -5 };
    FixedOffset::east_opt(hours * 3600).expect("valid offset")
}

/// US Eastern offset at a UTC instant (EDT −4 / EST −5).
pub fn us_eastern_offset(utc: DateTime<Utc>) -> FixedOffset {
    let year = utc.year();
    // Changes happen at 02:00 local: 07:00 UTC in March (EST), 06:00 UTC in
    // November (EDT).
    let start = nth_weekday(year, 3, Weekday::Sun, 2)
        .and_time(hm(7, 0))
        .and_utc();
    let end = nth_weekday(year, 11, Weekday::Sun, 1)
        .and_time(hm(6, 0))
        .and_utc();
    let hours = if utc >= start && utc < end { -4 } else { -5 };
    FixedOffset::east_opt(hours * 3600).expect("valid offset")
}

/// The exchange's UTC offset at an instant.
pub fn offset(market: Market, utc: DateTime<Utc>) -> FixedOffset {
    match market {
        Market::In => ist(),
        Market::Us => us_eastern_offset(utc),
        Market::Crypto | Market::Fx | Market::Commodity => utc_offset(),
        Market::Global(x) => x.info().session.tz.offset_at(utc),
    }
}

/// `utc` in the exchange's local time.
pub fn local(market: Market, utc: DateTime<Utc>) -> DateTime<FixedOffset> {
    utc.with_timezone(&offset(market, utc))
}

/// The exchange-local calendar date at `utc` — the date a session belongs to.
pub fn session_date(market: Market, utc: DateTime<Utc>) -> NaiveDate {
    local(market, utc).date_naive()
}

/// Exchange-local wall time on `date` as a UTC instant.
pub fn at(market: Market, date: NaiveDate, time: NaiveTime) -> DateTime<Utc> {
    let off = match market {
        Market::In => ist(),
        Market::Us => us_offset_for_local_date(date),
        Market::Crypto | Market::Fx | Market::Commodity => utc_offset(),
        Market::Global(x) => x.info().session.tz.offset_on_local_date(date),
    };
    off.from_local_datetime(&date.and_time(time))
        .single()
        .expect("fixed offsets are unambiguous")
        .with_timezone(&Utc)
}

fn table(market: Market) -> &'static [(i32, u32, u32, &'static str)] {
    match market {
        Market::In => NSE_HOLIDAYS,
        Market::Us => NYSE_HOLIDAYS,
        Market::Global(x) => x.info().holidays.closures,
        Market::Crypto | Market::Fx | Market::Commodity => &[],
    }
}

fn on(rows: &[(i32, u32, u32)], date: NaiveDate) -> bool {
    rows.iter()
        .any(|(y, m, d)| *y == date.year() && *m == date.month() && *d == date.day())
}

/// The holiday's name if the exchange is closed for a holiday on `date`.
pub fn holiday(market: Market, date: NaiveDate) -> Option<&'static str> {
    table(market)
        .iter()
        .find(|(y, m, d, _)| *y == date.year() && *m == date.month() && *d == date.day())
        .map(|(_, _, _, name)| *name)
}

/// A weekday that is not an exchange holiday. Crypto trades every day.
pub fn is_trading_day(market: Market, date: NaiveDate) -> bool {
    if market == Market::Crypto {
        return true;
    }
    !matches!(date.weekday(), Weekday::Sat | Weekday::Sun) && holiday(market, date).is_none()
}

/// Regular-session open and close (exchange-local) on a trading day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hours {
    pub open: NaiveTime,
    pub close: NaiveTime,
}

/// Session hours on `date`, or None when the exchange is closed all day.
pub fn hours(market: Market, date: NaiveDate) -> Option<Hours> {
    if !is_trading_day(market, date) {
        return None;
    }
    Some(match market {
        Market::In => Hours {
            open: hm(9, 15),
            close: hm(15, 30),
        },
        // No sessions: the whole (UTC) day counts as open.
        Market::Crypto | Market::Fx | Market::Commodity => Hours {
            open: hm(0, 0),
            close: hm(23, 59),
        },
        Market::Us => {
            let early = on(NYSE_EARLY_CLOSES, date);
            Hours {
                open: hm(9, 30),
                close: if early { hm(13, 0) } else { hm(16, 0) },
            }
        }
        Market::Global(x) => {
            let info = x.info();
            let s = info.session;
            let close = match s.half_day_close {
                Some((h, m)) if on(info.holidays.half_days, date) => hm(h, m),
                _ => hm(s.close.0, s.close.1),
            };
            Hours {
                open: hm(s.open.0, s.open.1),
                close,
            }
        }
    })
}

/// The midday break on `date` (Tokyo, Hong Kong, Singapore), if the
/// exchange has one and is still trading after it.
pub fn lunch_break(market: Market, date: NaiveDate) -> Option<(NaiveTime, NaiveTime)> {
    let h = hours(market, date)?;
    let ((sh, sm), (eh, em)) = market.info()?.session.lunch?;
    let (start, end) = (hm(sh, sm), hm(eh, em));
    // A half day that closes at the break has no afternoon session.
    (h.close > start).then_some((start, end))
}

/// Bot phase at `utc` (see [`Phase`]).
pub fn market_phase(market: Market, utc: DateTime<Utc>) -> Phase {
    let now = local(market, utc);
    let Some(h) = hours(market, now.date_naive()) else {
        return Phase::Closed;
    };
    let t = now.time();
    let m = |mins: i64| Duration::minutes(mins);
    if t < h.open - m(15) || t >= h.close + m(30) {
        Phase::Closed
    } else if t < h.open {
        Phase::PreMarket
    } else if t < h.open + m(15) {
        Phase::Opening
    } else if t < h.close - m(90) {
        Phase::Active
    } else if t < h.close - m(15) {
        Phase::WindDown
    } else if t < h.close - m(10) {
        Phase::SquareOff
    } else {
        Phase::PostMarket
    }
}

/// True during the regular session (open ≤ t < close), outside any midday
/// break.
pub fn is_open(market: Market, utc: DateTime<Utc>) -> bool {
    let now = local(market, utc);
    let date = now.date_naive();
    let t = now.time();
    let in_lunch = lunch_break(market, date).is_some_and(|(start, end)| t >= start && t < end);
    hours(market, date).is_some_and(|h| t >= h.open && t < h.close) && !in_lunch
}

/// When the regular session closes on `date` (UTC), if it trades.
pub fn close_at(market: Market, date: NaiveDate) -> Option<DateTime<Utc>> {
    hours(market, date).map(|h| at(market, date, h.close))
}

/// When positions are squared off on `date`: 15 minutes before the close.
pub fn square_off_at(market: Market, date: NaiveDate) -> Option<DateTime<Utc>> {
    hours(market, date).map(|h| at(market, date, h.close - Duration::minutes(15)))
}

/// The next regular-session open at or after `utc` (today's if it is still
/// ahead). Looks up to 30 days ahead, far longer than any exchange closure.
pub fn next_open(market: Market, utc: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let today = session_date(market, utc);
    (0..30).find_map(|i| {
        let date = today + Duration::days(i);
        let open = at(market, date, hours(market, date)?.open);
        (open >= utc).then_some(open)
    })
}

/// "Wed 21 Oct 09:15 IST" for messages.
pub fn format_local(market: Market, utc: DateTime<Utc>) -> String {
    format!(
        "{} {}",
        local(market, utc).format("%a %d %b %H:%M"),
        tz_label(market)
    )
}

/// Why the exchange is closed at `utc`, for messages: "Dussehra",
/// "weekend", "lunch break", or "outside market hours".
pub fn closed_reason(market: Market, utc: DateTime<Utc>) -> String {
    let now = local(market, utc);
    let date = now.date_naive();
    if let Some(name) = holiday(market, date) {
        format!("holiday: {}", name)
    } else if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
        "weekend".into()
    } else if lunch_break(market, date)
        .is_some_and(|(start, end)| now.time() >= start && now.time() < end)
    {
        "lunch break".into()
    } else {
        "outside market hours".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn holiday_tables_list_weekdays_in_covered_years() {
        for (market, rows) in [(Market::In, NSE_HOLIDAYS), (Market::Us, NYSE_HOLIDAYS)] {
            for (y, m, d, name) in rows {
                let day = date(*y, *m, *d);
                assert!(
                    !matches!(day.weekday(), Weekday::Sat | Weekday::Sun),
                    "{day} {name} is a weekend"
                );
                assert_ne!(
                    calendar_coverage(market, *y),
                    CalendarCoverage::Missing,
                    "{day} is in an uncovered year"
                );
            }
        }
        for (y, m, d) in NYSE_EARLY_CLOSES {
            let day = date(*y, *m, *d);
            assert!(
                is_trading_day(Market::Us, day),
                "{day} early close on a closed day"
            );
        }
    }

    #[test]
    fn nse_2026_has_fifteen_weekday_holidays() {
        let n = NSE_HOLIDAYS.iter().filter(|(y, ..)| *y == 2026).count();
        assert_eq!(n, 15);
        assert_eq!(holiday(Market::In, date(2026, 10, 20)), Some("Dussehra"));
        assert!(!is_trading_day(Market::In, date(2026, 10, 20)));
        assert!(is_trading_day(Market::In, date(2026, 10, 21)));
        assert!(!is_trading_day(Market::In, date(2026, 10, 24))); // Saturday
    }

    #[test]
    fn ist_is_independent_of_the_local_zone() {
        // 03:45 UTC = 09:15 IST, the NSE open.
        let t = utc("2026-10-21T03:45:00Z");
        assert_eq!(local(Market::In, t).time(), hm(9, 15));
        assert!(is_open(Market::In, t));
        assert!(!is_open(Market::In, utc("2026-10-21T03:44:59Z")));
        assert!(!is_open(Market::In, utc("2026-10-21T10:00:00Z"))); // 15:30 IST
        // 20:00 UTC on the 20th is already the 21st in India.
        assert_eq!(
            session_date(Market::In, utc("2026-10-20T20:00:00Z")),
            date(2026, 10, 21)
        );
    }

    #[test]
    fn nse_phases_follow_the_bot_schedule() {
        let at_ist = |hh: u32, mm: u32| at(Market::In, date(2026, 10, 21), hm(hh, mm));
        assert_eq!(market_phase(Market::In, at_ist(8, 59)), Phase::Closed);
        assert_eq!(market_phase(Market::In, at_ist(9, 0)), Phase::PreMarket);
        assert_eq!(market_phase(Market::In, at_ist(9, 15)), Phase::Opening);
        assert_eq!(market_phase(Market::In, at_ist(9, 30)), Phase::Active);
        assert_eq!(market_phase(Market::In, at_ist(13, 59)), Phase::Active);
        assert_eq!(market_phase(Market::In, at_ist(14, 0)), Phase::WindDown);
        assert_eq!(market_phase(Market::In, at_ist(15, 15)), Phase::SquareOff);
        assert_eq!(market_phase(Market::In, at_ist(15, 20)), Phase::PostMarket);
        assert_eq!(market_phase(Market::In, at_ist(15, 59)), Phase::PostMarket);
        assert_eq!(market_phase(Market::In, at_ist(16, 0)), Phase::Closed);
    }

    #[test]
    fn holidays_are_closed_all_day() {
        let t = at(Market::In, date(2026, 10, 20), hm(11, 0));
        assert_eq!(market_phase(Market::In, t), Phase::Closed);
        assert_eq!(closed_reason(Market::In, t), "holiday: Dussehra");
        let sat = at(Market::In, date(2026, 10, 24), hm(11, 0));
        assert_eq!(closed_reason(Market::In, sat), "weekend");
    }

    #[test]
    fn next_open_skips_weekends_and_holidays() {
        // Monday 19 Oct after the close → Tuesday is Dussehra → Wednesday 09:15.
        let t = at(Market::In, date(2026, 10, 19), hm(16, 0));
        let open = next_open(Market::In, t).unwrap();
        assert_eq!(open, at(Market::In, date(2026, 10, 21), hm(9, 15)));
        assert_eq!(format_local(Market::In, open), "Wed 21 Oct 09:15 IST");
        // Friday evening → Monday.
        let fri = at(Market::In, date(2026, 10, 23), hm(18, 0));
        assert_eq!(
            next_open(Market::In, fri).unwrap(),
            at(Market::In, date(2026, 10, 26), hm(9, 15))
        );
        // Before today's open → today.
        let early = at(Market::In, date(2026, 10, 21), hm(7, 0));
        assert_eq!(
            next_open(Market::In, early).unwrap(),
            at(Market::In, date(2026, 10, 21), hm(9, 15))
        );
    }

    #[test]
    fn us_eastern_tracks_daylight_saving() {
        // 2026: DST from Sun 8 Mar to Sun 1 Nov.
        assert_eq!(nth_weekday(2026, 3, Weekday::Sun, 2), date(2026, 3, 8));
        assert_eq!(nth_weekday(2026, 11, Weekday::Sun, 1), date(2026, 11, 1));
        // Summer: 13:30 UTC = 09:30 EDT.
        let summer = utc("2026-07-01T13:30:00Z");
        assert_eq!(local(Market::Us, summer).time(), hm(9, 30));
        assert!(is_open(Market::Us, summer));
        // Winter: 14:30 UTC = 09:30 EST.
        let winter = utc("2026-01-15T14:30:00Z");
        assert_eq!(local(Market::Us, winter).time(), hm(9, 30));
        assert!(is_open(Market::Us, winter));
        assert!(!is_open(Market::Us, utc("2026-01-15T13:30:00Z")));
        // Changeover instants.
        assert_eq!(
            us_eastern_offset(utc("2026-03-08T06:59:00Z")).local_minus_utc(),
            -5 * 3600
        );
        assert_eq!(
            us_eastern_offset(utc("2026-03-08T07:00:00Z")).local_minus_utc(),
            -4 * 3600
        );
        assert_eq!(
            us_eastern_offset(utc("2026-11-01T05:59:00Z")).local_minus_utc(),
            -4 * 3600
        );
        assert_eq!(
            us_eastern_offset(utc("2026-11-01T06:00:00Z")).local_minus_utc(),
            -5 * 3600
        );
    }

    #[test]
    fn nyse_holidays_and_early_closes() {
        assert!(!is_trading_day(Market::Us, date(2026, 11, 26)));
        let h = hours(Market::Us, date(2026, 11, 27)).unwrap();
        assert_eq!(h.close, hm(13, 0));
        // 12:50 ET on an early-close day is already post-market.
        let t = at(Market::Us, date(2026, 11, 27), hm(12, 50));
        assert_eq!(market_phase(Market::Us, t), Phase::PostMarket);
        let normal = at(Market::Us, date(2026, 11, 30), hm(12, 50));
        assert_eq!(market_phase(Market::Us, normal), Phase::Active);
        assert_eq!(
            square_off_at(Market::Us, date(2026, 11, 30)).unwrap(),
            at(Market::Us, date(2026, 11, 30), hm(15, 45))
        );
    }

    fn global(x: crate::market::Exchange) -> Market {
        Market::Global(x)
    }

    #[test]
    fn european_summer_time() {
        use crate::market::Exchange::{De, Uk};
        // 2026: last Sundays are 29 March and 25 October.
        assert_eq!(last_sunday(2026, 3), date(2026, 3, 29));
        assert_eq!(last_sunday(2026, 10), date(2026, 10, 25));
        assert_eq!(last_sunday(2026, 12), date(2026, 12, 27));
        // London opens at 08:00: 07:00 UTC in summer (BST), 08:00 in winter.
        assert!(is_open(global(Uk), utc("2026-07-01T07:00:00Z")));
        assert!(!is_open(global(Uk), utc("2026-07-01T06:59:00Z")));
        assert!(is_open(global(Uk), utc("2026-01-15T08:00:00Z")));
        assert!(!is_open(global(Uk), utc("2026-01-15T07:59:00Z")));
        // Frankfurt is an hour ahead: 09:00 CEST = 07:00 UTC.
        assert_eq!(
            local(global(De), utc("2026-07-01T07:00:00Z")).time(),
            hm(9, 0)
        );
        assert_eq!(
            local(global(De), utc("2026-01-15T08:00:00Z")).time(),
            hm(9, 0)
        );
        // The switch happens at 01:00 UTC.
        let off = |s| offset(global(Uk), utc(s)).local_minus_utc();
        assert_eq!(off("2026-03-29T00:59:00Z"), 0);
        assert_eq!(off("2026-03-29T01:00:00Z"), 3600);
        assert_eq!(off("2026-10-25T00:59:00Z"), 3600);
        assert_eq!(off("2026-10-25T01:00:00Z"), 0);
        // Local wall time → UTC on a summer date.
        assert_eq!(
            at(global(Uk), date(2026, 7, 1), hm(16, 30)),
            utc("2026-07-01T15:30:00Z")
        );
    }

    #[test]
    fn asian_sessions_have_lunch_breaks() {
        use crate::market::Exchange::{Hk, Jp, Sg};
        // Tokyo: 09:00–11:30 and 12:30–15:30 JST (UTC+9).
        assert!(is_open(global(Jp), utc("2026-10-21T00:00:00Z")));
        let lunch = utc("2026-10-21T02:45:00Z"); // 11:45 JST
        assert!(!is_open(global(Jp), lunch));
        assert_eq!(closed_reason(global(Jp), lunch), "lunch break");
        assert!(is_open(global(Jp), utc("2026-10-21T03:30:00Z"))); // 12:30
        assert!(!is_open(global(Jp), utc("2026-10-21T06:30:00Z"))); // 15:30
        assert_eq!(
            format_local(global(Jp), utc("2026-10-21T00:00:00Z")),
            "Wed 21 Oct 09:00 JST"
        );
        // Hong Kong: 12:00–13:00 HKT break.
        assert!(!is_open(global(Hk), utc("2026-10-21T04:30:00Z")));
        assert!(is_open(global(Hk), utc("2026-10-21T05:00:00Z")));
        // Singapore trades until 17:00 SGT.
        assert!(is_open(global(Sg), utc("2026-10-21T08:30:00Z")));
        assert!(!is_open(global(Sg), utc("2026-10-21T09:00:00Z")));
        // No break in London.
        assert_eq!(
            lunch_break(global(crate::market::Exchange::Uk), date(2026, 10, 21)),
            None
        );
    }

    #[test]
    fn toronto_and_sydney_time_zones() {
        use crate::market::Exchange::{Au, Ca};
        // Toronto follows US Eastern: 09:30 EDT = 13:30 UTC.
        assert!(is_open(global(Ca), utc("2026-07-02T13:30:00Z")));
        assert!(!is_open(global(Ca), utc("2026-07-02T13:29:00Z")));
        // Sydney: AEDT (+11) in January, AEST (+10) in July.
        assert_eq!(
            offset(global(Au), utc("2026-01-15T00:00:00Z")).local_minus_utc(),
            11 * 3600
        );
        assert_eq!(
            offset(global(Au), utc("2026-07-15T00:00:00Z")).local_minus_utc(),
            10 * 3600
        );
        // 2026 changes: Sat 4 Apr 16:00 UTC (back to +10), Sat 3 Oct 16:00
        // UTC (forward to +11).
        let off = |s| offset(global(Au), utc(s)).local_minus_utc() / 3600;
        assert_eq!(off("2026-04-04T15:59:00Z"), 11);
        assert_eq!(off("2026-04-04T16:00:00Z"), 10);
        assert_eq!(off("2026-10-03T15:59:00Z"), 10);
        assert_eq!(off("2026-10-03T16:00:00Z"), 11);
        // 10:00 AEDT on Thu 15 Jan is 23:00 UTC on the 14th.
        assert_eq!(
            at(global(Au), date(2026, 1, 15), hm(10, 0)),
            utc("2026-01-14T23:00:00Z")
        );
        assert!(is_open(global(Au), utc("2026-01-14T23:00:00Z")));
        assert_eq!(
            session_date(global(Au), utc("2026-01-14T23:00:00Z")),
            date(2026, 1, 15)
        );
    }

    #[test]
    fn global_holiday_tables_are_weekdays_in_covered_years() {
        for x in crate::market::Exchange::ALL {
            let info = x.info();
            let market = Market::Global(x);
            let table = info.holidays;
            for (y, m, d, name) in table.closures {
                let day = date(*y, *m, *d);
                assert!(
                    !matches!(day.weekday(), Weekday::Sat | Weekday::Sun),
                    "{} {day} {name} is a weekend",
                    info.code
                );
                assert_ne!(
                    calendar_coverage(market, *y),
                    CalendarCoverage::Missing,
                    "{} {day} is in an uncovered year",
                    info.code
                );
                assert!(!is_trading_day(market, day));
            }
            for (y, m, d) in table.half_days {
                let day = date(*y, *m, *d);
                assert!(is_trading_day(market, day), "{} half day {day}", info.code);
                assert!(
                    info.session.half_day_close.is_some(),
                    "{} lists half days without a close time",
                    info.code
                );
            }
            // Every exchange has a list for the current year.
            assert_ne!(
                calendar_coverage(market, 2026),
                CalendarCoverage::Missing,
                "{}",
                info.code
            );
        }
    }

    #[test]
    fn global_holidays_and_half_days() {
        use crate::market::Exchange::{Au, Hk, Jp, Sg, Uk};
        assert_eq!(
            holiday(global(Jp), date(2026, 9, 22)),
            Some("Citizens' Holiday")
        );
        assert!(!is_trading_day(global(Uk), date(2026, 8, 31)));
        assert!(is_trading_day(global(Uk), date(2026, 9, 1)));
        // Half days: London closes at 12:30, Hong Kong at noon with no
        // afternoon session, Sydney at 14:10.
        assert_eq!(
            hours(global(Uk), date(2026, 12, 24)).unwrap().close,
            hm(12, 30)
        );
        assert_eq!(
            hours(global(Uk), date(2026, 12, 23)).unwrap().close,
            hm(16, 30)
        );
        assert_eq!(lunch_break(global(Hk), date(2026, 12, 24)), None);
        assert!(!is_open(
            global(Hk),
            at(global(Hk), date(2026, 12, 24), hm(13, 30))
        ));
        assert!(is_open(
            global(Hk),
            at(global(Hk), date(2026, 12, 23), hm(13, 30))
        ));
        assert_eq!(
            hours(global(Au), date(2026, 12, 31)).unwrap().close,
            hm(14, 10)
        );
        // SGX 2027 is provisional; years without a list warn.
        assert_eq!(
            calendar_coverage(global(Sg), 2027),
            CalendarCoverage::Provisional
        );
        assert!(coverage_warning(global(Sg), 2027).unwrap().contains("SGX"));
        use crate::market::Exchange::Ca;
        assert_eq!(
            calendar_coverage(global(Ca), 2027),
            CalendarCoverage::Provisional
        );
        assert!(coverage_warning(global(Ca), 2027).unwrap().contains("TSX"));
        // TSX 2027: New Year's Day (Fri), Good Friday and the observed
        // Christmas and Boxing Day are closed; Christmas Eve is a half day.
        for d in [
            date(2027, 1, 1),
            date(2027, 3, 26),
            date(2027, 12, 27),
            date(2027, 12, 28),
        ] {
            assert!(!is_trading_day(global(Ca), d), "{d}");
        }
        assert!(is_trading_day(global(Ca), date(2027, 3, 25)));
        assert!(is_trading_day(global(Ca), date(2027, 12, 24)));
        assert_eq!(
            hours(global(Ca), date(2027, 12, 24)).unwrap().close,
            hm(13, 0)
        );
        // NSE closes on Good Friday 2027 (provisional list).
        assert!(!is_trading_day(Market::In, date(2027, 3, 26)));
        assert!(is_trading_day(Market::In, date(2027, 3, 25)));
        // Weekday fallback in an uncovered year.
        assert!(is_trading_day(global(Jp), date(2030, 1, 7)));
        assert_eq!(
            closed_reason(global(Jp), at(global(Jp), date(2026, 9, 22), hm(10, 0))),
            "holiday: Citizens' Holiday"
        );
    }

    #[test]
    fn commodities_trade_on_weekdays() {
        assert!(is_trading_day(Market::Commodity, date(2026, 10, 21)));
        assert!(!is_trading_day(Market::Commodity, date(2026, 10, 24)));
        assert_eq!(
            calendar_coverage(Market::Commodity, 2030),
            CalendarCoverage::Official
        );
    }

    #[test]
    fn coverage_reports_missing_years() {
        assert_eq!(
            calendar_coverage(Market::In, 2026),
            CalendarCoverage::Official
        );
        assert_eq!(
            calendar_coverage(Market::In, 2027),
            CalendarCoverage::Provisional
        );
        assert_eq!(
            calendar_coverage(Market::In, 2030),
            CalendarCoverage::Missing
        );
        assert!(coverage_warning(Market::In, 2026).is_none());
        assert!(coverage_warning(Market::Us, 2030).unwrap().contains("NYSE"));
        // An uncovered year still treats weekdays as trading days.
        assert!(is_trading_day(Market::In, date(2030, 1, 2)));
    }
}
