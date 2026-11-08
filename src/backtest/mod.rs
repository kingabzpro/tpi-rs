//! Portfolio backtesting, metrics, six-month benchmarks and walk-forward analysis.

pub mod benchmark;
pub mod engine;
pub mod metrics;
pub mod types;

pub use benchmark::{BenchmarkReport, run_benchmark, tune_grid};
pub use engine::{PreparedData, buy_hold_curve, cash_curve, equal_weight_curve};
pub use metrics::{
    compute_metrics, compute_windows, daily_returns, relative_metrics, six_month_windows,
};
pub use types::{
    BacktestConfig, BacktestResult, ComparisonResult, DailyDecision, EquityPoint, FeeModel,
    FoldResult, Metrics, OosResult, RelativeMetrics, StabilityRow, Trade, TuneParams,
    WalkForwardResult, WindowResult,
};
