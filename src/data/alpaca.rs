//! Minimal Alpaca REST client: daily bars, discovery, calendar, account and
//! order execution. Auth headers follow Alpaca's `APCA-API-KEY-ID` /
//! `APCA-API-SECRET-KEY` scheme.
//!
//! Retries are only applied to idempotent GET requests. Order submission is
//! never retried blindly: callers reconcile by `client_order_id` after an
//! ambiguous network failure.

use crate::config::AlpacaConfig;
use crate::error::{Error, Result};
use crate::model::Bar;
use chrono::{DateTime, NaiveDate, Utc};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const MAX_BARS_PER_PAGE: usize = 10_000;
const SYMBOLS_PER_BARS_REQUEST: usize = 10;
const BARS_CONCURRENCY: usize = 4;
const MAX_PAGES: usize = 100;

static ORDER_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct AlpacaClient {
    http: reqwest::Client,
    data_url: String,
    trading_url: String,
    key_id: String,
    secret_key: String,
    pub feed: String,
    pub adjustment: String,
    max_retries: u32,
    limiter: Arc<RateLimiter>,
}

impl AlpacaClient {
    pub fn new(cfg: &AlpacaConfig, paper: bool) -> Result<Self> {
        if !cfg.has_credentials() {
            return Err(Error::MissingCredentials);
        }
        Self::with_urls(
            cfg,
            cfg.trading_url(paper),
            cfg.key_id.clone(),
            cfg.secret_key.clone(),
        )
    }

