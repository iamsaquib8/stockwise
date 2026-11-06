//! Portfolio X-Ray analytics engine: look-through overlap across stocks,
//! mutual funds, and ETFs, plus asset-class, market-cap, and sector concentration.
//!
//! Many investors hold direct equities alongside mutual funds and ETFs, creating
//! hidden concentration risk where a single company constitutes 20-30% of total
//! net worth across different wrappers.
//!
//! This module aggregates direct holdings and decomposed fund constituents into:
//! 1. True effective company exposure:
//!    `Weight_effective(S) = (DirectValue(S) + Σ FundValue(F) * w_F(S)) / TotalPortfolioValue`
//! 2. Macro Allocation Breakdown:
//!    - Asset Class Split: Equity, Debt, Gold/Commodity, Cash/Liquid
//!    - Market Cap Split: % Large Cap vs % Mid Cap vs % Small Cap
//!    - Sector Concentration: Exposure across standardized GICS sectors
//! 3. Risk Concentration Alerts:
//!    - Identifies any stock whose combined effective weight exceeds 10% of total portfolio value.

use crate::funds::FundProfile;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Concentration alert threshold in percentage (10.0%).
pub const CONCENTRATION_ALERT_THRESHOLD: f64 = 10.0;

/// Small epsilon for floating-point comparisons.
const EPS: f64 = 1e-9;

/// Individual fund contribution towards an effective company holding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FundContribution {
    pub fund_symbol: String,
    pub value: f64,
    pub weight_in_fund_pct: f64,
    pub effective_weight_pct: f64,
}

/// Aggregated effective company exposure across direct equity and look-through fund holdings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectiveExposure {
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub direct_value: f64,
    pub direct_weight_pct: f64,
    pub fund_value: f64,
    pub fund_weight_pct: f64,
    pub total_value: f64,
    pub effective_weight_pct: f64,
    pub is_concentrated: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub via_funds: Vec<FundContribution>,
}

/// Market capitalization breakdown (Large Cap, Mid Cap, Small Cap, Unclassified).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MarketCapBreakdown {
    pub large_cap_value: f64,
    pub large_cap_pct: f64,
    pub mid_cap_value: f64,
    pub mid_cap_pct: f64,
    pub small_cap_value: f64,
    pub small_cap_pct: f64,
    pub unclassified_value: f64,
    pub unclassified_pct: f64,
}

/// Exposure to an economic sector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectorExposure {
    pub sector: String,
    pub value: f64,
    pub weight_pct: f64,
}

/// Broad asset class allocation across Equity, Debt, Gold/Commodity, and Cash/Liquid.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssetClassSplit {
    pub equity_value: f64,
    pub equity_pct: f64,
    pub debt_value: f64,
    pub debt_pct: f64,
    pub commodity_value: f64,
    pub commodity_pct: f64,
    pub cash_value: f64,
    pub cash_pct: f64,
}

/// Information about a mutual fund or ETF analyzed for look-through.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalyzedFund {
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub look_through_available: bool,
    pub holdings_count: usize,
    pub coverage_pct: f64,
}

/// Complete Portfolio X-Ray Report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortfolioXrayReport {
    pub portfolio_name: String,
    pub base_currency: String,
    pub total_value: f64,
    pub effective_holdings: Vec<EffectiveExposure>,
    pub concentrations_above_10pct: Vec<EffectiveExposure>,
    pub market_cap_breakdown: MarketCapBreakdown,
    pub sector_allocation: Vec<SectorExposure>,
    pub asset_class_split: AssetClassSplit,
    pub funds_analyzed: Vec<AnalyzedFund>,
}

/// Single holding input passed into the X-Ray analytics engine.
#[derive(Debug, Clone, PartialEq)]
pub struct XrayInputHolding {
    pub symbol: String,
    pub name: Option<String>,
    pub shares: f64,
    pub price: f64,
    pub value: f64,
    pub sector: Option<String>,
    pub market_cap: Option<f64>,
    pub currency: Option<String>,
    pub is_fund: bool,
    pub is_sgb: bool,
    pub fund_category: Option<String>,
}

/// Market capitalization bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketCapCategory {
    LargeCap,
    MidCap,
    SmallCap,
    Unclassified,
}

/// High-level asset class for portfolio allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetClassType {
    Equity,
    Debt,
    Commodity,
    Cash,
}

/// Strip exchange suffixes (`.NS`, `.BO`, `=X`) for ticker matching.
pub fn canonical_ticker(symbol: &str) -> String {
    let s = symbol.trim().to_ascii_uppercase();
    if let Some(bare) = s.strip_suffix(".NS") {
        bare.to_string()
    } else if let Some(bare) = s.strip_suffix(".BO") {
        bare.to_string()
    } else if let Some(bare) = s.strip_suffix("=X") {
        bare.to_string()
    } else {
        s
    }
}

/// Returns the benchmark index tracked by known Indian ETFs, if applicable.
pub fn index_for_etf(symbol: &str) -> Option<&'static str> {
    let clean = canonical_ticker(symbol);
    match clean.as_str() {
        "NIFTYBEES" | "SETFNIF50" | "HDFCNIFETF" | "ICICINIFTY" | "NIFTYETF" => Some("NIFTY 50"),
        "JUNIORBEES" | "SETFNN50" | "NEXT50" | "NEXT50ETF" => Some("NIFTY NEXT 50"),
        "BANKBEES" | "SETFBANK" | "HDFCBANKETF" | "KOTAKBKETF" => Some("NIFTY BANK"),
        "MID150BEES" | "MIDCAPETF" => Some("NIFTY MIDCAP 150"),
        "ITBEES" | "SETFIT" => Some("NIFTY IT"),
        "AUTOBEES" => Some("NIFTY AUTO"),
        "PHARMABEES" => Some("NIFTY PHARMA"),
        "PSUBNKBEES" => Some("NIFTY PSU BANK"),
        "INFRABEES" => Some("NIFTY INFRA"),
        "CPSEETF" => Some("NIFTY CPSE"),
        _ => None,
    }
}

/// Check if a symbol represents a known ETF.
pub fn is_known_etf(symbol: &str) -> bool {
    let clean = canonical_ticker(symbol);
    if index_for_etf(&clean).is_some() {
        return true;
    }
    clean.ends_with("BEES")
        || clean.ends_with("ETF")
        || clean.starts_with("SETF")
        || matches!(
            clean.as_str(),
            "GOLDBEES"
                | "SILVERBEES"
                | "LIQUIDBEES"
                | "GOLD1"
                | "SILVER1"
                | "GOLDIETF"
                | "SILVERIETF"
                | "HDFCGOLD"
                | "HDFCSILVER"
                | "SETFGOLD"
                | "GOLDCASE"
                | "MON100"
                | "MAFANG"
        )
}

