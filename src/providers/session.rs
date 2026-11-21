//! Polite HTTP for the exchanges' own websites (NSE, BSE).
//!
//! NSE and BSE publish no free market-data API. Their websites load prices
//! from JSON endpoints that answer browser sessions; StockWise reads the same
//! endpoints, gently:
//!
//! - **One session per site per process.** NSE wants the cookies its home
//!   page sets, so the first request loads that page once; later requests
//!   reuse the cookies. An expired session (HTTP 401) is renewed once.
//! - **At most three requests a second** across every caller, with random
//!   jitter on top of the spacing.
//! - **Retries with backoff** for timeouts, HTTP 429 (honouring
//!   `Retry-After`, capped) and 5xx — never for a refusal.
//! - **A refusal is final.** HTTP 403 or an "Access Denied" page means the
//!   site does not want automated requests right now: StockWise stops asking
//!   it for a while (`BLOCK_FOR`) and the caller falls back to Yahoo. It never
//!   retries a refusal with different headers.
//! - **Caching** in the shared Redis / in-memory cache, and on disk for big
//!   reference files (symbol masters) that change daily.

use crate::api::{STATS, SharedCache};
use std::collections::HashSet;
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The browser-like User-Agent StockWise already uses for NSE option chains.
pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                              (KHTML, like Gecko) Chrome/129.0 Safari/537.36";
/// Attempts per request (the first try plus two retries).
const MAX_ATTEMPTS: u32 = 3;
/// Never sleep longer than this before a retry, whatever `Retry-After` says.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(10);
/// How long a refusal (or an unreachable site) keeps StockWise away. A CLI
/// run is shorter than this; long-running `serve`, `tui` and daemons try
/// again afterwards.
const BLOCK_FOR: Duration = Duration::from_secs(15 * 60);
/// A 401 renews the session cookies, but not more often than this.
const RENEW_AFTER: Duration = Duration::from_secs(60);

/// Where a site lives and how its pages expect to be asked.
#[derive(Clone, Debug)]
pub struct Site {
    /// Short name for messages: `NSE`, `BSE`.
    pub name: &'static str,
    /// API base URL, without a trailing slash.
    pub base: String,
    /// Page loaded once for session cookies before the first API call
    /// (a full URL), or `None` when the API needs no cookies.
    pub home: Option<String>,
    /// Sent as `Referer` with every API call: the page that would make it.
    pub referer: String,
    /// Sent as `Origin` when the site checks it (BSE).
    pub origin: Option<String>,
}

/// Why a request produced no usable answer. Messages name the site and
/// never include cookies.
#[derive(Debug, Clone, PartialEq)]
pub enum FetchError {
    /// The site refused (HTTP 403, an "Access Denied" page, or a session
    /// that would not renew).
    Blocked(String),
    /// The endpoint is gone (HTTP 404) — usually a site redesign.
    NotFound(String),
    /// The answer is not in the shape StockWise reads.
    Format(String),
    /// Unreachable, timing out, rate limited or failing after retries.
    Unavailable(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Blocked(m)
            | FetchError::NotFound(m)
            | FetchError::Format(m)
            | FetchError::Unavailable(m) => write!(f, "{}", m),
        }
    }
}

impl std::error::Error for FetchError {}

/// Spaces requests out: consecutive requests start at least `interval`
/// apart, plus up to `jitter` of random delay.
pub struct Limiter {
    next: Mutex<Instant>,
    interval: Duration,
    jitter: Duration,
}

impl Limiter {
    pub fn new(interval: Duration, jitter: Duration) -> Self {
        Self {
            next: Mutex::new(Instant::now()),
            interval,
            jitter,
        }
    }

    /// At most three requests a second (350 ms apart) plus 0–150 ms jitter.
    pub fn polite() -> Self {
        Self::new(Duration::from_millis(350), Duration::from_millis(150))
    }

    /// No spacing (tests against a local mock).
    pub fn none() -> Self {
        Self::new(Duration::ZERO, Duration::ZERO)
    }

    /// Wait for the next slot. Slots are reserved under the lock, so
    /// concurrent callers queue on one schedule; the lock is never held
    /// across the sleep.
    pub async fn wait(&self) {
        let slot = {
            let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
            let slot = (*next).max(Instant::now());
            *next = slot + self.interval + random_up_to(self.jitter);
            slot
        };
        tokio::time::sleep_until(slot.into()).await;
    }
}

/// A random duration in `0..=max` (no `rand` crate: the std hasher is
/// randomly keyed per process).
fn random_up_to(max: Duration) -> Duration {
    if max.is_zero() {
        return max;
    }
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    );
    Duration::from_millis(h.finish() % (max.as_millis() as u64 + 1))
}

