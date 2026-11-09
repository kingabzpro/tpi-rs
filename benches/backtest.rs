use chrono::{Duration, NaiveDate};
use criterion::{Criterion, criterion_group, criterion_main};
use std::collections::HashMap;
use std::hint::black_box;
use tpi_rs::backtest::engine::{self, PreparedData};
use tpi_rs::backtest::types::{BacktestConfig, FeeModel};
use tpi_rs::data::synthetic::synthetic_universe;
use tpi_rs::model::{Bar, Horizon, RiskProfile, StrategyMode};

fn data(symbols: usize, years: i64) -> (PreparedData, BacktestConfig) {
    let start = NaiveDate::from_ymd_opt(2023, 11, 1).unwrap();
    let warm = start - Duration::days(450);
    let names: Vec<String> = (0..symbols).map(|i| format!("SYM{i:03}")).collect();
    let mut all = names.clone();
    all.push("SPY".into());
    all.push("QQQ".into());
    let mut bars: HashMap<String, Vec<Bar>> =
        synthetic_universe(&all, warm, start + Duration::days(years * 365));
    bars.retain(|_, b| !b.is_empty());
    let data = PreparedData::new(&bars, &names, &["SPY".to_string(), "QQQ".to_string()]).unwrap();
    let cfg = BacktestConfig {
        start,
        end: start + Duration::days(years * 365),
        initial_capital: 10_000.0,
        horizon: Horizon::Medium,
        strategy: StrategyMode::Tpi,
        risk: RiskProfile::Balanced,
        vol_target_pct: 0.0,
        risk_pct_override: None,
        fees: FeeModel {
            commission_bps: 0.0,
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
        label: "benchmark".into(),
    };
    (data, cfg)
}

fn bench_backtest(c: &mut Criterion) {
    let (prepared, cfg) = data(50, 1);
    let mut group = c.benchmark_group("backtest");
    group.sample_size(10);
    group.bench_function("50_symbols_1_year", |bench| {
        bench.iter(|| {
            engine::run(
                black_box(&prepared),
                black_box(&cfg),
                Horizon::Medium.params(),
            )
            .unwrap()
        })
    });
    group.finish();
}

criterion_group!(benches, bench_backtest);
criterion_main!(benches);
