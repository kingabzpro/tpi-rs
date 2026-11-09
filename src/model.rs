use chrono::{DateTime, NaiveDate, Utc};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A daily OHLCV candle. Prices are split/dividend adjusted when loaded from Alpaca
/// with `adjustment=all`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    pub date: NaiveDate,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

impl Bar {
    pub fn is_finite(&self) -> bool {
        self.open.is_finite()
            && self.high.is_finite()
            && self.low.is_finite()
            && self.close.is_finite()
            && self.volume.is_finite()
    }

    pub fn typical_price(&self) -> f64 {
        (self.high + self.low + self.close) / 3.0
    }

    pub fn median_price(&self) -> f64 {
        (self.high + self.low) / 2.0
    }
}

impl ta::High for Bar {
    fn high(&self) -> f64 {
        self.high
    }
}
impl ta::Low for Bar {
    fn low(&self) -> f64 {
        self.low
    }
}
impl ta::Close for Bar {
    fn close(&self) -> f64 {
        self.close
    }
}
impl ta::Open for Bar {
    fn open(&self) -> f64 {
        self.open
    }
}
impl ta::Volume for Bar {
    fn volume(&self) -> f64 {
        self.volume
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum Horizon {
    /// Short-term setup, normally 1 to 5 days, with a trend-following exit.
    Short,
    /// 1 to 4 weeks.
    Medium,
    /// 1 to 6 months.
    Long,
}

impl Horizon {
    pub fn label(self) -> &'static str {
        match self {
            Self::Short => "Short-term (1-5 day setup, trend exit)",
            Self::Medium => "Medium (1-4 weeks)",
            Self::Long => "Long (1-6 months)",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Medium => "medium",
            Self::Long => "long",
        }
    }

    pub fn params(self) -> HorizonParams {
        match self {
            Self::Short => HorizonParams {
                breakout_period: 20,
                confirm_period: 40,
                exit_period: 10,
                atr_stop_mult: 2.0,
                max_hold_days: 252,
                rsi_min: 50.0,
                rsi_max: 78.0,
                buy_threshold: 62.0,
                min_atr_pct: 0.8,
                max_atr_pct: 7.0,
            },
            Self::Medium => HorizonParams {
                breakout_period: 20,
                confirm_period: 40,
                exit_period: 20,
                atr_stop_mult: 2.5,
                max_hold_days: 20,
                rsi_min: 50.0,
                rsi_max: 75.0,
                buy_threshold: 68.0,
                min_atr_pct: 0.6,
                max_atr_pct: 8.0,
            },
            Self::Long => HorizonParams {
                breakout_period: 40,
                confirm_period: 40,
                exit_period: 20,
                atr_stop_mult: 3.5,
                max_hold_days: 126,
                rsi_min: 50.0,
                rsi_max: 75.0,
                buy_threshold: 72.0,
                min_atr_pct: 0.5,
                max_atr_pct: 9.0,
            },
        }
    }
}

impl fmt::Display for Horizon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.short_label())
    }
}

/// Tunable horizon parameters. Defaults implement the TPI-inspired rules.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HorizonParams {
    /// Donchian breakout trigger (prior N highs, current bar excluded).
    pub breakout_period: usize,
    /// Longer Donchian channel used for trend confirmation.
    pub confirm_period: usize,
    /// Prior N-day low used for channel exits.
    pub exit_period: usize,
    /// ATR multiple for the initial and trailing stop.
    pub atr_stop_mult: f64,
    pub max_hold_days: usize,
    pub rsi_min: f64,
    pub rsi_max: f64,
    /// Minimum composite score required for a BUY.
    pub buy_threshold: f64,
    pub min_atr_pct: f64,
    pub max_atr_pct: f64,
}

impl Default for HorizonParams {
    fn default() -> Self {
        Horizon::Medium.params()
    }
}