/// Default human-readable name for known ETFs when quote data is missing or incomplete.
pub fn default_etf_name(symbol: &str) -> Option<&'static str> {
    let clean = canonical_ticker(symbol);
    match clean.as_str() {
        "NIFTYBEES" => Some("Nippon India ETF Nifty 50 BeES"),
        "JUNIORBEES" => Some("Nippon India ETF Nifty Next 50 BeES"),
        "BANKBEES" => Some("Nippon India ETF Nifty Bank BeES"),
        "MID150BEES" => Some("Nippon India ETF Nifty Midcap 150 BeES"),
        "ITBEES" => Some("Nippon India ETF Nifty IT BeES"),
        "AUTOBEES" => Some("Nippon India ETF Nifty Auto BeES"),
        "PHARMABEES" => Some("Nippon India ETF Nifty Pharma BeES"),
        "PSUBNKBEES" => Some("Nippon India ETF Nifty PSU Bank BeES"),
        "INFRABEES" => Some("Nippon India ETF Nifty Infrastructure BeES"),
        "CPSEETF" => Some("CPSE ETF"),
        "GOLDBEES" => Some("Nippon India ETF Gold BeES"),
        "SILVERBEES" => Some("Nippon India ETF Silver BeES"),
        "LIQUIDBEES" => Some("Nippon India ETF Liquid BeES"),
        "SETFNIF50" => Some("SBI Nifty 50 ETF"),
        "SETFNN50" => Some("SBI Nifty Next 50 ETF"),
        "SETFBANK" => Some("SBI Nifty Bank ETF"),
        "SETFGOLD" => Some("SBI Gold ETF"),
        _ => None,
    }
}

/// Default category for known ETFs.
pub fn default_etf_category(symbol: &str) -> Option<&'static str> {
    let clean = canonical_ticker(symbol);
    match clean.as_str() {
        "NIFTYBEES" | "SETFNIF50" | "HDFCNIFETF" | "ICICINIFTY" | "NIFTYETF" => {
            Some("Equity Scheme - Large Cap (NIFTY 50)")
        }
        "JUNIORBEES" | "SETFNN50" | "NEXT50" | "NEXT50ETF" => {
            Some("Equity Scheme - Large & Mid Cap (NIFTY NEXT 50)")
        }
        "BANKBEES" | "SETFBANK" | "HDFCBANKETF" | "KOTAKBKETF" => {
            Some("Equity Scheme - Sectoral / Banking (NIFTY BANK)")
        }
        "MID150BEES" | "MIDCAPETF" => Some("Equity Scheme - Mid Cap (NIFTY MIDCAP 150)"),
        "ITBEES" | "SETFIT" => Some("Equity Scheme - Sectoral / Technology (NIFTY IT)"),
        "AUTOBEES" => Some("Equity Scheme - Sectoral / Auto (NIFTY AUTO)"),
        "PHARMABEES" => Some("Equity Scheme - Sectoral / Pharma (NIFTY PHARMA)"),
        "PSUBNKBEES" => Some("Equity Scheme - Sectoral / PSU Bank (NIFTY PSU BANK)"),
        "INFRABEES" => Some("Equity Scheme - Sectoral / Infrastructure (NIFTY INFRA)"),
        "CPSEETF" => Some("Equity Scheme - Thematic / CPSE"),
        "GOLDBEES" | "SETFGOLD" | "HDFCGOLD" | "GOLDIETF" | "GOLD1" | "GOLDCASE" => {
            Some("Commodity - Gold")
        }
        "SILVERBEES" | "SILVERIETF" | "HDFCSILVER" | "SILVER1" => Some("Commodity - Silver"),
        "LIQUIDBEES" => Some("Debt Scheme - Liquid / Overnight"),
        _ => {
            if clean.ends_with("BEES") || clean.ends_with("ETF") {
                Some("Exchange Traded Fund")
            } else {
                None
            }
        }
    }
}

/// Default economic sector for known ETFs when constituent look-through is not available.
pub fn default_etf_sector(symbol: &str) -> Option<&'static str> {
    let clean = canonical_ticker(symbol);
    match clean.as_str() {
        "BANKBEES" | "SETFBANK" | "HDFCBANKETF" | "KOTAKBKETF" | "PSUBNKBEES" => {
            Some("Financial Services")
        }
        "ITBEES" | "SETFIT" => Some("Technology"),
        "AUTOBEES" => Some("Consumer Cyclical"),
        "PHARMABEES" => Some("Healthcare"),
        "INFRABEES" => Some("Industrials"),
        "CPSEETF" => Some("Energy"),
        "GOLDBEES" | "SETFGOLD" | "HDFCGOLD" | "GOLDIETF" | "GOLD1" | "GOLDCASE" | "SILVERBEES"
        | "SILVERIETF" | "HDFCSILVER" | "SILVER1" => Some("Precious Metals"),
        "LIQUIDBEES" => Some("Cash / Liquid"),
        "NIFTYBEES" | "SETFNIF50" | "HDFCNIFETF" | "ICICINIFTY" | "NIFTYETF" | "JUNIORBEES"
        | "SETFNN50" | "NEXT50" | "NEXT50ETF" | "MID150BEES" | "MIDCAPETF" => {
            Some("Broad Market / Index")
        }
        _ => None,
    }
}

/// Standardize sector names to canonical GICS sectors or broad categories.
pub fn normalize_sector(raw: Option<&str>) -> String {
    let s = match raw {
        Some(s) if !s.trim().is_empty() => s.trim().to_ascii_lowercase(),
        _ => return "Unclassified".to_string(),
    };
    if s.contains("tech")
        || s.contains("software")
        || s.contains("information")
        || s == "it"
        || s.contains("nifty it")
    {
        "Technology".to_string()
    } else if s.contains("finan") || s.contains("bank") || s.contains("insur") {
        "Financial Services".to_string()
    } else if s.contains("health") || s.contains("pharm") || s.contains("biotech") {
        "Healthcare".to_string()
    } else if s.contains("cyclical") || s.contains("discretionary") || s.contains("auto") {
        "Consumer Cyclical".to_string()
    } else if s.contains("defensive") || s.contains("staple") || s.contains("fmcg") {
        "Consumer Defensive".to_string()
    } else if s.contains("energy")
        || s.contains("oil")
        || s.contains("gas")
        || s.contains("petro")
        || s.contains("cpse")
    {
        "Energy".to_string()
    } else if s.contains("industr")
        || s.contains("capital goods")
        || s.contains("engineering")
        || s.contains("infra")
    {
        "Industrials".to_string()
    } else if s.contains("basic")
        || s.contains("material")
        || s.contains("metal")
        || s.contains("mining")
        || s.contains("chemic")
    {
        "Basic Materials".to_string()
    } else if s.contains("comm") || s.contains("telecom") || s.contains("media") {
        "Communication Services".to_string()
    } else if s.contains("util") || s.contains("power") {
        "Utilities".to_string()
    } else if s.contains("real") || s.contains("realt") || s.contains("reit") {
        "Real Estate".to_string()
    } else if s.contains("gold") || s.contains("silver") || s.contains("precious") {
        "Precious Metals".to_string()
    } else if s.contains("liquid") || s.contains("cash") || s.contains("overnight") {
        "Cash / Liquid".to_string()
    } else if s.contains("index")
        || s.contains("broad")
        || s.contains("nifty")
        || s.contains("sensex")
    {
        "Broad Market / Index".to_string()
    } else {
        let mut chars = s.chars();
        match chars.next() {
            None => "Other".to_string(),
            Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        }
    }
}

