//! Localized user-facing text: a typed message catalog for English and Hindi.
//!
//! Every message is a `Msg` variant with one string per language, declared
//! together in the `messages!` table below, so a key cannot exist without a
//! translation. Placeholders are positional (`{0}`, `{1}`) and filled with
//! [`tf`]; a test checks both languages use the same ones.
//!
//! Only human-readable output is localized. `--json` / `--csv` output and
//! numbers stay language-neutral; Indian amounts keep lakh/crore grouping in
//! both languages.

use clap::ValueEnum;
use std::fmt::Display;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Lang {
    /// English (default)
    #[value(alias = "english")]
    En,
    /// हिन्दी (Hindi)
    #[value(alias = "hindi")]
    Hi,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Hi => "hi",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Lang::from_str(value.trim(), true).ok()
    }
}

static LANG: OnceLock<(Lang, LangSource)> = OnceLock::new();

/// Set the output language. Called once at startup; later calls are ignored.
pub fn set_lang(lang: Lang, source: LangSource) {
    let _ = LANG.set((lang, source));
}

/// The output language (English until `set_lang` is called).
pub fn lang() -> Lang {
    LANG.get().map_or(Lang::En, |(lang, _)| *lang)
}

/// Where the active language came from.
pub fn lang_source() -> LangSource {
    LANG.get()
        .map_or(LangSource::Default, |(_, source)| *source)
}

/// Where the language came from, for `doctor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LangSource {
    Flag,
    StockwiseLang,
    /// `lang` in the config file.
    Config,
    Locale,
    Default,
}

/// Pick the language before clap parses the command line (the help text is
/// built from it): `--lang` beats `STOCKWISE_LANG`, which beats a Hindi POSIX
/// locale (`LC_ALL`, `LC_MESSAGES`, `LANG` = `hi_IN.UTF-8`). Anything else is
/// English. An invalid `--lang` or `STOCKWISE_LANG` falls through here and is
/// reported by clap.
pub fn detect(args: &[String], env: impl Fn(&str) -> Option<String>) -> (Lang, LangSource) {
    if let Some(lang) = lang_flag(args).as_deref().and_then(Lang::parse) {
        return (lang, LangSource::Flag);
    }
    if let Some(lang) = env("STOCKWISE_LANG").as_deref().and_then(Lang::parse) {
        return (lang, LangSource::StockwiseLang);
    }
    // POSIX precedence: the first non-empty of these decides.
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| env(k))
        .find(|v| !v.is_empty());
    if let Some(locale) = locale {
        let code = locale.split(['_', '.', '@', '-']).next().unwrap_or("");
        if code.eq_ignore_ascii_case("hi") {
            return (Lang::Hi, LangSource::Locale);
        }
    }
    (Lang::En, LangSource::Default)
}

/// [`detect`], with `lang` from the config file between `STOCKWISE_LANG`
/// and the locale: an explicit setting beats the system default.
pub fn detect_with_config(
    args: &[String],
    env: impl Fn(&str) -> Option<String>,
    file: Option<&str>,
) -> (Lang, LangSource) {
    let detected = detect(args, env);
    match (detected.1, file.and_then(Lang::parse)) {
        (LangSource::Locale | LangSource::Default, Some(lang)) => (lang, LangSource::Config),
        _ => detected,
    }
}

/// Whether `value` names a language (`en`, `hi`, ...).
pub fn is_lang(value: &str) -> bool {
    Lang::parse(value).is_some()
}

/// The value of `--lang X` / `--lang=X`, ignoring anything after `--`.
fn lang_flag(args: &[String]) -> Option<String> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if arg == "--lang" {
            return iter.next().cloned();
        }
        if let Some(v) = arg.strip_prefix("--lang=") {
            return Some(v.to_string());
        }
    }
    None
}

/// Text of `msg` in the active language.
pub fn t(msg: Msg) -> &'static str {
    msg.text(lang())
}

/// `t(msg)` with `{0}`, `{1}`, ... replaced by `args`.
pub fn tf(msg: Msg, args: &[&dyn Display]) -> String {
    fill(t(msg), args)
}

/// [`tf`] in a given language — for text that stays English whatever
/// `--lang` says (notifications, `--json`).
pub fn tf_in(lang: Lang, msg: Msg, args: &[&dyn Display]) -> String {
    fill(msg.text(lang), args)
}

fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = template.to_string();
    for (i, arg) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), &arg.to_string());
    }
    out
}

macro_rules! messages {
    ($($key:ident => $en:literal, $hi:literal;)*) => {
        /// A localized message. See the table in `i18n.rs`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Msg {
            $($key,)*
        }

        impl Msg {
            #[cfg(test)]
            pub const ALL: &'static [Msg] = &[$(Msg::$key,)*];

            pub fn text(self, lang: Lang) -> &'static str {
                match lang {
                    Lang::En => match self { $(Msg::$key => $en,)* },
                    Lang::Hi => match self { $(Msg::$key => $hi,)* },
                }
            }
        }
    };
}