impl HorizonParams {
    pub fn validate(&self) -> crate::Result<()> {
        if self.breakout_period < 2 || self.confirm_period < self.breakout_period {
            return Err(crate::Error::config(
                "confirm_period must be >= breakout_period >= 2",
            ));
        }
        if self.exit_period == 0 || self.max_hold_days == 0 {
            return Err(crate::Error::config(
                "exit_period and max_hold_days must be positive",
            ));
        }
        if !(0.0 < self.atr_stop_mult && self.atr_stop_mult < 20.0) {
            return Err(crate::Error::config("atr_stop_mult must be in (0, 20)"));
        }
        if !(0.0..=100.0).contains(&self.rsi_min) || !(0.0..=100.0).contains(&self.rsi_max) {
            return Err(crate::Error::config("RSI bounds must be within 0..=100"));
        }
        if self.rsi_min >= self.rsi_max {
            return Err(crate::Error::config("rsi_min must be below rsi_max"));
        }
        if !(0.0..=100.0).contains(&self.buy_threshold) {
            return Err(crate::Error::config("buy_threshold must be within 0..=100"));
        }
        if self.min_atr_pct >= self.max_atr_pct {
            return Err(crate::Error::config(
                "min_atr_pct must be below max_atr_pct",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Analyze,
    Scan,
    Backtest,
    Benchmark,
    Paper,
    Live,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Analyze => "Analyze",
            Self::Scan => "Scan",
            Self::Backtest => "Backtest",
            Self::Benchmark => "3-Year Benchmark",
            Self::Paper => "Paper Trading",
            Self::Live => "Live Trading",
        }
    }

    pub fn needs_credentials(self) -> bool {
        matches!(
            self,
            Self::Analyze
                | Self::Scan
                | Self::Backtest
                | Self::Benchmark
                | Self::Paper
                | Self::Live
        )
    }

    pub fn is_trading(self) -> bool {
        matches!(self, Self::Paper | Self::Live)
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum StrategyMode {
    /// Pure TPI trend/breakout rules.
    Tpi,
    /// TPI plus a five-level technical rating gate.
    Consensus,
    /// Run both and compare (reports, scans and backtests).
    Compare,
}

impl StrategyMode {
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Tpi => "TPI",
            Self::Consensus => "CONS",
            Self::Compare => "CMP",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tpi => "TPI",
            Self::Consensus => "TPI + Technical Consensus",
            Self::Compare => "TPI vs Consensus (compare)",
        }
    }
}

impl fmt::Display for StrategyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum RiskProfile {
    Conservative,
    Balanced,
    Aggressive,
}

impl RiskProfile {
    pub fn label(self) -> &'static str {
        match self {
            Self::Conservative => "Conservative",
            Self::Balanced => "Balanced",
            Self::Aggressive => "Aggressive",
        }
    }

    pub fn params(self) -> RiskParams {
        match self {
            Self::Conservative => RiskParams {
                risk_per_trade_pct: 0.5,
                max_position_pct: 50.0,
                vol_target_pct: 0.0,
            },
            Self::Balanced => RiskParams {
                risk_per_trade_pct: 1.0,
                max_position_pct: 75.0,
                vol_target_pct: 0.0,
            },
            Self::Aggressive => RiskParams {
                risk_per_trade_pct: 1.5,
                max_position_pct: 100.0,
                vol_target_pct: 0.0,
            },
        }
    }
}

impl fmt::Display for RiskProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RiskParams {
    /// Equity percentage risked between entry and initial stop.
    pub risk_per_trade_pct: f64,
    /// Hard cap on a single position as a percentage of equity.
    pub max_position_pct: f64,
    /// Annualized position volatility target, percent. Zero keeps risk-budget sizing.
    #[serde(default)]
    pub vol_target_pct: f64,
}

impl RiskParams {
    /// CLI overrides. A volatility target replaces the dollar risk budget with an
    /// inverse-volatility notional, so per-trade loss then scales with stop distance.
    pub fn with_overrides(mut self, risk_pct: Option<f64>, vol_target_pct: f64) -> Self {
        if let Some(pct) = risk_pct.filter(|pct| pct.is_finite() && *pct > 0.0) {
            self.risk_per_trade_pct = pct;
        }
        self.vol_target_pct = if vol_target_pct.is_finite() {
            vol_target_pct.max(0.0)
        } else {
            0.0
        };
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Buy,
    Hold,
    Sell,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Hold => "HOLD",
            Self::Sell => "SELL",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// TradingView-style five-level technical rating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rating {
    StrongSell,
    Sell,
    Neutral,
    Buy,
    StrongBuy,
}

impl Rating {
    pub fn from_score(score: f64) -> Self {
        if score >= 0.5 {
            Self::StrongBuy
        } else if score >= 0.1 {
            Self::Buy
        } else if score <= -0.5 {
            Self::StrongSell
        } else if score <= -0.1 {
            Self::Sell
        } else {
            Self::Neutral
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::StrongSell => "Strong Sell",
            Self::Sell => "Sell",
            Self::Neutral => "Neutral",
            Self::Buy => "Buy",
            Self::StrongBuy => "Strong Buy",
        }
    }

