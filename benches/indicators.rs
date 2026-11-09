use chrono::{Duration, NaiveDate};
use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use tpi_rs::data::synthetic::synthetic_bars;
use tpi_rs::indicators::Series;
use tpi_rs::indicators::ratings::{rating_fast, technical_rating};
use tpi_rs::model::{Horizon, RiskProfile, StrategyMode};
use tpi_rs::strategy::{EvalConfig, evaluate_fast};

fn bars(n: usize) -> Vec<tpi_rs::model::Bar> {
    let end = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let start = end - Duration::days((n as i64 * 7 / 5) + 30);
    let mut series = synthetic_bars("BENCH", start, end);
    if series.len() > n {
        series = series.split_off(series.len() - n);
    }
    series
}

fn bench_indicators(c: &mut Criterion) {
    let b = bars(5000);
    let mut group = c.benchmark_group("indicators");
    group.bench_function("series_5000_bars", |bench| {
        bench.iter(|| Series::compute(black_box(&b)))
    });
    let s = Series::compute(&b);
    group.bench_function("rating_detail_5000", |bench| {
        bench.iter(|| technical_rating(black_box(&s), black_box(4999)))
    });
    group.bench_function("rating_fast_5000", |bench| {
        bench.iter(|| rating_fast(black_box(&s), black_box(4999)))
    });
    let cfg = EvalConfig {
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
        vol_target_pct: 0.0,
        risk_pct_override: None,
    };
    group.bench_function("evaluate_fast_5000", |bench| {
        bench.iter(|| {
            evaluate_fast(
                black_box("BENCH"),
                black_box(&s),
                black_box(4999),
                Horizon::Medium.params(),
                cfg,
            )
        })
    });
    group.finish();
}

criterion_group!(benches, bench_indicators);
criterion_main!(benches);
