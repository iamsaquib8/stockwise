use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const BASE_URL: &str = "https://apiconnect.angelone.in";

// ── Config persistence ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AngelConfig {
    pub api_key: String,
    pub client_id: String,
    pub password: String,
    pub totp_secret: String,
}

fn config_path() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .context("Cannot determine local data directory")?
        .join("stockwise");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("angel_config.json"))
}

impl AngelConfig {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(AngelConfig::default());
        }
        let data = std::fs::read_to_string(&path)?;
        let config: AngelConfig = serde_json::from_str(&data)?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, data)?;
        Ok(())
    }

    pub fn is_configured(&self) -> bool {
        !self.api_key.is_empty() && !self.client_id.is_empty()
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
    jwt_token: String,
    refresh_token: String,
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
    jwt_token: String,
    #[serde(rename = "refreshToken")]
    refresh_token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Holding {
    pub tradingsymbol: Option<String>,
    pub exchange: Option<String>,
    pub quantity: Option<i64>,
    pub averageprice: Option<f64>,
    pub ltp: Option<f64>,
    pub pnl: Option<f64>,
    pub pnlpercentage: Option<f64>,
    pub symboltoken: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Position {
    pub tradingsymbol: Option<String>,
    pub exchange: Option<String>,
    pub quantity: Option<String>,
    pub buyavgprice: Option<String>,
    pub ltp: Option<String>,
    pub pnl: Option<String>,
    pub symboltoken: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HoldingsResponse {
    holdings: Option<Vec<Holding>>,
}

#[derive(Debug, Serialize)]
struct OrderRequest {
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
pub struct OrderResponse {
    pub orderid: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OrderStatus {
    pub orderid: Option<String>,
    pub tradingsymbol: Option<String>,
    pub transactiontype: Option<String>,
    pub quantity: Option<String>,
    pub price: Option<String>,
    pub status: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchScrip {
    pub symboltoken: Option<String>,
    pub tradingsymbol: Option<String>,
    pub exchange: Option<String>,
}

impl AngelClient {
    pub async fn new() -> Result<Self> {
        let config = AngelConfig::load()?;
        if !config.is_configured() {
            bail!("Angel One not configured. Run: stockwise trade setup");
        }

        let totp = generate_totp(&config.totp_secret)
            .context("Failed to generate TOTP. Check your TOTP secret.")?;

        let client = reqwest::Client::builder()
            .user_agent("stockwise-cli")
            .build()?;

        // Login
        let login_body = serde_json::json!({
            "clientcode": config.client_id,
            "password": config.password,
            "totp": totp
        });

        let resp: ApiResponse<LoginData> = client
            .post(format!(
                "{}/rest/auth/angelbroking/user/v1/loginByPassword",
                BASE_URL
            ))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("X-UserType", "USER")
            .header("X-SourceID", "WEB")
            .header("X-ClientLocalIP", "127.0.0.1")
            .header("X-ClientPublicIP", "127.0.0.1")
            .header("X-MACAddress", "00:00:00:00:00:00")
            .header("X-PrivateKey", &config.api_key)
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
            jwt_token: data.jwt_token,
            refresh_token: data.refresh_token,
        })
    }

    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "Authorization",
            format!("Bearer {}", self.jwt_token).parse().unwrap(),
        );
        headers.insert("Content-Type", "application/json".parse().unwrap());
        headers.insert("Accept", "application/json".parse().unwrap());
        headers.insert("X-UserType", "USER".parse().unwrap());
        headers.insert("X-SourceID", "WEB".parse().unwrap());
        headers.insert("X-ClientLocalIP", "127.0.0.1".parse().unwrap());
        headers.insert("X-ClientPublicIP", "127.0.0.1".parse().unwrap());
        headers.insert("X-MACAddress", "00:00:00:00:00:00".parse().unwrap());
        headers.insert("X-PrivateKey", self.config.api_key.parse().unwrap());
        headers
    }

    pub async fn get_holdings(&self) -> Result<Vec<Holding>> {
        let resp: ApiResponse<Vec<Holding>> = self
            .client
            .get(format!(
                "{}/rest/secure/angelbroking/portfolio/v1/getHolding",
                BASE_URL
            ))
            .headers(self.auth_headers())
            .send()
            .await?
            .json()
            .await?;

        Ok(resp.data.unwrap_or_default())
    }

    pub async fn get_positions(&self) -> Result<Vec<Position>> {
        let resp: ApiResponse<Vec<Position>> = self
            .client
            .get(format!(
                "{}/rest/secure/angelbroking/order/v1/getPosition",
                BASE_URL
            ))
            .headers(self.auth_headers())
            .send()
            .await?
            .json()
            .await?;

        Ok(resp.data.unwrap_or_default())
    }

    pub async fn search_scrip(&self, symbol: &str, exchange: &str) -> Result<Option<String>> {
        let body = serde_json::json!({
            "exchange": exchange,
            "searchscrip": symbol
        });
        let resp: ApiResponse<Vec<SearchScrip>> = self
            .client
            .post(format!(
                "{}/rest/secure/angelbroking/order/v1/searchScrip",
                BASE_URL
            ))
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await?
            .json()
            .await?;

        Ok(resp
            .data
            .and_then(|v| v.into_iter().next())
            .and_then(|s| s.symboltoken))
    }

    pub async fn place_order(
        &self,
        symbol: &str,
        token: &str,
        exchange: &str,
        transaction: &str, // BUY or SELL
        qty: u32,
        order_type: &str, // MARKET, LIMIT, SL
        price: f64,
        trigger: f64,
    ) -> Result<OrderResponse> {
        let order = OrderRequest {
            variety: "NORMAL".into(),
            tradingsymbol: symbol.into(),
            symboltoken: token.into(),
            transactiontype: transaction.into(),
            exchange: exchange.into(),
            ordertype: order_type.into(),
            producttype: "DELIVERY".into(),
            duration: "DAY".into(),
            quantity: qty.to_string(),
            price: if order_type == "MARKET" {
                "0".into()
            } else {
                format!("{:.2}", price)
            },
            // Angel One only accepts a trigger price on stop-loss order types;
            // sending it on MARKET/LIMIT orders is rejected.
            triggerprice: if matches!(order_type, "SL" | "SL-M") && trigger > 0.0 {
                format!("{:.2}", trigger)
            } else {
                "0".into()
            },
        };

        let resp: ApiResponse<OrderResponse> = self
            .client
            .post(format!(
                "{}/rest/secure/angelbroking/order/v1/placeOrder",
                BASE_URL
            ))
            .headers(self.auth_headers())
            .json(&order)
            .send()
            .await
            .context("Failed to place order")?
            .json()
            .await
            .context("Failed to parse order response")?;

        if !resp.status {
            bail!("Order failed: {}", resp.message);
        }

        resp.data.context("No order response data")
    }

    pub async fn get_order_book(&self) -> Result<Vec<OrderStatus>> {
        let resp: ApiResponse<Vec<OrderStatus>> = self
            .client
            .get(format!(
                "{}/rest/secure/angelbroking/order/v1/getOrderBook",
                BASE_URL
            ))
            .headers(self.auth_headers())
            .send()
            .await?
            .json()
            .await?;

        Ok(resp.data.unwrap_or_default())
    }
}