/// Delay before retry `attempt` (0-based): exponential with jitter, or the
/// server's `Retry-After` seconds, capped.
fn backoff(attempt: u32, base: Duration, retry_after: Option<u64>) -> Duration {
    retry_after
        .map(Duration::from_secs)
        .unwrap_or_else(|| base * 2u32.pow(attempt) + random_up_to(base))
        .min(MAX_RETRY_DELAY)
}

/// How one HTTP answer is handled.
#[derive(Debug, PartialEq)]
enum Verdict {
    Ok,
    /// Session cookies expired (HTTP 401): renew them once.
    Expired,
    Blocked(String),
    NotFound,
    /// Worth another try after a pause.
    Retry(String),
    Fail(String),
}

/// An HTML refusal page served in place of JSON (Akamai's "Access Denied").
pub fn looks_blocked(body: &str) -> bool {
    let head: String = body.trim_start().chars().take(600).collect();
    head.starts_with('<') && head.to_ascii_lowercase().contains("access denied")
}

fn classify(status: u16, body: &str) -> Verdict {
    match status {
        200..=299 if looks_blocked(body) => Verdict::Blocked("an \"Access Denied\" page".into()),
        200..=299 => Verdict::Ok,
        401 => Verdict::Expired,
        403 => Verdict::Blocked("HTTP 403".into()),
        404 => Verdict::NotFound,
        429 => Verdict::Retry("HTTP 429, rate limited".into()),
        500..=599 => Verdict::Retry(format!("HTTP {}", status)),
        _ => Verdict::Fail(format!("HTTP {}", status)),
    }
}

/// Where big reference files are kept between runs.
#[derive(Clone, Debug)]
pub enum DiskCache {
    /// `cache/` in the data directory (`STOCKWISE_HOME`).
    DataDir,
    Dir(PathBuf),
    Off,
}

/// The session cookies' load: when, and a counter that tells a request which
/// cookies it was sent with.
#[derive(Clone, Copy, Debug, Default)]
struct CookieState {
    at: Option<Instant>,
    /// 0 before the first load, then one more per load.
    generation: u64,
}

/// One site's session: cookies, spacing, cache and refusal state.
pub struct Session {
    site: Site,
    client: reqwest::Client,
    limiter: Limiter,
    /// When the session cookies were last fetched, and which load that was.
    cookies: tokio::sync::Mutex<CookieState>,
    cache: OnceLock<SharedCache>,
    disk: DiskCache,
    blocked_until: Mutex<Option<Instant>>,
    /// Whether a refusal remembered on disk was looked for yet.
    refusal_checked: std::sync::atomic::AtomicBool,
    /// Endpoints that refused us while the session itself works (NSE refuses `quote-equity` but serves the option chain): only
    /// that path is left alone, until the time given.
    blocked_paths: Mutex<std::collections::HashMap<String, Instant>>,
    noticed: Mutex<HashSet<String>>,
    retry_base: Duration,
    renew_after: Duration,
}