/// Classify market cap into Large, Mid, or Small Cap based on quote market cap and currency.
///
/// Thresholds:
/// - India (INR): Large Cap >= ₹20,000 Cr; Mid Cap ₹5,000 Cr to ₹20,000 Cr; Small Cap < ₹5,000 Cr.
/// - US / Global (USD & others): Large Cap >= $10B; Mid Cap $2B to $10B; Small Cap < $2B.
pub fn classify_market_cap(market_cap: Option<f64>, currency: Option<&str>) -> MarketCapCategory {
    let Some(mc) = market_cap.filter(|v| *v > 0.0) else {
        return MarketCapCategory::Unclassified;
    };
    let is_inr = currency.is_some_and(|c| c.eq_ignore_ascii_case("INR"));
    if is_inr {
        // ₹20,000 Crore = 200,000,000,000 INR
        // ₹5,000 Crore = 50,000,000,000 INR
        if mc >= 200_000_000_000.0 {
            MarketCapCategory::LargeCap
        } else if mc >= 50_000_000_000.0 {
            MarketCapCategory::MidCap
        } else {
            MarketCapCategory::SmallCap
        }
    } else {
        // $10 Billion = 10,000,000,000 USD
        // $2 Billion = 2,000,000,000 USD
        if mc >= 10_000_000_000.0 {
            MarketCapCategory::LargeCap
        } else if mc >= 2_000_000_000.0 {
            MarketCapCategory::MidCap
        } else {
            MarketCapCategory::SmallCap
        }
    }
}

/// Estimate fund market cap weights `(large_pct, mid_pct, small_pct)` from fund category and name.
pub fn classify_fund_market_cap(category: Option<&str>, name: Option<&str>) -> (f64, f64, f64) {
    let text = format!("{} {}", category.unwrap_or(""), name.unwrap_or("")).to_ascii_lowercase();

    if text.contains("large & mid")
        || text.contains("large and mid")
        || text.contains("next 50")
        || text.contains("next50")
        || text.contains("junior")
    {
        (0.50, 0.50, 0.0)
    } else if text.contains("flexi cap")
        || text.contains("multi cap")
        || text.contains("flexicap")
        || text.contains("multicap")
    {
        // Standard Indian Flexi/Multi cap benchmark distribution
        (0.65, 0.25, 0.10)
    } else if text.contains("large cap")
        || text.contains("large blend")
        || text.contains("large growth")
        || text.contains("large value")
        || text.contains("nifty 50")
        || text.contains("nifty50")
        || text.contains("nifty bees")
        || text.contains("niftybees")
        || text.contains("bank")
        || text.contains("s&p 500")
        || text.contains("sp500")
        || text.contains("mega")
        || text.contains("top 100")
    {
        (1.00, 0.0, 0.0)
    } else if text.contains("mid cap")
        || text.contains("mid-cap")
        || text.contains("midcap")
        || text.contains("mid blend")
        || text.contains("mid growth")
        || text.contains("mid 150")
        || text.contains("mid150")
    {
        (0.0, 1.00, 0.0)
    } else if text.contains("small cap")
        || text.contains("small-cap")
        || text.contains("smallcap")
        || text.contains("russell 2000")
        || text.contains("micro")
    {
        (0.0, 0.0, 1.00)
    } else {
        // Default to Large Cap for general index/blend equity funds
        (1.00, 0.0, 0.0)
    }
}

/// Classify asset class for a portfolio holding.
pub fn classify_asset_class(
    symbol: &str,
    is_fund: bool,
    is_sgb: bool,
    category: Option<&str>,
) -> AssetClassType {
    let sym_upper = symbol.to_ascii_uppercase();
    if is_sgb
        || sym_upper.contains("SGB")
        || sym_upper.contains("GOLD")
        || sym_upper.contains("SILVER")
    {
        return AssetClassType::Commodity;
    }
    let cat_lower = category.unwrap_or("").to_ascii_lowercase();
    if cat_lower.contains("liquid")
        || cat_lower.contains("overnight")
        || cat_lower.contains("money market")
        || sym_upper == "CASH"
        || sym_upper == "LIQUID"
        || sym_upper.contains("LIQUID")
    {
        return AssetClassType::Cash;
    }
    if cat_lower.contains("debt")
        || cat_lower.contains("bond")
        || cat_lower.contains("gilt")
        || cat_lower.contains("fixed income")
        || cat_lower.contains("treasury")
    {
        return AssetClassType::Debt;
    }
    if cat_lower.contains("commodity") || cat_lower.contains("precious") {
        return AssetClassType::Commodity;
    }
    if is_fund && cat_lower.contains("arbitrage") {
        return AssetClassType::Debt;
    }
    AssetClassType::Equity
}

fn round_2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn pct(part: f64, total: f64) -> f64 {
    if total > EPS {
        round_2((part / total) * 100.0)
    } else {
        0.0
    }
}

#[derive(Default)]
struct CompanyAccumulator {
    display_symbol: String,
    name: Option<String>,
    direct_value: f64,
    fund_value: f64,
    via_funds: Vec<FundContribution>,
}

