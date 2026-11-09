use crate::model::{Action, Horizon, HorizonParams, Rating, RiskProfile};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FeeModel {
    pub commission_bps: f64,
    pub spread_bps: f64,
    pub slippage_bps: f64,
    /// SEC Section 31 fee, sell side.
    pub sec_fee_bps: f64,
    /// FINRA TAF per share, sell side.
    pub finra_taf_per_share: f64,
}

impl FeeModel {
    pub fn buy_fill(&self, open: f64) -> f64 {
        open * (1.0 + (self.slippage_bps + self.spread_bps / 2.0) / 10_000.0)
    }

    pub fn sell_fill(&self, price: f64) -> f64 {
        (price * (1.0 - (self.slippage_bps + self.spread_bps / 2.0) / 10_000.0)).max(0.0)
    }

    pub fn buy_commission(&self, notional: f64) -> f64 {
        notional * self.commission_bps / 10_000.0
    }

    pub fn sell_fees(&self, notional: f64, shares: f64) -> f64 {
        notional * self.commission_bps / 10_000.0
            + notional * self.sec_fee_bps / 10_000.0
            + shares * self.finra_taf_per_share
    }

    /// Implicit half-spread + slippage cost, reported separately from commissions.
    pub fn implicit_cost(&self, notional: f64) -> f64 {
        notional * (self.slippage_bps + self.spread_bps / 2.0) / 10_000.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestConfig {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub initial_capital: f64,
    pub horizon: Horizon,
    pub strategy: crate::model::StrategyMode,
    pub risk: RiskProfile,
    /// Annualized volatility target for sizing, percent. Zero keeps risk-budget sizing.
    #[serde(default)]
    pub vol_target_pct: f64,
    /// Override for the equity percentage risked per trade.
    #[serde(default)]
    pub risk_pct_override: Option<f64>,
    pub fees: FeeModel,
    pub allow_fractional: bool,
    pub risk_free_rate_pct: f64,
    pub stale_bar_days: i64,
    pub min_history_bars: usize,
    pub max_hold_days_override: Option<usize>,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquityPoint {
    pub date: NaiveDate,
    pub equity: f64,
    pub cash: f64,
    pub position_value: f64,
    pub drawdown_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub symbol: String,
    pub entry_date: NaiveDate,
    pub exit_date: NaiveDate,
    pub entry_price: f64,
    pub exit_price: f64,
    pub shares: f64,
    pub entry_notional: f64,
    pub gross_pnl: f64,
    pub fees: f64,
    pub implicit_costs: f64,
    pub net_pnl: f64,
    pub net_return_pct: f64,
    pub hold_days: usize,
    pub entry_stop: f64,
    pub exit_reason: String,
    pub entry_score: f64,
    pub entry_rating: Rating,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyDecision {
    pub date: NaiveDate,
    pub action: Action,
    pub symbol: Option<String>,
    pub score: Option<f64>,
    pub rating: Option<Rating>,
    pub candidates: usize,
    pub equity: f64,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowResult {
    pub label: String,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub return_pct: f64,
    pub max_drawdown_pct: f64,
    pub trades: usize,
    pub win_rate_pct: f64,
    pub final_equity: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub initial_equity: f64,
    pub final_equity: f64,
    pub total_return_pct: f64,
    pub cagr_pct: f64,
    pub annual_volatility_pct: f64,
    pub sharpe: f64,
    pub sortino: f64,
    pub calmar: f64,
    pub max_drawdown_pct: f64,
    pub best_day_pct: f64,
    pub worst_day_pct: f64,
    pub exposure_pct: f64,
    pub trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub win_rate_pct: f64,
    pub profit_factor: f64,
    pub avg_win_pct: f64,
    pub avg_loss_pct: f64,
    pub avg_trade_pct: f64,
    pub total_fees: f64,
    pub period_days: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonResult {
    pub label: String,
    pub equity: Vec<EquityPoint>,
    pub metrics: Metrics,
    pub windows: Vec<WindowResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelativeMetrics {
    pub benchmark: String,
    pub alpha_annual_pct: f64,
    pub beta: f64,
    pub correlation: f64,
    pub tracking_error_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestResult {
    pub label: String,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub initial_capital: f64,
    pub final_equity: f64,
    pub horizon: Horizon,
    pub strategy: crate::model::StrategyMode,
    pub risk: RiskProfile,
    pub params: HorizonParams,
    pub fees: FeeModel,
    pub metrics: Metrics,
    pub windows: Vec<WindowResult>,
    pub equity: Vec<EquityPoint>,
    pub trades: Vec<Trade>,
    pub decisions: Vec<DailyDecision>,
    pub comparisons: Vec<ComparisonResult>,
    pub relative: Vec<RelativeMetrics>,
    pub warnings: Vec<String>,
    pub data_note: String,
    pub universe_size: usize,
    pub trading_days: usize,
    pub elapsed_ms: u128,
}

impl BacktestResult {
    pub fn summary(&self) -> String {
        format!(
            "{}: ${:.2} -> ${:.2} ({:+.2}%), CAGR {:+.2}%, Sharpe {:.2}, max DD {:.2}%, {} trades",
            self.label,
            self.initial_capital,
            self.metrics.final_equity,
            self.metrics.total_return_pct,
            self.metrics.cagr_pct,
            self.metrics.sharpe,
            self.metrics.max_drawdown_pct,
            self.metrics.trades
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TuneParams {
    pub atr_mult: f64,
    pub rsi_max: f64,
    pub exit_period: usize,
}

impl TuneParams {
    pub fn apply(&self, mut base: HorizonParams) -> HorizonParams {
        base.atr_stop_mult = self.atr_mult;
        base.rsi_max = self.rsi_max;
        base.exit_period = self.exit_period;
        base
    }
}

impl std::fmt::Display for TuneParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "atr={:.1} rsi_max={:.0} exit={}",
            self.atr_mult, self.rsi_max, self.exit_period
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoldResult {
    pub train_start: NaiveDate,
    pub train_end: NaiveDate,
    pub test_start: NaiveDate,
    pub test_end: NaiveDate,
    pub selected: TuneParams,
    pub in_sample: Metrics,
    pub out_of_sample: Metrics,
    pub selected_trades: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OosResult {
    pub train_start: NaiveDate,
    pub train_end: NaiveDate,
    pub test_start: NaiveDate,
    pub test_end: NaiveDate,
    pub selected: TuneParams,
    pub in_sample: Metrics,
    pub out_of_sample: Metrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StabilityRow {
    pub params: TuneParams,
    pub window_returns_pct: Vec<f64>,
    pub mean_return_pct: f64,
    pub stdev_return_pct: f64,
    pub worst_window_pct: f64,
    pub best_window_pct: f64,
    pub positive_windows: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalkForwardResult {
    pub folds: Vec<FoldResult>,
    pub grid_size: usize,
    pub oos: Option<OosResult>,
    pub stability: Vec<StabilityRow>,
    pub warnings: Vec<String>,
}