    pub fn rank(self) -> i8 {
        match self {
            Self::StrongSell => -2,
            Self::Sell => -1,
            Self::Neutral => 0,
            Self::Buy => 1,
            Self::StrongBuy => 2,
        }
    }

    pub fn is_buy(self) -> bool {
        self.rank() >= 1
    }

    pub fn is_sell(self) -> bool {
        self.rank() <= -1
    }
}

impl fmt::Display for Rating {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vote {
    Buy,
    Neutral,
    Sell,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatingDetail {
    pub rating: Rating,
    /// (buy - sell) / total votes, in [-1, 1].
    pub score: f64,
    pub buy_votes: usize,
    pub neutral_votes: usize,
    pub sell_votes: usize,
    pub ma_buy: usize,
    pub ma_neutral: usize,
    pub ma_sell: usize,
    pub osc_buy: usize,
    pub osc_neutral: usize,
    pub osc_sell: usize,
    pub votes: Vec<RatingVote>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatingVote {
    pub name: String,
    pub vote: Vote,
    pub value: Option<f64>,
}

impl RatingDetail {
    pub fn empty() -> Self {
        Self {
            rating: Rating::Neutral,
            score: 0.0,
            buy_votes: 0,
            neutral_votes: 0,
            sell_votes: 0,
            ma_buy: 0,
            ma_neutral: 0,
            ma_sell: 0,
            osc_buy: 0,
            osc_neutral: 0,
            osc_sell: 0,
            votes: Vec::new(),
        }
    }

    pub fn from_votes(votes: Vec<RatingVote>) -> Self {
        let is_ma = |name: &str| {
            name.starts_with("EMA")
                || name.starts_with("SMA")
                || name.starts_with("Ichimoku")
                || name.starts_with("VWMA")
                || name.starts_with("HMA")
        };
        let ma_votes: Vec<&RatingVote> = votes.iter().filter(|v| is_ma(&v.name)).collect();
        let osc_votes: Vec<&RatingVote> = votes.iter().filter(|v| !is_ma(&v.name)).collect();

        let count = |set: &[&RatingVote], v| set.iter().filter(|x| x.vote == v).count();
        let buy = count(&ma_votes, Vote::Buy) + count(&osc_votes, Vote::Buy);
        let sell = count(&ma_votes, Vote::Sell) + count(&osc_votes, Vote::Sell);
        let neutral = votes.len().saturating_sub(buy + sell);
        let score = if votes.is_empty() {
            0.0
        } else {
            (buy as f64 - sell as f64) / votes.len() as f64
        };
        Self {
            rating: Rating::from_score(score),
            score,
            buy_votes: buy,
            neutral_votes: neutral,
            sell_votes: sell,
            ma_buy: count(&ma_votes, Vote::Buy),
            ma_neutral: count(&ma_votes, Vote::Neutral),
            ma_sell: count(&ma_votes, Vote::Sell),
            osc_buy: count(&osc_votes, Vote::Buy),
            osc_neutral: count(&osc_votes, Vote::Neutral),
            osc_sell: count(&osc_votes, Vote::Sell),
            votes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndicatorSnapshot {
    pub close: f64,
    pub ema20: Option<f64>,
    pub ema50: Option<f64>,
    pub ema200: Option<f64>,
    pub ema50_slope_pct: Option<f64>,
    pub rsi14: Option<f64>,
    pub atr14: Option<f64>,
    pub atr_pct: Option<f64>,
    pub adx14: Option<f64>,
    pub plus_di: Option<f64>,
    pub minus_di: Option<f64>,
    pub donchian20_high: Option<f64>,
    pub donchian20_low: Option<f64>,
    pub donchian20_mid: Option<f64>,
    pub donchian40_high: Option<f64>,
    pub donchian40_low: Option<f64>,
    pub macd: Option<f64>,
    pub macd_signal: Option<f64>,
    pub macd_hist: Option<f64>,
    pub stoch_k: Option<f64>,
    pub cci20: Option<f64>,
    pub volume_sma20: Option<f64>,
    pub volume_ratio: Option<f64>,
    pub dist_ema20_atr: Option<f64>,
}

impl IndicatorSnapshot {
    pub fn empty(close: f64) -> Self {
        Self {
            close,
            ema20: None,
            ema50: None,
            ema200: None,
            ema50_slope_pct: None,
            rsi14: None,
            atr14: None,
            atr_pct: None,
            adx14: None,
            plus_di: None,
            minus_di: None,
            donchian20_high: None,
            donchian20_low: None,
            donchian20_mid: None,
            donchian40_high: None,
            donchian40_low: None,
            macd: None,
            macd_signal: None,
            macd_hist: None,
            stoch_k: None,
            cci20: None,
            volume_sma20: None,
            volume_ratio: None,
            dist_ema20_atr: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub trend: f64,
    pub breakout: f64,
    pub momentum: f64,
    pub confirmation: f64,
    pub volatility: f64,
}

impl ScoreBreakdown {
    pub const TREND_MAX: f64 = 35.0;
    pub const BREAKOUT_MAX: f64 = 25.0;
    pub const MOMENTUM_MAX: f64 = 20.0;
    pub const CONFIRMATION_MAX: f64 = 10.0;
    pub const VOLATILITY_MAX: f64 = 10.0;

    pub fn total(&self) -> f64 {
        (self.trend + self.breakout + self.momentum + self.confirmation + self.volatility)
            .clamp(0.0, 100.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub symbol: String,
    pub as_of: NaiveDate,
    pub horizon: Horizon,
    pub strategy: StrategyMode,
    pub action: Action,
    /// Composite TPI score in 0..=100.
    pub strength: f64,
    pub rating: Rating,
    pub rating_detail: RatingDetail,
    pub price: f64,
    pub indicators: IndicatorSnapshot,
    pub components: ScoreBreakdown,
    /// Indicated entry price (last close; executions happen at the next session open).
    pub entry: f64,
    pub stop: f64,
    pub target: f64,
    pub stop_distance_pct: f64,
    pub risk_per_share: f64,
    pub risk_amount: f64,
    pub suggested_shares: f64,
    pub suggested_notional: f64,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub stale: bool,
}

impl Signal {
    pub fn summary_line(&self) -> String {
        format!(
            "{} {} strength={:.0} rating={} close={:.2} stop={:.2}",
            self.symbol, self.action, self.strength, self.rating, self.price, self.stop
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataSource {
    Alpaca,
    Cache,
    Synthetic,
    Mixed,
}

impl DataSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Alpaca => "Alpaca",
            Self::Cache => "cache",
            Self::Synthetic => "SYNTHETIC DEMO",
            Self::Mixed => "Alpaca+cache",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataProvenance {
    pub source: DataSource,
    pub feed: String,
    pub adjusted: bool,
    pub fetched_at: DateTime<Utc>,
    pub note: String,
}

impl DataProvenance {
    pub fn synthetic() -> Self {
        Self {
            source: DataSource::Synthetic,
            feed: "synthetic".to_string(),
            adjusted: false,
            fetched_at: Utc::now(),
            note:
                "Deterministic generated series. NOT market data; for offline demo and tests only."
                    .to_string(),
        }
    }

    pub fn alpaca(feed: &str, adjusted: bool) -> Self {
        Self {
            source: DataSource::Alpaca,
            feed: feed.to_string(),
            adjusted,
            fetched_at: Utc::now(),
            note: if adjusted {
                "Alpaca daily bars, adjustment=all (splits and dividends embedded)".to_string()
            } else {
                "Alpaca daily bars, raw prices".to_string()
            },
        }
    }
}