    /// Construct with explicit URLs/credentials (used by tests with a local mock).
    pub fn with_urls(
        cfg: &AlpacaConfig,
        trading_url: impl Into<String>,
        key_id: String,
        secret_key: String,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_secs))
            .user_agent(format!("tpi-rs/{}", crate::VERSION))
            .pool_max_idle_per_host(8)
            .build()
            .map_err(|e| Error::config(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            http,
            data_url: cfg.data_url.trim_end_matches('/').to_string(),
            trading_url: trading_url.into().trim_end_matches('/').to_string(),
            key_id,
            secret_key,
            feed: cfg.feed.clone(),
            adjustment: cfg.adjustment.clone(),
            max_retries: cfg.max_retries,
            limiter: Arc::new(RateLimiter::new(cfg.rate_limit_per_min)),
        })
    }

    pub fn trading_url(&self) -> &str {
        &self.trading_url
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.header("APCA-API-KEY-ID", &self.key_id)
            .header("APCA-API-SECRET-KEY", &self.secret_key)
            .header("accept", "application/json")
    }

    // -----------------------------------------------------------------------
    // Market data (GET, retried)
    // -----------------------------------------------------------------------

    /// Fetch daily bars for up to 10 symbols per request with pagination.
    /// Returns only symbols that have data.
    pub async fn daily_bars(
        &self,
        symbols: &[String],
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<HashMap<String, Vec<Bar>>> {
        let unique: Vec<String> = {
            let mut v: Vec<String> = symbols
                .iter()
                .map(|s| s.trim().to_ascii_uppercase())
                .filter(|s| !s.is_empty())
                .collect();
            v.sort();
            v.dedup();
            v
        };
        let chunks: Vec<Vec<String>> = unique
            .chunks(SYMBOLS_PER_BARS_REQUEST)
            .map(|c| c.to_vec())
            .collect();
        let results = stream::iter(
            chunks
                .into_iter()
                .map(|chunk| async move { self.daily_bars_chunk(&chunk, start, end).await }),
        )
        .buffer_unordered(BARS_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;

        let mut out: HashMap<String, Vec<Bar>> = HashMap::new();
        for result in results {
            match result {
                Ok(map) => {
                    for (symbol, bars) in map {
                        out.entry(symbol).or_default().extend(bars);
                    }
                }
                Err(e) => return Err(e),
            }
        }
        for bars in out.values_mut() {
            bars.sort_by_key(|b| b.date);
            bars.dedup_by_key(|b| b.date);
        }
        Ok(out)
    }

    async fn daily_bars_chunk(
        &self,
        symbols: &[String],
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<HashMap<String, Vec<Bar>>> {
        let mut out: HashMap<String, Vec<Bar>> = HashMap::new();
        let mut page_token: Option<String> = None;
        for page in 0..MAX_PAGES {
            let mut query = vec![
                ("symbols", symbols.join(",")),
                ("timeframe", "1Day".to_string()),
                ("start", format!("{start}T00:00:00Z")),
                ("end", format!("{end}T23:59:59Z")),
                ("limit", MAX_BARS_PER_PAGE.to_string()),
                ("adjustment", self.adjustment.clone()),
                ("feed", self.feed.clone()),
            ];
            if let Some(token) = &page_token {
                query.push(("page_token", token.clone()));
            }
            let path = "/v2/stocks/bars";
            let resp: MultiBarsResponse = self.get_json_at(&self.data_url, path, &query).await?;
            for (symbol, raw_bars) in resp.bars.unwrap_or_default() {
                let entry = out.entry(symbol.to_ascii_uppercase()).or_default();
                for raw in raw_bars {
                    if let Some(bar) = raw.into_bar(start, end) {
                        entry.push(bar);
                    }
                }
            }
            match resp.next_page_token {
                Some(t) if !t.is_empty() => page_token = Some(t),
                _ => {
                    break;
                }
            }
            if page == MAX_PAGES - 1 {
                return Err(Error::Api {
                    status: 0,
                    path: path.to_string(),
                    message: "bar pagination exceeded safety limit".to_string(),
                });
            }
        }
        Ok(out)
    }

    pub async fn most_active(&self, top: usize) -> Result<Vec<String>> {
        let top = top.clamp(1, 100);
        let path = "/v1beta1/screener/stocks/most-actives";
        let query = [("by", "volume".to_string()), ("top", top.to_string())];
        let resp: MostActivesResponse = self.get_json_at(&self.data_url, path, &query).await?;
        Ok(resp
            .most_actives
            .unwrap_or_default()
            .into_iter()
            .map(|a| a.symbol.to_ascii_uppercase())
            .filter(|s| !s.is_empty())
            .collect())
    }

    pub async fn clock(&self) -> Result<MarketClock> {
        let path = "/v2/clock";
        let raw: RawClock = self.get_json_at(&self.trading_url, path, &[]).await?;
        Ok(MarketClock {
            is_open: raw.is_open,
            next_open: parse_rfc3339(&raw.next_open)?,
            next_close: parse_rfc3339(&raw.next_close)?,
            timestamp: parse_rfc3339(&raw.timestamp)?,
        })
    }

    pub async fn calendar(&self, start: NaiveDate, end: NaiveDate) -> Result<Vec<CalendarDay>> {
        let path = "/v2/calendar";
        let query = [("start", start.to_string()), ("end", end.to_string())];
        let raw: Vec<RawCalendarDay> = self.get_json_at(&self.trading_url, path, &query).await?;
        raw.into_iter()
            .map(|d| {
                Ok(CalendarDay {
                    date: NaiveDate::parse_from_str(&d.date, "%Y-%m-%d").map_err(|e| {
                        Error::Api {
                            status: 200,
                            path: path.to_string(),
                            message: format!("invalid calendar date '{}': {e}", d.date),
                        }
                    })?,
                    open: d.open,
                    close: d.close,
                    settlement_date: d.settlement_date,
                })
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Trading (GET retried, writes never retried)
    // -----------------------------------------------------------------------

    pub async fn account(&self) -> Result<Account> {
        let path = "/v2/account";
        let raw: RawAccount = self.get_json_at(&self.trading_url, path, &[]).await?;
        Ok(raw.into_account())
    }

    pub async fn positions(&self) -> Result<Vec<Position>> {
        let path = "/v2/positions";
        let raw: Vec<RawPosition> = self.get_json_at(&self.trading_url, path, &[]).await?;
        Ok(raw.into_iter().map(|p| p.into_position()).collect())
    }

    pub async fn orders(&self, status: &str) -> Result<Vec<Order>> {
        let path = "/v2/orders";
        let query = [
            ("status", status.to_string()),
            ("limit", "100".to_string()),
            ("direction", "desc".to_string()),
        ];
        let raw: Vec<RawOrder> = self.get_json_at(&self.trading_url, path, &query).await?;
        Ok(raw.into_iter().map(|o| o.into_order()).collect())
    }

    /// Look up an order by client id. Returns None on 404.
    pub async fn order_by_client_id(&self, client_order_id: &str) -> Result<Option<Order>> {
        let path = "/v2/orders:by_client_order_id";
        let query = [("client_order_id", client_order_id.to_string())];
        let raw: Option<RawOrder> = self
            .get_optional_json(&self.trading_url, path, &query)
            .await?;
        Ok(raw.map(|o| o.into_order()))
    }

    pub async fn submit_order(&self, req: &OrderRequest) -> Result<Order> {
        let path = "/v2/orders";
        let value = self
            .write_json(reqwest::Method::POST, &self.trading_url, path, Some(req))
            .await?;
        let raw: RawOrder = serde_json::from_value(value).map_err(|e| Error::Api {
            status: 200,
            path: path.to_string(),
            message: format!("invalid order response: {e}"),
        })?;
        Ok(raw.into_order())
    }

    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        let path = format!("/v2/orders/{order_id}");
        self.write_empty(reqwest::Method::DELETE, &self.trading_url, &path)
            .await
    }

    pub async fn cancel_all_orders(&self) -> Result<()> {
        let path = "/v2/orders";
        self.write_empty(reqwest::Method::DELETE, &self.trading_url, path)
            .await
    }

    pub async fn close_position(&self, symbol: &str) -> Result<Order> {
        let symbol = symbol.trim().to_ascii_uppercase();
        let path = format!("/v2/positions/{symbol}");
        let value = self
            .write_json(
                reqwest::Method::DELETE,
                &self.trading_url,
                &path,
                None::<&()>,
            )
            .await?;
        let raw: RawOrder = serde_json::from_value(value).map_err(|e| Error::Api {
            status: 200,
            path: path.to_string(),
            message: format!("invalid close-position response: {e}"),
        })?;
        Ok(raw.into_order())
    }

    /// Cancel every open order, then close every position. Used by the kill switch.
    pub async fn emergency_flatten(&self) -> Result<FlattenReport> {
        let mut report = FlattenReport::default();
        self.cancel_all_orders().await?;
        report.orders_cancelled = true;
        let positions = self.positions().await?;
        for p in positions {
            match self.close_position(&p.symbol).await {
                Ok(order) => {
                    report
                        .closed
                        .push(format!("{} qty {} -> {}", p.symbol, p.qty, order.status));
                }
                Err(e) => report.errors.push(format!("{}: {e}", p.symbol)),
            }
        }
        Ok(report)
    }

    pub fn new_client_order_id(symbol: &str) -> String {
        let seq = ORDER_SEQ.fetch_add(1, Ordering::Relaxed);
        let symbol: String = symbol
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.')
            .take(10)
            .collect();
        format!(
            "tpi-{}-{}-{:03}",
            Utc::now().timestamp_millis(),
            symbol,
            seq % 1000
        )
    }

    // -----------------------------------------------------------------------
    // HTTP internals
    // -----------------------------------------------------------------------

    async fn get_json_at<T: serde::de::DeserializeOwned>(
        &self,
        base: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let url = format!("{base}{path}");
        let mut attempt = 0u32;
        loop {
            self.limiter.acquire().await;
            let response = self
                .auth(self.http.get(&url))
                .query(query)
                .send()
                .await
                .map_err(|source| Error::Network {
                    path: path.to_string(),
                    source,
                })?;
            let status = response.status();
            if status.is_success() {
                return response.json::<T>().await.map_err(|e| Error::Api {
                    status: status.as_u16(),
                    path: path.to_string(),
                    message: format!("invalid JSON response: {e}"),
                });
            }
            let retry_after = retry_after_secs(&response);
            let body = response.text().await.unwrap_or_default();
            let retryable = status.as_u16() == 429 || status.is_server_error();
            if retryable && attempt < self.max_retries {
                let base_delay = retry_after.unwrap_or_else(|| 0.5 * 2f64.powi(attempt as i32));
                sleep_with_jitter(base_delay).await;
                attempt += 1;
                continue;
            }
            return Err(api_error(status.as_u16(), path, &body, retry_after));
        }
    }

    async fn get_optional_json<T: serde::de::DeserializeOwned>(
        &self,
        base: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Option<T>> {
        let url = format!("{base}{path}");
        let mut attempt = 0u32;
        loop {
            self.limiter.acquire().await;
            let response = self
                .auth(self.http.get(&url))
                .query(query)
                .send()
                .await
                .map_err(|source| Error::Network {
                    path: path.to_string(),
                    source,
                })?;
            let status = response.status();
            if status.is_success() {
                return response
                    .json::<T>()
                    .await
                    .map(Some)
                    .map_err(|e| Error::Api {
                        status: status.as_u16(),
                        path: path.to_string(),
                        message: format!("invalid JSON response: {e}"),
                    });
            }
            if status.as_u16() == 404 {
                return Ok(None);
            }
            let retry_after = retry_after_secs(&response);
            let body = response.text().await.unwrap_or_default();
            let retryable = status.as_u16() == 429 || status.is_server_error();
            if retryable && attempt < self.max_retries {
                let base_delay = retry_after.unwrap_or_else(|| 0.5 * 2f64.powi(attempt as i32));
                sleep_with_jitter(base_delay).await;
                attempt += 1;
                continue;
            }
            return Err(api_error(status.as_u16(), path, &body, retry_after));
        }
    }

    async fn write_json<B: Serialize + ?Sized>(
        &self,
        method: reqwest::Method,
        base: &str,
        path: &str,
        body: Option<&B>,
    ) -> Result<serde_json::Value> {
        let url = format!("{base}{path}");
        self.limiter.acquire().await;
        let mut rb = self.auth(self.http.request(method, &url));
        if let Some(body) = body {
            rb = rb.json(body);
        }
        let response = rb.send().await.map_err(|source| Error::Network {
            path: path.to_string(),
            source,
        })?;
        let status = response.status();
        let retry_after = retry_after_secs(&response);
        let text = response.text().await.unwrap_or_default();
        if status.is_success() || status.as_u16() == 207 {
            if text.trim().is_empty() {
                return Ok(serde_json::Value::Null);
            }
            return serde_json::from_str(&text).map_err(|e| Error::Api {
                status: status.as_u16(),
                path: path.to_string(),
                message: format!("invalid JSON response: {e}"),
            });
        }
        Err(api_error(status.as_u16(), path, &text, retry_after))
    }

    async fn write_empty(&self, method: reqwest::Method, base: &str, path: &str) -> Result<()> {
        let _ = self.write_json(method, base, path, None::<&()>).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Rate limiting and retry helpers
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct RateLimiter {
    inner: Mutex<Bucket>,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    capacity: f64,
    refill_per_sec: f64,
    last: Instant,
}

impl RateLimiter {
    fn new(per_minute: u32) -> Self {
        let capacity = per_minute.max(1) as f64;
        Self {
            inner: Mutex::new(Bucket {
                tokens: capacity,
                capacity,
                refill_per_sec: capacity / 60.0,
                last: Instant::now(),
            }),
        }
    }

    async fn acquire(&self) {
        loop {
            let wait = {
                let mut bucket = self.inner.lock().await;
                let elapsed = bucket.last.elapsed().as_secs_f64();
                bucket.tokens =
                    (bucket.tokens + elapsed * bucket.refill_per_sec).min(bucket.capacity);
                bucket.last = Instant::now();
                if bucket.tokens >= 1.0 {
                    bucket.tokens -= 1.0;
                    return;
                }
                (1.0 - bucket.tokens) / bucket.refill_per_sec
            };
            tokio::time::sleep(Duration::from_secs_f64(wait.min(5.0))).await;
        }
    }
}

fn retry_after_secs(response: &reqwest::Response) -> Option<f64> {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
}

async fn sleep_with_jitter(base_secs: f64) {
    // Deterministic-enough jitter without a RNG dependency.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let jitter = (nanos % 250) as f64 / 1000.0;
    tokio::time::sleep(Duration::from_secs_f64(base_secs.clamp(0.1, 30.0) + jitter)).await;
}

fn api_error(status: u16, path: &str, body: &str, retry_after: Option<f64>) -> Error {
    if status == 429 {
        return Error::RateLimited {
            path: path.to_string(),
            retry_after_secs: retry_after.unwrap_or(60.0).ceil() as u64,
        };
    }
    let parsed: Option<ApiErrorBody> = serde_json::from_str(body).ok();
    let mut message = parsed
        .as_ref()
        .and_then(|p| p.message.clone())
        .unwrap_or_else(|| {
            let trimmed = body.trim();
            if trimmed.is_empty() {
                "empty response body".to_string()
            } else {
                trimmed.chars().take(300).collect()
            }
        });
    match status {
        401 => message.push_str(
            " (check ALPACA_API_KEY/ALPACA_SECRET_KEY and that paper vs live URLs match)",
        ),
        403 => {
            message.push_str(" (check the market-data feed; sip needs a subscription, iex is free)")
        }
        _ => {}
    }
    Error::Api {
        status,
        path: path.to_string(),
        message,
    }
}

fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| Error::Api {
            status: 200,
            path: "/v2/clock".to_string(),
            message: format!("invalid timestamp '{value}': {e}"),
        })
}

// ---------------------------------------------------------------------------
// Serde shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    message: Option<String>,
    #[allow(dead_code)]
    code: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct MultiBarsResponse {
    #[serde(default)]
    bars: Option<BTreeMap<String, Vec<RawBar>>>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawBar {
    t: String,
    o: f64,
    h: f64,
    l: f64,
    c: f64,
    v: f64,
    #[allow(dead_code)]
    #[serde(default)]
    n: Option<f64>,
    #[allow(dead_code)]
    #[serde(default)]
    vw: Option<f64>,
}

impl RawBar {
    fn into_bar(self, start: NaiveDate, end: NaiveDate) -> Option<Bar> {
        let date = DateTime::parse_from_rfc3339(&self.t).ok()?.date_naive();
        if date < start || date > end {
            return None;
        }
        let bar = Bar {
            date,
            open: self.o,
            high: self.h,
            low: self.l,
            close: self.c,
            volume: self.v,
        };
        if bar.is_finite() && bar.open > 0.0 && bar.high > 0.0 && bar.low > 0.0 && bar.close > 0.0 {
            Some(bar)
        } else {
            None
        }
    }
}

#[derive(Debug, Deserialize)]
struct MostActivesResponse {
    #[serde(default)]
    most_actives: Option<Vec<MostActive>>,
}

#[derive(Debug, Deserialize)]
struct MostActive {
    symbol: String,
}

#[derive(Debug, Deserialize)]
struct RawClock {
    is_open: bool,
    next_open: String,
    next_close: String,
    timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketClock {
    pub is_open: bool,
    pub next_open: DateTime<Utc>,
    pub next_close: DateTime<Utc>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct RawCalendarDay {
    date: String,
    open: String,
    close: String,
    #[serde(default)]
    settlement_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarDay {
    pub date: NaiveDate,
    pub open: String,
    pub close: String,
    pub settlement_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawAccount {
    #[serde(default)]
    id: String,
    #[serde(default)]
    account_number: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    currency: String,
    #[serde(default)]
    cash: String,
    #[serde(default)]
    buying_power: String,
    #[serde(default)]
    non_marginable_buying_power: Option<String>,
    #[serde(default)]
    equity: String,
    #[serde(default)]
    last_equity: String,
    #[serde(default)]
    long_market_value: Option<String>,
    #[serde(default)]
    portfolio_value: Option<String>,
    #[serde(default)]
    pattern_day_trader: Option<bool>,
    #[serde(default)]
    trading_blocked: Option<bool>,
    #[serde(default)]
    account_blocked: Option<bool>,
    #[serde(default)]
    daytrade_count: Option<i64>,
}

impl RawAccount {
    fn into_account(self) -> Account {
        Account {
            id: self.id,
            account_number: self.account_number,
            status: self.status,
            currency: self.currency,
            cash: dec(&self.cash),
            buying_power: dec(&self.buying_power),
            non_marginable_buying_power: self.non_marginable_buying_power.as_deref().map(dec),
            equity: dec(&self.equity),
            last_equity: dec(&self.last_equity),
            long_market_value: self.long_market_value.as_deref().map(dec),
            portfolio_value: self.portfolio_value.as_deref().map(dec),
            pattern_day_trader: self.pattern_day_trader.unwrap_or(false),
            trading_blocked: self.trading_blocked.unwrap_or(false),
            account_blocked: self.account_blocked.unwrap_or(false),
            daytrade_count: self.daytrade_count.unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub account_number: String,
    pub status: String,
    pub currency: String,
    pub cash: f64,
    pub buying_power: f64,
    pub non_marginable_buying_power: Option<f64>,
    pub equity: f64,
    pub last_equity: f64,
    pub long_market_value: Option<f64>,
    pub portfolio_value: Option<f64>,
    pub pattern_day_trader: bool,
    pub trading_blocked: bool,
    pub account_blocked: bool,
    pub daytrade_count: i64,
}

impl Account {
    pub fn available_cash(&self) -> f64 {
        self.non_marginable_buying_power
            .unwrap_or(self.buying_power)
            .max(0.0)
    }

    pub fn daily_pnl_pct(&self) -> f64 {
        if self.last_equity > 0.0 {
            (self.equity - self.last_equity) / self.last_equity * 100.0
        } else {
            0.0
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawPosition {
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    qty: String,
    #[serde(default)]
    qty_available: Option<String>,
    #[serde(default)]
    avg_entry_price: String,
    #[serde(default)]
    side: String,
    #[serde(default)]
    market_value: Option<String>,
    #[serde(default)]
    cost_basis: Option<String>,
    #[serde(default)]
    unrealized_pl: Option<String>,
    #[serde(default)]
    unrealized_plpc: Option<String>,
    #[serde(default)]
    current_price: Option<String>,
}

impl RawPosition {
    fn into_position(self) -> Position {
        Position {
            symbol: self.symbol,
            qty: dec(&self.qty),
            qty_available: self.qty_available.as_deref().map(dec),
            avg_entry_price: dec(&self.avg_entry_price),
            side: self.side,
            market_value: self.market_value.as_deref().map(dec),
            cost_basis: self.cost_basis.as_deref().map(dec),
            unrealized_pl: self.unrealized_pl.as_deref().map(dec),
            unrealized_plpc: self.unrealized_plpc.as_deref().map(dec),
            current_price: self.current_price.as_deref().map(dec),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub symbol: String,
    pub qty: f64,
    pub qty_available: Option<f64>,
    pub avg_entry_price: f64,
    pub side: String,
    pub market_value: Option<f64>,
    pub cost_basis: Option<f64>,
    pub unrealized_pl: Option<f64>,
    pub unrealized_plpc: Option<f64>,
    pub current_price: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct RawOrder {
    #[serde(default)]
    id: String,
    #[serde(default)]
    client_order_id: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    side: String,
    #[serde(default)]
    qty: Option<String>,
    #[serde(default)]
    filled_qty: Option<String>,
    #[serde(default)]
    filled_avg_price: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default, rename = "type")]
    order_type: String,
    #[serde(default)]
    limit_price: Option<String>,
    #[serde(default)]
    stop_price: Option<String>,
    #[serde(default)]
    submitted_at: Option<String>,
    #[serde(default)]
    filled_at: Option<String>,
    #[serde(default)]
    notional: Option<String>,
}

impl RawOrder {
    fn into_order(self) -> Order {
        Order {
            id: self.id,
            client_order_id: self.client_order_id,
            symbol: self.symbol,
            side: self.side,
            qty: self.qty.as_deref().map(dec),
            filled_qty: self.filled_qty.as_deref().map(dec).unwrap_or(0.0),
            filled_avg_price: self.filled_avg_price.as_deref().map(dec),
            status: self.status,
            order_type: self.order_type,
            limit_price: self.limit_price.as_deref().map(dec),
            stop_price: self.stop_price.as_deref().map(dec),
            submitted_at: self
                .submitted_at
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc)),
            filled_at: self
                .filled_at
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc)),
            notional: self.notional.as_deref().map(dec),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub client_order_id: String,
    pub symbol: String,
    pub side: String,
    pub qty: Option<f64>,
    pub filled_qty: f64,
    pub filled_avg_price: Option<f64>,
    pub status: String,
    pub order_type: String,
    pub limit_price: Option<f64>,
    pub stop_price: Option<f64>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub filled_at: Option<DateTime<Utc>>,
    pub notional: Option<f64>,
}

impl Order {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "filled" | "canceled" | "expired" | "rejected" | "done_for_day"
        )
    }

    pub fn is_filled(&self) -> bool {
        self.status == "filled"
    }

    pub fn effective_price(&self) -> Option<f64> {
        self.filled_avg_price.or(self.limit_price)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OrderRequest {
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qty: Option<String>,
    pub side: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub time_in_force: String,
    pub client_order_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extended_hours: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notional: Option<String>,
}

impl OrderRequest {
    pub fn market(symbol: &str, qty: f64, side: &str) -> Self {
        Self {
            symbol: symbol.to_string(),
            qty: Some(format_qty(qty)),
            side: side.to_string(),
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            client_order_id: AlpacaClient::new_client_order_id(symbol),
            limit_price: None,
            stop_price: None,
            extended_hours: None,
            notional: None,
        }
    }

    pub fn notional(symbol: &str, notional: f64, side: &str) -> Self {
        Self {
            symbol: symbol.to_string(),
            qty: None,
            side: side.to_string(),
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            client_order_id: AlpacaClient::new_client_order_id(symbol),
            limit_price: None,
            stop_price: None,
            extended_hours: None,
            notional: Some(format!("{notional:.2}")),
        }
    }

    pub fn with_type(mut self, order_type: &str) -> Self {
        self.order_type = order_type.to_string();
        self
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FlattenReport {
    pub orders_cancelled: bool,
    pub closed: Vec<String>,
    pub errors: Vec<String>,
}

fn dec(value: &str) -> f64 {
    value.trim().parse::<f64>().unwrap_or(0.0)
}

fn format_qty(qty: f64) -> String {
    if (qty - qty.round()).abs() < 1e-9 {
        format!("{:.0}", qty)
    } else {
        format!("{qty:.6}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AlpacaConfig;

    #[test]
    fn query_strings_are_encoded_and_ids_are_safe() {
        let id = AlpacaClient::new_client_order_id("BRK.B");
        assert!(id.starts_with("tpi-"));
        assert!(id.contains("BRK.B"));
        assert!(id.len() <= 48);
        assert_eq!(format_qty(10.0), "10");
        assert_eq!(format_qty(10.5), "10.500000");
    }

    #[test]
    fn rate_limiter_refills_tokens() {
        let limiter = RateLimiter::new(60);
        let bucket = limiter.inner.try_lock().unwrap();
        assert!(bucket.tokens >= 1.0);
    }

    #[test]
    fn account_conversion_handles_strings() {
        let cfg = AlpacaConfig::default();
        let client =
            AlpacaClient::with_urls(&cfg, "http://127.0.0.1:1", "k".into(), "s".into()).unwrap();
        assert_eq!(client.trading_url(), "http://127.0.0.1:1");
    }

    #[test]
    fn raw_account_parses() {
        let raw: RawAccount = serde_json::from_str(
            r#"{"id":"a","account_number":"1","status":"ACTIVE","currency":"USD",
                "cash":"1000.50","buying_power":"2000","non_marginable_buying_power":"900",
                "equity":"1001","last_equity":"990","pattern_day_trader":false,
                "trading_blocked":false,"account_blocked":false,"daytrade_count":0}"#,
        )
        .unwrap();
        let acc = raw.into_account();
        assert!((acc.cash - 1000.5).abs() < 1e-9);
        assert!((acc.available_cash() - 900.0).abs() < 1e-9);
        assert!(acc.daily_pnl_pct() > 1.0);
    }
}