/// Compute portfolio X-Ray look-through exposures and macro allocations.
pub fn compute_xray(
    portfolio_name: &str,
    base_currency: &str,
    holdings: &[XrayInputHolding],
    fund_profiles: &HashMap<String, FundProfile>,
) -> PortfolioXrayReport {
    let total_value: f64 = holdings.iter().map(|h| h.value).sum();

    if total_value <= EPS {
        return PortfolioXrayReport {
            portfolio_name: portfolio_name.to_string(),
            base_currency: base_currency.to_string(),
            total_value: 0.0,
            effective_holdings: Vec::new(),
            concentrations_above_10pct: Vec::new(),
            market_cap_breakdown: MarketCapBreakdown::default(),
            sector_allocation: Vec::new(),
            asset_class_split: AssetClassSplit::default(),
            funds_analyzed: Vec::new(),
        };
    }

    let mut companies: HashMap<String, CompanyAccumulator> = HashMap::new();
    let mut sector_values: HashMap<String, f64> = HashMap::new();
    let mut asset_classes = AssetClassSplit::default();
    let mut large_cap_val = 0.0;
    let mut mid_cap_val = 0.0;
    let mut small_cap_val = 0.0;
    let mut unclassified_cap_val = 0.0;
    let mut equity_val = 0.0;
    let mut funds_analyzed = Vec::new();

    for h in holdings {
        let is_fund = h.is_fund || is_known_etf(&h.symbol) || fund_profiles.contains_key(&h.symbol);
        let category = h.fund_category.as_deref().or_else(|| {
            if is_known_etf(&h.symbol) {
                default_etf_category(&h.symbol)
            } else {
                None
            }
        });
        let asset_class = classify_asset_class(&h.symbol, is_fund, h.is_sgb, category);

        match asset_class {
            AssetClassType::Equity => {
                asset_classes.equity_value += h.value;
                equity_val += h.value;
            }
            AssetClassType::Debt => {
                asset_classes.debt_value += h.value;
            }
            AssetClassType::Commodity => {
                asset_classes.commodity_value += h.value;
                *sector_values
                    .entry("Precious Metals".to_string())
                    .or_default() += h.value;
            }
            AssetClassType::Cash => {
                asset_classes.cash_value += h.value;
            }
        }

        if is_fund {
            let profile_opt = fund_profiles
                .get(&h.symbol)
                .or_else(|| fund_profiles.get(&canonical_ticker(&h.symbol)));

            let look_through_available = profile_opt.is_some_and(|p| !p.holdings.is_empty());
            let (holdings_count, coverage_pct) = if let Some(p) = profile_opt {
                let cov: f64 = p.holdings.iter().map(|c| c.weight).sum();
                (p.holdings.len(), round_2(cov * 100.0))
            } else {
                (0, 0.0)
            };

            funds_analyzed.push(AnalyzedFund {
                symbol: h.symbol.clone(),
                name: h
                    .name
                    .clone()
                    .or_else(|| profile_opt.and_then(|p| p.name.clone()))
                    .or_else(|| {
                        if is_known_etf(&h.symbol) {
                            default_etf_name(&h.symbol).map(str::to_string)
                        } else {
                            None
                        }
                    }),
                look_through_available,
                holdings_count,
                coverage_pct,
            });

            // Decompose constituent holdings
            if let Some(profile) = profile_opt {
                for c in &profile.holdings {
                    let c_val = h.value * c.weight;
                    let c_key = c
                        .symbol
                        .as_deref()
                        .map(canonical_ticker)
                        .or_else(|| c.key())
                        .unwrap_or_else(|| "UNKNOWN".to_string());

                    let entry = companies
                        .entry(c_key)
                        .or_insert_with(|| CompanyAccumulator {
                            display_symbol: c
                                .symbol
                                .clone()
                                .unwrap_or_else(|| c.name.clone().unwrap_or_default()),
                            name: c.name.clone(),
                            direct_value: 0.0,
                            fund_value: 0.0,
                            via_funds: Vec::new(),
                        });

                    entry.fund_value += c_val;
                    if entry.name.is_none() && c.name.is_some() {
                        entry.name = c.name.clone();
                    }

                    entry.via_funds.push(FundContribution {
                        fund_symbol: h.symbol.clone(),
                        value: round_2(c_val),
                        weight_in_fund_pct: round_2(c.weight * 100.0),
                        effective_weight_pct: pct(c_val, total_value),
                    });
                }

                // Decompose sector weights
                if !profile.sectors.is_empty() {
                    for (sec, w) in &profile.sectors {
                        let sec_name = normalize_sector(Some(sec));
                        *sector_values.entry(sec_name).or_default() += h.value * w;
                    }
                } else if asset_class == AssetClassType::Equity {
                    let sec_source = h
                        .fund_category
                        .as_deref()
                        .or(profile.category.as_deref())
                        .or(h.sector.as_deref())
                        .or_else(|| {
                            if is_known_etf(&h.symbol) {
                                default_etf_sector(&h.symbol)
                            } else {
                                None
                            }
                        });
                    let sec_name = normalize_sector(sec_source);
                    *sector_values.entry(sec_name).or_default() += h.value;
                }

                // Market cap breakdown for the fund
                if asset_class == AssetClassType::Equity {
                    let cat_source = h
                        .fund_category
                        .as_deref()
                        .or(profile.category.as_deref())
                        .or_else(|| {
                            if is_known_etf(&h.symbol) {
                                default_etf_category(&h.symbol)
                            } else {
                                None
                            }
                        });
                    let name_source = h.name.as_deref().or(profile.name.as_deref()).or_else(|| {
                        if is_known_etf(&h.symbol) {
                            default_etf_name(&h.symbol)
                        } else {
                            None
                        }
                    });
                    let (large_p, mid_p, small_p) =
                        classify_fund_market_cap(cat_source, name_source);
                    large_cap_val += h.value * large_p;
                    mid_cap_val += h.value * mid_p;
                    small_cap_val += h.value * small_p;
                }
            } else {
                // Fund without constituent look-through
                if asset_class == AssetClassType::Equity {
                    let sec_source =
                        h.fund_category
                            .as_deref()
                            .or(h.sector.as_deref())
                            .or_else(|| {
                                if is_known_etf(&h.symbol) {
                                    default_etf_sector(&h.symbol)
                                } else {
                                    None
                                }
                            });
                    let sec_name = normalize_sector(sec_source);
                    *sector_values.entry(sec_name).or_default() += h.value;

                    let cat_source = h.fund_category.as_deref().or_else(|| {
                        if is_known_etf(&h.symbol) {
                            default_etf_category(&h.symbol)
                        } else {
                            None
                        }
                    });
                    let name_source = h.name.as_deref().or_else(|| {
                        if is_known_etf(&h.symbol) {
                            default_etf_name(&h.symbol)
                        } else {
                            None
                        }
                    });
                    let (large_p, mid_p, small_p) =
                        classify_fund_market_cap(cat_source, name_source);
                    large_cap_val += h.value * large_p;
                    mid_cap_val += h.value * mid_p;
                    small_cap_val += h.value * small_p;
                }
            }
        } else if asset_class == AssetClassType::Equity {
            // Direct stock holding
            let c_key = canonical_ticker(&h.symbol);
            let entry = companies
                .entry(c_key)
                .or_insert_with(|| CompanyAccumulator {
                    display_symbol: h.symbol.clone(),
                    name: h.name.clone(),
                    direct_value: 0.0,
                    fund_value: 0.0,
                    via_funds: Vec::new(),
                });

            entry.direct_value += h.value;
            if entry.name.is_none() && h.name.is_some() {
                entry.name = h.name.clone();
            }

            let sec_name = normalize_sector(h.sector.as_deref());
            *sector_values.entry(sec_name).or_default() += h.value;

            // Direct stock market cap
            match classify_market_cap(h.market_cap, h.currency.as_deref()) {
                MarketCapCategory::LargeCap => large_cap_val += h.value,
                MarketCapCategory::MidCap => mid_cap_val += h.value,
                MarketCapCategory::SmallCap => small_cap_val += h.value,
                MarketCapCategory::Unclassified => unclassified_cap_val += h.value,
            }
        }
    }

    // Build effective holdings list
    let mut effective_holdings = Vec::with_capacity(companies.len());
    for (_key, acc) in companies {
        let tot = acc.direct_value + acc.fund_value;
        let eff_pct = pct(tot, total_value);
        let is_concentrated = eff_pct >= CONCENTRATION_ALERT_THRESHOLD;

        let mut via_funds = acc.via_funds;
        via_funds.sort_by(|a, b| b.value.total_cmp(&a.value));

        effective_holdings.push(EffectiveExposure {
            symbol: acc.display_symbol,
            name: acc.name,
            direct_value: round_2(acc.direct_value),
            direct_weight_pct: pct(acc.direct_value, total_value),
            fund_value: round_2(acc.fund_value),
            fund_weight_pct: pct(acc.fund_value, total_value),
            total_value: round_2(tot),
            effective_weight_pct: eff_pct,
            is_concentrated,
            via_funds,
        });
    }

    effective_holdings.sort_by(|a, b| b.total_value.total_cmp(&a.total_value));

    let concentrations_above_10pct: Vec<EffectiveExposure> = effective_holdings
        .iter()
        .filter(|e| e.is_concentrated)
        .cloned()
        .collect();

    // Asset class percentages
    asset_classes.equity_value = round_2(asset_classes.equity_value);
    asset_classes.equity_pct = pct(asset_classes.equity_value, total_value);
    asset_classes.debt_value = round_2(asset_classes.debt_value);
    asset_classes.debt_pct = pct(asset_classes.debt_value, total_value);
    asset_classes.commodity_value = round_2(asset_classes.commodity_value);
    asset_classes.commodity_pct = pct(asset_classes.commodity_value, total_value);
    asset_classes.cash_value = round_2(asset_classes.cash_value);
    asset_classes.cash_pct = pct(asset_classes.cash_value, total_value);

    // Market cap breakdown percentages against total equity value
    let market_cap_base = if equity_val > EPS {
        equity_val
    } else {
        total_value
    };
    let market_cap_breakdown = MarketCapBreakdown {
        large_cap_value: round_2(large_cap_val),
        large_cap_pct: pct(large_cap_val, market_cap_base),
        mid_cap_value: round_2(mid_cap_val),
        mid_cap_pct: pct(mid_cap_val, market_cap_base),
        small_cap_value: round_2(small_cap_val),
        small_cap_pct: pct(small_cap_val, market_cap_base),
        unclassified_value: round_2(unclassified_cap_val),
        unclassified_pct: pct(unclassified_cap_val, market_cap_base),
    };

    // Sector allocation
    let mut sector_allocation: Vec<SectorExposure> = sector_values
        .into_iter()
        .map(|(sec, val)| SectorExposure {
            sector: sec,
            value: round_2(val),
            weight_pct: pct(val, total_value),
        })
        .collect();
    sector_allocation.sort_by(|a, b| b.value.total_cmp(&a.value));

    PortfolioXrayReport {
        portfolio_name: portfolio_name.to_string(),
        base_currency: base_currency.to_string(),
        total_value: round_2(total_value),
        effective_holdings,
        concentrations_above_10pct,
        market_cap_breakdown,
        sector_allocation,
        asset_class_split: asset_classes,
        funds_analyzed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::funds::Holding as FundConstituent;

    #[test]
    fn test_xray_empty_portfolio() {
        let report = compute_xray("default", "USD", &[], &HashMap::new());
        assert_eq!(report.total_value, 0.0);
        assert!(report.effective_holdings.is_empty());
        assert!(report.concentrations_above_10pct.is_empty());
        assert_eq!(report.asset_class_split.equity_value, 0.0);
    }

    #[test]
    fn test_xray_direct_stocks_only() {
        let holdings = vec![
            XrayInputHolding {
                symbol: "AAPL".to_string(),
                name: Some("Apple Inc.".to_string()),
                shares: 100.0,
                price: 150.0,
                value: 15000.0,
                sector: Some("Technology".to_string()),
                market_cap: Some(2_500_000_000_000.0),
                currency: Some("USD".to_string()),
                is_fund: false,
                is_sgb: false,
                fund_category: None,
            },
            XrayInputHolding {
                symbol: "MSFT".to_string(),
                name: Some("Microsoft Corp.".to_string()),
                shares: 20.0,
                price: 250.0,
                value: 5000.0,
                sector: Some("Technology".to_string()),
                market_cap: Some(2_000_000_000_000.0),
                currency: Some("USD".to_string()),
                is_fund: false,
                is_sgb: false,
                fund_category: None,
            },
        ];

        let report = compute_xray("default", "USD", &holdings, &HashMap::new());
        assert_eq!(report.total_value, 20000.0);
        assert_eq!(report.effective_holdings.len(), 2);

        let aapl = &report.effective_holdings[0];
        assert_eq!(aapl.symbol, "AAPL");
        assert_eq!(aapl.direct_value, 15000.0);
        assert_eq!(aapl.fund_value, 0.0);
        assert_eq!(aapl.total_value, 15000.0);
        assert_eq!(aapl.effective_weight_pct, 75.0);
        assert!(aapl.is_concentrated);

        let msft = &report.effective_holdings[1];
        assert_eq!(msft.symbol, "MSFT");
        assert_eq!(msft.effective_weight_pct, 25.0);
        assert!(msft.is_concentrated);

        // Both exceed 10%
        assert_eq!(report.concentrations_above_10pct.len(), 2);

        // Macro breakdown
        assert_eq!(report.asset_class_split.equity_pct, 100.0);
        assert_eq!(report.market_cap_breakdown.large_cap_pct, 100.0);
        assert_eq!(report.sector_allocation.len(), 1);
        assert_eq!(report.sector_allocation[0].sector, "Technology");
        assert_eq!(report.sector_allocation[0].weight_pct, 100.0);
    }

    #[test]
    fn test_xray_look_through_fund_decomposition() {
        let mut fund_profiles = HashMap::new();
        fund_profiles.insert(
            "SPY".to_string(),
            FundProfile {
                symbol: "SPY".to_string(),
                name: Some("SPDR S&P 500 ETF Trust".to_string()),
                family: Some("State Street".to_string()),
                category: Some("Large Blend".to_string()),
                expense_ratio: Some(0.0009),
                currency: Some("USD".to_string()),
                holdings: vec![
                    FundConstituent {
                        symbol: Some("AAPL".to_string()),
                        name: Some("Apple Inc.".to_string()),
                        weight: 0.07, // 7.0%
                    },
                    FundConstituent {
                        symbol: Some("MSFT".to_string()),
                        name: Some("Microsoft Corp.".to_string()),
                        weight: 0.065, // 6.5%
                    },
                    FundConstituent {
                        symbol: Some("NVDA".to_string()),
                        name: Some("NVIDIA Corp.".to_string()),
                        weight: 0.06, // 6.0%
                    },
                ],
                sectors: vec![
                    ("technology".to_string(), 0.35),
                    ("financial_services".to_string(), 0.15),
                    ("healthcare".to_string(), 0.12),
                ],
            },
        );

        let holdings = vec![
            // Direct holding: AAPL $10,000
            XrayInputHolding {
                symbol: "AAPL".to_string(),
                name: Some("Apple Inc.".to_string()),
                shares: 100.0,
                price: 100.0,
                value: 10000.0,
                sector: Some("Technology".to_string()),
                market_cap: Some(2_500_000_000_000.0),
                currency: Some("USD".to_string()),
                is_fund: false,
                is_sgb: false,
                fund_category: None,
            },
            // ETF holding: SPY $40,000
            XrayInputHolding {
                symbol: "SPY".to_string(),
                name: Some("SPDR S&P 500 ETF Trust".to_string()),
                shares: 80.0,
                price: 500.0,
                value: 40000.0,
                sector: None,
                market_cap: None,
                currency: Some("USD".to_string()),
                is_fund: true,
                is_sgb: false,
                fund_category: Some("Large Blend".to_string()),
            },
        ];

        let report = compute_xray("default", "USD", &holdings, &fund_profiles);
        assert_eq!(report.total_value, 50000.0);

        // AAPL total: Direct 10,000 + Fund (40,000 * 0.07 = 2,800) = 12,800 (25.6%)
        let aapl = report
            .effective_holdings
            .iter()
            .find(|h| h.symbol == "AAPL")
            .unwrap();
        assert_eq!(aapl.direct_value, 10000.0);
        assert_eq!(aapl.direct_weight_pct, 20.0);
        assert_eq!(aapl.fund_value, 2800.0);
        assert_eq!(aapl.fund_weight_pct, 5.6);
        assert_eq!(aapl.total_value, 12800.0);
        assert_eq!(aapl.effective_weight_pct, 25.6);
        assert!(aapl.is_concentrated);
        assert_eq!(aapl.via_funds.len(), 1);
        assert_eq!(aapl.via_funds[0].fund_symbol, "SPY");
        assert_eq!(aapl.via_funds[0].value, 2800.0);
        assert_eq!(aapl.via_funds[0].weight_in_fund_pct, 7.0);

        // MSFT total: Direct 0 + Fund (40,000 * 0.065 = 2,600) = 2,600 (5.2%)
        let msft = report
            .effective_holdings
            .iter()
            .find(|h| h.symbol == "MSFT")
            .unwrap();
        assert_eq!(msft.direct_value, 0.0);
        assert_eq!(msft.fund_value, 2600.0);
        assert_eq!(msft.effective_weight_pct, 5.2);
        assert!(!msft.is_concentrated);

        // NVDA total: Direct 0 + Fund (40,000 * 0.06 = 2,400) = 2,400 (4.8%)
        let nvda = report
            .effective_holdings
            .iter()
            .find(|h| h.symbol == "NVDA")
            .unwrap();
        assert_eq!(nvda.direct_value, 0.0);
        assert_eq!(nvda.fund_value, 2400.0);
        assert_eq!(nvda.effective_weight_pct, 4.8);
        assert!(!nvda.is_concentrated);

        // Only AAPL is concentrated (>10%)
        assert_eq!(report.concentrations_above_10pct.len(), 1);
        assert_eq!(report.concentrations_above_10pct[0].symbol, "AAPL");

        // Sector concentration check
        // Direct AAPL = $10,000 Tech. SPY Tech = 40,000 * 0.35 = $14,000. Total Tech = $24,000 (48.0%)
        let tech = report
            .sector_allocation
            .iter()
            .find(|s| s.sector == "Technology")
            .unwrap();
        assert_eq!(tech.value, 24000.0);
        assert_eq!(tech.weight_pct, 48.0);

        // Funds analyzed
        assert_eq!(report.funds_analyzed.len(), 1);
        assert!(report.funds_analyzed[0].look_through_available);
        assert_eq!(report.funds_analyzed[0].holdings_count, 3);
    }

    #[test]
    fn test_xray_multi_fund_overlap() {
        let mut fund_profiles = HashMap::new();
        fund_profiles.insert(
            "SPY".to_string(),
            FundProfile {
                symbol: "SPY".to_string(),
                name: Some("SPDR S&P 500 ETF Trust".to_string()),
                family: None,
                category: Some("Large Blend".to_string()),
                expense_ratio: None,
                currency: None,
                holdings: vec![
                    FundConstituent {
                        symbol: Some("AAPL".to_string()),
                        name: Some("Apple Inc.".to_string()),
                        weight: 0.07, // 7%
                    },
                    FundConstituent {
                        symbol: Some("MSFT".to_string()),
                        name: Some("Microsoft Corp.".to_string()),
                        weight: 0.06, // 6%
                    },
                ],
                sectors: vec![("technology".to_string(), 0.30)],
            },
        );
        fund_profiles.insert(
            "QQQ".to_string(),
            FundProfile {
                symbol: "QQQ".to_string(),
                name: Some("Invesco QQQ Trust".to_string()),
                family: None,
                category: Some("Large Growth".to_string()),
                expense_ratio: None,
                currency: None,
                holdings: vec![
                    FundConstituent {
                        symbol: Some("AAPL".to_string()),
                        name: Some("Apple Inc.".to_string()),
                        weight: 0.09, // 9%
                    },
                    FundConstituent {
                        symbol: Some("MSFT".to_string()),
                        name: Some("Microsoft Corp.".to_string()),
                        weight: 0.08, // 8%
                    },
                ],
                sectors: vec![("technology".to_string(), 0.50)],
            },
        );

        let holdings = vec![
            XrayInputHolding {
                symbol: "SPY".to_string(),
                name: Some("SPY".to_string()),
                shares: 100.0,
                price: 300.0,
                value: 30000.0,
                sector: None,
                market_cap: None,
                currency: Some("USD".to_string()),
                is_fund: true,
                is_sgb: false,
                fund_category: Some("Large Blend".to_string()),
            },
            XrayInputHolding {
                symbol: "QQQ".to_string(),
                name: Some("QQQ".to_string()),
                shares: 50.0,
                price: 400.0,
                value: 20000.0,
                sector: None,
                market_cap: None,
                currency: Some("USD".to_string()),
                is_fund: true,
                is_sgb: false,
                fund_category: Some("Large Growth".to_string()),
            },
        ];

        let report = compute_xray("default", "USD", &holdings, &fund_profiles);
        assert_eq!(report.total_value, 50000.0);

        // AAPL: SPY (30,000 * 0.07 = 2,100) + QQQ (20,000 * 0.09 = 1,800) = 3,900 (7.8%)
        let aapl = report
            .effective_holdings
            .iter()
            .find(|h| h.symbol == "AAPL")
            .unwrap();
        assert_eq!(aapl.total_value, 3900.0);
        assert_eq!(aapl.effective_weight_pct, 7.8);
        assert_eq!(aapl.via_funds.len(), 2);

        // MSFT: SPY (30,000 * 0.06 = 1,800) + QQQ (20,000 * 0.08 = 1,600) = 3,400 (6.8%)
        let msft = report
            .effective_holdings
            .iter()
            .find(|h| h.symbol == "MSFT")
            .unwrap();
        assert_eq!(msft.total_value, 3400.0);
        assert_eq!(msft.effective_weight_pct, 6.8);
        assert_eq!(msft.via_funds.len(), 2);

        assert_eq!(report.funds_analyzed.len(), 2);
    }

    #[test]
    fn test_xray_concentration_alert_threshold() {
        // Direct stock 8% + fund look-through 3% = 11% (exceeds 10%)
        let mut fund_profiles = HashMap::new();
        fund_profiles.insert(
            "ETF1".to_string(),
            FundProfile {
                symbol: "ETF1".to_string(),
                name: None,
                family: None,
                category: Some("Large Cap".to_string()),
                expense_ratio: None,
                currency: None,
                holdings: vec![FundConstituent {
                    symbol: Some("ABC".to_string()),
                    name: Some("ABC Corp".to_string()),
                    weight: 0.10, // 10% of ETF1
                }],
                sectors: vec![],
            },
        );

        let holdings = vec![
            XrayInputHolding {
                symbol: "ABC".to_string(),
                name: Some("ABC Corp".to_string()),
                shares: 80.0,
                price: 100.0,
                value: 8000.0, // 8.0% of $100k
                sector: Some("Technology".to_string()),
                market_cap: Some(50_000_000_000.0),
                currency: Some("USD".to_string()),
                is_fund: false,
                is_sgb: false,
                fund_category: None,
            },
            XrayInputHolding {
                symbol: "ETF1".to_string(),
                name: None,
                shares: 300.0,
                price: 100.0,
                value: 30000.0, // ABC constituent is 30,000 * 0.10 = 3,000 (3.0%)
                sector: None,
                market_cap: None,
                currency: Some("USD".to_string()),
                is_fund: true,
                is_sgb: false,
                fund_category: Some("Large Cap".to_string()),
            },
            XrayInputHolding {
                symbol: "OTHER".to_string(),
                name: None,
                shares: 620.0,
                price: 100.0,
                value: 62000.0, // 62%
                sector: Some("Healthcare".to_string()),
                market_cap: Some(50_000_000_000.0),
                currency: Some("USD".to_string()),
                is_fund: false,
                is_sgb: false,
                fund_category: None,
            },
        ];

        let report = compute_xray("test", "USD", &holdings, &fund_profiles);
        assert_eq!(report.total_value, 100000.0);

        let abc = report
            .effective_holdings
            .iter()
            .find(|h| h.symbol == "ABC")
            .unwrap();
        assert_eq!(abc.direct_value, 8000.0);
        assert_eq!(abc.fund_value, 3000.0);
        assert_eq!(abc.total_value, 11000.0);
        assert_eq!(abc.effective_weight_pct, 11.0);
        assert!(abc.is_concentrated);

        assert!(
            report
                .concentrations_above_10pct
                .iter()
                .any(|c| c.symbol == "ABC")
        );
    }

    #[test]
    fn test_xray_macro_allocation() {
        let holdings = vec![
            XrayInputHolding {
                symbol: "RELIANCE.NS".to_string(),
                name: Some("Reliance Industries".to_string()),
                shares: 10.0,
                price: 5000.0,
                value: 50000.0,
                sector: Some("Energy".to_string()),
                market_cap: Some(2_000_000_000_000.0), // Large cap
                currency: Some("INR".to_string()),
                is_fund: false,
                is_sgb: false,
                fund_category: None,
            },
            XrayInputHolding {
                symbol: "SGBNOV26.NS".to_string(),
                name: Some("Sovereign Gold Bond".to_string()),
                shares: 4.0,
                price: 5000.0,
                value: 20000.0,
                sector: None,
                market_cap: None,
                currency: Some("INR".to_string()),
                is_fund: false,
                is_sgb: true,
                fund_category: None,
            },
            XrayInputHolding {
                symbol: "MF:120000".to_string(),
                name: Some("HDFC Short Term Debt Fund".to_string()),
                shares: 1000.0,
                price: 20.0,
                value: 20000.0,
                sector: None,
                market_cap: None,
                currency: Some("INR".to_string()),
                is_fund: true,
                is_sgb: false,
                fund_category: Some("Debt Scheme - Short Duration Fund".to_string()),
            },
            XrayInputHolding {
                symbol: "CASH".to_string(),
                name: Some("Cash".to_string()),
                shares: 1.0,
                price: 10000.0,
                value: 10000.0,
                sector: None,
                market_cap: None,
                currency: Some("INR".to_string()),
                is_fund: false,
                is_sgb: false,
                fund_category: None,
            },
        ];

        let report = compute_xray("macro_test", "INR", &holdings, &HashMap::new());
        assert_eq!(report.total_value, 100000.0);

        assert_eq!(report.asset_class_split.equity_value, 50000.0);
        assert_eq!(report.asset_class_split.equity_pct, 50.0);
        assert_eq!(report.asset_class_split.commodity_value, 20000.0);
        assert_eq!(report.asset_class_split.commodity_pct, 20.0);
        assert_eq!(report.asset_class_split.debt_value, 20000.0);
        assert_eq!(report.asset_class_split.debt_pct, 20.0);
        assert_eq!(report.asset_class_split.cash_value, 10000.0);
        assert_eq!(report.asset_class_split.cash_pct, 10.0);
    }

    #[test]
    fn test_xray_indian_equity_look_through() {
        let mut fund_profiles = HashMap::new();
        fund_profiles.insert(
            "MF:120503".to_string(),
            FundProfile {
                symbol: "MF:120503".to_string(),
                name: Some("Parag Parikh Flexi Cap Fund".to_string()),
                family: Some("PPFAS".to_string()),
                category: Some("Equity Scheme - Flexi Cap Fund".to_string()),
                expense_ratio: Some(0.0065),
                currency: Some("INR".to_string()),
                holdings: vec![FundConstituent {
                    symbol: Some("HDFCBANK.NS".to_string()),
                    name: Some("HDFC Bank Ltd".to_string()),
                    weight: 0.08, // 8%
                }],
                sectors: vec![("financial_services".to_string(), 0.30)],
            },
        );

        let holdings = vec![
            XrayInputHolding {
                symbol: "HDFCBANK.NS".to_string(),
                name: Some("HDFC Bank Ltd".to_string()),
                shares: 100.0,
                price: 1500.0,
                value: 150000.0,
                sector: Some("Financial Services".to_string()),
                market_cap: Some(1_100_000_000_000.0), // > 20000 Cr -> Large Cap
                currency: Some("INR".to_string()),
                is_fund: false,
                is_sgb: false,
                fund_category: None,
            },
            XrayInputHolding {
                symbol: "MF:120503".to_string(),
                name: Some("Parag Parikh Flexi Cap Fund".to_string()),
                shares: 1000.0,
                price: 50.0,
                value: 50000.0, // Look-through HDFCBANK: 50,000 * 0.08 = 4,000
                sector: None,
                market_cap: None,
                currency: Some("INR".to_string()),
                is_fund: true,
                is_sgb: false,
                fund_category: Some("Equity Scheme - Flexi Cap Fund".to_string()),
            },
        ];

        let report = compute_xray("indian_test", "INR", &holdings, &fund_profiles);
        assert_eq!(report.total_value, 200000.0);

        let hdfc = report
            .effective_holdings
            .iter()
            .find(|h| canonical_ticker(&h.symbol) == "HDFCBANK")
            .unwrap();
        assert_eq!(hdfc.direct_value, 150000.0);
        assert_eq!(hdfc.direct_weight_pct, 75.0);
        assert_eq!(hdfc.fund_value, 4000.0);
        assert_eq!(hdfc.fund_weight_pct, 2.0);
        assert_eq!(hdfc.total_value, 154000.0);
        assert_eq!(hdfc.effective_weight_pct, 77.0);
        assert!(hdfc.is_concentrated);
    }

    #[test]
    fn test_xray_indian_etf_niftybees_without_profile() {
        // Issue #220: NIFTYBEES without a profile must NOT be flagged as single-stock concentration risk,
        // must be listed under funds analyzed with look_through_available: false,
        // and must categorize market cap as Large Cap (not Unclassified).
        let holdings = vec![
            XrayInputHolding {
                symbol: "NIFTYBEES.NS".to_string(),
                name: None,
                shares: 100.0,
                price: 250.0,
                value: 25000.0, // 50% of portfolio
                sector: None,
                market_cap: None,
                currency: Some("INR".to_string()),
                is_fund: false, // tests that is_known_etf identifies it even if caller missed is_fund
                is_sgb: false,
                fund_category: None,
            },
            XrayInputHolding {
                symbol: "TCS.NS".to_string(),
                name: Some("Tata Consultancy Services".to_string()),
                shares: 5.0,
                price: 5000.0,
                value: 25000.0, // 50% of portfolio
                sector: Some("Technology".to_string()),
                market_cap: Some(1_400_000_000_000.0),
                currency: Some("INR".to_string()),
                is_fund: false,
                is_sgb: false,
                fund_category: None,
            },
        ];

        let fund_profiles = HashMap::new();
        let report = compute_xray("etf_test", "INR", &holdings, &fund_profiles);
        assert_eq!(report.total_value, 50000.0);

        // 1. NIFTYBEES is NOT flagged as a concentrated single-stock risk!
        // Only TCS should be flagged.
        assert_eq!(report.concentrations_above_10pct.len(), 1);
        assert_eq!(report.concentrations_above_10pct[0].symbol, "TCS.NS");
        assert!(
            !report
                .concentrations_above_10pct
                .iter()
                .any(|c| canonical_ticker(&c.symbol) == "NIFTYBEES")
        );

        // 2. NIFTYBEES is listed under funds analyzed
        assert_eq!(report.funds_analyzed.len(), 1);
        let fund = &report.funds_analyzed[0];
        assert_eq!(fund.symbol, "NIFTYBEES.NS");
        assert!(!fund.look_through_available);
        assert_eq!(fund.holdings_count, 0);
        assert_eq!(fund.coverage_pct, 0.0);
        assert_eq!(fund.name.as_deref(), Some("Nippon India ETF Nifty 50 BeES"));

        // 3. Asset class is 100% Equity
        assert_eq!(report.asset_class_split.equity_value, 50000.0);
        assert_eq!(report.asset_class_split.equity_pct, 100.0);

        // 4. Market cap is 100% Large Cap (0% Unclassified)
        assert_eq!(report.market_cap_breakdown.large_cap_value, 50000.0);
        assert_eq!(report.market_cap_breakdown.large_cap_pct, 100.0);
        assert_eq!(report.market_cap_breakdown.unclassified_value, 0.0);
        assert_eq!(report.market_cap_breakdown.unclassified_pct, 0.0);

        // 5. Sector allocation includes Technology (TCS) and Broad Market / Index (NIFTYBEES)
        assert_eq!(report.sector_allocation.len(), 2);
        let bm = report
            .sector_allocation
            .iter()
            .find(|s| s.sector == "Broad Market / Index")
            .unwrap();
        assert_eq!(bm.value, 25000.0);
        assert_eq!(bm.weight_pct, 50.0);
    }

    #[test]
    fn test_xray_indian_etf_bankbees_without_profile() {
        let holdings = vec![XrayInputHolding {
            symbol: "BANKBEES.NS".to_string(),
            name: None,
            shares: 100.0,
            price: 500.0,
            value: 50000.0,
            sector: None,
            market_cap: None,
            currency: Some("INR".to_string()),
            is_fund: true,
            is_sgb: false,
            fund_category: None,
        }];

        let report = compute_xray("bank_test", "INR", &holdings, &HashMap::new());
        assert_eq!(report.total_value, 50000.0);
        assert!(report.concentrations_above_10pct.is_empty());

        assert_eq!(report.funds_analyzed.len(), 1);
        assert!(!report.funds_analyzed[0].look_through_available);
        assert_eq!(
            report.funds_analyzed[0].name.as_deref(),
            Some("Nippon India ETF Nifty Bank BeES")
        );

        assert_eq!(report.market_cap_breakdown.large_cap_pct, 100.0);
        assert_eq!(report.sector_allocation.len(), 1);
        assert_eq!(report.sector_allocation[0].sector, "Financial Services");
    }

    #[test]
    fn test_xray_indian_etf_with_index_constituents() {
        let mut fund_profiles = HashMap::new();
        // Simulate index constituents returned by client.index_constituents("NIFTY 50")
        fund_profiles.insert(
            "NIFTYBEES.NS".to_string(),
            FundProfile {
                symbol: "NIFTYBEES.NS".to_string(),
                name: Some("Nippon India ETF Nifty 50 BeES".to_string()),
                family: None,
                category: Some("NIFTY 50".to_string()),
                expense_ratio: None,
                currency: Some("INR".to_string()),
                holdings: vec![
                    FundConstituent {
                        symbol: Some("RELIANCE.NS".to_string()),
                        name: Some("Reliance Industries Ltd".to_string()),
                        weight: 0.10, // 10%
                    },
                    FundConstituent {
                        symbol: Some("TCS.NS".to_string()),
                        name: Some("Tata Consultancy Services".to_string()),
                        weight: 0.08, // 8%
                    },
                ],
                sectors: vec![],
            },
        );

        let holdings = vec![
            XrayInputHolding {
                symbol: "RELIANCE.NS".to_string(),
                name: Some("Reliance Industries Ltd".to_string()),
                shares: 10.0,
                price: 2500.0,
                value: 25000.0, // Direct: 25%
                sector: Some("Energy".to_string()),
                market_cap: Some(2_000_000_000_000.0),
                currency: Some("INR".to_string()),
                is_fund: false,
                is_sgb: false,
                fund_category: None,
            },
            XrayInputHolding {
                symbol: "NIFTYBEES.NS".to_string(),
                name: Some("Nippon India ETF Nifty 50 BeES".to_string()),
                shares: 300.0,
                price: 250.0,
                value: 75000.0, // Fund: 75%
                sector: None,
                market_cap: None,
                currency: Some("INR".to_string()),
                is_fund: true,
                is_sgb: false,
                fund_category: Some("NIFTY 50".to_string()),
            },
        ];

        let report = compute_xray("overlap_test", "INR", &holdings, &fund_profiles);
        assert_eq!(report.total_value, 100000.0);

        // Funds analyzed: look-through available with 2 holdings
        assert_eq!(report.funds_analyzed.len(), 1);
        assert!(report.funds_analyzed[0].look_through_available);
        assert_eq!(report.funds_analyzed[0].holdings_count, 2);

        // RELIANCE combined exposure:
        // Direct: 25,000 (25.0%) + via NIFTYBEES (75,000 * 0.10 = 7,500, 7.5%) = 32,500 (32.5%)
        let rel = report
            .effective_holdings
            .iter()
            .find(|h| canonical_ticker(&h.symbol) == "RELIANCE")
            .unwrap();
        assert_eq!(rel.direct_value, 25000.0);
        assert_eq!(rel.fund_value, 7500.0);
        assert_eq!(rel.total_value, 32500.0);
        assert_eq!(rel.effective_weight_pct, 32.5);
        assert!(rel.is_concentrated);

        // NIFTYBEES itself is not a direct holding in companies
        assert!(
            !report
                .effective_holdings
                .iter()
                .any(|h| canonical_ticker(&h.symbol) == "NIFTYBEES")
        );
    }

    #[test]
    fn test_xray_indian_commodity_and_cash_etfs() {
        let holdings = vec![
            XrayInputHolding {
                symbol: "GOLDBEES.NS".to_string(),
                name: None,
                shares: 100.0,
                price: 50.0,
                value: 5000.0,
                sector: None,
                market_cap: None,
                currency: Some("INR".to_string()),
                is_fund: true,
                is_sgb: false,
                fund_category: None,
            },
            XrayInputHolding {
                symbol: "LIQUIDBEES.NS".to_string(),
                name: None,
                shares: 10.0,
                price: 1000.0,
                value: 10000.0,
                sector: None,
                market_cap: None,
                currency: Some("INR".to_string()),
                is_fund: true,
                is_sgb: false,
                fund_category: None,
            },
        ];

        let report = compute_xray("macro_etf_test", "INR", &holdings, &HashMap::new());
        assert_eq!(report.total_value, 15000.0);

        assert_eq!(report.asset_class_split.commodity_value, 5000.0);
        assert_eq!(report.asset_class_split.commodity_pct, 33.33);
        assert_eq!(report.asset_class_split.cash_value, 10000.0);
        assert_eq!(report.asset_class_split.cash_pct, 66.67);

        assert_eq!(report.funds_analyzed.len(), 2);
        assert!(report.concentrations_above_10pct.is_empty());
    }

    #[test]
    fn test_known_etf_helpers() {
        assert!(is_known_etf("NIFTYBEES"));
        assert!(is_known_etf("NIFTYBEES.NS"));
        assert!(is_known_etf("BANKBEES.NS"));
        assert!(is_known_etf("JUNIORBEES"));
        assert!(is_known_etf("GOLDBEES"));
        assert!(is_known_etf("SILVERBEES"));
        assert!(is_known_etf("LIQUIDBEES"));
        assert!(is_known_etf("SETFNIF50"));
        assert!(is_known_etf("MYFUND_ETF"));
        assert!(!is_known_etf("RELIANCE.NS"));
        assert!(!is_known_etf("AAPL"));

        assert_eq!(index_for_etf("NIFTYBEES.NS"), Some("NIFTY 50"));
        assert_eq!(index_for_etf("JUNIORBEES.NS"), Some("NIFTY NEXT 50"));
        assert_eq!(index_for_etf("BANKBEES"), Some("NIFTY BANK"));
        assert_eq!(index_for_etf("MID150BEES.NS"), Some("NIFTY MIDCAP 150"));
        assert_eq!(index_for_etf("GOLDBEES"), None);

        assert_eq!(
            default_etf_name("NIFTYBEES"),
            Some("Nippon India ETF Nifty 50 BeES")
        );
        assert_eq!(
            default_etf_name("BANKBEES"),
            Some("Nippon India ETF Nifty Bank BeES")
        );
        assert_eq!(
            default_etf_category("NIFTYBEES"),
            Some("Equity Scheme - Large Cap (NIFTY 50)")
        );
        assert_eq!(default_etf_sector("BANKBEES"), Some("Financial Services"));
        assert_eq!(
            default_etf_sector("NIFTYBEES"),
            Some("Broad Market / Index")
        );
    }
}
