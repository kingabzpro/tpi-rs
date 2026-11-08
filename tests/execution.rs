//! Execution/accounting integration tests: next-open fills, costs, stops,
//! final liquidation, and a full-run vs truncated-run no-lookahead check.

use chrono::{Duration, NaiveDate};
use std::collections::HashMap;
use tpi_rs::backtest::engine::{self, PreparedData};
use tpi_rs::backtest::types::{BacktestConfig, FeeModel};
use tpi_rs::data::synthetic::synthetic_universe;
use tpi_rs::indicators::Series;
use tpi_rs::model::{Action, Bar, Horizon, RiskProfile, StrategyMode};
use tpi_rs::strategy::{EvalConfig, evaluate_fast};

fn cfg(start: NaiveDate, end: NaiveDate) -> BacktestConfig {
    BacktestConfig {
        start,
        end,
        initial_capital: 10_000.0,
        horizon: Horizon::Medium,
        strategy: StrategyMode::Tpi,
        risk: RiskProfile::Balanced,
        fees: FeeModel {
            commission_bps: 10.0,
            spread_bps: 2.0,
            slippage_bps: 5.0,
            sec_fee_bps: 0.3,
            finra_taf_per_share: 0.000166,
        },
        allow_fractional: true,
        risk_free_rate_pct: 0.0,
        stale_bar_days: 5,
        min_history_bars: 220,
        max_hold_days_override: None,
        label: "execution-test".into(),
    }
}

fn build(
    symbols: &[String],
    start: NaiveDate,
    end: NaiveDate,
) -> (PreparedData, HashMap<String, Vec<Bar>>) {
    let warm = start - Duration::days(450);
    let mut all = symbols.to_vec();
    all.push("SPY".into());
    all.push("QQQ".into());
    let bars = synthetic_universe(&all, warm, end);
    let data = PreparedData::new(&bars, symbols, &["SPY".to_string(), "QQQ".to_string()]).unwrap();
    (data, bars)
}

fn eval_cfg() -> EvalConfig {
    EvalConfig {
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
    }
}

#[test]
fn entries_fill_at_next_open_with_costs_and_final_liquidation() {
    let start = NaiveDate::from_ymd_opt(2024, 6, 3).unwrap();
    let end = NaiveDate::from_ymd_opt(2025, 11, 1).unwrap();
    let symbols: Vec<String> = (0..50).map(|i| format!("T{i:02}")).collect();
    let (data, bars) = build(&symbols, start, end);
    let config = cfg(start, end);
    let result = engine::run(&data, &config, Horizon::Medium.params()).unwrap();
    assert!(
        result.trades.len() >= 3,
        "expected some trades, got {}",
        result.trades.len()
    );

    for trade in &result.trades {
        let series = Series::compute(bars.get(&trade.symbol).unwrap());
        let entry_i = series
            .dates
            .binary_search(&trade.entry_date)
            .expect("entry date exists");
        let expected_entry = config.fees.buy_fill(series.open[entry_i]);
        assert!(
            (trade.entry_price - expected_entry).abs() < 1e-9,
            "{} entry {:.6} != next open fill {:.6}",
            trade.symbol,
            trade.entry_price,
            expected_entry
        );
        assert!(trade.entry_date > start);

        // The prior session must have produced a BUY for the traded symbol.
        let prior = entry_i - 1;
        let prior_signal = evaluate_fast(
            &trade.symbol,
            &series,
            prior,
            Horizon::Medium.params(),
            eval_cfg(),
        )
        .unwrap();
        assert_eq!(
            prior_signal.action,
            Action::Buy,
            "{} was bought without a prior-session BUY",
            trade.symbol
        );
        assert!(trade.fees > 0.0, "commission was not charged");
        assert!(
            (trade.net_pnl - (trade.gross_pnl - trade.fees)).abs() < 1e-6,
            "net PnL does not subtract fees"
        );
        assert!(trade.exit_date >= trade.entry_date);
        assert!(trade.hold_days >= 1);
    }

    let last = result.equity.last().unwrap();
    assert!(
        (last.position_value).abs() < 1e-9,
        "position not liquidated"
    );
    assert!(
        (last.cash - last.equity).abs() < 1e-6,
        "final cash/equity mismatch"
    );
    assert!(last.date <= end);
}

#[test]
fn full_history_and_truncated_history_agree_before_cutoff() {
    let start = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
    let end = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();
    let cutoff = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
    let symbols: Vec<String> = (0..12).map(|i| format!("S{i:02}")).collect();
    let (full_data, full_bars) = build(&symbols, start, end);

    let mut truncated_bars = full_bars.clone();
    for bars in truncated_bars.values_mut() {
        bars.retain(|b| b.date <= cutoff);
    }
    let truncated = PreparedData::new(
        &truncated_bars,
        &symbols,
        &["SPY".to_string(), "QQQ".to_string()],
    )
    .unwrap();
    let mut full_cfg = cfg(start, end);
    let mut trunc_cfg = cfg(start, cutoff);
    full_cfg.label = "full".into();
    trunc_cfg.label = "truncated".into();

    let full = engine::run(&full_data, &full_cfg, Horizon::Medium.params()).unwrap();
    let short = engine::run(&truncated, &trunc_cfg, Horizon::Medium.params()).unwrap();

    let map: HashMap<NaiveDate, f64> = short.equity.iter().map(|p| (p.date, p.equity)).collect();
    for point in full.equity.iter().filter(|p| p.date < cutoff) {
        let expected = map
            .get(&point.date)
            .unwrap_or_else(|| panic!("missing truncated equity for {}", point.date));
        assert!(
            (point.equity - expected).abs() < 1e-6,
            "future data changed equity at {}: {} vs {}",
            point.date,
            point.equity,
            expected
        );
    }
    for (a, b) in full.decisions.iter().zip(short.decisions.iter()) {
        if a.date < cutoff {
            assert_eq!(a.date, b.date);
            assert_eq!(a.action, b.action);
            assert_eq!(a.symbol, b.symbol);
            assert_eq!(a.candidates, b.candidates);
        }
    }
}

#[test]
fn equity_is_never_negative_across_horizons_and_profiles() {
    let start = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
    let end = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
    let symbols: Vec<String> = (0..20).map(|i| format!("H{i:02}")).collect();
    let (data, _) = build(&symbols, start, end);
    for horizon in [Horizon::Short, Horizon::Medium, Horizon::Long] {
        for risk in [
            RiskProfile::Conservative,
            RiskProfile::Balanced,
            RiskProfile::Aggressive,
        ] {
            let mut c = cfg(start, end);
            c.horizon = horizon;
            c.risk = risk;
            let result = engine::run(&data, &c, horizon.params()).unwrap();
            assert!(
                result.equity.iter().all(|p| p.equity >= -1e-9),
                "negative equity for {horizon:?}/{risk:?}"
            );
            assert!(result.metrics.max_drawdown_pct >= 0.0);
            assert!(result.metrics.max_drawdown_pct <= 100.0);
        }
    }
}
