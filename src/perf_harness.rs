//! Reproducible performance harness: latency percentiles, throughput, process
//! memory and CPU utilization on deterministic synthetic data.
//!
//! Run with `tpi --perf`. Results are hardware-specific and labelled as such.

use crate::backtest::engine::{self, PreparedData};
use crate::backtest::types::{BacktestConfig, FeeModel};
use crate::config::Config;
use crate::data::synthetic::synthetic_universe;
use crate::error::Result;
use crate::indicators::Series;
use crate::model::{Bar, Horizon, RiskProfile, StrategyMode};
use crate::perf::Stopwatch;
use crate::report;
use crate::strategy::{EvalConfig, evaluate_fast};
use chrono::Duration;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::time::Instant;

#[derive(Debug, Clone, Serialize)]
pub struct PerfReport {
    pub generated_at: String,
    pub symbols: usize,
    pub history_days: usize,
    pub rayon_threads: usize,
    pub indicator_parallel_ms: f64,
    pub indicator_parallel_symbols_per_sec: f64,
    pub indicator_bars_per_sec: f64,
    pub indicator_single_thread_ms: f64,
    pub scan_mean_ms: f64,
    pub scan_p50_ms: f64,
    pub scan_p95_ms: f64,
    pub scan_symbols_per_sec: f64,
    pub backtest_ms: f64,
    pub backtest_symbol_days_per_sec: f64,
    pub working_set_mb: f64,
    pub peak_working_set_mb: f64,
    pub cpu_seconds: f64,
    pub wall_seconds: f64,
    pub cpu_percent: f64,
    pub binary_mb: f64,
    pub json_path: String,
}