messages! {
    // ── Help ──
    HelpUsage => "Usage:", "उपयोग:";
    HelpOptions => "Options:", "विकल्प:";
    HelpPrintHelp => "Print help for a command", "किसी कमांड की सहायता दिखाएँ";
    GroupStartHere => "Start here", "यहाँ से शुरू करें";
    GroupAnalysis => "Analysis", "विश्लेषण";
    GroupMarket => "Market", "बाज़ार";
    GroupStrategies => "Strategies & bots", "रणनीतियाँ और बॉट";
    GroupFunds => "Funds & F&O", "फंड और F&O";
    GroupCommoditiesBonds => "Commodities & bonds", "कमोडिटी और बॉन्ड";
    GroupPortfolio => "Portfolio", "पोर्टफोलियो";
    GroupInterfaces => "Interfaces", "इंटरफ़ेस";
    GroupLiveTrading => "Live trading", "लाइव ट्रेडिंग";
    GroupSetup => "Setup", "सेटअप";
    GroupPlanning => "Planning", "योजना";
    GroupAi => "AI assistants", "एआई सहायक";
    AboutQuote => "Real-time quote for one or more stocks", "एक या अधिक शेयरों का ताज़ा भाव";
    AboutDeep => "Everything about a stock in one command", "एक कमांड में किसी शेयर की पूरी जानकारी";
    AboutMarkets => "Major market indices (US + India)", "प्रमुख बाज़ार सूचकांक (अमेरिका + भारत)";
    AboutDashboard => "Markets + portfolio + watchlist + alerts on one screen", "बाज़ार, पोर्टफोलियो, वॉचलिस्ट और अलर्ट एक ही स्क्रीन पर";
    AboutSearch => "Find a ticker by company name", "कंपनी के नाम से टिकर खोजें";
    AboutDoctor => "Check your setup: data directory, network, cache, AI, broker", "अपना सेटअप जाँचें: डेटा फ़ोल्डर, नेटवर्क, कैश, AI, ब्रोकर";
    AboutPortfolio => "Track holdings and P&L (default: show)", "होल्डिंग्स और लाभ/हानि ट्रैक करें (डिफ़ॉल्ट: दिखाएँ)";
    AboutWatch => "Manage your watchlist (default: show)", "अपनी वॉचलिस्ट संभालें (डिफ़ॉल्ट: दिखाएँ)";
    AboutAlert => "Price and technical alerts (default: list)", "भाव और तकनीकी अलर्ट (डिफ़ॉल्ट: सूची)";
    AboutCompletions => "Print a shell completion script (bash, zsh, fish, powershell, elvish)", "शेल कम्प्लीशन स्क्रिप्ट दिखाएँ (bash, zsh, fish, powershell, elvish)";

    // ── Markets ──
    MarketIndia => "India", "भारत";
    MarketUs => "US", "अमेरिका";
    MarketCrypto => "Crypto", "क्रिप्टो";
    MarketFx => "FX", "विदेशी मुद्रा";
    MarketCommodity => "Commodities", "कमोडिटी";
    MarketUk => "UK", "ब्रिटेन";
    MarketDe => "Germany", "जर्मनी";
    MarketJp => "Japan", "जापान";
    MarketHk => "Hong Kong", "हांगकांग";
    MarketSg => "Singapore", "सिंगापुर";
    MarketCa => "Canada", "कनाडा";
    MarketAu => "Australia", "ऑस्ट्रेलिया";
    SectionIndianMarkets => "Indian Markets", "भारतीय बाज़ार";
    SectionUsMarkets => "US Markets", "अमेरिकी बाज़ार";
    SectionCrypto => "Crypto (24/7)", "क्रिप्टो (24/7)";
    SectionFx => "Currencies (FX)", "मुद्राएँ (FX)";
    SectionGlobal => "Global Markets", "वैश्विक बाज़ार";
    SectionCommodities => "Gold & Silver", "सोना और चाँदी";
    SessionCrypto => "Trades 24/7 — change is since 00:00 UTC", "24/7 ट्रेडिंग — बदलाव 00:00 UTC से";
    SessionFxOpen => "FX trades 24 hours, Monday to Friday", "विदेशी मुद्रा बाज़ार सोमवार से शुक्रवार, 24 घंटे खुला";
    SessionFxWeekend => "FX market closed for the weekend — showing Friday's close", "सप्ताहांत पर विदेशी मुद्रा बाज़ार बंद — शुक्रवार का बंद भाव";

    // ── Quote labels ──
    Price => "Price", "कीमत";
    Change => "Change", "बदलाव";
    Open => "Open", "ओपन";
    DayRange => "Day Range", "दिन की रेंज";
    WeekRange52 => "52-Week Range", "52-सप्ताह रेंज";
    Volume => "Volume", "वॉल्यूम";
    AvgVolume3M => "Avg Volume (3M)", "औसत वॉल्यूम (3 माह)";
    MarketCap => "Market Cap", "मार्केट कैप";
    PeTtm => "P/E (TTM)", "P/E (TTM)";
    EpsTtm => "EPS (TTM)", "EPS (TTM)";
    Ma50 => "50-Day MA", "50-दिन MA";
    Ma200 => "200-Day MA", "200-दिन MA";
    DividendYield => "Dividend Yield", "डिविडेंड यील्ड";
    DeliveryPercent => "Delivery", "डिलीवरी";
    UniverseFallback => "NSE's {0} member list is unavailable ({1}); scanning the built-in list of {2} stocks", "NSE की {0} सदस्य सूची उपलब्ध नहीं ({1}); {2} शेयरों की अंतर्निहित सूची देखी जा रही है";
    CircuitLimits => "Circuit Limits", "सर्किट सीमा";
    Unknown => "Unknown", "अज्ञात";
    DataFrom => "Data: {0}", "डेटा: {0}";
    NoQuoteFor => "No quote for {0}{1}", "{0} का भाव नहीं मिला{1}";
    HintNotFoundIn => " (Indian market is the default — for US stocks add `-m us`; use `stockwise search <name>` to find a ticker)", " (भारतीय बाज़ार डिफ़ॉल्ट है — अमेरिकी शेयरों के लिए `-m us` जोड़ें; टिकर खोजने के लिए `stockwise search <नाम>` चलाएँ)";
    HintNotFoundUs => " (use `stockwise search <name>` to find a ticker; Indian stocks need `-m in` or a .NS suffix)", " (टिकर खोजने के लिए `stockwise search <नाम>` चलाएँ; भारतीय शेयरों के लिए `-m in` या .NS जोड़ें)";
    HintNotFoundCrypto => " (use the coin's ticker, e.g. BTC or ETH, or a full pair like BTC-INR)", " (कॉइन का टिकर लिखें, जैसे BTC या ETH, या पूरा जोड़ा जैसे BTC-INR)";
    HintNotFoundFx => " (use a currency pair such as USDINR or EURUSD)", " (मुद्रा जोड़ा लिखें, जैसे USDINR या EURUSD)";
    HintNotFoundCommodity => " (use GOLD or SILVER, an NSE ETF such as GOLDBEES, or a futures symbol like GC=F)", " (GOLD या SILVER, NSE ETF जैसे GOLDBEES, या फ़्यूचर्स सिंबल जैसे GC=F लिखें)";
    HintNotFoundGlobal => " (bare tickers get this exchange's suffix, e.g. -m uk VOD → VOD.L; use `stockwise search <name>` to find a ticker)", " (बिना सफ़िक्स वाले टिकर को इस एक्सचेंज का सफ़िक्स मिलता है, जैसे -m uk VOD → VOD.L; टिकर खोजने के लिए `stockwise search <नाम>` चलाएँ)";

    // ── Search ──
    SearchTitle => "Search: {0}", "खोज: {0}";
    Symbol => "Symbol", "सिंबल";
    Name => "Name", "नाम";
    Type => "Type", "प्रकार";
    Exchange => "Exchange", "एक्सचेंज";
    NoTickersMatch => "No tickers match '{0}'", "'{0}' से कोई टिकर नहीं मिला";

    // ── Dashboard ──
    DashboardTitle => "Dashboard — {0}", "डैशबोर्ड — {0}";
    MarketIndices => "Market Indices", "बाज़ार सूचकांक";
    TopCoins => "Top Coins", "प्रमुख कॉइन";
    MajorPairs => "Major Pairs", "प्रमुख मुद्रा जोड़े";
    PortfolioCount => "Portfolio ({0} holdings)", "पोर्टफोलियो ({0} होल्डिंग्स)";
    WatchlistCount => "Watchlist ({0} symbols)", "वॉचलिस्ट ({0} सिंबल)";
    TotalValue => "Total Value", "कुल मूल्य";
    TotalInvested => "Total Invested", "कुल निवेश";
    PnL => "P&L", "लाभ/हानि";
    PnLPct => "P&L %", "लाभ/हानि %";
    TriggeredAlerts => "Triggered Alerts", "ट्रिगर हुए अलर्ट";
    GetStarted => "Get started", "शुरुआत करें";
    GetStartedTrack => "track a holding (symbol, shares, cost)", "होल्डिंग जोड़ें (सिंबल, शेयर, लागत)";
    GetStartedImport => "or import from your broker", "या अपने ब्रोकर से इम्पोर्ट करें";
    GetStartedWatch => "watch a few stocks", "कुछ शेयरों पर नज़र रखें";
    GetStartedAlert => "get alerted on a price level", "किसी भाव पर अलर्ट पाएँ";

    // ── Portfolio ──
    YourPortfolio => "Your Portfolio", "आपका पोर्टफोलियो";
    Shares => "Shares", "शेयर";
    AvgCost => "Avg Cost", "औसत लागत";
    Value => "Value", "मूल्य";
    Total => "TOTAL", "कुल";
    PortfolioEmpty => "Portfolio is empty. Use 'stockwise portfolio add <SYMBOL> <SHARES> <COST>' to add holdings.", "पोर्टफोलियो खाली है। होल्डिंग जोड़ने के लिए 'stockwise portfolio add <SYMBOL> <SHARES> <COST>' चलाएँ।";

    // ── Indian number units (lakh / crore) ──
    UnitLakhCrore => "L Cr", " लाख करोड़";
    UnitThousandCrore => "K Cr", " हज़ार करोड़";
    UnitCrore => " Cr", " करोड़";
    UnitLakh => "L", " लाख";

    // ── Doctor ──
    DoctorTitle => "StockWise {0} — doctor", "StockWise {0} — डॉक्टर";
    DoctorMarket => "Market", "बाज़ार";
    DoctorMarketFromEnv => "from STOCKWISE_MARKET", "STOCKWISE_MARKET से";
    DoctorMarketDefault => "default; change with -m or STOCKWISE_MARKET", "डिफ़ॉल्ट; -m या STOCKWISE_MARKET से बदलें";
    DoctorLanguage => "Language", "भाषा";
    DoctorLangFlag => "from --lang", "--lang से";
    DoctorLangEnv => "from STOCKWISE_LANG", "STOCKWISE_LANG से";
    DoctorLangConfig => "from the config file", "कॉन्फ़िग फ़ाइल से";
    DoctorLangLocale => "from your locale (LANG)", "आपके लोकेल (LANG) से";
    DoctorLangDefault => "default; --lang hi or STOCKWISE_LANG=hi for Hindi", "डिफ़ॉल्ट; हिन्दी के लिए --lang hi या STOCKWISE_LANG=hi";
    DoctorDataDir => "Data directory", "डेटा फ़ोल्डर";
    DoctorDataFiles => "Data files", "डेटा फ़ाइलें";
    DoctorNoFilesYet => "none yet (created on first use)", "अभी कोई नहीं (पहली बार उपयोग पर बनेंगी)";
    DoctorReachable => "reachable ({0} ms)", "उपलब्ध ({0} ms)";
    DoctorYahooUnreachable => "unreachable — check your internet connection or proxy", "पहुँच से बाहर — अपना इंटरनेट कनेक्शन या प्रॉक्सी जाँचें";
    DoctorDataProvider => "Data provider", "डेटा प्रदाता";
    DoctorProviderDefault => "Yahoo Finance (default; set STOCKWISE_DATA_PROVIDER for another)", "Yahoo Finance (डिफ़ॉल्ट; दूसरे के लिए STOCKWISE_DATA_PROVIDER सेट करें)";
    DoctorProviderAnswered => "{0} answered for {1} ({2} ms); Yahoo fills missing fields", "{1} के लिए {0} ने जवाब दिया ({2} ms); बाकी फ़ील्ड Yahoo से";
    DoctorProviderFellBack => "{0} did not answer ({1}); using Yahoo instead", "{0} से जवाब नहीं मिला ({1}); Yahoo का उपयोग हो रहा है";
    DoctorProviderNoKey => "{0} selected but {1} is not set; using Yahoo", "{0} चुना गया पर {1} सेट नहीं है; Yahoo का उपयोग हो रहा है";
    DoctorProviderUnknown => "unknown provider '{0}' — use yahoo, alphavantage, massive (polygon), twelvedata, nse, bse or exchange", "अज्ञात प्रदाता '{0}' — yahoo, alphavantage, massive (polygon), twelvedata, nse, bse या exchange चुनें";
    DoctorExchangeNote => "NSE/BSE website data is unofficial; when they refuse automated requests, Yahoo answers", "NSE/BSE वेबसाइट का डेटा अनौपचारिक है; जब वे स्वचालित अनुरोध रोकते हैं, तब Yahoo जवाब देता है";
    DoctorRedis => "Redis cache", "Redis कैश";
    DoctorConnected => "connected", "जुड़ा हुआ";
    DoctorRedisNotUsed => "not used (optional; in-memory cache instead)", "उपयोग में नहीं (वैकल्पिक; मेमोरी कैश चल रहा है)";
    DoctorAi => "AI", "AI";
    DoctorOllamaReady => "Ollama model {0} ready — use `report` or add --ai", "Ollama मॉडल {0} तैयार — `report` चलाएँ या --ai जोड़ें";
    DoctorOllamaNoModel => "Ollama is running, but model {0} is not pulled — run `ollama pull {0}`", "Ollama चल रहा है, पर मॉडल {0} डाउनलोड नहीं है — `ollama pull {0}` चलाएँ";
    DoctorOllamaDown => "Ollama not running (optional; needed for `report` and --ai)", "Ollama नहीं चल रहा (वैकल्पिक; `report` और --ai के लिए ज़रूरी)";
    DoctorCloudReady => "{0}, model {1} — key from {2}; prompts and market data are sent to {0}", "{0}, मॉडल {1} — कुंजी {2} से; प्रॉम्प्ट और मार्केट डेटा {0} को भेजा जाता है";
    DoctorCloudNoKey => "{0} selected but {1} is not set", "{0} चुना गया पर {1} सेट नहीं है";
    DoctorAiUnknown => "unknown provider '{0}' — use ollama, openai, anthropic or gemini", "अज्ञात प्रदाता '{0}' — ollama, openai, anthropic या gemini चुनें";
    DoctorAngel => "Angel One", "Angel One";
    DoctorAngelConfigured => "configured for {0}{1}", "{0} के लिए कॉन्फ़िगर{1}";
    DoctorAngelMaxOrder => ", max order ₹{0}", ", अधिकतम ऑर्डर ₹{0}";
    DoctorAngelNotConfigured => "not configured (optional; `trade setup` for live trading)", "कॉन्फ़िगर नहीं (वैकल्पिक; लाइव ट्रेडिंग के लिए `trade setup`)";
    DoctorKeyring => "Keyring", "कीरिंग";
    DoctorKeyringAvailable => "{0} available for broker secrets", "ब्रोकर सीक्रेट्स के लिए {0} उपलब्ध";
    DoctorKeyringNotUsed => "not used ({0}); broker secrets stay in owner-only files", "उपयोग में नहीं ({0}); ब्रोकर सीक्रेट्स केवल आपकी पढ़ी जा सकने वाली फ़ाइलों में रहते हैं";
    DoctorBrokerPlaceholder => "placeholder values in {0} — `trade config --broker {1}`", "{0} में अभी नमूना मान हैं — `trade config --broker {1}`";
    DoctorBrokerOrdersOn => "live orders enabled", "लाइव ऑर्डर चालू";
    DoctorBrokerOrdersOff => "live orders off (read-only)", "लाइव ऑर्डर बंद (केवल देखना)";
    DoctorAngelStopGttOn => "SL/SL-M and GTT enabled", "SL/SL-M और GTT चालू";
    DoctorMarketDataDown => "Market data is unreachable", "मार्केट डेटा तक पहुँच नहीं हो पा रही";

    // ── Goal planner ──
    GoalSipTitle => "Goal: {0} in {1} years", "लक्ष्य: {1} साल में {0}";
    GoalRetireTitle => "Retirement in {0} years", "{0} साल में रिटायरमेंट";
    GoalEducationTitle => "Education goal in {0} years", "{0} साल में शिक्षा का लक्ष्य";
    GoalMonthlySip => "Monthly SIP needed", "ज़रूरी मासिक SIP";
    GoalFinalSip => "SIP in the last year", "आख़िरी साल में SIP";
    GoalStepUp => "Yearly step-up", "सालाना बढ़ोतरी";
    GoalExpectedReturn => "Expected return", "अनुमानित रिटर्न";
    GoalTotalInvested => "Total invested", "कुल निवेश";
    GoalWealthGained => "Wealth gained", "कुल बढ़त";
    GoalLumpSum => "Or a lump sum today", "या आज एकमुश्त निवेश";
    GoalSavings => "Existing savings", "मौजूदा बचत";
    GoalSavingsGrowTo => "Savings grow to", "बचत बढ़कर होगी";
    GoalCovered => "Your savings already cover this goal at the expected return.", "अनुमानित रिटर्न पर आपकी मौजूदा बचत से यह लक्ष्य पूरा हो जाता है।";
    GoalYearByYear => "Year by year", "साल-दर-साल";
    GoalYear => "Year", "साल";
    GoalSipColumn => "Monthly SIP", "मासिक SIP";
    GoalInvested => "Invested", "निवेश";
    GoalExpenseToday => "Expense today", "आज का ख़र्च";
    GoalExpenseAtRetirement => "Expense at retirement", "रिटायरमेंट पर ख़र्च";
    GoalInflation => "Inflation", "महँगाई";
    GoalYearsInRetirement => "Years in retirement", "रिटायरमेंट के साल";
    GoalReturnBefore => "Return before retirement", "रिटायरमेंट से पहले रिटर्न";
    GoalReturnAfter => "Return after retirement", "रिटायरमेंट के बाद रिटर्न";
    GoalCorpusNeeded => "Corpus needed at retirement", "रिटायरमेंट पर ज़रूरी राशि";
    GoalCostToday => "Cost today", "आज की लागत";
    GoalFutureCost => "Cost in {0} years", "{0} साल बाद लागत";
    GoalPerYear => "{0}% a year", "{0}% सालाना";
    GoalPerMonth => "{0} a month", "{0} प्रति माह";
    GoalAssumptions => "Assumes the same return every year, SIPs at the start of each month and a monthly rate of annual ÷ 12. Returns are not guaranteed — not financial advice.", "हर साल एक जैसा रिटर्न, हर महीने की शुरुआत में SIP और मासिक दर = सालाना ÷ 12 मानकर। रिटर्न की गारंटी नहीं — यह वित्तीय सलाह नहीं है।";

    // ── Benchmark ──
    BenchTitle => "Portfolio vs {0}", "पोर्टफोलियो बनाम {0}";
    BenchPortfolio => "Portfolio", "पोर्टफोलियो";
    BenchGrowth => "Growth of 100", "100 की बढ़त";
    BenchPeriod => "Period", "अवधि";
    BenchTradingDays => "{0} trading days", "{0} कारोबारी दिन";
    BenchPortfolioReturn => "Portfolio return", "पोर्टफोलियो रिटर्न";
    BenchIndexReturn => "{0} return", "{0} रिटर्न";
    BenchExcess => "Excess return", "अतिरिक्त रिटर्न";
    BenchAnnualized => "Annualized", "वार्षिक";
    BenchAlpha => "Alpha (annualized)", "अल्फ़ा (वार्षिक)";
    BenchBeta => "Beta", "बीटा";
    BenchTrackingError => "Tracking error", "ट्रैकिंग एरर";
    BenchInfoRatio => "Information ratio", "इन्फ़ॉर्मेशन रेशियो";
    BenchCorrelation => "Correlation", "सहसंबंध";
    BenchByHolding => "Attribution by holding", "होल्डिंग के अनुसार योगदान";
    BenchBySector => "Attribution by sector", "सेक्टर के अनुसार योगदान";
    BenchSector => "Sector", "सेक्टर";
    BenchAvgWeight => "Avg wt", "औसत भार";
    BenchReturn => "Return", "रिटर्न";
    BenchContribution => "Contrib", "योगदान";
    BenchVsIndex => "vs index", "इंडेक्स से";
    BenchFlagged => "Partial history", "अधूरा इतिहास";
    BenchTooFewDays => "Fewer than {0} daily returns — alpha and beta need a longer period.", "{0} से कम दैनिक रिटर्न — अल्फ़ा और बीटा के लिए लंबी अवधि चाहिए।";
    BenchNothingHeld => "Nothing in this portfolio was held with a price during the last {0}.", "पिछले {0} में इस पोर्टफोलियो में कीमत वाली कोई होल्डिंग नहीं थी।";
    BenchConverted => "{0} converted to {1} at daily exchange rates", "{0} को दैनिक विनिमय दर पर {1} में बदला गया";
    BenchMethod => "Time-weighted, from daily closes: trades count at the day's close and dividends are left out on both sides. Contributions are percentage points and add up to the totals.", "समय-भारित, दैनिक बंद भाव से: सौदे उसी दिन के बंद भाव पर गिने जाते हैं और डिविडेंड दोनों तरफ़ शामिल नहीं। योगदान प्रतिशत अंकों में हैं और जुड़कर कुल के बराबर होते हैं।";

    // ── Tax export ──
    TaxExportTitle => "Capital-gains schedule — {0}", "पूंजीगत लाभ विवरण — {0}";
    TaxExportEstimate => "Estimate — verify with your broker's tax P&L", "अनुमान — अपने ब्रोकर के टैक्स P&L से मिलान करें";
    TaxExportWrote => "Wrote {0} sales to {1}", "{0} बिक्री {1} में लिखी गईं";
    TaxExportNoSales => "No sales recorded in {0}. Record one with `stockwise portfolio sell` or import a tradebook.", "{0} में कोई बिक्री दर्ज नहीं। `stockwise portfolio sell` से दर्ज करें या ट्रेडबुक इम्पोर्ट करें।";
    TaxExportSales => "Sales", "बिक्री";
    TaxExportIsinMissing => "ISIN unknown for {0} — fill it in before filing", "{0} का ISIN नहीं मिला — रिटर्न भरने से पहले जोड़ें";

    // ── Alerts ──
    AlertsTitle => "Alerts", "अलर्ट";
    AlertAbove => "above {0}", "{0} से ऊपर";
    AlertBelow => "below {0}", "{0} से नीचे";
    AlertRsiBelow => "RSI({0}) below {1}", "RSI({0}) {1} से नीचे";
    AlertRsiAbove => "RSI({0}) above {1}", "RSI({0}) {1} से ऊपर";
    AlertCrossAbove => "close crosses above the {0}-day {1}", "क्लोज़ {0}-दिन के {1} को ऊपर की ओर पार करे";
    AlertCrossBelow => "close crosses below the {0}-day {1}", "क्लोज़ {0}-दिन के {1} को नीचे की ओर पार करे";
    AlertBreakoutHigh => "close above the previous {0}-day high", "क्लोज़ पिछले {0} दिनों के उच्चतम से ऊपर";
    AlertBreakdownLow => "close below the previous {0}-day low", "क्लोज़ पिछले {0} दिनों के न्यूनतम से नीचे";
    AlertChangeAbove => "day change of {0} or better", "दिन का बदलाव {0} या उससे अधिक";
    AlertChangeBelow => "day change of {0} or worse", "दिन का बदलाव {0} या उससे कम";
    AlertVolumeSpike => "volume {0}× its {1}-day average", "वॉल्यूम {1}-दिन के औसत का {0}×";
    AlertResultsIn => "results due within {0} days", "{0} दिनों के भीतर नतीजे";

    // ── Calendar ──
    CalendarTitle => "Results & corporate calendar — next {0} days", "नतीजे और कॉर्पोरेट कैलेंडर — अगले {0} दिन";
    CalDate => "Date", "तारीख";
    CalWhen => "When", "कब";
    CalEvent => "Event", "इवेंट";
    CalDetails => "Details", "विवरण";
    CalSource => "Source", "स्रोत";
    CalToday => "today", "आज";
    CalTomorrow => "tomorrow", "कल";
    CalInDays => "in {0} days", "{0} दिन में";
    CalEstimated => "estimated", "अनुमानित";
    CalThisWeek => "{0} event(s) in the next 7 days", "अगले 7 दिनों में {0} इवेंट";
    CalNoEvents => "No results, dividends or splits in the next {0} days for {1} symbol(s).", "अगले {0} दिनों में {1} सिंबल के लिए कोई नतीजे, डिविडेंड या स्प्लिट नहीं।";
    CalNoSymbols => "Nothing to look up — add holdings or watchlist symbols, or name them: `stockwise calendar TCS INFY`.", "देखने के लिए कुछ नहीं — होल्डिंग या वॉचलिस्ट में सिंबल जोड़ें, या नाम दें: `stockwise calendar TCS INFY`।";
    CalSources => "Source: Yahoo Finance (results, ex-dividend and payment dates).", "स्रोत: Yahoo Finance (नतीजे, एक्स-डिविडेंड और भुगतान की तारीखें)।";
    CalSourcesNse => "Indian stocks: also NSE board meetings and corporate actions (filed results dates, dividends, splits, bonus issues).", "भारतीय शेयर: NSE बोर्ड बैठकें और कॉर्पोरेट एक्शन भी (दाखिल नतीजों की तारीखें, डिविडेंड, स्प्लिट, बोनस)।";
    EventResults => "Results", "नतीजे";
    EventBoardMeeting => "Board meeting", "बोर्ड बैठक";
    EventExDividend => "Ex-dividend", "एक्स-डिविडेंड";
    EventDividendPaid => "Dividend paid", "डिविडेंड भुगतान";
    EventSplit => "Split", "स्प्लिट";
    EventBonus => "Bonus", "बोनस";
    EventRights => "Rights issue", "राइट्स इश्यू";
    EventOther => "Corporate action", "कॉर्पोरेट एक्शन";

    // ── Web dashboard (`serve`) ──
    ServeDashboard => "Dashboard", "डैशबोर्ड";
    ServeOpenFailed => "Could not open a browser ({0}); open {1} yourself", "ब्राउज़र नहीं खुल सका ({0}); {1} खुद खोलें";

    // ── Commodities (`commodity`) ──
    Gold => "Gold", "सोना";
    Silver => "Silver", "चाँदी";
    CommoditySpotTitle => "Gold & Silver — spot", "सोना और चाँदी — हाजिर भाव";
    CommoditySpotSource => "Spot ≈ COMEX front-month futures (GC=F, SI=F) at USD/INR {0}; 1 troy oz = 31.1035 g.", "हाजिर ≈ COMEX निकटतम फ़्यूचर्स (GC=F, SI=F), USD/INR {0} पर; 1 ट्रॉय औंस = 31.1035 ग्राम।";
    CommodityDutyNote => "₹ prices are international prices converted at USD/INR — they exclude import duty and GST, so jewellers' and MCX prices are higher.", "₹ भाव अंतरराष्ट्रीय भाव को USD/INR से बदलकर हैं — इनमें आयात शुल्क और GST शामिल नहीं, इसलिए ज्वैलर और MCX के भाव ऊँचे होते हैं।";
    CommodityMcxNote => "MCX gold and silver futures have no free public feed, so StockWise shows COMEX contracts (`commodity futures`).", "MCX सोना-चाँदी फ़्यूचर्स का कोई मुफ़्त सार्वजनिक फ़ीड नहीं है, इसलिए StockWise COMEX कॉन्ट्रैक्ट दिखाता है (`commodity futures`)।";
    CommodityFuturesTitle => "Gold & Silver futures — COMEX", "सोना और चाँदी फ़्यूचर्स — COMEX";
    CommodityFuturesNote => "COMEX (CME Group) contracts via Yahoo Finance, USD per troy ounce, front month. Not MCX: MCX has no free public feed.", "COMEX (CME Group) कॉन्ट्रैक्ट, Yahoo Finance से, USD प्रति ट्रॉय औंस, निकटतम माह। MCX नहीं: MCX का कोई मुफ़्त सार्वजनिक फ़ीड नहीं।";
    CommodityEtfsTitle => "Gold & Silver ETFs — NSE", "सोना और चाँदी ETF — NSE";
    CommodityEtfsNote => "₹ per unit. Each ETF holds a different amount of metal per unit, so compare their % moves, not their prices.", "₹ प्रति यूनिट। हर ETF की एक यूनिट में धातु की मात्रा अलग होती है, इसलिए भाव नहीं, % बदलाव की तुलना करें।";
    SgbTitle => "Sovereign Gold Bonds — NSE", "सॉवरेन गोल्ड बॉन्ड — NSE";
    SgbMatures => "Matures", "परिपक्वता";
    SgbYield => "Yield", "यील्ड";
    SgbSeries => "Series", "श्रृंखला";
    SgbIssuePrice => "Issue ₹/g", "निर्गम ₹/ग्राम";
    SgbInterest => "Interest/yr", "ब्याज/वर्ष";
    SgbPremium => "Premium", "प्रीमियम";
    SgbNoQuotes => "No SGB prices from NSE ({0}) — listing outstanding tranches without prices", "NSE से SGB भाव नहीं मिले ({0}) — बिना भाव के बकाया किश्तें दिखाई जा रही हैं";
    SgbGoldBasis => "Premium is over 1 g of gold at the international spot price, {0} (no import duty or GST); domestic gold, and so SGBs, usually trade above it. Yield = 2.5% interest on the issue price over today's price.", "प्रीमियम 1 ग्राम सोने के अंतरराष्ट्रीय हाजिर भाव {0} (बिना आयात शुल्क या GST) पर है; घरेलू सोना, और इसलिए SGB, आमतौर पर इससे ऊपर रहते हैं। यील्ड = निर्गम मूल्य पर 2.5% ब्याज ÷ आज का भाव।";
    SgbNote => "SGBs pay 2.5% a year on the issue price, half-yearly; 1 unit = 1 g. RBI redeems at the IBJA 999 average of the 3 business days before, 8 years after issue (early redemption from year 5, on interest dates).", "SGB निर्गम मूल्य पर सालाना 2.5% ब्याज देते हैं, छमाही; 1 यूनिट = 1 ग्राम। RBI जारी होने के 8 साल बाद पिछले 3 कारोबारी दिनों के IBJA 999 औसत पर भुनाता है (5वें साल से, ब्याज की तारीखों पर, जल्दी भुनाना संभव)।";
    SgbSymbolsUnconfirmed => "Symbols come from a public list of tranches, not from NSE — check them before you trade.", "सिंबल किश्तों की एक सार्वजनिक सूची से हैं, NSE से नहीं — ट्रेड से पहले जाँच लें।";

    // ── Bonds (`bonds`) ──
    BondsYieldsTitle => "India G-sec & T-bill yields — {0}, {1}", "भारत G-sec और T-bill यील्ड — {0}, {1}";
    BondsTenor => "Tenor", "अवधि";
    BondsSecurity => "Security", "प्रतिभूति";
    BondsYtm => "Yield to maturity", "परिपक्वता यील्ड";
    BondsIndia10y => "India 10Y:", "भारत 10 वर्ष:";
    BondsTenYearIs => "{0} at {1}", "{0}, {1} पर";
    BondsCcilNote => "Indicative yields; T-bill and SDL rows are primary-market cut-offs. CCIL data, for personal non-commercial use.", "सांकेतिक यील्ड; T-bill और SDL पंक्तियाँ प्राथमिक बाज़ार कट-ऑफ़ हैं। CCIL डेटा, केवल निजी गैर-व्यावसायिक उपयोग के लिए।";
    BondsRbiNote => "RBI benchmark yields (CCIL was not available). T-bill rows are the last auction cut-offs; N/A when RBI shows none.", "RBI बेंचमार्क यील्ड (CCIL उपलब्ध नहीं था)। T-bill पंक्तियाँ पिछली नीलामी के कट-ऑफ़ हैं; RBI न दिखाए तो N/A।";
    BondsSourceUrl => "Source: {0}", "स्रोत: {0}";
    BondsListTitle => "Government of India securities outstanding — RBI", "बकाया भारत सरकार प्रतिभूतियाँ — RBI";
    BondsListEmpty => "No outstanding G-sec matches '{0}'", "'{0}' से कोई बकाया G-sec नहीं मिली";
    BondsOutstanding => "Outstanding ₹ Cr", "बकाया ₹ करोड़";
    BondsListNote => "{0} securities. NSE symbols follow NSE's naming (7.18% GS 2033 → 718GS2033); FRBs and green bonds have their own. Price a bond with `stockwise bonds calc`.", "{0} प्रतिभूतियाँ। NSE सिंबल NSE के नामकरण पर हैं (7.18% GS 2033 → 718GS2033); FRB और ग्रीन बॉन्ड के अपने हैं। `stockwise bonds calc` से बॉन्ड का भाव निकालें।";
    BondsMaturity => "Maturity", "परिपक्वता";
    BondsSettlement => "Settlement", "निपटान";
    BondsDaysLeft => "{0} days to maturity", "परिपक्वता में {0} दिन";
    BondsCleanPrice => "Clean price (per ₹100)", "क्लीन मूल्य (प्रति ₹100)";
    BondsAccrued => "Accrued interest", "उपार्जित ब्याज";
    BondsDirtyPrice => "Dirty price", "डर्टी मूल्य";
    BondsSemiAnnual => "(semi-annual)", "(छमाही)";
    BondsTbillBasis => "(discount yield, 365-day)", "(डिस्काउंट यील्ड, 365 दिन)";
    BondsCurrentYield => "Current yield", "वर्तमान यील्ड";
    BondsMacaulay => "Macaulay duration (years)", "मैकॉले अवधि (वर्ष)";
    BondsModified => "Modified duration", "संशोधित अवधि";
    BondsNextCoupon => "Next coupon", "अगला कूपन";
    BondsCouponsLeft => "{0} ({1} left)", "{0} ({1} बाकी)";
    BondsCalcNote => "Half-yearly coupons, 30/360 day count, as for G-secs. NSE quotes G-secs at dirty prices: add --dirty when you enter one.", "छमाही कूपन, 30/360 दिन-गणना, जैसा G-sec में होता है। NSE G-sec का डर्टी भाव दिखाता है: वह भाव डालें तो --dirty जोड़ें।";
    BondsTbillNote => "T-bill yield = (100 − price) / price × 365 / days, the convention RBI auctions use.", "T-bill यील्ड = (100 − मूल्य) / मूल्य × 365 / दिन, RBI नीलामी की परिपाटी।";
    India10yName => "India 10Y G-sec", "भारत 10Y G-sec";

    // ── Practice: replays, challenges, scores, cards (`sim`) ──
    PracticeDisclaimer => "Paper trading — not financial advice.", "पेपर ट्रेडिंग — यह वित्तीय सलाह नहीं है।";
    PracticeCardReplay => "paper trading replay", "पेपर ट्रेडिंग रीप्ले";
    PracticeCardLive => "paper trading challenge", "पेपर ट्रेडिंग चुनौती";
    PracticeScore => "Score", "स्कोर";
    PracticeGrade => "Grade", "ग्रेड";
    PracticeNotScored => "Not scored (fewer than {0} trading days)", "स्कोर नहीं ({0} से कम ट्रेडिंग दिन)";
    PracticeTradingDays => "{0} trading days", "{0} ट्रेडिंग दिन";
    PracticeYou => "You", "आप";
    PracticeReturn => "Return", "रिटर्न";
    PracticeMaxDrawdown => "Max drawdown", "अधिकतम गिरावट";
    PracticeInvested => "Invested", "निवेशित";
    PracticeNoBadges => "No badges this time", "इस बार कोई बैज नहीं";
    PracticePassed => "passed", "सफल";
    PracticeFailed => "failed", "असफल";
    PracticeChallenge => "Challenge", "चुनौती";
    PracticeScorecard => "Scorecard", "स्कोरकार्ड";
    PracticeBenchmark => "Benchmark", "बेंचमार्क";
    PracticeCash => "Cash", "नकद";
    PracticeEquity => "Equity", "कुल मूल्य";
    PracticePositions => "Positions", "पोज़िशन";
    PracticeOrders => "Pending orders (fill at the next open)", "बाकी ऑर्डर (अगले दिन खुलने पर भरेंगे)";
    PracticeMarket => "Market on this day", "इस दिन का बाज़ार";
    PracticeDay => "Day {0} of {1}", "दिन {0} / {1}";
    PracticeBadges => "Badges", "बैज";
    PracticeNewBadge => "New badge: {0}", "नया बैज: {0}";
    PracticeStreaks => "Streaks", "लगातार सफलता";
    PracticeStreakDisciplined => "Disciplined runs in a row", "लगातार अनुशासित रन";
    PracticeStreakChallenges => "Challenges passed in a row", "लगातार सफल चुनौतियाँ";
    PracticeBestScores => "Best scores", "सर्वश्रेष्ठ स्कोर";
    PracticeLocked => "Not earned yet", "अभी नहीं मिला";
    PracticeRiskAdjusted => "Risk-adjusted return", "जोखिम-समायोजित रिटर्न";
    PracticeDrawdownControl => "Drawdown control", "गिरावट पर नियंत्रण";
    PracticeDiscipline => "Discipline", "अनुशासन";
    PracticeDiversification => "Diversification", "विविधीकरण";
    PracticeDebrief => "What happened", "क्या हुआ था";
    BadgeKeptStops => "Kept your stops", "स्टॉप-लॉस निभाया";
    BadgeKeptStopsHow => "Three or more buys, every one with a stop-loss", "तीन या अधिक खरीद, हर एक के साथ स्टॉप-लॉस";
    BadgeDiversified => "Diversified across 5 sectors", "5 सेक्टरों में विविधता";
    BadgeDiversifiedHow => "Hold stocks from five sectors at the same time", "एक साथ पाँच सेक्टरों के शेयर रखें";
    BadgeSurvived => "Survived the crash", "गिरावट में टिके रहे";
    BadgeSurvivedHow => "The benchmark falls 20% or more; you lose at most half as much while 30%+ invested", "बेंचमार्क 20% या अधिक गिरे; 30%+ निवेशित रहते हुए आपकी गिरावट उसकी आधी से ज़्यादा न हो";
    BadgeBeatIndex => "Beat the index", "इंडेक्स को पछाड़ा";
    BadgeBeatIndexHow => "Higher return than the benchmark with a smaller drawdown, 30%+ invested", "30%+ निवेशित रहते हुए कम गिरावट के साथ बेंचमार्क से अधिक रिटर्न";
    BadgeSteady => "Steady hands", "स्थिर हाथ";
    BadgeSteadyHow => "20+ days at least half invested with a drawdown under 10%", "20+ दिन कम से कम आधा निवेशित, गिरावट 10% से कम";
    BadgeRiskAdjusted => "Risk-adjusted winner", "जोखिम-समायोजित विजेता";
    BadgeRiskAdjustedHow => "Sharpe ratio of 1 or more over 20+ days, above the benchmark's", "20+ दिनों में 1 या अधिक शार्प अनुपात, बेंचमार्क से ऊपर";
    BadgeChallenge => "Challenge complete", "चुनौती पूरी";
    BadgeChallengeHow => "Pass any challenge", "कोई भी चुनौती पूरी करें";
    BadgeSip => "Never missed a SIP", "कोई SIP नहीं छूटी";
    BadgeSipHow => "Pass the SIP challenge without missing a month", "बिना कोई महीना छोड़े SIP चुनौती पूरी करें";
    BadgeDistance => "Went the distance", "पूरा सफ़र तय किया";
    BadgeDistanceHow => "Play a replay to its last day", "रीप्ले को आखिरी दिन तक खेलें";
    BadgeStreak => "Three in a row", "लगातार तीन";
    BadgeStreakHow => "Three disciplined runs in a row: every buy stopped and sized, no rule broken", "लगातार तीन अनुशासित रन: हर खरीद पर स्टॉप और सही आकार, कोई नियम नहीं टूटा";

    // ── Errors ──
    ErrorPrefix => "Error:", "त्रुटि:";
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flag_beats_env_beats_locale() {
        let env = env_from(&[("STOCKWISE_LANG", "en"), ("LANG", "hi_IN.UTF-8")]);
        assert_eq!(
            detect(&args(&["stockwise", "--lang", "hi", "quote", "TCS"]), &env),
            (Lang::Hi, LangSource::Flag)
        );
        assert_eq!(
            detect(&args(&["stockwise", "quote", "--lang=hi"]), &env),
            (Lang::Hi, LangSource::Flag)
        );
        assert_eq!(
            detect(&args(&["stockwise", "quote"]), &env),
            (Lang::En, LangSource::StockwiseLang)
        );
        let env = env_from(&[("LANG", "hi_IN.UTF-8")]);
        assert_eq!(
            detect(&args(&["stockwise"]), &env),
            (Lang::Hi, LangSource::Locale)
        );
    }

    #[test]
    fn config_file_beats_locale_but_not_env_or_flag() {
        let locale = env_from(&[("LANG", "en_US.UTF-8")]);
        assert_eq!(
            detect_with_config(&args(&["stockwise"]), &locale, Some("hi")),
            (Lang::Hi, LangSource::Config)
        );
        let env = env_from(&[("STOCKWISE_LANG", "en")]);
        assert_eq!(
            detect_with_config(&args(&["stockwise"]), &env, Some("hi")),
            (Lang::En, LangSource::StockwiseLang)
        );
        assert_eq!(
            detect_with_config(&args(&["stockwise", "--lang", "en"]), &locale, Some("hi")),
            (Lang::En, LangSource::Flag)
        );
        assert_eq!(
            detect_with_config(&args(&["stockwise"]), &locale, None),
            (Lang::En, LangSource::Default)
        );
    }

    #[test]
    fn locale_detection_follows_posix_precedence() {
        let env = env_from(&[("LC_ALL", "en_US.UTF-8"), ("LANG", "hi_IN.UTF-8")]);
        assert_eq!(detect(&args(&["stockwise"]), &env).0, Lang::En);
        let env = env_from(&[("LC_ALL", ""), ("LC_MESSAGES", "hi_IN"), ("LANG", "C")]);
        assert_eq!(detect(&args(&["stockwise"]), &env).0, Lang::Hi);
        let env = env_from(&[("LANG", "C.UTF-8")]);
        assert_eq!(
            detect(&args(&["stockwise"]), &env),
            (Lang::En, LangSource::Default)
        );
        // `hi` must be the language code, not a substring of another one.
        let env = env_from(&[("LANG", "chi_CN.UTF-8")]);
        assert_eq!(detect(&args(&["stockwise"]), &env).0, Lang::En);
    }

    #[test]
    fn invalid_values_fall_through_and_args_after_double_dash_are_ignored() {
        let env = env_from(&[("STOCKWISE_LANG", "klingon")]);
        assert_eq!(
            detect(&args(&["stockwise", "--lang", "xx"]), &env),
            (Lang::En, LangSource::Default)
        );
        assert_eq!(
            detect(&args(&["stockwise", "--", "--lang", "hi"]), env_from(&[])).0,
            Lang::En
        );
        assert_eq!(
            detect(&args(&["stockwise", "--lang", "Hindi"]), env_from(&[])).0,
            Lang::Hi
        );
    }

    #[test]
    fn every_message_has_both_languages_with_matching_placeholders() {
        let placeholders = |s: &str| -> Vec<String> {
            let mut found: Vec<String> = (0..10)
                .map(|i| format!("{{{i}}}"))
                .filter(|p| s.contains(p.as_str()))
                .collect();
            found.sort();
            found
        };
        for msg in Msg::ALL {
            let en = msg.text(Lang::En);
            let hi = msg.text(Lang::Hi);
            assert!(!en.is_empty() && !hi.is_empty(), "{msg:?} is empty");
            assert_eq!(
                placeholders(en),
                placeholders(hi),
                "{msg:?} placeholders differ"
            );
        }
    }

    #[test]
    fn hindi_strings_are_actually_hindi() {
        // Labels that are the same in both languages are fine (P/E, AI); the
        // catalog as a whole must not be English copied over.
        let translated = Msg::ALL
            .iter()
            .filter(|m| m.text(Lang::Hi) != m.text(Lang::En))
            .count();
        assert!(
            translated * 10 >= Msg::ALL.len() * 9,
            "{translated}/{}",
            Msg::ALL.len()
        );
    }

    #[test]
    fn fill_replaces_positional_placeholders() {
        assert_eq!(fill("{0} of {1}, {0}", &[&"a", &2]), "a of 2, a");
        assert_eq!(
            Msg::PortfolioCount.text(Lang::Hi).replace("{0}", "3"),
            "पोर्टफोलियो (3 होल्डिंग्स)"
        );
    }
}
