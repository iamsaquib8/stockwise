//! Angel One SmartAPI: password + TOTP login, exact instrument lookup,
//! holdings, positions, orders (MARKET, LIMIT, SL, SL-M) and GTT rules.

use super::http::{self, unknown_state};
use super::secrets::{self, Secret, Storage};
use super::{
    Broker, BrokerKind, Endpoint, Gtt, GttRequest, Holding, Instrument, Order, OrderAck, OrderKind,
    OrderRequest, Position, Profile, Shape, Trade,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const BASE_URL: &str = "https://apiconnect.angelone.in";
pub const CONFIG_FILE: &str = "angel_config.json";
pub const SECRET_KEYS: &[&str] = &["api_key", "password", "totp_secret"];
const LABEL: &str = "Angel One";

pub const SETUP: &[&str] = &[
    "Go to smartapi.angelone.in and create an app; note its API Key",
    "Your Client ID (Angel One login ID) and login PIN",
    "Your TOTP secret (base32, from the Angel One TOTP setup / authenticator)",
    "Run `stockwise trade config` and fill in the file it creates",
];

pub fn status(migrate: bool) -> Result<super::ConfigStatus> {
    let (config, storage) = AngelConfig::load_with_storage(migrate)?;
    let mut status = super::status_of(
        BrokerKind::Angel,
        config.is_configured(),
        client_summary(&config.client_id),
        storage,
        None,
    )?;
    status.identifiers = vec![config.client_id.clone()];
    Ok(status)
}

fn client_summary(id: &str) -> String {
    format!("client {}", super::redact_id(id))
}

// ── Config persistence ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AngelConfig {
    #[serde(default)]
    pub api_key: Secret,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub password: Secret,
    #[serde(default)]
    pub totp_secret: Secret,
    /// Refuse any single order whose estimated value exceeds this (₹).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_order_value: Option<f64>,
}

impl AngelConfig {
    /// Load the config, moving plaintext secrets into the OS keyring when
    /// one is available and this is an interactive run.
    pub fn load() -> Result<Self> {
        Ok(Self::load_with_storage(secrets::auto_migrate())?.0)
    }

    /// Load without migrating; also returns where the secrets live.
    pub fn load_with_storage(migrate: bool) -> Result<(Self, Storage)> {
        let loaded = secrets::load(CONFIG_FILE, SECRET_KEYS, secrets::backend(), migrate)?;
        Ok((loaded.config, loaded.storage))
    }

    /// Write the config (and the keyring entry when `storage` is the keyring).
    pub fn save_to(&self, storage: &Storage) -> Result<()> {
        secrets::save(CONFIG_FILE, self, SECRET_KEYS, storage, secrets::backend())
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Storage::File)
    }

    pub fn is_configured(&self) -> bool {
        self.api_key.is_real()
            && secrets::is_real(&self.client_id)
            && self.password.is_real()
            && self.totp_secret.is_real()
    }

    /// Template with placeholder values for the user to edit.
    pub fn template() -> Self {
        let p = secrets::PLACEHOLDER_PREFIX;
        Self {
            api_key: Secret::new(format!("{p}API_KEY")),
            client_id: format!("{p}CLIENT_ID"),
            password: Secret::new(format!("{p}PASSWORD_OR_PIN")),
            totp_secret: Secret::new(format!("{p}TOTP_SECRET")),
            max_order_value: None,
        }
    }
}

// ── TOTP generation ──

fn generate_totp(secret: &str) -> Result<String> {
    // TOTP: RFC 6238, base32-encoded secret, 30-second window, 6 digits
    let secret_bytes = base32_decode(secret)?;
    let time_step = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs()
        / 30;

    let msg = time_step.to_be_bytes();
    let hash = hmac_sha1(&secret_bytes, &msg);

    let offset = (hash[19] & 0x0f) as usize;
    let code = ((hash[offset] as u32 & 0x7f) << 24
        | (hash[offset + 1] as u32) << 16
        | (hash[offset + 2] as u32) << 8
        | hash[offset + 3] as u32)
        % 1_000_000;

    Ok(format!("{:06}", code))
}