pub fn run(cfg: &Config) -> Result<()> {
    let symbols = 100usize;
    let names: Vec<String> = (0..symbols).map(|i| format!("PF{i:03}")).collect();
    let end = chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let start = chrono::NaiveDate::from_ymd_opt(2023, 11, 1).unwrap();
    let warm = start - Duration::days(450);
    let mut all = names.clone();
    all.push("SPY".into());
    all.push("QQQ".into());
    let bars: HashMap<String, Vec<Bar>> = synthetic_universe(&all, warm, end);
    let history_days = bars[&names[0]].len();
    println!(
        "Perf harness: {} synthetic symbols x {} bars (deterministic data, not market data)",
        symbols, history_days
    );
    let total_watch = Stopwatch::start();

    // Warm up Rayon and the allocator.
    let _ = names.par_iter().map(|s| Series::compute(&bars[s])).count();

    let t = Instant::now();
    let series: HashMap<String, Series> = names
        .par_iter()
        .map(|s| (s.clone(), Series::compute(&bars[s])))
        .collect();
    let indicator_parallel_ms = t.elapsed().as_secs_f64() * 1000.0;
    let indicator_parallel_symbols_per_sec = symbols as f64 / (indicator_parallel_ms / 1000.0);
    let indicator_bars_per_sec = (symbols * history_days) as f64 / (indicator_parallel_ms / 1000.0);

    let t = Instant::now();
    for s in &names {
        let _ = Series::compute(&bars[s]);
    }
    let indicator_single_thread_ms = t.elapsed().as_secs_f64() * 1000.0;

    let eval_cfg = EvalConfig {
        horizon: Horizon::Medium,
        strategy: StrategyMode::Tpi,
        risk: RiskProfile::Balanced,
        equity: 10_000.0,
        cash: 10_000.0,
        allow_fractional: true,
        require_rating_buy: true,
        volume_ratio_min: 1.0,
        min_history_bars: 220,
        long_only: true,
    };
    // Warm-up evaluation.
    for s in &names {
        let _ = evaluate_fast(
            s,
            &series[s],
            series[s].len() - 1,
            Horizon::Medium.params(),
            eval_cfg,
        );
    }
    let mut latencies = Vec::with_capacity(40);
    for _ in 0..40 {
        let t = Instant::now();
        names
            .par_iter()
            .map(|s| {
                evaluate_fast(
                    s,
                    &series[s],
                    series[s].len() - 1,
                    Horizon::Medium.params(),
                    eval_cfg,
                )
            })
            .collect::<Vec<_>>();
        latencies.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let scan_mean_ms = latencies.iter().sum::<f64>() / latencies.len() as f64;
    let scan_p50_ms = latencies[latencies.len() / 2];
    let scan_p95_ms = latencies[(latencies.len() * 95 / 100).min(latencies.len() - 1)];
    let scan_symbols_per_sec = symbols as f64 / (scan_mean_ms / 1000.0);

    let universe: Vec<String> = names.iter().take(50).cloned().collect();
    let data = PreparedData::new(&bars, &universe, &["SPY".into(), "QQQ".into()])?;
    let backtest_cfg = BacktestConfig {
        start,
        end,
        initial_capital: 10_000.0,
        horizon: Horizon::Medium,
        strategy: StrategyMode::Tpi,
        risk: RiskProfile::Balanced,
        fees: FeeModel {
            commission_bps: cfg.backtest.commission_bps,
            spread_bps: cfg.backtest.spread_bps,
            slippage_bps: cfg.backtest.slippage_bps,
            sec_fee_bps: cfg.backtest.sec_fee_bps,
            finra_taf_per_share: cfg.backtest.finra_taf_per_share,
        },
        allow_fractional: true,
        risk_free_rate_pct: cfg.strategy.risk_free_rate_pct,
        stale_bar_days: cfg.backtest.stale_bar_days,
        min_history_bars: cfg.strategy.min_history_bars,
        max_hold_days_override: None,
        label: "perf".into(),
    };
    // Warm up the backtest path once, then measure it.
    let _ = engine::run(&data, &backtest_cfg, Horizon::Medium.params())?;
    let t = Instant::now();
    let result = engine::run(&data, &backtest_cfg, Horizon::Medium.params())?;
    let backtest_ms = t.elapsed().as_secs_f64() * 1000.0;
    let sessions = result.trading_days as f64;
    let backtest_symbol_days_per_sec =
        (data.universe.len() as f64 * sessions) / (backtest_ms / 1000.0);

    let stats = total_watch.finish();
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    let binary_mb = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len() as f64 / (1024.0 * 1024.0))
        .unwrap_or(0.0);

    let prefix = report::timestamp_prefix();
    let json_path = cfg.report_dir().join(format!("perf-{prefix}.json"));
    let report = PerfReport {
        generated_at: chrono::Utc::now().to_rfc3339(),
        symbols,
        history_days,
        rayon_threads: threads,
        indicator_parallel_ms,
        indicator_parallel_symbols_per_sec,
        indicator_bars_per_sec,
        indicator_single_thread_ms,
        scan_mean_ms,
        scan_p50_ms,
        scan_p95_ms,
        scan_symbols_per_sec,
        backtest_ms,
        backtest_symbol_days_per_sec,
        working_set_mb: stats.working_set_mb,
        peak_working_set_mb: stats.peak_working_set_mb,
        cpu_seconds: stats.cpu_seconds,
        wall_seconds: stats.wall_seconds,
        cpu_percent: stats.cpu_percent(),
        binary_mb,
        json_path: json_path.display().to_string(),
    };
    report::ensure_dir(&cfg.report_dir())?;
    report::write_json(&json_path, &report)?;

    println!();
    println!("TPI-RS performance report (synthetic data)");
    println!("{}", "-".repeat(72));
    println!("Threads (available parallelism): {}", threads);
    println!(
        "Indicator series: {:.2} ms parallel for {} symbols ({} bars)",
        indicator_parallel_ms, symbols, history_days
    );
    println!(
        "  throughput: {:.0} symbols/s, {:.0} bars/s | single-thread: {:.1} ms",
        indicator_parallel_symbols_per_sec, indicator_bars_per_sec, indicator_single_thread_ms
    );
    println!(
        "Scan evaluation (100 symbols): mean {:.2} ms, p50 {:.2} ms, p95 {:.2} ms, {:.0} symbols/s",
        scan_mean_ms, scan_p50_ms, scan_p95_ms, scan_symbols_per_sec
    );
    println!(
        "Backtest (50 symbols x {} sessions): {:.1} ms, {:.0} symbol-days/s",
        result.trading_days, backtest_ms, backtest_symbol_days_per_sec
    );
    println!(
        "Process: working set {:.1} MB, peak {:.1} MB, CPU {:.2} s / wall {:.2} s = {:.0}% CPU",
        stats.working_set_mb,
        stats.peak_working_set_mb,
        stats.cpu_seconds,
        stats.wall_seconds,
        stats.cpu_percent()
    );
    println!("Release binary: {:.1} MB", binary_mb);
    println!("JSON: {}", json_path.display());
    println!("{}", "-".repeat(72));
    Ok(())
}