impl Session {
    pub fn new(site: Site, limiter: Limiter) -> Self {
        Self {
            site,
            client: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .cookie_store(true)
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(20))
                .build()
                .expect("HTTP client builds"),
            limiter,
            cookies: tokio::sync::Mutex::new(CookieState::default()),
            cache: OnceLock::new(),
            disk: DiskCache::DataDir,
            blocked_until: Mutex::new(None),
            refusal_checked: std::sync::atomic::AtomicBool::new(false),
            blocked_paths: Mutex::new(std::collections::HashMap::new()),
            noticed: Mutex::new(HashSet::new()),
            retry_base: Duration::from_millis(500),
            renew_after: RENEW_AFTER,
        }
    }

    /// For tests: no spacing between requests or retries, files in `disk`.
    #[cfg(test)]
    pub fn for_tests(site: Site, disk: DiskCache) -> Self {
        let mut s = Self::new(site, Limiter::none());
        s.disk = disk;
        s.retry_base = Duration::from_millis(1);
        s.renew_after = Duration::ZERO;
        s
    }

    pub fn name(&self) -> &'static str {
        self.site.name
    }

    /// Share the Redis / in-memory cache. The first cache handed in stays.
    pub fn set_cache(&self, cache: SharedCache) {
        let _ = self.cache.set(cache);
    }

    /// Whether the site refused us (or was unreachable) recently. A refusal
    /// is remembered on disk for `BLOCK_FOR`, so the next run does not spend
    /// a request (and the user's time) asking again.
    pub fn is_blocked(&self) -> bool {
        self.load_refusal();
        self.blocked_until
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|until| Instant::now() < until)
    }

    /// Whether `path` alone refused us recently (in this run or, through the
    /// file on disk, an earlier one).
    fn path_blocked(&self, path: &str) -> bool {
        self.load_refusal();
        self.blocked_paths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .is_some_and(|until| Instant::now() < *until)
    }

    fn block_path(&self, path: &str) {
        self.blocked_paths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.to_string(), Instant::now() + BLOCK_FOR);
        // Remember it for the next runs: `<unix seconds> <path>` per line.
        if let Some(file) = self.refused_paths_file() {
            let now = crate::selfupdate::now_secs();
            let mut lines: Vec<String> = std::fs::read_to_string(&file)
                .unwrap_or_default()
                .lines()
                .filter(|l| {
                    l.split_once(' ').is_some_and(|(at, p)| {
                        p != path
                            && at
                                .parse::<u64>()
                                .is_ok_and(|at| now.saturating_sub(at) < BLOCK_FOR.as_secs())
                    })
                })
                .map(str::to_string)
                .collect();
            lines.push(format!("{now} {path}"));
            let _ = std::fs::write(file, lines.join("\n"));
        }
    }

    fn refused_paths_file(&self) -> Option<PathBuf> {
        self.refusal_file().map(|f| {
            f.with_file_name(format!(
                "{}-refused-paths.txt",
                self.site.name.to_ascii_lowercase()
            ))
        })
    }

    /// The endpoint refusals an earlier run remembered.
    fn load_path_refusals(&self) {
        let Some(text) = self
            .refused_paths_file()
            .and_then(|p| std::fs::read_to_string(p).ok())
        else {
            return;
        };
        let now = crate::selfupdate::now_secs();
        let mut paths = self.blocked_paths.lock().unwrap_or_else(|e| e.into_inner());
        for (at, path) in text.lines().filter_map(|l| l.split_once(' ')) {
            let Ok(at) = at.parse::<u64>() else { continue };
            let age = Duration::from_secs(now.saturating_sub(at));
            if age < BLOCK_FOR {
                paths.insert(path.to_string(), Instant::now() + (BLOCK_FOR - age));
            }
        }
    }

    fn block(&self) {
        *self.blocked_until.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Instant::now() + BLOCK_FOR);
    }

    /// The file that remembers a refusal across runs.
    fn refusal_file(&self) -> Option<PathBuf> {
        // Unit tests share one data directory: only a test's own folder.
        #[cfg(test)]
        if matches!(self.disk, DiskCache::DataDir) {
            return None;
        }
        self.disk_path(&format!(
            "{}-refused.txt",
            self.site.name.to_ascii_lowercase()
        ))
    }

    /// Remember a refusal (not an outage: a network that comes back should
    /// be used at once) for the next runs.
    fn remember_refusal(&self) {
        if let Some(path) = self.refusal_file() {
            let _ = std::fs::write(path, crate::selfupdate::now_secs().to_string());
        }
    }

    /// Once per process: pick up a refusal an earlier run remembered.
    fn load_refusal(&self) {
        if self.refusal_checked.swap(true, Ordering::Relaxed) {
            return;
        }
        self.load_path_refusals();
        let Some(at) = self
            .refusal_file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| t.trim().parse::<u64>().ok())
        else {
            return;
        };
        let age = Duration::from_secs(crate::selfupdate::now_secs().saturating_sub(at));
        if age < BLOCK_FOR {
            *self.blocked_until.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Instant::now() + (BLOCK_FOR - age));
        }
    }

    /// Say once per process and problem, on stderr, why the site isn't
    /// answering and what StockWise does instead. A refusal (or an
    /// unreachable site) is announced once; the requests skipped after it
    /// are not announced again.
    pub fn notice(&self, err: &FetchError, fallback: &str) {
        let key = match err {
            FetchError::Blocked(_) | FetchError::Unavailable(_) => "stopped".to_string(),
            _ => format!("{} — {}", err, fallback),
        };
        if self
            .noticed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key)
        {
            eprintln!("  \u{24d8} {} — {}", err, fallback);
        }
    }

    /// A format error for `what` (`"quote: no priceInfo"`).
    pub fn format_error(&self, what: &str) -> FetchError {
        FetchError::Format(format!(
            "{} changed its response format ({})",
            self.site.name, what
        ))
    }

    fn url(&self, path: &str, query: &[(&str, &str)]) -> Result<reqwest::Url, FetchError> {
        let full = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{}", self.site.base, path)
        };
        let mut url = reqwest::Url::parse(&full)
            .map_err(|_| FetchError::Format(format!("bad {} URL {}", self.site.name, full)))?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url)
    }

    /// Load the home page for session cookies, once. After a 401, `expired`
    /// is the cookie generation the failed request was sent with: when
    /// another task has loaded newer cookies since, they are reused (the
    /// caller just retries); only one task renews, the rest wait on the lock.
    /// A request that already carried the newest cookies and still got a 401
    /// is a real refusal, and a renewal is not repeated within `renew_after`.
    async fn ensure_cookies(&self, expired: Option<u64>) -> Result<(), FetchError> {
        let Some(home) = &self.site.home else {
            return Ok(());
        };
        let mut loaded = self.cookies.lock().await;
        match (loaded.at, expired) {
            (Some(_), None) => return Ok(()),
            (Some(_), Some(seen)) if loaded.generation != seen => return Ok(()),
            (Some(at), Some(_)) if at.elapsed() < self.renew_after => {
                return Err(FetchError::Blocked(format!(
                    "{} ended the session and would not renew it",
                    self.site.name
                )));
            }
            _ => {}
        }
        self.limiter.wait().await;
        STATS.requests.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        let sent = self
            .client
            .get(home)
            .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .send()
            .await;
        crate::netlog::request(
            self.site.name,
            home,
            sent.as_ref().ok().map(|r| r.status().as_u16()),
            started.elapsed(),
        );
        let resp = sent.map_err(|e| {
            FetchError::Unavailable(format!(
                "{} is unreachable ({})",
                self.site.name,
                e.without_url()
            ))
        })?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        if status == 403 || looks_blocked(&body) {
            return Err(FetchError::Blocked(format!(
                "{} refused the session (HTTP {})",
                self.site.name, status
            )));
        }
        // Any other answer: carry on; the API call says whether it worked.
        loaded.at = Some(Instant::now());
        loaded.generation += 1;
        Ok(())
    }

    /// GET a JSON endpoint. `path` is relative to the site's base URL (or a
    /// full URL); `ttl` is how long the answer stays cached (zero: never).
    pub async fn get_json(
        &self,
        path: &str,
        query: &[(&str, &str)],
        ttl: Duration,
    ) -> Result<serde_json::Value, FetchError> {
        let body = self.get_text(path, query, ttl).await?;
        serde_json::from_str(&body).map_err(|_| self.format_error("response is not JSON"))
    }

    /// GET any text endpoint (CSV downloads), with the same rules.
    pub async fn get_text(
        &self,
        path: &str,
        query: &[(&str, &str)],
        ttl: Duration,
    ) -> Result<String, FetchError> {
        let url = self.url(path, query)?;
        let cache_key = format!("exchange:{}:{}", self.site.name.to_ascii_lowercase(), url);
        if !ttl.is_zero()
            && let Some(cache) = self.cache.get()
            && let Some(body) = cache.lock().await.get(&cache_key).await
        {
            crate::netlog::cache_hit(self.site.name, url.as_str());
            return Ok(body);
        }
        let path = url.path().to_string();
        let result = self.fetch(url).await;
        match &result {
            Ok(body) if !ttl.is_zero() => {
                if let Some(cache) = self.cache.get() {
                    cache.lock().await.set(cache_key, body.clone(), ttl).await;
                }
            }
            Ok(_) => {}
            // One endpoint refused, the session works: only that path waits.
            Err(FetchError::Blocked(_)) if self.path_blocked(&path) => {
                STATS.errors.fetch_add(1, Ordering::Relaxed);
            }
            Err(e @ (FetchError::Blocked(_) | FetchError::Unavailable(_))) => {
                STATS.errors.fetch_add(1, Ordering::Relaxed);
                if !self.is_blocked() {
                    self.block();
                    if matches!(e, FetchError::Blocked(_)) {
                        self.remember_refusal();
                    }
                }
            }
            Err(_) => {
                STATS.errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        result
    }

    async fn fetch(&self, url: reqwest::Url) -> Result<String, FetchError> {
        let name = self.site.name;
        if self.is_blocked() {
            return Err(FetchError::Blocked(format!(
                "{} refused an earlier request; not asking again for now",
                name
            )));
        }
        if self.path_blocked(url.path()) {
            return Err(FetchError::Blocked(format!(
                "{} refused {} earlier; not asking again for now",
                name,
                url.path()
            )));
        }
        self.ensure_cookies(None).await?;
        let mut renewed = false;
        let mut attempt = 0;
        loop {
            // Which cookies this request goes out with (a renewal in flight
            // makes this wait until it is done).
            let generation = self.cookies.lock().await.generation;
            self.limiter.wait().await;
            STATS.requests.fetch_add(1, Ordering::Relaxed);
            let mut req = self
                .client
                .get(url.clone())
                .header(reqwest::header::ACCEPT, "application/json, text/plain, */*")
                .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
                .header(reqwest::header::REFERER, &self.site.referer);
            if let Some(origin) = &self.site.origin {
                req = req.header(reqwest::header::ORIGIN, origin);
            }
            let last = attempt + 1 >= MAX_ATTEMPTS;
            let started = Instant::now();
            let sent = req.send().await;
            crate::netlog::request(
                name,
                url.as_str(),
                sent.as_ref().ok().map(|r| r.status().as_u16()),
                started.elapsed(),
            );
            let resp = match sent {
                Ok(resp) => resp,
                Err(_) if !last => {
                    tokio::time::sleep(backoff(attempt, self.retry_base, None)).await;
                    attempt += 1;
                    continue;
                }
                Err(e) => {
                    return Err(FetchError::Unavailable(format!(
                        "{} is unreachable ({})",
                        name,
                        e.without_url()
                    )));
                }
            };
            let status = resp.status().as_u16();
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok());
            let body = resp.text().await.map_err(|e| {
                FetchError::Unavailable(format!(
                    "{} response unreadable ({})",
                    name,
                    e.without_url()
                ))
            })?;
            match classify(status, &body) {
                Verdict::Ok => return Ok(body),
                Verdict::Expired if self.site.home.is_some() && !renewed => {
                    renewed = true;
                    self.ensure_cookies(Some(generation)).await?;
                }
                Verdict::Expired => {
                    return Err(FetchError::Blocked(format!(
                        "{} refused the request (HTTP 401)",
                        name
                    )));
                }
                Verdict::Blocked(why) => {
                    // The cookie page answered, so this is the endpoint's
                    // refusal (NSE's CDN refuses `quote-equity` but serves
                    // the option chain): only this path waits. A site with
                    // no cookie page has nothing else to tell it apart.
                    if self.site.home.is_some() {
                        self.block_path(url.path());
                    }
                    return Err(FetchError::Blocked(format!(
                        "{} refused the request ({})",
                        name, why
                    )));
                }
                Verdict::NotFound => {
                    return Err(FetchError::NotFound(format!(
                        "{} no longer serves {}",
                        name,
                        url.path()
                    )));
                }
                Verdict::Retry(_) if !last => {
                    tokio::time::sleep(backoff(attempt, self.retry_base, retry_after)).await;
                    attempt += 1;
                }
                Verdict::Retry(why) | Verdict::Fail(why) => {
                    return Err(FetchError::Unavailable(format!(
                        "{} did not answer ({})",
                        name, why
                    )));
                }
            }
        }
    }

    fn disk_path(&self, file: &str) -> Option<PathBuf> {
        let dir = match &self.disk {
            DiskCache::Off => return None,
            DiskCache::Dir(dir) => dir.clone(),
            DiskCache::DataDir => crate::store::data_dir().ok()?.join("cache"),
        };
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir.join(file))
    }

    /// A reference file kept on disk as `file` for `max_age`. A failed
    /// download falls back to an older copy (with a notice).
    pub async fn get_text_on_disk(
        &self,
        file: &str,
        path: &str,
        query: &[(&str, &str)],
        max_age: Duration,
        valid: fn(&str) -> bool,
    ) -> Result<String, FetchError> {
        let disk = self.disk_path(file);
        let fresh = |p: &PathBuf| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < max_age)
        };
        if let Some(p) = &disk
            && fresh(p)
            && let Ok(text) = std::fs::read_to_string(p)
            && valid(&text)
        {
            return Ok(text);
        }
        let fetched = self.get_text(path, query, Duration::ZERO).await;
        match fetched {
            Ok(text) if valid(&text) => {
                if let Some(p) = &disk {
                    // Best effort: a read-only data directory still works.
                    let _ = crate::pagecache::write_atomic(p, &text);
                }
                Ok(text)
            }
            other => {
                // Unreachable and unparseable both fall back to the saved copy:
                // a 200 with a maintenance page is no more useful than a 503.
                let error = match other {
                    Ok(_) => self.format_error(file),
                    Err(e) => e,
                };
                match disk.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
                    Some(text) if valid(&text) => {
                        self.notice(&error, &format!("using the saved copy of {}", file));
                        Ok(text)
                    }
                    _ => Err(error),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn a_plain_403_from_one_endpoint_leaves_the_others_open() {
        // The cookie page answers, one API refuses with a plain 403, another works.
        let server = crate::testutil::MockServer::start(vec![
            (200, "<html>home</html>".into()),
            (403, r#"{"msg":"forbidden"}"#.into()),
            (200, r#"{"ok":true}"#.into()),
        ])
        .await;
        let s = Session::for_tests(super::super::nse::site(&server.url), DiskCache::Off);
        assert!(
            s.get_json("/api/refused", &[], Duration::ZERO)
                .await
                .is_err()
        );
        assert!(!s.is_blocked(), "the session stays open");
        assert!(s.get_json("/api/works", &[], Duration::ZERO).await.is_ok());
        // The refused path is not asked again.
        let before = server.requests().len();
        assert!(
            s.get_json("/api/refused", &[], Duration::ZERO)
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), before);
    }

    #[tokio::test]
    async fn an_endpoint_refusal_is_remembered_by_the_next_session() {
        let dir = std::env::temp_dir().join(format!(
            "sw-path-refusal-{}-{}",
            std::process::id(),
            crate::selfupdate::now_secs()
        ));
        let server = crate::testutil::MockServer::start(vec![
            (200, "<html>home</html>".into()),
            (403, "<HTML>Access Denied</HTML>".into()),
            (200, r#"{"ok":true}"#.into()),
        ])
        .await;
        let site = || super::super::nse::site(&server.url);
        let first = Session::for_tests(site(), DiskCache::Dir(dir.clone()));
        assert!(
            first
                .get_json("/api/refused", &[], Duration::ZERO)
                .await
                .is_err()
        );
        let next = Session::for_tests(site(), DiskCache::Dir(dir.clone()));
        let before = server.requests().len();
        assert!(
            next.get_json("/api/refused", &[], Duration::ZERO)
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), before, "not asked again");
        assert!(!next.is_blocked());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_refusal_is_remembered_by_the_next_session_but_an_outage_is_not() {
        let dir = std::env::temp_dir().join(format!(
            "sw-refusal-{}-{}",
            std::process::id(),
            crate::selfupdate::now_secs()
        ));
        let site = || super::super::nse::site("http://127.0.0.1:9");
        let first = Session::for_tests(site(), DiskCache::Dir(dir.clone()));
        assert!(!first.is_blocked());
        first.block();
        first.remember_refusal();
        let next = Session::for_tests(site(), DiskCache::Dir(dir.clone()));
        assert!(next.is_blocked(), "the refusal carries over");
        // An old refusal has run out.
        std::fs::write(
            next.refusal_file().unwrap(),
            (crate::selfupdate::now_secs() - BLOCK_FOR.as_secs() - 5).to_string(),
        )
        .unwrap();
        let later = Session::for_tests(site(), DiskCache::Dir(dir.clone()));
        assert!(!later.is_blocked());
        let _ = std::fs::remove_dir_all(dir);
    }

    use super::*;
    use crate::testutil::MockServer;
    use std::sync::Arc;

    fn site(base: &str, home: bool) -> Site {
        Site {
            name: "NSE",
            base: base.to_string(),
            home: home.then(|| format!("{}/", base)),
            referer: format!("{}/", base),
            origin: None,
        }
    }

    #[test]
    fn verdicts_never_retry_a_refusal() {
        assert_eq!(classify(200, "{}"), Verdict::Ok);
        assert_eq!(classify(401, ""), Verdict::Expired);
        assert!(matches!(classify(403, ""), Verdict::Blocked(_)));
        assert!(matches!(
            classify(
                200,
                "<HTML><HEAD>\n<TITLE>Access Denied</TITLE>\n</HEAD><BODY>"
            ),
            Verdict::Blocked(_)
        ));
        assert_eq!(classify(404, ""), Verdict::NotFound);
        assert!(matches!(classify(429, ""), Verdict::Retry(_)));
        assert!(matches!(classify(503, ""), Verdict::Retry(_)));
        assert!(matches!(classify(418, ""), Verdict::Fail(_)));
        // JSON that mentions the words is not a refusal page.
        assert!(!looks_blocked(r#"{"msg":"Access Denied"}"#));
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let base = Duration::from_millis(100);
        let first = backoff(0, base, None);
        assert!(first >= base && first <= base * 2, "{first:?}");
        let third = backoff(2, base, None);
        assert!(third >= base * 4, "{third:?}");
        assert_eq!(backoff(0, base, Some(3)), Duration::from_secs(3));
        assert_eq!(backoff(0, base, Some(3600)), MAX_RETRY_DELAY);
    }

    #[tokio::test]
    async fn limiter_keeps_requests_apart() {
        let limiter = Limiter::new(Duration::from_millis(40), Duration::from_millis(10));
        let started = Instant::now();
        for _ in 0..4 {
            limiter.wait().await;
        }
        // Three gaps of at least 40 ms after the first, immediate request.
        assert!(started.elapsed() >= Duration::from_millis(120));
        assert!(Limiter::polite().interval >= Duration::from_millis(334));
    }

    #[tokio::test]
    async fn loads_cookies_once_and_sends_them_back() {
        let server = MockServer::start_with_headers(vec![
            (
                200,
                vec![("set-cookie".into(), "nsit=abc123; Path=/".into())],
                "<html>home</html>".into(),
            ),
            (200, vec![], r#"{"ok":1}"#.into()),
        ])
        .await;
        let s = Session::for_tests(site(&server.url, true), DiskCache::Off);
        assert_eq!(
            s.get_json("/api/a", &[], Duration::ZERO).await.unwrap()["ok"],
            1
        );
        assert_eq!(
            s.get_json("/api/b", &[("q", "M&M")], Duration::ZERO)
                .await
                .unwrap()["ok"],
            1
        );
        let reqs = server.requests();
        assert_eq!(reqs.len(), 3, "home page once, then two API calls");
        assert_eq!(reqs[0].target, "/");
        assert_eq!(reqs[1].header("cookie"), Some("nsit=abc123"));
        assert_eq!(reqs[2].target, "/api/b?q=M%26M");
        assert!(reqs[2].header("referer").is_some());
        assert!(
            reqs[2]
                .header("accept")
                .unwrap()
                .contains("application/json")
        );
    }

    #[tokio::test]
    async fn a_refusal_blocks_without_retrying() {
        let server = MockServer::start(vec![
            (200, "<html>home</html>".into()),
            (403, "<HTML>Access Denied</HTML>".into()),
            (200, r#"{"ok":1}"#.into()),
        ])
        .await;
        let s = Session::for_tests(site(&server.url, true), DiskCache::Off);
        let err = s.get_json("/api/a", &[], Duration::ZERO).await.unwrap_err();
        assert!(matches!(err, FetchError::Blocked(_)), "{err:?}");
        assert!(err.to_string().contains("NSE refused"), "{err}");
        // The cookie page answered, so only the refusing endpoint waits.
        assert!(!s.is_blocked());
        assert!(s.get_json("/api/a", &[], Duration::ZERO).await.is_err());
        assert_eq!(server.requests().len(), 2, "no retry, no second ask");
        assert!(s.get_json("/api/b", &[], Duration::ZERO).await.is_ok());
    }

    #[tokio::test]
    async fn a_refused_cookie_page_blocks_the_whole_session() {
        let server = MockServer::start(vec![(403, "<HTML>Access Denied</HTML>".into())]).await;
        let s = Session::for_tests(site(&server.url, true), DiskCache::Off);
        assert!(s.get_json("/api/a", &[], Duration::ZERO).await.is_err());
        assert!(s.is_blocked());
        assert!(s.get_json("/api/b", &[], Duration::ZERO).await.is_err());
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn server_errors_are_retried_then_succeed() {
        let server = MockServer::start(vec![
            (503, "busy".into()),
            (500, "busy".into()),
            (200, r#"{"ok":1}"#.into()),
        ])
        .await;
        let s = Session::for_tests(site(&server.url, false), DiskCache::Off);
        assert_eq!(
            s.get_json("/x", &[], Duration::ZERO).await.unwrap()["ok"],
            1
        );
        assert_eq!(server.requests().len(), 3);
    }

    #[tokio::test]
    async fn retries_stop_after_three_attempts() {
        let server = MockServer::start(vec![(503, "busy".into())]).await;
        let s = Session::for_tests(site(&server.url, false), DiskCache::Off);
        let err = s.get_json("/x", &[], Duration::ZERO).await.unwrap_err();
        assert!(matches!(err, FetchError::Unavailable(_)), "{err:?}");
        assert_eq!(server.requests().len(), 3);
    }

    #[tokio::test]
    async fn an_expired_session_is_renewed_once() {
        let server = MockServer::start(vec![
            (200, "<html>home</html>".into()),
            (401, "".into()),
            (200, "<html>home again</html>".into()),
            (200, r#"{"ok":1}"#.into()),
        ])
        .await;
        let s = Session::for_tests(site(&server.url, true), DiskCache::Off);
        assert_eq!(
            s.get_json("/api/a", &[], Duration::ZERO).await.unwrap()["ok"],
            1
        );
        let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
        assert_eq!(targets, ["/", "/api/a", "/", "/api/a"]);
    }

    #[tokio::test]
    async fn a_401_right_after_loading_cookies_is_a_refusal() {
        let server =
            MockServer::start(vec![(200, "<html>home</html>".into()), (401, "".into())]).await;
        let mut s = Session::for_tests(site(&server.url, true), DiskCache::Off);
        s.renew_after = RENEW_AFTER;
        // Renewing straight away would only loop: stop instead.
        let err = s.get_json("/api/a", &[], Duration::ZERO).await.unwrap_err();
        assert!(matches!(err, FetchError::Blocked(_)), "{err:?}");
        assert_eq!(server.requests().len(), 2);
    }

    /// A tiny site: `/` hands out a new session cookie each time; `/api`
    /// accepts only the second session onwards (the first one has "expired"),
    /// after a short pause so concurrent callers all meet the 401.
    async fn expiring_site() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::AtomicUsize;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let homes = Arc::new(AtomicUsize::new(0));
        let counter = homes.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let counter = counter.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    let reply = if head.starts_with("GET / ") {
                        let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                        format!(
                            "HTTP/1.1 200 X\r\nset-cookie: sid={n}; Path=/\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi"
                        )
                    } else {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        if head.contains("sid=2") {
                            "HTTP/1.1 200 X\r\ncontent-length: 8\r\nconnection: close\r\n\r\n{\"ok\":1}"
                                .to_string()
                        } else {
                            "HTTP/1.1 401 X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                                .to_string()
                        }
                    };
                    let _ = stream.write_all(reply.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        (url, homes)
    }

    #[tokio::test]
    async fn concurrent_requests_on_an_expired_session_renew_it_once() {
        let (url, homes) = expiring_site().await;
        let mut s = Session::for_tests(site(&url, true), DiskCache::Off);
        // A renewal spacing the callers' 401s fall inside: a second task
        // that finds the cookies already renewed must reuse them, not treat
        // that as a refusal and lock the provider out.
        s.renew_after = Duration::from_millis(300);
        s.ensure_cookies(None).await.unwrap();
        tokio::time::sleep(Duration::from_millis(350)).await;
        let s = &s;
        let (a, b, c, d) = tokio::join!(
            s.get_json("/api/a", &[], Duration::ZERO),
            s.get_json("/api/b", &[], Duration::ZERO),
            s.get_json("/api/c", &[], Duration::ZERO),
            s.get_json("/api/d", &[], Duration::ZERO),
        );
        for r in [a, b, c, d] {
            assert_eq!(r.expect("request succeeds after the renewal")["ok"], 1);
        }
        assert!(!s.is_blocked(), "no false lockout");
        assert_eq!(
            homes.load(Ordering::SeqCst),
            2,
            "the first load plus one renewal for all four callers"
        );
    }

    #[tokio::test]
    async fn not_found_and_non_json_are_reported() {
        let server = MockServer::start(vec![(404, "gone".into()), (200, "<html>".into())]).await;
        let s = Session::for_tests(site(&server.url, false), DiskCache::Off);
        let err = s
            .get_json("/api/search", &[], Duration::ZERO)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::NotFound(_)), "{err:?}");
        let err = s.get_json("/api/x", &[], Duration::ZERO).await.unwrap_err();
        assert!(matches!(err, FetchError::Format(_)), "{err:?}");
        assert!(err.to_string().contains("changed its response format"));
        assert!(!s.is_blocked());
    }

    #[tokio::test]
    async fn reference_files_are_kept_on_disk() {
        let dir = std::env::temp_dir().join(format!("sw-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let server = MockServer::start(vec![(200, "SYMBOL,NAME\nTCS,Tata".into())]).await;
        let s = Session::for_tests(site(&server.url, false), DiskCache::Dir(dir.clone()));
        let valid = |t: &str| t.starts_with("SYMBOL");
        let day = Duration::from_secs(86_400);
        let a = s
            .get_text_on_disk("list.csv", "/list.csv", &[], day, valid)
            .await
            .unwrap();
        let b = s
            .get_text_on_disk("list.csv", "/list.csv", &[], day, valid)
            .await
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(server.requests().len(), 1, "second read comes from disk");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_invalid_200_uses_the_saved_reference_file() {
        let dir = std::env::temp_dir().join(format!("sw-session-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let server = MockServer::start(vec![
            (200, "SYMBOL,NAME\nTCS,Tata".into()),
            (200, "<html>maintenance</html>".into()),
        ])
        .await;
        let s = Session::for_tests(site(&server.url, false), DiskCache::Dir(dir.clone()));
        let valid = |t: &str| t.starts_with("SYMBOL");
        let day = Duration::from_secs(86_400);
        let saved = s
            .get_text_on_disk("list.csv", "/list.csv", &[], day, valid)
            .await
            .unwrap();
        // Zero max age forces the refetch that returns the invalid page.
        let served = s
            .get_text_on_disk("list.csv", "/list.csv", &[], Duration::ZERO, valid)
            .await
            .unwrap();
        assert_eq!(served, saved, "the saved copy stands in for a bad answer");
        assert_eq!(server.requests().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_invalid_200_without_a_saved_copy_is_a_format_error() {
        let server = MockServer::start(vec![(200, "<html>maintenance</html>".into())]).await;
        let s = Session::for_tests(site(&server.url, false), DiskCache::Off);
        let err = s
            .get_text_on_disk("list.csv", "/list.csv", &[], Duration::ZERO, |_| false)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Format(_)), "{err:?}");
    }
}
