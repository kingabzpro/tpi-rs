use crate::error::{Error, Result};
use crate::model::{Horizon, HorizonParams, Mode, RiskProfile, StrategyMode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_UNIVERSE: &[&str] = &[
    "AAPL", "MSFT", "NVDA", "AMZN", "GOOGL", "META", "TSLA", "AVGO", "BRK.B", "JPM", "V", "MA",
    "UNH", "XOM", "JNJ", "WMT", "PG", "HD", "COST", "MRK", "ABBV", "CVX", "PEP", "KO", "ADBE",
    "CRM", "NFLX", "AMD", "INTC", "CSCO", "ORCL", "QCOM", "TXN", "IBM", "CAT", "BA", "GE", "DIS",
    "MCD", "NKE", "VZ", "T", "PFE", "LLY", "TMO", "ABT", "DHR", "WFC", "BAC", "GS",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub alpaca: AlpacaConfig,
    pub trading: TradingConfig,
    pub strategy: StrategySettings,
    pub backtest: BacktestSettings,
    pub cache: CacheSettings,
    pub paths: PathSettings,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AlpacaConfig {
    pub key_id: String,
    pub secret_key: String,
    pub data_url: String,
    pub paper_url: String,
    pub live_url: String,
    /// Alpaca market-data feed: iex (free), sip (subscription), delayed_sip, otc.
    pub feed: String,
    /// raw, split, dividend or all. `all` embeds corporate actions in prices.
    pub adjustment: String,
    pub rate_limit_per_min: u32,
    pub timeout_secs: u64,
    pub max_retries: u32,
}

impl Default for AlpacaConfig {
    fn default() -> Self {
        Self {
            key_id: String::new(),
            secret_key: String::new(),
            data_url: "https://data.alpaca.markets".to_string(),
            paper_url: "https://paper-api.alpaca.markets".to_string(),
            live_url: "https://api.alpaca.markets".to_string(),
            feed: "iex".to_string(),
            adjustment: "all".to_string(),
            rate_limit_per_min: 180,
            timeout_secs: 30,
            max_retries: 3,
        }
    }
}

impl AlpacaConfig {
    pub fn has_credentials(&self) -> bool {
        !self.key_id.trim().is_empty() && !self.secret_key.trim().is_empty()
    }

    pub fn trading_url(&self, paper: bool) -> &str {
        if paper {
            &self.paper_url
        } else {
            &self.live_url
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TradingConfig {
    /// Paper trading is the default and cannot be disabled by a flag alone.
    pub paper: bool,
    /// Live orders additionally require this setting, an environment flag and typed confirmation.
    pub allow_live: bool,
    pub max_order_notional_pct: f64,
    pub max_daily_loss_pct: f64,
    pub max_trades_per_day: u32,
    pub max_open_positions: usize,
    pub allow_fractional: bool,
    pub min_order_notional: f64,
    pub fill_timeout_secs: u64,
    pub poll_secs: u64,
    pub loop_interval_secs: u64,
    pub flatten_on_exit: bool,
    pub kill_switch_file: String,
    /// Maximum notional submitted by one live/paper order.
    pub max_single_order_notional: f64,
}

impl Default for TradingConfig {
    fn default() -> Self {
        Self {
            paper: true,
            allow_live: false,
            max_order_notional_pct: 25.0,
            max_daily_loss_pct: 3.0,
            max_trades_per_day: 4,
            max_open_positions: 1,
            allow_fractional: true,
            min_order_notional: 10.0,
            fill_timeout_secs: 30,
            poll_secs: 2,
            loop_interval_secs: 300,
            flatten_on_exit: false,
            kill_switch_file: "KILL".to_string(),
            max_single_order_notional: 10_000.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategySettings {
    pub short: HorizonParams,
    pub medium: HorizonParams,
    pub long: HorizonParams,
    /// Consensus mode requires at least a Buy rating.
    pub require_rating_buy: bool,
    /// Configurable risk-free rate used by Sharpe/Sortino (percent per year).
    pub risk_free_rate_pct: f64,
    /// Treat SELL as exit/flat only; the system does not short individual stocks.
    pub long_only: bool,
    /// Volume confirmation threshold versus the 20-day average.
    pub volume_ratio_min: f64,
    /// Minimum bars before a signal is considered valid.
    pub min_history_bars: usize,
}

impl Default for StrategySettings {
    fn default() -> Self {
        Self {
            short: Horizon::Short.params(),
            medium: Horizon::Medium.params(),
            long: Horizon::Long.params(),
            require_rating_buy: true,
            risk_free_rate_pct: 0.0,
            long_only: true,
            volume_ratio_min: 1.0,
            min_history_bars: 220,
        }
    }
}

impl StrategySettings {
    pub fn params(&self, horizon: Horizon) -> HorizonParams {
        match horizon {
            Horizon::Short => self.short,
            Horizon::Medium => self.medium,
            Horizon::Long => self.long,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BacktestSettings {
    pub initial_capital: f64,
    pub start: String,
    pub end: String,
    pub universe: Vec<String>,
    pub benchmark_symbols: Vec<String>,
    pub commission_bps: f64,
    pub spread_bps: f64,
    pub slippage_bps: f64,
    /// SEC Section 31 fee, sell side only (0.3 bps is a conservative round number).
    pub sec_fee_bps: f64,
    /// FINRA TAF per share on sells (USD).
    pub finra_taf_per_share: f64,
    pub warmup_trading_days: usize,
    pub allow_fractional: bool,
    pub stale_bar_days: i64,
    /// Max days a position may be held regardless of signals.
    pub max_hold_days_override: Option<usize>,
    pub walk_forward: WalkForwardSettings,
}

impl Default for BacktestSettings {
    fn default() -> Self {
        Self {
            initial_capital: 10_000.0,
            start: "2023-11-01".to_string(),
            end: "2026-11-01".to_string(),
            universe: DEFAULT_UNIVERSE.iter().map(|s| s.to_string()).collect(),
            benchmark_symbols: vec!["SPY".to_string(), "QQQ".to_string()],
            commission_bps: 0.0,
            spread_bps: 2.0,
            slippage_bps: 5.0,
            sec_fee_bps: 0.3,
            finra_taf_per_share: 0.000166,
            warmup_trading_days: 220,
            allow_fractional: true,
            stale_bar_days: 5,
            max_hold_days_override: None,
            walk_forward: WalkForwardSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WalkForwardSettings {
    pub train_months: u32,
    pub test_months: u32,
    pub min_trades_for_selection: usize,
    pub atr_mult_grid: Vec<f64>,
    pub rsi_max_grid: Vec<f64>,
    pub exit_period_grid: Vec<usize>,
    pub oos_months: u32,
}

impl Default for WalkForwardSettings {
    fn default() -> Self {
        Self {
            train_months: 12,
            test_months: 6,
            min_trades_for_selection: 2,
            atr_mult_grid: vec![2.0, 2.5, 3.0],
            rsi_max_grid: vec![70.0, 75.0, 80.0],
            exit_period_grid: vec![10, 20],
            oos_months: 6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheSettings {
    pub enabled: bool,
    pub path: String,
    /// Refetch a symbol's full requested range when its cache is older than this.
    pub ttl_hours: i64,
    /// Force refetch (same as --refresh).
    pub refresh: bool,
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            path: "data/tpi_cache.db".to_string(),
            ttl_hours: 12,
            refresh: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PathSettings {
    pub reports_dir: String,
    pub logs_dir: String,
}

impl Default for PathSettings {
    fn default() -> Self {
        Self {
            reports_dir: "reports".to_string(),
            logs_dir: "logs".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub mode: Option<Mode>,
    pub horizon: Option<Horizon>,
    pub strategy: Option<StrategyMode>,
    pub risk: Option<RiskProfile>,
    pub tickers: Vec<String>,
    pub capital: Option<f64>,
    pub universe: Option<Vec<String>>,
}

impl Config {
    /// Load configuration. Priority: explicit path, project `config/default.toml`,
    /// then `~/.tpi-rs/config.toml`. Environment variables override secrets and URLs.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let file = match path {
            Some(p) => {
                if !p.exists() {
                    return Err(Error::config(format!(
                        "config file not found: {}",
                        p.display()
                    )));
                }
                Some(p.to_path_buf())
            }
            None => default_config_candidates().into_iter().find(|p| p.exists()),
        };

        let mut cfg = if let Some(path) = file {
            let text = std::fs::read_to_string(&path).map_err(|e| Error::Io {
                path: path.clone(),
                source: e,
            })?;
            toml::from_str(&text)
                .map_err(|e| Error::config(format!("invalid TOML in {}: {e}", path.display())))?
        } else {
            Self::default()
        };
        cfg.apply_env();
        cfg.normalise();
        Ok(cfg)
    }

    pub fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("ALPACA_API_KEY") {
            self.alpaca.key_id = v;
        }
        if let Ok(v) = std::env::var("ALPACA_SECRET_KEY") {
            self.alpaca.secret_key = v;
        }
        if let Ok(v) = std::env::var("ALPACA_API_SECRET")
            && self.alpaca.secret_key.is_empty()
        {
            self.alpaca.secret_key = v;
        }
        if let Ok(v) = std::env::var("TPI_ALPACA_FEED") {
            self.alpaca.feed = v;
        }
        if let Ok(v) = std::env::var("TPI_DATA_URL") {
            self.alpaca.data_url = v;
        }
        if let Ok(v) = std::env::var("TPI_PAPER_URL") {
            self.alpaca.paper_url = v;
        }
        if let Ok(v) = std::env::var("TPI_LIVE_URL") {
            self.alpaca.live_url = v;
        }
        if let Ok(v) = std::env::var("TPI_ALLOW_LIVE") {
            self.trading.allow_live =
                matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = std::env::var("TPI_CACHE_PATH") {
            self.cache.path = v;
        }
        if let Ok(v) = std::env::var("TPI_REPORTS_DIR") {
            self.paths.reports_dir = v;
        }
        if let Ok(v) = std::env::var("TPI_CACHE_TTL_HOURS")
            && let Ok(n) = v.parse()
        {
            self.cache.ttl_hours = n;
        }
        if let Ok(v) = std::env::var("TPI_FEED") {
            self.alpaca.feed = v;
        }
    }

    pub fn normalise(&mut self) {
        self.alpaca.feed = self.alpaca.feed.trim().to_ascii_lowercase();
        self.alpaca.adjustment = self.alpaca.adjustment.trim().to_ascii_lowercase();
        self.backtest.universe = normalise_symbols(&self.backtest.universe);
        self.backtest.benchmark_symbols = normalise_symbols(&self.backtest.benchmark_symbols);
    }

    pub fn validate(&self) -> Result<()> {
        if !(1..=300).contains(&self.alpaca.rate_limit_per_min) {
            return Err(Error::config("alpaca.rate_limit_per_min must be 1..=300"));
        }
        if self.alpaca.timeout_secs == 0 {
            return Err(Error::config("alpaca.timeout_secs must be > 0"));
        }
        match self.alpaca.feed.as_str() {
            "iex" | "sip" | "delayed_sip" | "otc" | "boats" => {}
            other => {
                return Err(Error::config(format!(
                    "alpaca.feed '{other}' is invalid (use iex, sip, delayed_sip, otc or boats)"
                )));
            }
        }
        match self.alpaca.adjustment.as_str() {
            "raw" | "split" | "dividend" | "all" => {}
            other => {
                return Err(Error::config(format!(
                    "alpaca.adjustment '{other}' is invalid (raw, split, dividend, all)"
                )));
            }
        }
        for (name, p) in [
            ("strategy.short", self.strategy.short),
            ("strategy.medium", self.strategy.medium),
            ("strategy.long", self.strategy.long),
        ] {
            p.validate()
                .map_err(|e| Error::config(format!("{name}: {e}")))?;
        }
        if self.backtest.initial_capital <= 0.0 {
            return Err(Error::config("backtest.initial_capital must be > 0"));
        }
        if self.backtest.universe.is_empty() {
            return Err(Error::config("backtest.universe is empty"));
        }
        for fee in [
            self.backtest.commission_bps,
            self.backtest.spread_bps,
            self.backtest.slippage_bps,
            self.backtest.sec_fee_bps,
            self.backtest.finra_taf_per_share,
        ] {
            if fee < 0.0 || !fee.is_finite() {
                return Err(Error::config("fees and slippage must be finite and >= 0"));
            }
        }
        if self.backtest.warmup_trading_days < self.strategy.min_history_bars {
            return Err(Error::config(
                "backtest.warmup_trading_days must be >= strategy.min_history_bars",
            ));
        }
        if self.strategy.params(Horizon::Long).breakout_period > self.strategy.min_history_bars {
            return Err(Error::config(
                "strategy.min_history_bars must cover the longest channel",
            ));
        }
        if !self.strategy.long_only {
            return Err(Error::config(
                "strategy.long_only must remain true; short selling is not implemented",
            ));
        }
        if self.trading.max_open_positions == 0 {
            return Err(Error::config("trading.max_open_positions must be > 0"));
        }
        if !(0.0..=100.0).contains(&self.trading.max_daily_loss_pct) {
            return Err(Error::config("trading.max_daily_loss_pct must be 0..=100"));
        }
        Ok(())
    }

    pub fn horizon_params(&self, horizon: Horizon) -> HorizonParams {
        self.strategy.params(horizon)
    }

    pub fn report_dir(&self) -> PathBuf {
        PathBuf::from(&self.paths.reports_dir)
    }

    pub fn cache_path(&self) -> PathBuf {
        PathBuf::from(&self.cache.path)
    }

    pub fn ensure_directories(&self) -> Result<()> {
        for dir in [self.report_dir(), PathBuf::from(&self.paths.logs_dir)] {
            std::fs::create_dir_all(&dir).map_err(|source| Error::Io {
                path: dir.clone(),
                source,
            })?;
        }
        if self.cache.enabled
            && let Some(parent) = self.cache_path().parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        Ok(())
    }

    pub fn require_credentials(&self) -> Result<()> {
        if self.alpaca.has_credentials() {
            Ok(())
        } else {
            Err(Error::MissingCredentials)
        }
    }
}

fn normalise_symbols(symbols: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(symbols.len());
    for s in symbols {
        let sym = s.trim().to_ascii_uppercase();
        if !sym.is_empty() && !out.contains(&sym) {
            out.push(sym);
        }
    }
    out
}

fn default_config_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from("config/default.toml")];
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("config").join("default.toml"));
        if let Some(root) = dir.parent().and_then(|p| p.parent()) {
            candidates.push(root.join("config").join("default.toml"));
        }
    }
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        candidates.push(PathBuf::from(home).join(".tpi-rs").join("config.toml"));
    }
    candidates
}

/// Load `.env` from the current directory, then from the executable directory.
/// Missing files are ignored; explicit environment variables always win.
pub fn load_dotenv() {
    let _ = dotenvy::dotenv();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let _ = dotenvy::from_path(dir.join(".env"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        let cfg = Config::default();
        cfg.validate().unwrap();
        assert_eq!(cfg.backtest.initial_capital, 10_000.0);
        assert!(cfg.trading.paper);
    }

    #[test]
    fn invalid_feed_is_rejected() {
        let mut cfg = Config::default();
        cfg.alpaca.feed = "nasdaq".into();
        assert!(cfg.validate().is_err());
    }
}
