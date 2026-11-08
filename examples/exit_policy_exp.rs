//! Exit-policy experiment. Run with:
//!
//! cargo run --release --example exit_policy_exp
//! cargo run --release --example exit_policy_exp -- --offline

use chrono::{Duration, NaiveDate};
use serde_json::json;
use tpi_rs::backtest::BacktestResult;
use tpi_rs::backtest::engine::{self, PreparedData};
use tpi_rs::backtest::types::{BacktestConfig, FeeModel};
use tpi_rs::config::{self, Config};
use tpi_rs::data::MarketData;
use tpi_rs::model::{Horizon, RiskProfile, StrategyMode};
use tpi_rs::report;

#[tokio::main]
async fn main() -> tpi_rs::Result<()> {
    config::load_dotenv();
    let cfg = Config::load(None)?;
    cfg.validate()?;
    cfg.ensure_directories()?;
    let offline = std::env::args().any(|arg| arg == "--offline");
    let market = MarketData::new(&cfg, offline, false)?;

    let start = NaiveDate::parse_from_str(&cfg.backtest.start, "%Y-%m-%d")
        .map_err(|e| tpi_rs::Error::validation(format!("invalid start date: {e}")))?;
    let end = NaiveDate::parse_from_str(&cfg.backtest.end, "%Y-%m-%d")
        .map_err(|e| tpi_rs::Error::validation(format!("invalid end date: {e}")))?;
    let universe = cfg.backtest.universe.clone();
    let benchmarks = cfg.backtest.benchmark_symbols.clone();
    let mut symbols = universe.clone();
    for benchmark in &benchmarks {
        if !symbols.contains(benchmark) {
            symbols.push(benchmark.clone());
        }
    }
    let warm_days = ((cfg.backtest.warmup_trading_days.max(220) as i64) * 2).max(450);
    let load = market
        .load(&symbols, start - Duration::days(warm_days), end)
        .await?;
    for warning in &load.warnings {
        println!("NOTE: {warning}");
    }
    let data = PreparedData::new(&load.bars, &universe, &benchmarks)?;
    println!(
        "{} sessions, {} symbols, {} (offline={offline})",
        data.trading_days.len(),
        data.universe.len(),
        load.provenance.note
    );

    let mut results_json = Vec::new();
    for horizon in [Horizon::Short, Horizon::Medium, Horizon::Long] {
        let base = cfg.horizon_params(horizon);
        let mut arms = vec![
            ("baseline", base.max_hold_days),
            ("doubled", base.max_hold_days.saturating_mul(2)),
            ("no time stop", 10_000),
        ];
        if horizon == Horizon::Short {
            arms.push(("cap 252", 252));
        }
        println!(
            "\n=== {} (baseline max hold {}) ===",
            horizon.short_label(),
            base.max_hold_days
        );
        let mut baseline_result: Option<BacktestResult> = None;
        for (label, hold) in arms {
            let mut params = base;
            params.max_hold_days = hold;
            let config = backtest_config(&cfg, horizon, start, end);
            let result = engine::run(&data, &config, params)?;
            let max_hold_note = result
                .decisions
                .iter()
                .filter(|decision| decision.note.contains("max hold"))
                .count();
            print_row(label, &result, &baseline_result, max_hold_note);
            if baseline_result.is_none() {
                baseline_result = Some(result.clone());
            }
            results_json.push(json!({
                "horizon": horizon.short_label(),
                "arm": label,
                "max_hold_days": hold,
                "metrics": &result.metrics,
                "trades": result.trades.len(),
                "avg_hold_days": avg_hold(&result),
                "avg_invested_pct": avg_invested(&result),
                "max_hold_signals": max_hold_note,
                "top_trade_share_pct": top_trade_share(&result),
                "windows": &result.windows,
            }));
        }
    }

    let split = NaiveDate::from_ymd_opt(2025, 5, 1).unwrap();
    let mut split_json = Vec::new();
    println!("\n=== medium split validation ===");
    let base = cfg.horizon_params(Horizon::Medium);
    for (label, hold) in [
        ("baseline", base.max_hold_days),
        ("doubled", base.max_hold_days * 2),
        ("no time stop", 10_000),
    ] {
        for (period, p_start, p_end) in [("first half", start, split), ("second half", split, end)]
        {
            let mut params = base;
            params.max_hold_days = hold;
            let config = backtest_config(&cfg, Horizon::Medium, p_start, p_end);
            let result = engine::run(&data, &config, params)?;
            let m = &result.metrics;
            println!(
                "{label:<14} {period:<12} return {:>7.2}%  Sharpe {:>5.2}  maxDD {:>5.2}%  trades {:>3}  avg hold {:>5.1}",
                m.total_return_pct,
                m.sharpe,
                m.max_drawdown_pct,
                m.trades,
                avg_hold(&result)
            );
            split_json.push(json!({
                "arm": label,
                "period": period,
                "start": p_start.to_string(),
                "end": p_end.to_string(),
                "metrics": &m,
                "trades": result.trades.len(),
                "avg_hold_days": avg_hold(&result),
            }));
        }
    }

    println!(
        "
=== short split validation ==="
    );
    let base = cfg.horizon_params(Horizon::Short);
    for (label, hold) in [
        ("baseline", base.max_hold_days),
        ("doubled", base.max_hold_days * 2),
        ("no time stop", 10_000),
    ] {
        for (period, p_start, p_end) in [("first half", start, split), ("second half", split, end)]
        {
            let mut params = base;
            params.max_hold_days = hold;
            let config = backtest_config(&cfg, Horizon::Short, p_start, p_end);
            let result = engine::run(&data, &config, params)?;
            let m = &result.metrics;
            println!(
                "{label:<14} {period:<12} return {:>7.2}%  Sharpe {:>5.2}  maxDD {:>5.2}%  trades {:>3}  avg hold {:>5.1}",
                m.total_return_pct,
                m.sharpe,
                m.max_drawdown_pct,
                m.trades,
                avg_hold(&result)
            );
            split_json.push(json!({
                "horizon": "short",
                "arm": label,
                "period": period,
                "start": p_start.to_string(),
                "end": p_end.to_string(),
                "metrics": &m,
                "trades": result.trades.len(),
                "avg_hold_days": avg_hold(&result),
            }));
        }
    }

    let path = cfg.report_dir().join("exit-policy-experiment.json");
    report::write_json(
        &path,
        &json!({
            "generated_at": chrono::Utc::now().to_rfc3339(),
            "start": start.to_string(),
            "end": end.to_string(),
            "offline": offline,
            "results": results_json,
            "split_validation": split_json,
        }),
    )?;
    println!("\nWrote {}", path.display());
    Ok(())
}