fn base32_decode(input: &str) -> Result<Vec<u8>> {
    let input = input.trim().to_uppercase().replace(' ', "");
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits = 0u64;
    let mut bit_count = 0;
    let mut output = Vec::new();

    for ch in input.bytes() {
        if ch == b'=' {
            break;
        }
        let val = alphabet
            .iter()
            .position(|&c| c == ch)
            .context("Invalid base32 character")? as u64;
        bits = (bits << 5) | val;
        bit_count += 5;
        if bit_count >= 8 {
            bit_count -= 8;
            output.push((bits >> bit_count) as u8);
            bits &= (1 << bit_count) - 1;
        }
    }
    Ok(output)
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let block_size = 64;
    let mut k = vec![0u8; block_size];
    if key.len() > block_size {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut i_pad = vec![0x36u8; block_size];
    let mut o_pad = vec![0x5cu8; block_size];
    for j in 0..block_size {
        i_pad[j] ^= k[j];
        o_pad[j] ^= k[j];
    }

    i_pad.extend_from_slice(msg);
    let inner_hash = sha1(&i_pad);
    o_pad.extend_from_slice(&inner_hash);
    sha1(&o_pad)
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

// ── API Client ──

pub struct AngelClient {
    client: reqwest::Client,
    config: AngelConfig,
    base_url: String,
    jwt_token: Secret,
    refresh_token: Secret,
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    status: bool,
    message: String,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct LoginData {
    #[serde(rename = "jwtToken")]
    jwt_token: Secret,
    #[serde(rename = "refreshToken")]
    refresh_token: Secret,
}

#[derive(Debug, Serialize)]
struct OrderBody {
    variety: String,
    tradingsymbol: String,
    symboltoken: String,
    transactiontype: String,
    exchange: String,
    ordertype: String,
    producttype: String,
    duration: String,
    quantity: String,
    price: String,
    triggerprice: String,
}

#[derive(Debug, Deserialize)]
struct OrderResponse {
    orderid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchScrip {
    pub symboltoken: Option<String>,
    pub tradingsymbol: Option<String>,
    pub exchange: Option<String>,
}

/// Pick the cash-equity (`-EQ`) instrument for `symbol` from searchScrip
/// results. Only an exact match is accepted, so a typo or partial name can
/// never resolve to a different stock or series.
fn pick_equity(results: Vec<SearchScrip>, symbol: &str, exchange: &str) -> Result<Instrument> {
    let wanted = format!("{}-EQ", symbol.trim_end_matches("-EQ"));
    let mut matches = results.into_iter().filter(|s| {
        s.tradingsymbol.as_deref() == Some(wanted.as_str())
            && s.exchange.as_deref().is_none_or(|e| e == exchange)
    });
    let Some(hit) = matches.next() else {
        bail!(
            "No exact {} equity match for {} on {}. Check the symbol with `stockwise search`.",
            wanted,
            symbol,
            exchange
        );
    };
    if matches.next().is_some() {
        bail!(
            "Several instruments match {} on {}; refusing to guess.",
            wanted,
            exchange
        );
    }
    Ok(Instrument {
        symbol: symbol.trim_end_matches("-EQ").to_string(),
        tradingsymbol: wanted,
        token: hit.symboltoken.context("Broker returned no symbol token")?,
        exchange: exchange.to_string(),
    })
}

/// Unwrap an Angel One response envelope, turning `status: false` into an
/// error instead of an empty result.
fn expect_ok<T>(resp: ApiResponse<T>, what: &str) -> Result<Option<T>> {
    if !resp.status {
        bail!("Angel One {} failed: {}", what, resp.message);
    }
    Ok(resp.data)
}

/// `RELIANCE-EQ` → `RELIANCE`.
fn base_symbol(tradingsymbol: &str) -> String {
    tradingsymbol
        .rsplit_once('-')
        .map_or(tradingsymbol, |(b, _)| b)
        .to_string()
}

/// Angel One's `variety`, `ordertype` and trigger for an order kind.
/// Stop-loss orders need variety STOPLOSS; everything else is NORMAL.
fn order_fields(kind: &OrderKind) -> (&'static str, &'static str, String, String) {
    let px = |p: f64| format!("{:.2}", p);
    match *kind {
        OrderKind::Market => ("NORMAL", "MARKET", "0".into(), "0".into()),
        OrderKind::Limit { price } => ("NORMAL", "LIMIT", px(price), "0".into()),
        OrderKind::StopLimit { trigger, price } => {
            ("STOPLOSS", "STOPLOSS_LIMIT", px(price), px(trigger))
        }
        OrderKind::StopMarket { trigger } => {
            ("STOPLOSS", "STOPLOSS_MARKET", "0".into(), px(trigger))
        }
    }
}

impl AngelClient {
    /// Log in with the saved credentials (password + a fresh TOTP).
    pub async fn new() -> Result<Self> {
        let config = AngelConfig::load()?;
        if !config.is_configured() {
            bail!(
                "Angel One is not configured. Run `stockwise trade setup` for instructions, then edit {}",
                crate::store::path(CONFIG_FILE)?.display()
            );
        }
        Self::login(config, BASE_URL).await
    }

    async fn login(config: AngelConfig, base_url: &str) -> Result<Self> {
        let totp = generate_totp(config.totp_secret.expose())
            .context("Failed to generate TOTP. Check your TOTP secret.")?;

        let client = http::client()?;

        let login_body = serde_json::json!({
            "clientcode": config.client_id,
            "password": config.password.expose(),
            "totp": totp
        });

        let resp: ApiResponse<LoginData> = client
            .post(format!(
                "{}/rest/auth/angelbroking/user/v1/loginByPassword",
                base_url
            ))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("X-UserType", "USER")
            .header("X-SourceID", "WEB")
            .header("X-ClientLocalIP", "127.0.0.1")
            .header("X-ClientPublicIP", "127.0.0.1")
            .header("X-MACAddress", "00:00:00:00:00:00")
            .header("X-PrivateKey", config.api_key.expose())
            .json(&login_body)
            .send()
            .await
            .context("Failed to connect to Angel One API")?
            .json()
            .await
            .context("Failed to parse login response")?;

        if !resp.status {
            bail!("Angel One login failed: {}", resp.message);
        }

        let data = resp.data.context("No login data returned")?;

        Ok(Self {
            client,
            config,
            base_url: base_url.to_string(),
            jwt_token: data.jwt_token,
            refresh_token: data.refresh_token,
        })
    }

    fn auth_headers(&self) -> Result<reqwest::header::HeaderMap> {
        use reqwest::header::{HeaderMap, HeaderValue};
        let mut headers = HeaderMap::new();
        let mut auth = HeaderValue::from_str(&format!("Bearer {}", self.jwt_token.expose()))
            .context("Broker returned an invalid session token")?;
        auth.set_sensitive(true);
        headers.insert("Authorization", auth);
        for (name, value) in [
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
            ("X-UserType", "USER"),
            ("X-SourceID", "WEB"),
            ("X-ClientLocalIP", "127.0.0.1"),
            ("X-ClientPublicIP", "127.0.0.1"),
            ("X-MACAddress", "00:00:00:00:00:00"),
        ] {
            headers.insert(name, HeaderValue::from_static(value));
        }
        let mut key = HeaderValue::from_str(self.config.api_key.expose())
            .context("API key in the Angel One config contains invalid characters")?;
        key.set_sensitive(true);
        headers.insert("X-PrivateKey", key);
        Ok(headers)
    }

    fn url(&self, path: &str) -> String {
        format!("{}/rest/secure/angelbroking/{}", self.base_url, path)
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        what: &str,
    ) -> Result<Option<T>> {
        let resp: ApiResponse<T> = self
            .client
            .get(self.url(path))
            .headers(self.auth_headers()?)
            .send()
            .await?
            .json()
            .await?;
        expect_ok(resp, what)
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
        what: &str,
    ) -> Result<Option<T>> {
        let resp: ApiResponse<T> = self
            .client
            .post(self.url(path))
            .headers(self.auth_headers()?)
            .json(body)
            .send()
            .await?
            .json()
            .await?;
        expect_ok(resp, what)
    }

    /// Send an order-changing request. A transport or parse failure leaves
    /// the order state unknown, and the error says so.
    async fn post_order<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
        failed: &str,
    ) -> Result<Option<T>> {
        let resp: ApiResponse<T> = self
            .client
            .post(self.url(path))
            .headers(self.auth_headers()?)
            .json(body)
            .send()
            .await
            .with_context(|| format!("Could not reach Angel One. {}", unknown_state(LABEL)))?
            .json()
            .await
            .with_context(|| {
                format!(
                    "Unreadable response from Angel One. {}",
                    unknown_state(LABEL)
                )
            })?;
        if !resp.status {
            bail!("{failed}: {}", resp.message);
        }
        Ok(resp.data)
    }

    /// All GTT rules, page by page (a short page ends the walk).
    async fn gtt_rules(&self) -> Result<Vec<serde_json::Value>> {
        const COUNT: usize = 10;
        let mut all = Vec::new();
        for page in 1..=20 {
            let body = serde_json::json!({
                "status": ["NEW", "ACTIVE", "SENTTOEXCHANGE", "CANCELLED", "FORALL"],
                "page": page,
                "count": COUNT
            });
            // The docs show `data` as one object; the live API returns a list.
            let rows = match self
                .post::<serde_json::Value>("gtt/v1/ruleList", &body, "GTT list")
                .await?
            {
                Some(serde_json::Value::Array(a)) => a,
                Some(obj @ serde_json::Value::Object(_)) => vec![obj],
                _ => vec![],
            };
            let n = rows.len();
            all.extend(rows);
            if n < COUNT {
                break;
            }
        }
        Ok(all)
    }
}

/// Map one raw holding (numbers may be strings or numbers).
fn holding(h: &serde_json::Value) -> Option<Holding> {
    let sym = http::text(h.get("tradingsymbol"))?;
    Some(Holding {
        symbol: base_symbol(&sym),
        exchange: http::text(h.get("exchange")),
        isin: http::text(h.get("isin")),
        qty: http::num(h.get("quantity")).unwrap_or(0.0),
        avg_price: http::price(h.get("averageprice")),
        ltp: http::price(h.get("ltp")),
        pnl: http::num(h.get("profitandloss").or_else(|| h.get("pnl"))),
    })
}

fn position(p: &serde_json::Value) -> Option<Position> {
    let sym = http::text(p.get("tradingsymbol"))?;
    Some(Position {
        symbol: base_symbol(&sym),
        exchange: http::text(p.get("exchange")),
        product: http::text(p.get("producttype")),
        qty: http::num(p.get("netqty").or_else(|| p.get("quantity"))).unwrap_or(0.0),
        avg_price: http::price(p.get("netprice").or_else(|| p.get("buyavgprice"))),
        ltp: http::price(p.get("ltp")),
        pnl: http::num(p.get("pnl").or_else(|| p.get("unrealised"))),
    })
}

fn order(o: &serde_json::Value) -> Option<Order> {
    Some(Order {
        id: http::text(o.get("orderid"))?,
        symbol: http::text(o.get("tradingsymbol")).unwrap_or_default(),
        side: http::text(o.get("transactiontype")),
        order_type: http::text(o.get("ordertype")),
        qty: http::num(o.get("quantity")),
        price: http::price(o.get("price")),
        trigger: http::price(o.get("triggerprice")),
        status: http::text(o.get("status")),
        message: http::text(o.get("text")),
        filled_qty: http::num(o.get("filledshares")),
        avg_price: http::price(o.get("averageprice")),
        product: http::text(o.get("producttype")),
    })
}

/// One fill from the trade book. Angel One gives the fill time only; the
/// book is today's.
fn trade(t: &serde_json::Value) -> Option<Trade> {
    let sym = http::text(t.get("tradingsymbol"))?;
    let (date, time) = super::split_timestamp(http::text(t.get("filltime")).as_deref());
    Some(Trade {
        trade_id: http::text(t.get("fillid")),
        order_id: http::text(t.get("orderid"))?,
        symbol: super::strip_series(&sym).to_string(),
        exchange: http::text(t.get("exchange")),
        product: http::text(t.get("producttype")),
        side: super::side_from(http::text(t.get("transactiontype")).as_deref()),
        qty: http::num(t.get("fillsize")).unwrap_or(0.0),
        price: http::price(t.get("fillprice")),
        date,
        time,
    })
}

fn profile(p: &serde_json::Value) -> Profile {
    // `exchanges` / `products` are arrays in the docs; accept a
    // comma-separated string too.
    let list = |k: &str| match p.get(k) {
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|v| http::text(Some(v))).collect(),
        Some(serde_json::Value::String(s)) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    Profile {
        user_id: http::text(p.get("clientcode")),
        exchanges: list("exchanges"),
        products: list("products"),
    }
}

/// Fields read from each endpoint, and the ones SmartAPI's docs list.
fn shape(endpoint: Endpoint) -> Shape {
    match endpoint {
        Endpoint::Profile => Shape {
            required: "clientcode",
            documented: "name email mobileno exchanges products lastlogintime brokerid",
        },
        Endpoint::Holdings => Shape {
            required: "tradingsymbol quantity averageprice ltp",
            documented: "exchange isin t1quantity realisedquantity authorisedquantity product \
                collateralquantity collateraltype haircut symboltoken close profitandloss \
                pnlpercentage",
        },
        Endpoint::Positions => Shape {
            required: "tradingsymbol netqty|quantity",
            documented: "exchange symboltoken producttype symbolname instrumenttype priceden \
                pricenum genden gennum precision multiplier boardlotsize buyqty sellqty buyamount \
                sellamount symbolgroup strikeprice optiontype expirydate lotsize cfbuyqty \
                cfsellqty cfbuyamount cfsellamount buyavgprice sellavgprice avgnetprice netvalue \
                totalbuyvalue totalsellvalue cfbuyavgprice cfsellavgprice totalbuyavgprice \
                totalsellavgprice netprice ltp pnl unrealised realised close",
        },
        Endpoint::Orders => Shape {
            required: "orderid tradingsymbol transactiontype status quantity",
            documented: "variety ordertype producttype duration price triggerprice \
                disclosedquantity squareoff stoploss trailingstoploss exchange symboltoken \
                ordertag instrumenttype strikeprice optiontype expirydate lotsize cancelsize \
                averageprice filledshares unfilledshares text orderstatus updatetime exchtime \
                exchorderupdatetime fillid filltime parentorderid uniqueorderid exchangeorderid",
        },
        Endpoint::Trades => Shape {
            required: "orderid fillid exchange producttype tradingsymbol transactiontype fillsize \
                fillprice filltime",
            documented: "instrumenttype symbolgroup strikeprice optiontype expirydate marketlot \
                precision multiplier tradevalue",
        },
        Endpoint::Gtts => Shape {
            required: "id status tradingsymbol transactiontype qty triggerprice price",
            documented: "stoplossprice stoplosstriggerprice gttType createddate updateddate \
                expirydate clientid symboltoken exchange producttype disclosedqty",
        },
    }
}

fn gtt(r: &serde_json::Value) -> Option<Gtt> {
    Some(Gtt {
        id: http::text(r.get("id"))?,
        symbol: http::text(r.get("tradingsymbol")).unwrap_or_default(),
        side: http::text(r.get("transactiontype")),
        qty: http::num(r.get("qty")),
        trigger: http::price(r.get("triggerprice")),
        limit: http::price(r.get("price")),
        status: http::text(r.get("status")),
    })
}

#[async_trait]
impl Broker for AngelClient {
    fn kind(&self) -> BrokerKind {
        BrokerKind::Angel
    }

    async fn holdings(&self) -> Result<Vec<Holding>> {
        let raw = self.rows(Endpoint::Holdings).await?;
        Ok(raw.iter().filter_map(holding).collect())
    }

    async fn positions(&self) -> Result<Vec<Position>> {
        let raw = self.rows(Endpoint::Positions).await?;
        Ok(raw.iter().filter_map(position).collect())
    }

    async fn orders(&self) -> Result<Vec<Order>> {
        let raw = self.rows(Endpoint::Orders).await?;
        Ok(raw.iter().filter_map(order).collect())
    }

    async fn profile(&self) -> Result<Profile> {
        let raw = self.rows(Endpoint::Profile).await?;
        Ok(raw.first().map(profile).unwrap_or_default())
    }

    /// Today's fills; SmartAPI's trade book has no history.
    async fn trades(&self, _since: Option<chrono::NaiveDate>) -> Result<Vec<Trade>> {
        let raw = self.rows(Endpoint::Trades).await?;
        Ok(raw.iter().filter_map(trade).collect())
    }

    async fn rows(&self, endpoint: Endpoint) -> Result<Vec<serde_json::Value>> {
        let path = match endpoint {
            Endpoint::Profile => "user/v1/getProfile",
            Endpoint::Holdings => "portfolio/v1/getHolding",
            Endpoint::Positions => "order/v1/getPosition",
            Endpoint::Orders => "order/v1/getOrderBook",
            Endpoint::Trades => "order/v1/getTradeBook",
            Endpoint::Gtts => return self.gtt_rules().await,
        };
        let what = format!("{} request", endpoint.label());
        Ok(match self.get::<serde_json::Value>(path, &what).await? {
            Some(serde_json::Value::Array(a)) => a,
            Some(obj @ serde_json::Value::Object(_)) => vec![obj],
            _ => vec![],
        })
    }

    fn shape(&self, endpoint: Endpoint) -> Option<Shape> {
        Some(shape(endpoint))
    }

    /// Resolve a user-typed NSE/BSE symbol (e.g. `RELIANCE`) to its exact
    /// cash-equity instrument.
    async fn resolve_instrument(&self, symbol: &str, exchange: &str) -> Result<Instrument> {
        let body = serde_json::json!({
            "exchange": exchange,
            "searchscrip": symbol
        });
        let results: Vec<SearchScrip> = self
            .post("order/v1/searchScrip", &body, "symbol search")
            .await?
            .unwrap_or_default();
        pick_equity(results, symbol, exchange)
    }

    async fn place_order(&self, order: &OrderRequest) -> Result<OrderAck> {
        let (variety, ordertype, price, triggerprice) = order_fields(&order.kind);
        let body = OrderBody {
            variety: variety.into(),
            tradingsymbol: order.instrument.tradingsymbol.clone(),
            symboltoken: order.instrument.token.clone(),
            transactiontype: order.side.as_str().into(),
            exchange: order.instrument.exchange.clone(),
            ordertype: ordertype.into(),
            producttype: "DELIVERY".into(),
            duration: "DAY".into(),
            quantity: order.qty.to_string(),
            price,
            triggerprice,
        };
        let data: Option<OrderResponse> = self
            .post_order("order/v1/placeOrder", &body, "Order failed")
            .await?;
        Ok(OrderAck {
            order_id: data.and_then(|d| d.orderid).filter(|id| !id.is_empty()),
        })
    }

    fn supports_gtt(&self) -> bool {
        true
    }

    async fn gtt_create(&self, g: &GttRequest) -> Result<String> {
        let body = serde_json::json!({
            "tradingsymbol": g.instrument.tradingsymbol,
            "symboltoken": g.instrument.token,
            "exchange": g.instrument.exchange,
            "transactiontype": g.side.as_str(),
            "producttype": "DELIVERY",
            "price": format!("{:.2}", g.limit),
            "qty": g.qty.to_string(),
            "triggerprice": format!("{:.2}", g.trigger),
            "disclosedqty": "0",
            // Days; the default is undocumented, so ask for the maximum.
            "timeperiod": 365,
        });
        let data: Option<serde_json::Value> = self
            .post_order("gtt/v1/createRule", &body, "GTT failed")
            .await?;
        data.as_ref().and_then(|d| http::text(d.get("id"))).context(
            "Angel One accepted the GTT but returned no id — check `stockwise trade gtt list`",
        )
    }

    async fn gtt_list(&self) -> Result<Vec<Gtt>> {
        Ok(self.gtt_rules().await?.iter().filter_map(gtt).collect())
    }

    async fn gtt_cancel(&self, id: &str) -> Result<()> {
        // cancelRule needs the rule's symbol token and exchange.
        let rules = self.gtt_rules().await?;
        let rule = rules
            .iter()
            .find(|r| http::text(r.get("id")).as_deref() == Some(id))
            .with_context(|| format!("No Angel One GTT with id {id}"))?;
        let body = serde_json::json!({
            "id": id,
            "symboltoken": http::text(rule.get("symboltoken")).unwrap_or_default(),
            "exchange": http::text(rule.get("exchange")).unwrap_or_else(|| "NSE".into()),
        });
        self.post_order::<serde_json::Value>("gtt/v1/cancelRule", &body, "GTT cancel failed")
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brokers::Side;
    use crate::brokers::mock::{MockServer, Route};

    #[test]
    fn status_summary_masks_client_id() {
        let summary = client_summary("A1234567");
        assert_eq!(summary, "client ••67");
        assert!(!summary.contains("A1234567"));
    }

    fn scrip(sym: &str, token: &str) -> SearchScrip {
        SearchScrip {
            symboltoken: Some(token.into()),
            tradingsymbol: Some(sym.into()),
            exchange: Some("NSE".into()),
        }
    }

    #[test]
    fn pick_equity_takes_exact_eq_series_not_first_hit() {
        let results = vec![
            scrip("RELIANCE-BE", "1"),
            scrip("RELIANCEPP-EQ", "2"),
            scrip("RELIANCE-EQ", "2885"),
        ];
        let inst = pick_equity(results, "reliance".to_uppercase().as_str(), "NSE").unwrap();
        assert_eq!(inst.tradingsymbol, "RELIANCE-EQ");
        assert_eq!(inst.token, "2885");
        assert_eq!(inst.symbol, "RELIANCE");
    }

    #[test]
    fn pick_equity_rejects_partial_matches() {
        let results = vec![scrip("TATAMOTORS-EQ", "1"), scrip("TATASTEEL-EQ", "2")];
        assert!(pick_equity(results, "TATA", "NSE").is_err());
    }

    #[test]
    fn pick_equity_rejects_duplicates() {
        let results = vec![scrip("INFY-EQ", "1594"), scrip("INFY-EQ", "9999")];
        assert!(pick_equity(results, "INFY", "NSE").is_err());
    }

    #[test]
    fn placeholder_config_is_not_configured() {
        assert!(!AngelConfig::template().is_configured());
        assert!(!AngelConfig::default().is_configured());
    }

    #[test]
    fn config_file_format_is_unchanged() {
        // Files written by 0.3 load as before; secrets stay plain strings on disk.
        let cfg: AngelConfig = serde_json::from_str(
            r#"{"api_key":"k","client_id":"A1","password":"1234","totp_secret":"JBSWY3DPEHPK3PXP","max_order_value":5000}"#,
        )
        .unwrap();
        assert!(cfg.is_configured());
        let back = serde_json::to_value(&cfg).unwrap();
        assert_eq!(back["password"], "1234");
        assert!(!format!("{cfg:?}").contains("1234"));
    }

    #[test]
    fn stop_loss_orders_use_the_stoploss_variety() {
        assert_eq!(
            order_fields(&OrderKind::Market),
            ("NORMAL", "MARKET", "0".into(), "0".into())
        );
        assert_eq!(
            order_fields(&OrderKind::Limit { price: 1300.0 }),
            ("NORMAL", "LIMIT", "1300.00".into(), "0".into())
        );
        assert_eq!(
            order_fields(&OrderKind::StopLimit {
                trigger: 1250.0,
                price: 1245.5
            }),
            (
                "STOPLOSS",
                "STOPLOSS_LIMIT",
                "1245.50".into(),
                "1250.00".into()
            )
        );
        assert_eq!(
            order_fields(&OrderKind::StopMarket { trigger: 1250.0 }),
            ("STOPLOSS", "STOPLOSS_MARKET", "0".into(), "1250.00".into())
        );
    }

    #[test]
    fn order_status_accepts_numeric_price() {
        let o = order(&serde_json::json!({"orderid":"1","price":194.5,"quantity":"3"})).unwrap();
        assert_eq!(o.price, Some(194.5));
        assert_eq!(o.qty, Some(3.0));
    }

    fn test_config() -> AngelConfig {
        AngelConfig {
            api_key: Secret::new("test-key"),
            client_id: "A1".into(),
            password: Secret::new("1234"),
            totp_secret: Secret::new("JBSWY3DPEHPK3PXP"),
            max_order_value: None,
        }
    }

    const LOGIN: &str = r#"{"status":true,"message":"SUCCESS","errorcode":"","data":{"jwtToken":"jwt-abc","refreshToken":"ref-abc","feedToken":"feed"}}"#;

    async fn logged_in(routes: Vec<Route>) -> (MockServer, AngelClient) {
        let mut all = vec![Route::json(
            "POST",
            "/rest/auth/angelbroking/user/v1/loginByPassword",
            LOGIN,
        )];
        all.extend(routes);
        let server = MockServer::start(all).await;
        let client = AngelClient::login(test_config(), &server.url())
            .await
            .unwrap();
        (server, client)
    }

    #[tokio::test]
    async fn login_sends_totp_and_uses_the_jwt() {
        let (server, client) = logged_in(vec![Route::json(
            "GET",
            "/rest/secure/angelbroking/portfolio/v1/getHolding",
            include_str!("fixtures/angel_holdings.json"),
        )])
        .await;
        let login = server.last("POST", "/rest/auth/angelbroking/user/v1/loginByPassword");
        let body = login.json();
        assert_eq!(body["clientcode"], "A1");
        assert_eq!(body["totp"].as_str().unwrap().len(), 6);
        assert_eq!(login.header("X-PrivateKey"), Some("test-key"));

        let holdings = client.holdings().await.unwrap();
        let req = server.last("GET", "/rest/secure/angelbroking/portfolio/v1/getHolding");
        assert_eq!(req.header("Authorization"), Some("Bearer jwt-abc"));
        assert_eq!(holdings.len(), 2);
        assert_eq!(holdings[0].symbol, "TCS");
        assert_eq!(holdings[0].qty, 10.0);
        assert_eq!(holdings[0].avg_price, Some(3450.5));
        // ltp 0 means "no price", not a price of 0.
        assert_eq!(holdings[1].ltp, None);
    }

    #[tokio::test]
    async fn failed_status_is_an_error_not_an_empty_list() {
        let (_server, client) = logged_in(vec![Route::json(
            "GET",
            "/rest/secure/angelbroking/order/v1/getOrderBook",
            r#"{"status":false,"message":"Invalid Token","errorcode":"AG8001","data":null}"#,
        )])
        .await;
        let err = client.orders().await.unwrap_err().to_string();
        assert!(err.contains("Invalid Token"), "{err}");
    }

    #[tokio::test]
    async fn stop_loss_order_body() {
        let (server, client) = logged_in(vec![Route::json(
            "POST",
            "/rest/secure/angelbroking/order/v1/placeOrder",
            r#"{"status":true,"message":"SUCCESS","errorcode":"","data":{"script":"SBIN-EQ","orderid":"200910000000111","uniqueorderid":"34reqfachdfih"}}"#,
        )])
        .await;
        let ack = client
            .place_order(&OrderRequest {
                instrument: Instrument {
                    symbol: "SBIN".into(),
                    tradingsymbol: "SBIN-EQ".into(),
                    token: "3045".into(),
                    exchange: "NSE".into(),
                },
                side: Side::Sell,
                qty: 5,
                kind: OrderKind::StopLimit {
                    trigger: 790.0,
                    price: 789.5,
                },
            })
            .await
            .unwrap();
        assert_eq!(ack.order_id.as_deref(), Some("200910000000111"));
        let body = server
            .last("POST", "/rest/secure/angelbroking/order/v1/placeOrder")
            .json();
        assert_eq!(body["variety"], "STOPLOSS");
        assert_eq!(body["ordertype"], "STOPLOSS_LIMIT");
        assert_eq!(body["transactiontype"], "SELL");
        assert_eq!(body["triggerprice"], "790.00");
        assert_eq!(body["price"], "789.50");
        assert_eq!(body["quantity"], "5");
        assert_eq!(body["producttype"], "DELIVERY");
    }

    #[tokio::test]
    async fn unreachable_order_endpoint_reports_unknown_state() {
        // No placeOrder route: the mock answers 404 with a non-envelope body.
        let (_server, client) = logged_in(vec![]).await;
        let err = client
            .place_order(&OrderRequest {
                instrument: Instrument {
                    symbol: "SBIN".into(),
                    tradingsymbol: "SBIN-EQ".into(),
                    token: "3045".into(),
                    exchange: "NSE".into(),
                },
                side: Side::Buy,
                qty: 1,
                kind: OrderKind::Market,
            })
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("UNKNOWN"), "{err:#}");
    }

    #[tokio::test]
    async fn gtt_create_list_cancel() {
        let (server, client) = logged_in(vec![
            Route::json(
                "POST",
                "/rest/secure/angelbroking/gtt/v1/createRule",
                r#"{"status":true,"message":"SUCCESS","errorcode":"","data":{"id":"757"}}"#,
            ),
            Route::json(
                "POST",
                "/rest/secure/angelbroking/gtt/v1/ruleList",
                include_str!("fixtures/angel_gtt_list.json"),
            ),
            Route::json(
                "POST",
                "/rest/secure/angelbroking/gtt/v1/cancelRule",
                r#"{"status":true,"message":"SUCCESS","errorcode":"","data":{"id":"757"}}"#,
            ),
        ])
        .await;
        let id = client
            .gtt_create(&GttRequest {
                instrument: Instrument {
                    symbol: "SBIN".into(),
                    tradingsymbol: "SBIN-EQ".into(),
                    token: "3045".into(),
                    exchange: "NSE".into(),
                },
                side: Side::Sell,
                qty: 10,
                trigger: 780.0,
                limit: 779.0,
                last_price: 800.0,
            })
            .await
            .unwrap();
        assert_eq!(id, "757");
        let body = server
            .last("POST", "/rest/secure/angelbroking/gtt/v1/createRule")
            .json();
        assert_eq!(body["triggerprice"], "780.00");
        assert_eq!(body["price"], "779.00");
        assert_eq!(body["qty"], "10");

        let list = client.gtt_list().await.unwrap();
        assert_eq!(list[0].id, "757");
        assert_eq!(list[0].trigger, Some(780.0));

        client.gtt_cancel("757").await.unwrap();
        let cancel = server
            .last("POST", "/rest/secure/angelbroking/gtt/v1/cancelRule")
            .json();
        assert_eq!(cancel["symboltoken"], "3045");
        assert!(client.gtt_cancel("999").await.is_err());
    }

    fn read_routes() -> Vec<Route> {
        vec![
            Route::json(
                "GET",
                "/rest/secure/angelbroking/user/v1/getProfile",
                include_str!("fixtures/angel_profile.json"),
            ),
            Route::json(
                "GET",
                "/rest/secure/angelbroking/portfolio/v1/getHolding",
                include_str!("fixtures/angel_holdings.json"),
            ),
            Route::json(
                "GET",
                "/rest/secure/angelbroking/order/v1/getTradeBook",
                include_str!("fixtures/angel_trades.json"),
            ),
            Route::json(
                "POST",
                "/rest/secure/angelbroking/gtt/v1/ruleList",
                include_str!("fixtures/angel_gtt_list.json"),
            ),
        ]
    }

    #[tokio::test]
    async fn trade_book_and_profile_parse() {
        let (_s, client) = logged_in(read_routes()).await;
        let t = client.trades(None).await.unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].symbol, "ITC");
        assert_eq!(t[0].trade_id.as_deref(), Some("50005750"));
        assert_eq!(t[0].order_id, "201020000000095");
        assert_eq!((t[0].qty, t[0].price), (1.0, Some(175.0)));
        assert_eq!(t[0].side, Some(Side::Buy));
        // Time only: the trade book is today's.
        assert_eq!((t[0].date, t[0].time.as_deref()), (None, Some("13:27:53")));
        assert!(t[0].is_delivery_equity());
        assert!(
            !t[1].is_delivery_equity(),
            "INTRADAY is not a delivery fill"
        );

        let p = client.profile().await.unwrap();
        assert_eq!(p.user_id.as_deref(), Some("A1000042"));
        assert!(p.products.contains(&"CNC".to_string()));
    }

    #[tokio::test]
    async fn fixtures_match_the_documented_shape() {
        let (_s, client) = logged_in(read_routes()).await;
        crate::brokers::mock::assert_fixture_shapes(
            &client,
            &[
                Endpoint::Profile,
                Endpoint::Holdings,
                Endpoint::Trades,
                Endpoint::Gtts,
            ],
        )
        .await;
    }
}
