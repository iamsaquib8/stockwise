//! Key events engine.
//!
//! Turns raw headlines, exchange filings, analyst actions, insider trades and
//! corporate actions into a de-duplicated timeline of *events*, each with:
//!   - a category (Results, Order win, Analyst, ...) and sentiment
//!   - coverage (how many outlets reported it) and an official-filing flag
//!   - key figures pulled from the text (₹5,202 crore, 33%, ...)
//!   - the market's actual reaction: stock vs benchmark return on the
//!     reaction session, volume vs 20-day average, and drift since
//!   - an importance score used for ranking
//!
//! It also flags *unexplained* moves — sessions with an abnormal price or
//! volume move but no matching news — which often precede news.

use crate::research::{Headline, Research, Series};
use chrono::{DateTime, Datelike, NaiveDate, Timelike};
use colored::Colorize;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Results,
    Orders,
    Analyst,
    CorporateAction,
    Deal,
    Management,
    Regulatory,
    Capital,
    Insider,
    Product,
    PriceMove,
    Other,
}

impl Kind {
    pub fn label(&self) -> &'static str {
        match self {
            Kind::Results => "RESULTS",
            Kind::Orders => "ORDER WIN",
            Kind::Analyst => "ANALYST",
            Kind::CorporateAction => "CORP ACTION",
            Kind::Deal => "DEAL",
            Kind::Management => "MANAGEMENT",
            Kind::Regulatory => "REGULATORY",
            Kind::Capital => "CAPITAL",
            Kind::Insider => "INSIDER",
            Kind::Product => "BUSINESS",
            Kind::PriceMove => "PRICE MOVE",
            Kind::Other => "NEWS",
        }
    }

    /// Base importance of the category
    fn weight(&self) -> f64 {
        match self {
            Kind::Results => 3.0,
            Kind::Regulatory => 2.5,
            Kind::Orders | Kind::Deal | Kind::CorporateAction | Kind::Insider => 2.0,
            Kind::Analyst | Kind::Management | Kind::Capital => 1.5,
            Kind::Product | Kind::PriceMove => 1.0,
            Kind::Other => 0.5,
        }
    }

    /// Sentiment prior: order wins are good news unless the text says otherwise
    fn sentiment_prior(&self) -> f64 {
        match self {
            Kind::Orders => 0.4,
            Kind::CorporateAction => 0.2,
            Kind::Deal | Kind::Product => 0.1,
            Kind::Regulatory => -0.4,
            _ => 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Reaction {
    pub session: NaiveDate,
    pub stock: f64,
    pub bench: Option<f64>,
    pub excess: Option<f64>,
    pub volume_x: Option<f64>,
    /// Return from the pre-event close to the latest close
    pub since: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Event {
    /// Time of first report
    pub ts: i64,
    pub kind: Kind,
    /// -1 (very negative) … +1 (very positive)
    pub sentiment: f64,
    pub headline: String,
    pub summary: Option<String>,
    /// Structured data lines (results vs estimates, etc.)
    pub facts: Vec<String>,
    /// Other headlines covering the same event
    pub supporting: Vec<String>,
    pub publishers: Vec<String>,
    pub official: bool,
    pub figures: Vec<String>,
    pub reaction: Option<Reaction>,
    pub importance: f64,
    pub link: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Upcoming {
    pub ts: i64,
    pub label: String,
}

#[derive(Clone, Debug, Default)]
pub struct Digest {
    pub positive: usize,
    pub negative: usize,
    pub neutral: usize,
    /// Importance-weighted sentiment, -1 … +1
    pub flow: f64,
}

// ───────────────────────────────────────────────────────────────
// Text analysis (pure)
// ───────────────────────────────────────────────────────────────

fn has_any(t: &str, words: &[&str]) -> bool {
    words.iter().any(|w| t.contains(w))
}

/// Classify a headline/filing into an event category.
pub fn classify(text: &str) -> Kind {
    let t = format!(" {} ", text.to_lowercase());
    if has_any(
        &t,
        &[
            "insider",
            "promoter buys",
            "promoter sells",
            "promoters buy",
            "promoters sell",
            "block deal",
            "bulk deal",
            "pledge",
            "stake sale by promoter",
            "offloads stake",
            "sells stake",
            "buys stake in",
            "raises stake",
            "ofs ",
        ],
    ) {
        return Kind::Insider;
    }
    if has_any(
        &t,
        &[
            "sebi",
            "penalty",
            " fine ",
            " fined",
            "probe",
            " raid",
            "lawsuit",
            " court",
            "tax demand",
            "gst notice",
            "show cause",
            "show-cause",
            "investigation",
            "antitrust",
            "recall",
            " ban ",
            "litigation",
            "nclt",
            " cbi ",
            "fraud",
            "violation",
            "regulator",
            "warning letter",
            "usfda observation",
            "form 483",
        ],
    ) {
        return Kind::Regulatory;
    }
    if has_any(
        &t,
        &[
            "target price",
            "price target",
            "upgrade",
            "downgrade",
            " rating",
            "outperform",
            "underperform",
            "overweight",
            "underweight",
            "brokerage",
            "initiates coverage",
            "initiate coverage",
            "buy call",
            "sell call",
            "reiterate",
            "target to",
            "target of",
            "analysts ",
            "analyst ",
            "motilal",
            "jefferies",
            "clsa",
            "goldman",
            "morgan stanley",
            "nomura",
            "macquarie",
            "kotak institutional",
            "jp morgan",
            "jpmorgan",
        ],
    ) {
        return Kind::Analyst;
    }
    let quarter = has_any(
        &t,
        &[
            " q1", " q2", " q3", " q4", "quarter", " h1", " h2", "fy2", "fy3",
        ],
    );
    if has_any(
        &t,
        &[
            "financial result",
            "quarterly result",
            "earnings call",
            "earnings beat",
            "earnings miss",
            "beats estimates",
            "misses estimates",
            "guidance",
        ],
    ) || (quarter
        && has_any(
            &t,
            &[
                "result",
                "profit",
                "revenue",
                "earnings",
                " pat",
                "ebitda",
                "net income",
                " eps",
                "margin",
                "sales",
            ],
        ))
    {
        return Kind::Results;
    }
    if has_any(
        &t,
        &[
            "dividend",
            "bonus",
            "stock split",
            "share split",
            "split ",
            "buyback",
            "buy-back",
            "buy back",
            "record date",
        ],
    ) {
        return Kind::CorporateAction;
    }
    if has_any(
        &t,
        &[
            " qip",
            "fund raise",
            "fundraise",
            "raises rs",
            "raises ₹",
            "raises $",
            " ncd",
            "debenture",
            " bonds",
            "rights issue",
            "credit rating",
            "rating agency",
            " crisil",
            " icra",
            "allotment",
            " ipo",
        ],
    ) {
        return Kind::Capital;
    }
    if has_any(
        &t,
        &[
            " order",
            "contract",
            " bags ",
            " wins ",
            " win ",
            "secures",
            "awarded",
            "letter of award",
            " loa ",
            "tender",
            "notice to proceed",
            "mandate",
        ],
    ) {
        return Kind::Orders;
    }
    if has_any(
        &t,
        &[
            "acquire",
            "acquisition",
            "merger",
            "joint venture",
            " jv ",
            "partnership",
            " mou",
            "collaborat",
            "alliance",
            "divest",
            "demerger",
            "takeover",
            " stake ",
            "partners with",
            "tie-up",
            "tie up",
        ],
    ) {
        return Kind::Deal;
    }
    if has_any(
        &t,
        &[
            " ceo",
            " cfo",
            " coo",
            " md ",
            "managing director",
            "chairman",
            "appoint",
            "resign",
            "steps down",
            "board of directors",
            "director",
            "succession",
            "elevated",
            "re-appointment",
        ],
    ) {
        return Kind::Management;
    }
    if has_any(
        &t,
        &[
            "launch",
            "unveil",
            "commission",
            "deliver",
            "introduces",
            " plant",
            "capacity",
            "expansion",
            "facility",
            "rollout",
            "roll out",
            "inaugurat",
        ],
    ) {
        return Kind::Product;
    }
    if has_any(&t, &[" shares ", " stock ", "share price", " shares,"])
        && has_any(
            &t,
            &[
                " rise", " fall", " jump", "surge", "slump", "rally", "52-week", " gain",
                "decline", "tumble", "soar", " drop", "plunge", "crash",
            ],
        )
    {
        return Kind::PriceMove;
    }
    Kind::Other
}

const POSITIVE: &[(&str, f64)] = &[
    ("record", 1.0),
    ("surge", 1.0),
    ("soar", 1.0),
    ("jump", 0.8),
    ("rally", 0.8),
    ("beat", 1.0),
    ("outperform", 0.8),
    ("upgrade", 1.2),
    ("raises target", 1.0),
    ("target raised", 1.0),
    ("raised to", 0.6),
    ("bags", 0.8),
    ("wins", 0.8),
    ("secures", 0.8),
    ("bagged", 0.8),
    ("strong", 0.6),
    ("robust", 0.6),
    ("growth", 0.4),
    ("rise", 0.5),
    ("rises", 0.5),
    ("gain", 0.5),
    ("up ", 0.2),
    ("profit up", 0.8),
    ("bullish", 0.8),
    ("buy rating", 0.8),
    ("overweight", 0.6),
    ("approval", 0.7),
    ("approves", 0.5),
    ("expands", 0.5),
    ("expansion", 0.4),
    ("highest", 0.6),
    ("all-time high", 0.8),
    ("52-week high", 0.7),
    ("positive", 0.5),
    ("recovery", 0.4),
    ("improve", 0.5),
    ("boost", 0.6),
    ("upside", 0.5),
    ("milestone", 0.5),
    ("buyback", 0.6),
    ("bonus", 0.5),
    ("dividend", 0.3),
    ("mega", 0.4),
    ("major", 0.2),
    ("commissioned", 0.4),
    ("ahead of estimates", 1.0),
    ("tops estimates", 1.0),
    ("accumulate", 0.4),
    ("debt-free", 0.6),
];

const NEGATIVE: &[(&str, f64)] = &[
    ("fall", 0.6),
    ("falls", 0.6),
    ("drop", 0.6),
    ("plunge", 1.0),
    ("slump", 1.0),
    ("crash", 1.2),
    ("tumble", 1.0),
    ("decline", 0.6),
    ("miss", 1.0),
    ("misses", 1.0),
    ("downgrade", 1.2),
    ("underperform", 0.8),
    ("cut", 0.6),
    ("cuts", 0.6),
    ("lowers", 0.6),
    ("loss", 0.8),
    ("weak", 0.7),
    ("probe", 0.8),
    ("penalty", 0.9),
    ("fine", 0.3),
    ("raid", 1.0),
    ("lawsuit", 0.8),
    ("resign", 0.7),
    ("default", 1.2),
    ("pledge", 0.6),
    ("fraud", 1.5),
    ("concern", 0.5),
    ("pressure", 0.5),
    ("slowdown", 0.7),
    ("delay", 0.6),
    ("put on hold", 0.6),
    ("erosion", 0.8),
    ("eroder", 0.8),
    ("52-week low", 0.7),
    ("warning", 0.7),
    ("headwind", 0.6),
    ("bearish", 0.8),
    ("sell rating", 0.8),
    ("underweight", 0.6),
    ("declined", 0.6),
    ("challenges", 0.4),
    ("inflation", 0.3),
    ("below estimates", 1.0),
    ("disappoint", 0.9),
    ("subdued", 0.5),
    ("muted", 0.4),
    ("exit", 0.3),
    ("lower", 0.3),
    ("negative", 0.5),
    ("downside", 0.5),
    ("sell-off", 0.8),
    ("selloff", 0.8),
    ("tax demand", 0.8),
    ("show cause", 0.8),
];

/// Lexicon sentiment in [-1, 1] (0 when no sentiment words found).
pub fn lexicon_sentiment(text: &str) -> f64 {
    let t = format!(" {} ", text.to_lowercase());
    let score = |lex: &[(&str, f64)]| -> f64 {
        lex.iter()
            .filter(|(w, _)| t.contains(w))
            .map(|(_, s)| s)
            .sum()
    };
    let pos = score(POSITIVE);
    let neg = score(NEGATIVE);
    if pos + neg == 0.0 {
        return 0.0;
    }
    // Confidence grows with the amount of evidence
    let raw = (pos - neg) / (pos + neg);
    raw * ((pos + neg) / 1.5).min(1.0)
}

fn is_number_token(s: &str) -> bool {
    let s = s.trim_start_matches(['+', '-']);
    !s.is_empty()
        && s.chars().any(|c| c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
        && s.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// Extract salient figures: currency amounts (₹5,202 crore, Rs 1.47 trillion, $330) and percentages.
pub fn extract_figures(text: &str) -> Vec<String> {
    let units = [
        "crore",
        "cr",
        "lakh",
        "trillion",
        "tn",
        "billion",
        "bn",
        "million",
        "mn",
        "lakh-crore",
    ];
    let toks: Vec<String> = text
        .split_whitespace()
        .map(|t| {
            t.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | '(' | ')' | '"' | '\'' | '…'))
                .to_string()
        })
        .collect();
    let clean_num = |s: &str| s.trim_end_matches('.').to_string();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        let lower = t.to_lowercase();
        let mut figure: Option<(String, usize)> = None;

        // Currency prefix glued to the number: ₹5,202 / $330 / Rs5,000 / Rs.5,000
        let glued = ["₹", "$", "rs.", "rs", "inr", "usd"].iter().find_map(|p| {
            lower
                .strip_prefix(p)
                .filter(|rest| is_number_token(rest))
                .map(|_| *p)
        });
        if let Some(p) = glued {
            let num = clean_num(&t[p.len()..]);
            let sym = if p == "$" || p == "usd" { "$" } else { "₹" };
            figure = Some((format!("{}{}", sym, num), i + 1));
        } else if matches!(lower.as_str(), "₹" | "$" | "rs" | "rs." | "inr" | "usd")
            && toks.get(i + 1).is_some_and(|n| is_number_token(n))
        {
            let sym = if lower == "$" || lower == "usd" {
                "$"
            } else {
                "₹"
            };
            figure = Some((format!("{}{}", sym, clean_num(&toks[i + 1])), i + 2));
        }

        if let Some((mut f, mut next)) = figure {
            // Units: "crore", "lakh crore", "trillion"
            let mut unit_words = Vec::new();
            while let Some(u) = toks.get(next) {
                let ul = u.to_lowercase().trim_end_matches('.').to_string();
                if units.contains(&ul.as_str()) && unit_words.len() < 2 {
                    unit_words.push(ul);
                    next += 1;
                } else {
                    break;
                }
            }
            if !unit_words.is_empty() {
                f = format!("{} {}", f, unit_words.join(" "));
            }
            if !out.contains(&f) {
                out.push(f);
            }
            i = next;
            continue;
        }

        // Percentages: "16%", "+32.5%", "16 per cent"
        if let Some(n) = t.strip_suffix('%')
            && is_number_token(n)
        {
            let f = format!("{}%", n);
            if !out.contains(&f) {
                out.push(f);
            }
        } else if is_number_token(t)
            && toks
                .get(i + 1)
                .is_some_and(|p| p.eq_ignore_ascii_case("per"))
            && toks
                .get(i + 2)
                .is_some_and(|c| c.to_lowercase().starts_with("cent"))
        {
            let f = format!("{}%", clean_num(t));
            if !out.contains(&f) {
                out.push(f);
            }
            i += 3;
            continue;
        } else if let Some(n) = t.strip_suffix("percent").or_else(|| t.strip_suffix("pc"))
            && is_number_token(n)
        {
            let f = format!("{}%", n);
            if !out.contains(&f) {
                out.push(f);
            }
        }
        i += 1;
    }
    out.truncate(4);
    out
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "that", "this", "has", "have", "its", "into", "after",
    "over", "amid", "share", "shares", "stock", "stocks", "ltd", "limited", "company", "india",
    "says", "said", "will", "are", "was", "were", "their", "than", "news", "today", "what", "why",
    "how", "key", "about", "inc", "corp",
];

fn tokens(text: &str, exclude: &HashSet<String>) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3 && !STOPWORDS.contains(w) && !exclude.contains(*w))
        .map(|w| w.to_string())
        .collect()
}

/// Overlap coefficient |A∩B| / min(|A|,|B|)
fn similarity(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    a.intersection(b).count() as f64 / a.len().min(b.len()) as f64
}

/// Exchange filings that carry no investable information
pub fn is_routine_filing(h: &Headline) -> bool {
    let t = format!("{} {}", h.category.as_deref().unwrap_or(""), h.title).to_lowercase();
    has_any(
        &t,
        &[
            "trading window",
            "newspaper",
            "certificate",
            "loss of share",
            "duplicate share",
            "investor meet",
            "con. call",
            "conference call",
            "analysts/institutional",
            "esop",
            "esos",
            "link of recording",
            "transcript",
            "registrar",
            "postal ballot",
            "regulation 74",
            "reg. 74",
            "issue of duplicate",
            "audio recording",
            "video recording",
            "schedule of analyst",
            "intimation of analyst",
            "copy of",
            "address change",
            "change in address",
            "loss of certificate",
        ],
    )
}

// ───────────────────────────────────────────────────────────────
// Price reaction
// ───────────────────────────────────────────────────────────────

/// Map an event time to its reaction session and measure the move.
/// News after the close (15:30 IST / 16:00 ET) reacts in the next session.
pub fn reaction_for(ts: i64, prices: &Series, bench: &Series, is_india: bool) -> Option<Reaction> {
    let dt = DateTime::from_timestamp(ts, 0)?;
    let close_cutoff_utc = if is_india { 10 } else { 20 }; // 10:00 UTC = 15:30 IST; 20:00 UTC = 16:00 ET
    let mut date = dt.date_naive();
    if dt.hour() >= close_cutoff_utc {
        date = date.succ_opt()?;
    }
    let i = prices.index_on_or_after(date)?;
    if i == 0 {
        return None;
    }
    let prev = prices.close[i - 1];
    let stock = prices.close[i] / prev - 1.0;
    let session = prices.dates[i];
    let bench_ret = bench
        .index_of(session)
        .filter(|&j| j > 0)
        .map(|j| bench.close[j] / bench.close[j - 1] - 1.0);
    let volume_x = if i >= 20 && prices.volume[i] > 0.0 {
        let avg: f64 = prices.volume[i - 20..i].iter().sum::<f64>() / 20.0;
        (avg > 0.0).then(|| prices.volume[i] / avg)
    } else {
        None
    };
    let since = prices.last().map(|l| l / prev - 1.0);
    Some(Reaction {
        session,
        stock,
        bench: bench_ret,
        excess: bench_ret.map(|b| stock - b),
        volume_x,
        since,
    })
}

// ───────────────────────────────────────────────────────────────
// Event building
// ───────────────────────────────────────────────────────────────

struct Cluster {
    kind: Kind,
    items: Vec<Headline>,
    toks: HashSet<String>,
    first_ts: i64,
    last_ts: i64,
}

fn headline_score(h: &Headline) -> f64 {
    let mut s = 0.0;
    if !h.official {
        s += 1.0;
    }
    s += extract_figures(&h.title).len() as f64 * 0.5;
    let len = h.title.len();
    if (40..=150).contains(&len) {
        s += 0.5;
    }
    if h.title.ends_with('?') {
        s -= 0.5; // question headlines are usually opinion pieces
    }
    s
}

fn first_sentences(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = 0;
    for (i, c) in s.char_indices() {
        if i > max {
            break;
        }
        if c == '.' && i > 60 {
            cut = i + 1;
        }
    }
    if cut == 0 {
        let mut end = max.min(s.len());
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", s[..end].trim_end())
    } else {
        s[..cut].to_string()
    }
}

fn cluster_headlines(items: Vec<(Kind, Headline)>, exclude: &HashSet<String>) -> Vec<Cluster> {
    let mut clusters: Vec<Cluster> = Vec::new();
    let mut items = items;
    items.sort_by_key(|(_, h)| h.ts);
    for (kind, h) in items {
        let toks = tokens(&h.title, exclude);
        let best = clusters
            .iter_mut()
            .filter(|c| {
                (h.ts - c.last_ts).abs() <= 2 * 86_400 || (h.ts - c.first_ts).abs() <= 2 * 86_400
            })
            .map(|c| {
                let sim = similarity(&toks, &c.toks);
                let same_kind = c.kind == kind;
                // All results coverage within a few days is one event
                let results_window =
                    same_kind && kind == Kind::Results && (h.ts - c.first_ts).abs() <= 3 * 86_400;
                let score = if results_window {
                    1.0
                } else if same_kind && sim >= 0.45 {
                    sim
                } else if sim >= 0.65 {
                    sim * 0.9
                } else {
                    0.0
                };
                (score, c)
            })
            .filter(|(s, _)| *s > 0.0)
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        match best {
            Some((_, c)) => {
                c.toks.extend(toks);
                c.first_ts = c.first_ts.min(h.ts);
                c.last_ts = c.last_ts.max(h.ts);
                // Prefer a specific category over Other when merging
                if c.kind == Kind::Other {
                    c.kind = kind;
                }
                c.items.push(h);
            }
            None => clusters.push(Cluster {
                kind,
                first_ts: h.ts,
                last_ts: h.ts,
                items: vec![h],
                toks,
            }),
        }
    }
    clusters
}

fn event_from_cluster(c: Cluster, r: &Research) -> Event {
    let mut items = c.items;
    items.sort_by(|a, b| {
        headline_score(b)
            .partial_cmp(&headline_score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let lead = items[0].clone();
    let official = items.iter().any(|h| h.official);
    let mut publishers: Vec<String> = Vec::new();
    for h in &items {
        if !publishers.contains(&h.publisher) {
            publishers.push(h.publisher.clone());
        }
    }
    let lead_toks = tokens(&lead.title, &HashSet::new());
    let mut supporting: Vec<String> = Vec::new();
    for h in items.iter().skip(1) {
        if supporting.len() >= 2 {
            break;
        }
        let t = tokens(&h.title, &HashSet::new());
        if similarity(&lead_toks, &t) < 0.8 && !h.official {
            supporting.push(format!("{} ({})", h.title, h.publisher));
        }
    }
    // Summary: RSS description if present, else the filing text when the lead is a news headline
    let summary = items
        .iter()
        .find_map(|h| h.summary.clone())
        .map(|s| first_sentences(&s, 260))
        .or_else(|| {
            items
                .iter()
                .find(|h| h.official && !lead.official)
                .map(|h| format!("Exchange filing: {}", first_sentences(&h.title, 220)))
        });

    let all_text: String = items
        .iter()
        .map(|h| h.title.as_str())
        .collect::<Vec<_>>()
        .join(" . ");
    let mut figures = extract_figures(&lead.title);
    for f in extract_figures(&all_text) {
        if figures.len() >= 4 {
            break;
        }
        if !figures.contains(&f) {
            figures.push(f);
        }
    }
    // Sentiment: mean of item sentiments, plus category prior
    let lex: f64 = items
        .iter()
        .map(|h| lexicon_sentiment(&h.title))
        .sum::<f64>()
        / items.len() as f64;
    let sentiment = (lex + c.kind.sentiment_prior()).clamp(-1.0, 1.0);

    Event {
        ts: c.first_ts,
        kind: c.kind,
        sentiment,
        headline: lead.title.clone(),
        summary,
        facts: Vec::new(),
        supporting,
        publishers,
        official,
        figures,
        reaction: reaction_for(c.first_ts, &r.prices, &r.bench, r.is_india),
        importance: 0.0,
        link: lead.link.clone(),
    }
}

fn fmt_money(v: f64, r: &Research) -> String {
    crate::display::format_large_number(v, r.currency.as_deref())
}

fn pct(v: f64) -> String {
    format!("{:+.1}%", v * 100.0)
}

/// Structured results line for the latest reported quarter.
pub fn results_facts(r: &Research) -> Option<(NaiveDate, Vec<String>)> {
    let q = r.quarters.last()?;
    let find_q = |days_back: i64| {
        let target = q.end - chrono::Duration::days(days_back);
        r.quarters
            .iter()
            .find(|p| (p.end - target).num_days().abs() <= 20)
    };
    let yago = find_q(365);
    let prev = find_q(91);
    let mut parts = Vec::new();
    if let Some(rev) = q.revenue {
        let mut s = format!("Revenue {}", fmt_money(rev, r));
        let mut ch = Vec::new();
        if let Some(y) = yago.and_then(|p| p.revenue).filter(|v| *v > 0.0) {
            ch.push(format!("{} YoY", pct(rev / y - 1.0)));
        }
        if let Some(p) = prev.and_then(|p| p.revenue).filter(|v| *v > 0.0) {
            ch.push(format!("{} QoQ", pct(rev / p - 1.0)));
        }
        // Compare with consensus only when the calendar refers to this quarter
        if let (Some(est), Some(ed)) = (r.calendar.revenue_estimate, r.calendar.next_earnings)
            && ed <= r.now
            && let Some(ed_date) = DateTime::from_timestamp(ed, 0).map(|d| d.date_naive())
            && (ed_date - q.end).num_days() > 0
            && (ed_date - q.end).num_days() <= 100
            && est > 0.0
        {
            let diff = rev / est - 1.0;
            ch.push(format!(
                "{} est. by {:.1}%",
                if diff >= 0.0 { "beat" } else { "missed" },
                diff.abs() * 100.0
            ));
        }
        if !ch.is_empty() {
            s = format!("{} ({})", s, ch.join(", "));
        }
        parts.push(s);
    }
    if let Some(ni) = q.net_income {
        let mut s = format!("Net profit {}", fmt_money(ni, r));
        if let Some(y) = yago.and_then(|p| p.net_income).filter(|v| *v > 0.0) {
            s = format!("{} ({} YoY)", s, pct(ni / y - 1.0));
        }
        parts.push(s);
    }
    if let (Some(m), Some(my)) = (q.net_margin(), yago.and_then(|p| p.net_margin())) {
        parts.push(format!(
            "net margin {:.1}% ({:+.0} bps YoY)",
            m * 100.0,
            (m - my) * 10_000.0
        ));
    }
    if let Some(s) = r
        .surprises
        .iter()
        .find(|s| (s.quarter - q.end).num_days().abs() <= 20)
    {
        parts.push(format!(
            "EPS {:.2} vs est. {:.2} ({:+.1}%)",
            s.actual,
            s.estimate,
            s.surprise_pct * 100.0
        ));
    }
    if parts.is_empty() {
        return None;
    }
    Some((
        q.end,
        vec![format!("{}: {}", r.quarter_label(q.end), parts.join(" · "))],
    ))
}

fn analyst_events(r: &Research, cutoff: i64) -> Vec<Event> {
    let csym = r.csym();
    let recent: Vec<_> = r.actions.iter().filter(|a| a.ts >= cutoff).collect();
    let mut days: Vec<NaiveDate> = recent
        .iter()
        .filter_map(|a| DateTime::from_timestamp(a.ts, 0).map(|d| d.date_naive()))
        .collect();
    days.sort();
    days.dedup();
    let mut out = Vec::new();
    for day in days {
        let acts: Vec<_> = recent
            .iter()
            .filter(|a| DateTime::from_timestamp(a.ts, 0).map(|d| d.date_naive()) == Some(day))
            .collect();
        let describe = |a: &crate::research::AnalystAction| {
            let verb = match a.action.as_str() {
                "up" => format!("upgrades to {}", a.to_grade),
                "down" => format!("downgrades to {}", a.to_grade),
                "init" => format!("initiates at {}", a.to_grade),
                _ => format!("maintains {}", a.to_grade),
            };
            let pt = match (a.prior_target, a.target) {
                (Some(p), Some(t)) if (p - t).abs() > 0.01 => {
                    format!(" · PT {}{:.0} → {}{:.0}", csym, p, csym, t)
                }
                (_, Some(t)) => format!(" · PT {}{:.0}", csym, t),
                _ => String::new(),
            };
            format!("{} {}{}", a.firm, verb, pt)
        };
        let score = |a: &crate::research::AnalystAction| -> f64 {
            let mut s: f64 = match a.action.as_str() {
                "up" => 0.7,
                "down" => -0.7,
                _ => 0.0,
            };
            if a.action == "init" {
                s += lexicon_sentiment(
                    &a.to_grade
                        .replace("Buy", "buy rating")
                        .replace("Sell", "sell rating"),
                ) * 0.5;
            }
            if let (Some(p), Some(t)) = (a.prior_target, a.target) {
                s += ((t / p - 1.0) * 5.0).clamp(-0.4, 0.4);
            }
            s.clamp(-1.0, 1.0)
        };
        let ups = acts.iter().filter(|a| a.action == "up").count();
        let downs = acts.iter().filter(|a| a.action == "down").count();
        let raises = acts
            .iter()
            .filter(|a| matches!((a.prior_target, a.target), (Some(p), Some(t)) if t > p))
            .count();
        let cuts = acts
            .iter()
            .filter(|a| matches!((a.prior_target, a.target), (Some(p), Some(t)) if t < p))
            .count();
        let headline = if acts.len() == 1 {
            describe(acts[0])
        } else {
            let mut bits = Vec::new();
            if ups > 0 {
                bits.push(format!("{} upgrade{}", ups, if ups > 1 { "s" } else { "" }));
            }
            if downs > 0 {
                bits.push(format!(
                    "{} downgrade{}",
                    downs,
                    if downs > 1 { "s" } else { "" }
                ));
            }
            if raises > 0 {
                bits.push(format!(
                    "{} target raise{}",
                    raises,
                    if raises > 1 { "s" } else { "" }
                ));
            }
            if cuts > 0 {
                bits.push(format!(
                    "{} target cut{}",
                    cuts,
                    if cuts > 1 { "s" } else { "" }
                ));
            }
            format!("{} analyst actions: {}", acts.len(), bits.join(", "))
        };
        let supporting = if acts.len() > 1 {
            acts.iter().take(3).map(|a| describe(a)).collect()
        } else {
            vec![]
        };
        let ts = acts.iter().map(|a| a.ts).min().unwrap_or(0);
        let sentiment = acts.iter().map(|a| score(a)).sum::<f64>() / acts.len() as f64;
        out.push(Event {
            ts,
            kind: Kind::Analyst,
            sentiment,
            headline,
            summary: None,
            facts: vec![],
            supporting,
            publishers: acts.iter().map(|a| a.firm.clone()).collect(),
            official: false,
            figures: vec![],
            reaction: reaction_for(ts, &r.prices, &r.bench, r.is_india),
            importance: 0.0,
            link: None,
        });
    }
    out
}

fn insider_events(r: &Research, cutoff: i64) -> Vec<Event> {
    let recent: Vec<_> = r.insiders.iter().filter(|t| t.ts >= cutoff).collect();
    let mut keys: Vec<(NaiveDate, bool)> = recent
        .iter()
        .filter_map(|t| DateTime::from_timestamp(t.ts, 0).map(|d| (d.date_naive(), t.is_buy)))
        .collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .map(|(day, is_buy)| {
            let txns: Vec<_> = recent
                .iter()
                .filter(|t| {
                    t.is_buy == is_buy
                        && DateTime::from_timestamp(t.ts, 0).map(|d| d.date_naive()) == Some(day)
                })
                .collect();
            let value: f64 = txns.iter().map(|t| t.value).sum();
            let shares: f64 = txns.iter().map(|t| t.shares).sum();
            let who = if txns.len() == 1 {
                format!("{} ({})", txns[0].name, txns[0].relation)
            } else {
                format!("{} insiders", txns.len())
            };
            let headline = format!(
                "{} {} {} shares{}",
                who,
                if is_buy { "bought" } else { "sold" },
                crate::display::format_volume(shares as u64),
                if value > 0.0 {
                    format!(" worth {}", fmt_money(value, r))
                } else {
                    String::new()
                }
            );
            let ts = txns.iter().map(|t| t.ts).min().unwrap_or(0);
            Event {
                ts,
                kind: Kind::Insider,
                sentiment: if is_buy { 0.6 } else { -0.4 },
                headline,
                summary: None,
                facts: vec![],
                supporting: if txns.len() > 1 {
                    txns.iter()
                        .take(3)
                        .map(|t| format!("{} ({})", t.name, t.relation))
                        .collect()
                } else {
                    vec![]
                },
                publishers: vec!["Insider disclosure".into()],
                official: true,
                figures: vec![],
                reaction: reaction_for(ts, &r.prices, &r.bench, r.is_india),
                importance: 0.0,
                link: None,
            }
        })
        .collect()
}

/// Sessions with abnormal moves that no event explains.
fn unexplained_moves(r: &Research, cutoff: i64, explained: &HashSet<NaiveDate>) -> Vec<Event> {
    let p = &r.prices;
    let b = &r.bench;
    if p.len() < 60 {
        return vec![];
    }
    // Excess-return volatility over the past year
    let mut excess: Vec<(usize, f64)> = Vec::new();
    for i in 1..p.len() {
        let s = p.close[i] / p.close[i - 1] - 1.0;
        let bm = b
            .index_of(p.dates[i])
            .filter(|&j| j > 0)
            .map(|j| b.close[j] / b.close[j - 1] - 1.0)
            .unwrap_or(0.0);
        excess.push((i, s - bm));
    }
    let tail = &excess[excess.len().saturating_sub(250)..];
    let mean = tail.iter().map(|(_, e)| e).sum::<f64>() / tail.len() as f64;
    let sd = (tail.iter().map(|(_, e)| (e - mean).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
    let threshold = (2.5 * sd).max(0.03);
    let cutoff_date = DateTime::from_timestamp(cutoff, 0)
        .map(|d| d.date_naive())
        .unwrap_or_default();
    let mut out = Vec::new();
    for &(i, e) in excess.iter().filter(|(i, _)| p.dates[*i] >= cutoff_date) {
        let vol_x = if i >= 20 && p.volume[i] > 0.0 {
            let avg = p.volume[i - 20..i].iter().sum::<f64>() / 20.0;
            (avg > 0.0).then(|| p.volume[i] / avg)
        } else {
            None
        };
        let big_move = e.abs() >= threshold;
        let big_volume = vol_x.is_some_and(|v| v >= 3.0) && e.abs() >= 0.015;
        if !(big_move || big_volume) || explained.contains(&p.dates[i]) {
            continue;
        }
        let ts = p.dates[i]
            .and_hms_opt(12, 0, 0)
            .map(|d| d.and_utc().timestamp())
            .unwrap_or(0);
        let Some(reaction) =
            reaction_for(ts - if r.is_india { 7 * 3600 } else { 0 }, p, b, r.is_india)
        else {
            continue;
        };
        let headline = format!(
            "Unusual move: {} vs {} {}{} — no matching news found",
            pct(reaction.stock),
            r.bench_name,
            reaction.bench.map(pct).unwrap_or_else(|| "n/a".into()),
            vol_x
                .map(|v| format!(" on {:.1}× volume", v))
                .unwrap_or_default()
        );
        out.push(Event {
            ts,
            kind: Kind::PriceMove,
            sentiment: (e * 15.0).clamp(-0.6, 0.6),
            headline,
            summary: Some("Large moves without news often precede an announcement, block deal or rating change — check exchange filings.".into()),
            facts: vec![],
            supporting: vec![],
            publishers: vec!["Price data".into()],
            official: false,
            figures: vec![],
            reaction: Some(reaction),
            importance: 0.0,
            link: None,
        });
    }
    out
}

fn score_importance(e: &mut Event, r: &Research) {
    let age_days = ((r.now - e.ts) as f64 / 86_400.0).max(0.0);
    let recency = (1.0 - age_days / r.days.max(1) as f64).clamp(0.0, 1.0);
    let move_size = e
        .reaction
        .as_ref()
        .map(|x| x.excess.unwrap_or(x.stock).abs() * 100.0)
        .unwrap_or(0.0)
        .min(10.0);
    let vol = e
        .reaction
        .as_ref()
        .and_then(|x| x.volume_x)
        .map(|v| (v - 1.0).clamp(0.0, 4.0))
        .unwrap_or(0.0);
    e.importance = e.kind.weight()
        + (1.0 + e.publishers.len() as f64).ln() * 1.2
        + if e.official { 0.6 } else { 0.0 }
        + move_size * 0.25
        + vol * 0.2
        + e.sentiment.abs() * 0.6
        + (e.figures.len().min(3) as f64) * 0.2
        + if e.facts.is_empty() { 0.0 } else { 0.8 }
        + recency * 0.5;
}

/// Build the event timeline (newest first) for the research window.
pub fn build_events(r: &Research) -> Vec<Event> {
    let cutoff = r.now - r.days * 86_400;
    let exclude: HashSet<String> = r
        .short_name
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .chain(std::iter::once(r.base.to_lowercase().as_str()))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect();

    let mut items: Vec<(Kind, Headline)> = Vec::new();
    for h in &r.news {
        items.push((
            classify(&format!(
                "{} {}",
                h.title,
                h.summary.as_deref().unwrap_or("")
            )),
            h.clone(),
        ));
    }
    for f in r
        .filings
        .iter()
        .filter(|f| f.ts >= cutoff && !is_routine_filing(f))
    {
        let kind = classify(&format!(
            "{} {}",
            f.category.as_deref().unwrap_or(""),
            f.title
        ));
        if kind != Kind::Other {
            items.push((kind, f.clone()));
        }
    }

    let mut events: Vec<Event> = cluster_headlines(items, &exclude)
        .into_iter()
        .map(|c| event_from_cluster(c, r))
        .collect();

    // Attach structured results data to the results event that followed the quarter end
    if let Some((qend, facts)) = results_facts(r) {
        let qend_ts = qend
            .and_hms_opt(0, 0, 0)
            .map(|d| d.and_utc().timestamp())
            .unwrap_or(0);
        let host = events
            .iter_mut()
            .filter(|e| e.kind == Kind::Results && e.ts > qend_ts && e.ts - qend_ts < 80 * 86_400)
            .max_by_key(|e| e.publishers.len());
        match host {
            Some(e) => e.facts = facts,
            None => {
                if let Some(ed) = r
                    .calendar
                    .next_earnings
                    .filter(|ed| *ed <= r.now && *ed >= cutoff && *ed > qend_ts)
                {
                    events.push(Event {
                        ts: ed,
                        kind: Kind::Results,
                        sentiment: 0.0,
                        headline: format!("{} results reported", r.quarter_label(qend)),
                        summary: None,
                        facts,
                        supporting: vec![],
                        publishers: vec!["Company financials".into()],
                        official: true,
                        figures: vec![],
                        reaction: reaction_for(ed, &r.prices, &r.bench, r.is_india),
                        importance: 0.0,
                        link: None,
                    });
                }
            }
        }
    }

    events.extend(analyst_events(r, cutoff));
    events.extend(insider_events(r, cutoff));
    for ca in &r.corp_actions {
        let ts = ca
            .ex_date
            .and_hms_opt(4, 0, 0)
            .map(|d| d.and_utc().timestamp())
            .unwrap_or(0);
        if ts >= cutoff && ts <= r.now {
            events.push(Event {
                ts,
                kind: Kind::CorporateAction,
                sentiment: lexicon_sentiment(&ca.subject).max(0.2),
                headline: format!("Ex-date: {}", ca.subject),
                summary: None,
                facts: vec![],
                supporting: vec![],
                publishers: vec!["NSE".into()],
                official: true,
                figures: extract_figures(&ca.subject),
                reaction: None,
                importance: 0.0,
                link: None,
            });
        }
    }

    let explained: HashSet<NaiveDate> = events
        .iter()
        .filter_map(|e| e.reaction.as_ref().map(|x| x.session))
        .collect();
    events.extend(unexplained_moves(r, cutoff, &explained));

    for e in events.iter_mut() {
        score_importance(e, r);
    }
    events.sort_by_key(|e| std::cmp::Reverse(e.ts));
    events
}

/// Scheduled catalysts ahead.
pub fn upcoming(r: &Research) -> Vec<Upcoming> {
    let mut out = Vec::new();
    if let Some(ed) = r.calendar.next_earnings.filter(|t| *t > r.now - 86_400 / 2) {
        let mut label = format!(
            "Results{}",
            if r.calendar.earnings_is_estimate {
                " (estimated date)"
            } else {
                ""
            }
        );
        let mut est = Vec::new();
        if let Some(e) = r.calendar.eps_estimate {
            est.push(format!("EPS est. {:.2}", e));
        }
        if let Some(rv) = r.calendar.revenue_estimate {
            est.push(format!("revenue est. {}", fmt_money(rv, r)));
        }
        if !est.is_empty() {
            label = format!("{} — consensus {}", label, est.join(", "));
        }
        out.push(Upcoming { ts: ed, label });
    }
    if let Some(xd) = r.calendar.ex_dividend.filter(|t| *t > r.now) {
        out.push(Upcoming {
            ts: xd,
            label: "Ex-dividend date".into(),
        });
    }
    for ca in &r.corp_actions {
        let ts = ca
            .ex_date
            .and_hms_opt(4, 0, 0)
            .map(|d| d.and_utc().timestamp())
            .unwrap_or(0);
        if ts > r.now
            && !out
                .iter()
                .any(|u| (u.ts - ts).abs() < 86_400 && u.label.starts_with("Ex-dividend"))
        {
            out.push(Upcoming {
                ts,
                label: format!("Ex-date: {}", ca.subject),
            });
        }
    }
    out.sort_by_key(|u| u.ts);
    out
}

pub fn digest(events: &[Event]) -> Digest {
    let mut d = Digest::default();
    let (mut num, mut den) = (0.0, 0.0);
    for e in events {
        if e.sentiment > 0.15 {
            d.positive += 1;
        } else if e.sentiment < -0.15 {
            d.negative += 1;
        } else {
            d.neutral += 1;
        }
        num += e.sentiment * e.importance;
        den += e.importance;
    }
    d.flow = if den > 0.0 {
        (num / den).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    d
}

/// Optional: replace summaries with a local-LLM synthesis of all coverage.
pub async fn summarize_with_ai(events: &mut [Event], company: &str, max: usize) -> usize {
    let mut ai = crate::ai::AiClient::connect().await;
    if !ai.is_available().await {
        return 0;
    }
    let mut done = 0;
    let mut order: Vec<usize> = (0..events.len()).collect();
    order.sort_by(|&a, &b| {
        events[b]
            .importance
            .partial_cmp(&events[a].importance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for idx in order.into_iter().take(max) {
        let e = &events[idx];
        if e.kind == Kind::PriceMove {
            continue;
        }
        let mut src = vec![e.headline.clone()];
        src.extend(e.supporting.iter().cloned());
        if let Some(s) = &e.summary {
            src.push(s.clone());
        }
        src.extend(e.facts.iter().cloned());
        let prompt = format!(
            "Summarize this news event about {company} in at most 2 factual sentences (max 50 words). \
             Use only the information below, keep key numbers, no opinions or advice, no preamble.\n\n{}",
            src.iter()
                .map(|s| format!("- {}", s))
                .collect::<Vec<_>>()
                .join("\n")
        );
        if let Ok(text) = ai.generate(&prompt).await
            && !text.trim().is_empty()
        {
            events[idx].summary = Some(text.trim().replace('\n', " "));
            done += 1;
        }
    }
    done
}

// ───────────────────────────────────────────────────────────────
// Rendering
// ───────────────────────────────────────────────────────────────

pub fn sentiment_badge(s: f64) -> String {
    if s > 0.15 {
        "▲ POSITIVE".green().bold().to_string()
    } else if s < -0.15 {
        "▼ NEGATIVE".red().bold().to_string()
    } else {
        "● NEUTRAL".yellow().to_string()
    }
}

fn stars(importance: f64) -> String {
    let n = if importance >= 7.0 {
        3
    } else if importance >= 5.0 {
        2
    } else {
        1
    };
    format!("{}{}", "★".repeat(n).yellow(), "☆".repeat(3 - n).dimmed())
}

fn colored_pct(v: f64) -> String {
    let s = pct(v);
    if v >= 0.0 {
        s.green().to_string()
    } else {
        s.red().to_string()
    }
}

pub fn format_date(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0)
        .map(|d| {
            let d = d.date_naive();
            format!("{:02} {} {}", d.day(), month_abbr(d.month()), d.year())
        })
        .unwrap_or_default()
}

fn month_abbr(m: u32) -> &'static str {
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(m as usize).clamp(1, 12) - 1]
}

pub fn print_event(e: &Event, r: &Research) {
    println!();
    println!(
        "  {}  {}  {}  {}",
        format_date(e.ts).bold(),
        format!("[{}]", e.kind.label()).cyan(),
        sentiment_badge(e.sentiment),
        stars(e.importance)
    );
    println!("    {}", e.headline.bold());
    if let Some(s) = &e.summary {
        println!("    {}", s.dimmed());
    }
    for f in &e.facts {
        println!("    {} {}", "DATA".magenta().bold(), f);
    }
    if !e.figures.is_empty() {
        println!("    {} {}", "Key figures:".dimmed(), e.figures.join(" · "));
    }
    match &e.reaction {
        Some(x) => {
            let mut line = format!("{} on {}", colored_pct(x.stock), {
                let d = x.session;
                format!("{:02} {}", d.day(), month_abbr(d.month()))
            });
            if let (Some(b), Some(ex)) = (x.bench, x.excess) {
                line = format!(
                    "{} vs {} {} → excess {}",
                    line,
                    r.bench_name,
                    pct(b),
                    colored_pct(ex)
                );
            }
            if let Some(v) = x.volume_x {
                let vs = format!("{:.1}× avg volume", v);
                line = format!(
                    "{} · {}",
                    line,
                    if v >= 2.0 {
                        vs.yellow().bold().to_string()
                    } else {
                        vs
                    }
                );
            }
            if let Some(s) = x.since {
                line = format!("{} · since: {}", line, colored_pct(s));
            }
            println!("    {} {}", "Market reaction:".dimmed(), line);
        }
        None if e.kind != Kind::CorporateAction => {
            println!(
                "    {} {}",
                "Market reaction:".dimmed(),
                "pending (after last close)".dimmed()
            );
        }
        None => {}
    }
    for s in &e.supporting {
        println!("    {} {}", "also:".dimmed(), s.dimmed());
    }
    let mut src = e
        .publishers
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if e.publishers.len() > 3 {
        src = format!("{} +{}", src, e.publishers.len() - 3);
    }
    let coverage = format!(
        "{} source{}: {}",
        e.publishers.len(),
        if e.publishers.len() == 1 { "" } else { "s" },
        src
    );
    let official = if e.official
        && !e
            .publishers
            .iter()
            .all(|p| p.contains("filing") || p.contains("disclosure") || p == "NSE")
    {
        format!(" · {}", "confirmed by exchange filing".green())
    } else {
        String::new()
    };
    println!("    {} {}{}", "└".dimmed(), coverage.dimmed(), official);
}

pub fn print_digest(events: &[Event], r: &Research) {
    let d = digest(events);
    let flow = if d.flow > 0.15 {
        format!("{:+.2} Positive", d.flow)
            .green()
            .bold()
            .to_string()
    } else if d.flow < -0.15 {
        format!("{:+.2} Negative", d.flow).red().bold().to_string()
    } else {
        format!("{:+.2} Neutral", d.flow).yellow().to_string()
    };
    println!(
        "  Last {} days: {} events · {} · {} · {}   News-flow score: {}",
        r.days,
        events.len(),
        format!("{} positive", d.positive).green(),
        format!("{} negative", d.negative).red(),
        format!("{} neutral", d.neutral).yellow(),
        flow
    );
    if let Some(big) = events
        .iter()
        .filter(|e| e.reaction.as_ref().and_then(|x| x.excess).is_some())
        .max_by(|a, b| {
            let ea = a
                .reaction
                .as_ref()
                .and_then(|x| x.excess)
                .unwrap_or(0.0)
                .abs();
            let eb = b
                .reaction
                .as_ref()
                .and_then(|x| x.excess)
                .unwrap_or(0.0)
                .abs();
            ea.partial_cmp(&eb).unwrap_or(std::cmp::Ordering::Equal)
        })
        && let Some(ex) = big.reaction.as_ref().and_then(|x| x.excess)
        && ex.abs() >= 0.015
    {
        println!(
            "  Biggest market mover: {} ({} excess on {})",
            big.headline.bold(),
            colored_pct(ex),
            format_date(big.ts)
        );
    }
}

pub fn print_upcoming(up: &[Upcoming], now: i64) {
    for u in up {
        let days = ((u.ts - now) as f64 / 86_400.0).ceil() as i64;
        let when = match days {
            d if d <= 0 => "today".to_string(),
            1 => "tomorrow".to_string(),
            d => format!("in {} days", d),
        };
        println!(
            "  {} {}  {}  {}",
            "◆".magenta(),
            format_date(u.ts).bold(),
            format!("({})", when).dimmed(),
            u.label
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify() {
        assert_eq!(
            classify("L&T Q2 results: net profit rises 33% to ₹5,202 crore"),
            Kind::Results
        );
        assert_eq!(
            classify("L&T bags mega order for power transmission in Saudi Arabia"),
            Kind::Orders
        );
        assert_eq!(
            classify("CLSA maintains outperform on L&T, target price ₹4,842"),
            Kind::Analyst
        );
        assert_eq!(
            classify("Board recommends dividend of Rs 38 per share"),
            Kind::CorporateAction
        );
        assert_eq!(
            classify("SEBI imposes penalty on company"),
            Kind::Regulatory
        );
        assert_eq!(classify("Company appoints new CFO"), Kind::Management);
        assert_eq!(
            classify("INS Samarthak commissioned into Indian Navy"),
            Kind::Product
        );
        assert_eq!(
            classify("Navantia and L&T sign MoU to collaborate"),
            Kind::Deal
        );
        assert_eq!(classify("Something happened"), Kind::Other);
    }

    #[test]
    fn test_sentiment() {
        assert!(lexicon_sentiment("Profit jumps on record order inflows, beats estimates") > 0.5);
        assert!(lexicon_sentiment("Shares plunge after downgrade and weak guidance") < -0.5);
        assert_eq!(lexicon_sentiment("Company to hold AGM on Friday"), 0.0);
    }

    #[test]
    fn test_extract_figures() {
        let f = extract_figures(
            "L&T Q2 profit rises 33% to ₹5,202 crore; order inflow Rs 1.47 trillion",
        );
        assert!(f.contains(&"33%".to_string()), "{:?}", f);
        assert!(f.contains(&"₹5,202 crore".to_string()), "{:?}", f);
        assert!(f.contains(&"₹1.47 trillion".to_string()), "{:?}", f);
        let g = extract_figures(
            "Target raised to $330, revenue up 16 per cent, worth Rs 2,500 lakh crore.",
        );
        assert!(g.contains(&"$330".to_string()), "{:?}", g);
        assert!(g.contains(&"16%".to_string()), "{:?}", g);
        assert!(g.contains(&"₹2,500 lakh crore".to_string()), "{:?}", g);
    }

    #[test]
    fn test_clustering_merges_same_story() {
        let h = |t: &str, ts: i64, p: &str| Headline {
            ts,
            title: t.into(),
            summary: None,
            publisher: p.into(),
            link: None,
            official: false,
            category: None,
        };
        let items = vec![
            (
                Kind::Orders,
                h(
                    "L&T bags mega order for transmission lines in Saudi Arabia",
                    1_000_000,
                    "ET",
                ),
            ),
            (
                Kind::Orders,
                h(
                    "Larsen & Toubro wins mega transmission order in Saudi Arabia, UAE",
                    1_010_000,
                    "Mint",
                ),
            ),
            (
                Kind::Management,
                h("L&T appoints new chief financial officer", 1_020_000, "BS"),
            ),
        ];
        let exclude: HashSet<String> = ["larsen", "toubro"].iter().map(|s| s.to_string()).collect();
        let c = cluster_headlines(items, &exclude);
        assert_eq!(c.len(), 2);
        assert!(
            c.iter()
                .any(|c| c.items.len() == 2 && c.kind == Kind::Orders)
        );
    }

    #[test]
    fn test_reaction_after_close_maps_to_next_session() {
        let d = |dd| NaiveDate::from_ymd_opt(2026, 10, dd).unwrap();
        let prices = Series {
            dates: vec![d(26), d(27), d(28)],
            close: vec![100.0, 101.0, 105.0],
            volume: vec![0.0; 3],
            ..Default::default()
        };
        let bench = Series {
            dates: vec![d(26), d(27), d(28)],
            close: vec![1000.0, 1000.0, 1010.0],
            ..Default::default()
        };
        // 27 Oct 17:13 IST (11:43 UTC) — after the Indian close → reacts on 28 Oct
        let ts = NaiveDate::from_ymd_opt(2026, 10, 27)
            .unwrap()
            .and_hms_opt(11, 43, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        let x = reaction_for(ts, &prices, &bench, true).unwrap();
        assert_eq!(x.session, d(28));
        assert!((x.stock - (105.0 / 101.0 - 1.0)).abs() < 1e-9);
        assert!((x.excess.unwrap() - (105.0 / 101.0 - 1.0 - 0.01)).abs() < 1e-9);
    }

    #[test]
    fn test_routine_filings_filtered() {
        let f = |cat: &str, t: &str| Headline {
            ts: 0,
            title: t.into(),
            summary: None,
            publisher: "NSE filing".into(),
            link: None,
            official: true,
            category: Some(cat.into()),
        };
        assert!(is_routine_filing(&f(
            "Trading Window",
            "Closure of trading window"
        )));
        assert!(is_routine_filing(&f(
            "Analysts/Institutional Investor Meet/Con. Call Updates",
            "Link of Recording"
        )));
        assert!(!is_routine_filing(&f(
            "Award of Order",
            "L&T has received a mega order"
        )));
    }
}