fn backtest_config(
    cfg: &Config,
    horizon: Horizon,
    start: NaiveDate,
    end: NaiveDate,
) -> BacktestConfig {
    BacktestConfig {
        start,
        end,
        initial_capital: cfg.backtest.initial_capital,
        horizon,
        strategy: StrategyMode::Tpi,
        risk: RiskProfile::Balanced,
        fees: FeeModel {
            commission_bps: cfg.backtest.commission_bps,
            spread_bps: cfg.backtest.spread_bps,
            slippage_bps: cfg.backtest.slippage_bps,
            sec_fee_bps: cfg.backtest.sec_fee_bps,
            finra_taf_per_share: cfg.backtest.finra_taf_per_share,
        },
        allow_fractional: cfg.backtest.allow_fractional,
        risk_free_rate_pct: cfg.strategy.risk_free_rate_pct,
        stale_bar_days: cfg.backtest.stale_bar_days,
        min_history_bars: cfg.strategy.min_history_bars,
        max_hold_days_override: cfg.backtest.max_hold_days_override,
        label: format!("exit experiment {}", horizon.short_label()),
    }
}

fn print_row(
    label: &str,
    result: &BacktestResult,
    baseline: &Option<BacktestResult>,
    max_hold_signals: usize,
) {
    let m = &result.metrics;
    let delta = baseline
        .as_ref()
        .map(|base| m.total_return_pct - base.metrics.total_return_pct)
        .unwrap_or(0.0);
    println!(
        "{label:<14} final ${:>9.2}  return {:>7.2}%  delta {:>7.2}%  CAGR {:>6.2}%  Sharpe {:>5.2}  maxDD {:>5.2}%  trades {:>3}  avg hold {:>5.1}  max-hold signals {:>3}",
        m.final_equity,
        m.total_return_pct,
        delta,
        m.cagr_pct,
        m.sharpe,
        m.max_drawdown_pct,
        m.trades,
        avg_hold(result),
        max_hold_signals
    );
    println!(
        "{:<14} top winning trade share {:.1}%",
        "",
        top_trade_share(result)
    );
}

fn top_trade_share(result: &BacktestResult) -> f64 {
    let total: f64 = result
        .trades
        .iter()
        .map(|trade| trade.net_pnl.max(0.0))
        .sum();
    if total <= 0.0 {
        return 0.0;
    }
    let top = result
        .trades
        .iter()
        .map(|trade| trade.net_pnl.max(0.0))
        .fold(0.0, f64::max);
    top / total * 100.0
}

fn avg_hold(result: &BacktestResult) -> f64 {
    if result.trades.is_empty() {
        return 0.0;
    }
    result
        .trades
        .iter()
        .map(|trade| trade.hold_days as f64)
        .sum::<f64>()
        / result.trades.len() as f64
}

fn avg_invested(result: &BacktestResult) -> f64 {
    if result.equity.is_empty() {
        return 0.0;
    }
    let total: f64 = result
        .equity
        .iter()
        .map(|point| {
            if point.equity > 0.0 {
                point.position_value / point.equity * 100.0
            } else {
                0.0
            }
        })
        .sum();
    total / result.equity.len() as f64
}
