//! Three-year benchmark orchestration: six consecutive six-month windows on one
//! continuous portfolio, SPY/QQQ/buy-and-hold/cash comparisons, walk-forward
//! parameter selection, parameter stability and an untouched OOS holdout.

use crate::backtest::engine::{self, PreparedData};
use crate::backtest::metrics::compute_windows;
use crate::backtest::types::{
    BacktestConfig, BacktestResult, FoldResult, Metrics, OosResult, StabilityRow, TuneParams,
    WalkForwardResult,
};
use crate::config::WalkForwardSettings;
use crate::error::{Error, Result};
use crate::model::{DataProvenance, HorizonParams, StrategyMode};
use chrono::{Months, NaiveDate};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub generated_at: chrono::DateTime<chrono::Utc>,
    pub provenance: DataProvenance,
    pub universe_size: usize,
    pub headlines: Vec<BacktestResult>,
    pub walk_forward: Option<WalkForwardResult>,
    pub warnings: Vec<String>,
    pub elapsed_ms: u128,
}

pub fn run_benchmark(
    data: &PreparedData,
    base_cfg: &BacktestConfig,
    base_params: HorizonParams,
    settings: &WalkForwardSettings,
    provenance: DataProvenance,
    run_validation: bool,
) -> Result<BenchmarkReport> {
    let started = Instant::now();
    let mut warnings = Vec::new();

    // Headline runs: one per strategy leg. A Compare request runs both.
    let mut headlines = Vec::new();
    let legs: Vec<(StrategyMode, String)> = match base_cfg.strategy {
        StrategyMode::Compare => vec![
            (StrategyMode::Tpi, format!("{} [TPI]", base_cfg.label)),
            (
                StrategyMode::Consensus,
                format!("{} [TPI+Consensus]", base_cfg.label),
            ),
        ],
        other => vec![(other, base_cfg.label.clone())],
    };
    for (strategy, label) in &legs {
        let mut cfg = base_cfg.clone();
        cfg.strategy = *strategy;
        cfg.label = label.clone();
        headlines.push(engine::run(data, &cfg, base_params)?);
    }

    let mut walk_forward_report = None;
    if run_validation {
        let mut wf_cfg = base_cfg.clone();
        let validation_strategy = if base_cfg.strategy == StrategyMode::Compare {
            StrategyMode::Tpi
        } else {
            base_cfg.strategy
        };
        wf_cfg.strategy = validation_strategy;
        wf_cfg.label = format!("walk-forward [{}]", validation_strategy.short_label());
        match walk_forward(data, &wf_cfg, base_params, settings) {
            Ok(wf) => {
                warnings.extend(wf.warnings.iter().cloned());
                walk_forward_report = Some(wf);
            }
            Err(e) => warnings.push(format!("walk-forward analysis skipped: {e}")),
        }
        if base_cfg.strategy == StrategyMode::Compare {
            warnings.push(
                "Walk-forward, stability and OOS were computed for the TPI leg only".to_string(),
            );
        }
    }

    Ok(BenchmarkReport {
        generated_at: chrono::Utc::now(),
        provenance,
        universe_size: data.universe.len(),
        headlines,
        walk_forward: walk_forward_report,
        warnings,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

pub fn tune_grid(settings: &WalkForwardSettings) -> Vec<TuneParams> {
    let atr = if settings.atr_mult_grid.is_empty() {
        vec![2.5]
    } else {
        settings.atr_mult_grid.clone()
    };
    let rsi = if settings.rsi_max_grid.is_empty() {
        vec![75.0]
    } else {
        settings.rsi_max_grid.clone()
    };
    let exit = if settings.exit_period_grid.is_empty() {
        vec![20]
    } else {
        settings.exit_period_grid.clone()
    };
    let mut out = Vec::new();
    for a in &atr {
        for r in &rsi {
            for e in &exit {
                let p = TuneParams {
                    atr_mult: *a,
                    rsi_max: *r,
                    exit_period: *e,
                };
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

fn walk_forward(
    data: &PreparedData,
    base_cfg: &BacktestConfig,
    base_params: HorizonParams,
    settings: &WalkForwardSettings,
) -> Result<WalkForwardResult> {
    let grid = tune_grid(settings);
    if grid.is_empty() {
        return Err(Error::validation("walk-forward parameter grid is empty"));
    }
    if grid.len() > 200 {
        return Err(Error::validation(
            "walk-forward grid exceeds 200 combinations; narrow the grid",
        ));
    }
    let mut warnings = Vec::new();
    let mut folds = Vec::new();

    let train_months = settings.train_months.max(1);
    let test_months = settings.test_months.max(1);
    let mut train_start = base_cfg.start;
    let mut fold_no = 0;
    while fold_no < 12 {
        let train_end = train_start
            .checked_add_months(Months::new(train_months))
            .unwrap_or(base_cfg.end);
        let test_start = train_end;
        let test_end = test_start
            .checked_add_months(Months::new(test_months))
            .unwrap_or(base_cfg.end)
            .min(base_cfg.end);
        if train_end >= base_cfg.end || test_start >= base_cfg.end || test_end <= test_start {
            break;
        }
        let best = select_best(
            data,
            base_cfg,
            base_params,
            &grid,
            train_start,
            train_end,
            settings.min_trades_for_selection,
        )?;
        let selected = best.0;
        let mut test_cfg = base_cfg.clone();
        test_cfg.start = test_start;
        test_cfg.end = test_end;
        test_cfg.label = format!("fold {} test", fold_no + 1);
        let oos = engine::run(data, &test_cfg, selected.apply(base_params))?;
        folds.push(FoldResult {
            train_start,
            train_end,
            test_start,
            test_end,
            selected,
            in_sample: best.1,
            out_of_sample: oos.metrics.clone(),
            selected_trades: oos.trades.len(),
        });
        fold_no += 1;
        train_start = train_start
            .checked_add_months(Months::new(test_months))
            .unwrap_or(base_cfg.end);
    }
    if folds.is_empty() {
        warnings.push(
            "Not enough history for a 12-month train / 6-month test walk-forward fold".to_string(),
        );
    }

    let oos_months = settings.oos_months.max(1);
    let oos_start = base_cfg
        .end
        .checked_sub_months(Months::new(oos_months))
        .unwrap_or(base_cfg.start);
    let oos = if oos_start > base_cfg.start
        && (oos_start - base_cfg.start).num_days() >= 180
        && oos_start < base_cfg.end
    {
        match select_best(
            data,
            base_cfg,
            base_params,
            &grid,
            base_cfg.start,
            oos_start,
            settings.min_trades_for_selection,
        ) {
            Ok((selected, in_sample)) => {
                let mut test_cfg = base_cfg.clone();
                test_cfg.start = oos_start;
                test_cfg.end = base_cfg.end;
                test_cfg.label = "untouched OOS".to_string();
                match engine::run(data, &test_cfg, selected.apply(base_params)) {
                    Ok(result) => Some(OosResult {
                        train_start: base_cfg.start,
                        train_end: oos_start,
                        test_start: oos_start,
                        test_end: base_cfg.end,
                        selected,
                        in_sample,
                        out_of_sample: result.metrics.clone(),
                    }),
                    Err(e) => {
                        warnings.push(format!("OOS evaluation skipped: {e}"));
                        None
                    }
                }
            }
            Err(e) => {
                warnings.push(format!("OOS parameter selection skipped: {e}"));
                None
            }
        }
    } else {
        warnings.push("Not enough history for an untouched OOS holdout".to_string());
        None
    };

    let stability = parameter_stability(data, base_cfg, base_params, &grid)?;

    Ok(WalkForwardResult {
        folds,
        grid_size: grid.len(),
        oos,
        stability,
        warnings,
    })
}

/// Return the best parameter set and its in-sample metrics. Combos run in
/// parallel; the ranking is deterministic (Sharpe, then total return, then
/// lower ATR multiple).
fn select_best(
    data: &PreparedData,
    base_cfg: &BacktestConfig,
    base_params: HorizonParams,
    grid: &[TuneParams],
    start: NaiveDate,
    end: NaiveDate,
    min_trades: usize,
) -> Result<(TuneParams, Metrics)> {
    let results: Vec<(TuneParams, Metrics)> = grid
        .par_iter()
        .map(|p| {
            let mut cfg = base_cfg.clone();
            cfg.start = start;
            cfg.end = end;
            cfg.label = format!("IS {p}");
            let result = engine::run(data, &cfg, p.apply(base_params))?;
            Ok((*p, result.metrics))
        })
        .collect::<Result<Vec<_>>>()?;
    if results.is_empty() {
        return Err(Error::NoData("no parameter combinations evaluated".into()));
    }
    let eligible: Vec<&(TuneParams, Metrics)> = results
        .iter()
        .filter(|(_, m)| m.trades >= min_trades)
        .collect();
    let pool: Vec<&(TuneParams, Metrics)> = if eligible.is_empty() {
        results.iter().collect()
    } else {
        eligible
    };
    let best = pool
        .into_iter()
        .max_by(|(pa, a), (pb, b)| {
            score(a)
                .partial_cmp(&score(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    pb.atr_mult
                        .partial_cmp(&pa.atr_mult)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        })
        .expect("non-empty pool");
    Ok((best.0, best.1.clone()))
}

fn score(m: &Metrics) -> f64 {
    let sharpe = if m.sharpe.is_finite() {
        m.sharpe
    } else {
        -99.0
    };
    // Return dominates only when Sharpe ties; keep both on comparable scales.
    sharpe * 1000.0 + m.total_return_pct
}

fn parameter_stability(
    data: &PreparedData,
    base_cfg: &BacktestConfig,
    base_params: HorizonParams,
    grid: &[TuneParams],
) -> Result<Vec<StabilityRow>> {
    let runs: Vec<(TuneParams, Vec<f64>)> = grid
        .par_iter()
        .map(|p| {
            let mut cfg = base_cfg.clone();
            cfg.label = format!("stability {p}");
            let result = engine::run(data, &cfg, p.apply(base_params))?;
            let windows = compute_windows(&result.equity, &result.trades, cfg.start, cfg.end);
            Ok((*p, windows.iter().map(|w| w.return_pct).collect::<Vec<_>>()))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut rows: Vec<StabilityRow> = runs
        .into_iter()
        .map(|(params, returns)| {
            let n = returns.len() as f64;
            let mean = if n > 0.0 {
                returns.iter().sum::<f64>() / n
            } else {
                0.0
            };
            let stdev = if n > 1.0 {
                (returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
            } else {
                0.0
            };
            StabilityRow {
                params,
                mean_return_pct: mean,
                stdev_return_pct: stdev,
                worst_window_pct: returns
                    .iter()
                    .copied()
                    .fold(f64::INFINITY, f64::min)
                    .max(-999.0),
                best_window_pct: returns
                    .iter()
                    .copied()
                    .fold(f64::NEG_INFINITY, f64::max)
                    .min(999.0),
                positive_windows: returns.iter().filter(|r| **r > 0.0).count(),
                window_returns_pct: returns,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        score_from_stability(b)
            .partial_cmp(&score_from_stability(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(rows)
}

fn score_from_stability(r: &StabilityRow) -> f64 {
    r.mean_return_pct - 0.5 * r.stdev_return_pct
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest::types::FeeModel;
    use crate::data::synthetic::synthetic_universe;
    use crate::model::{Horizon, RiskProfile, StrategyMode};
    use chrono::Duration;
    use std::collections::HashMap;

    fn config(start: NaiveDate, end: NaiveDate) -> BacktestConfig {
        BacktestConfig {
            start,
            end,
            initial_capital: 10_000.0,
            horizon: Horizon::Medium,
            strategy: StrategyMode::Tpi,
            risk: RiskProfile::Balanced,
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
            label: "synthetic benchmark".to_string(),
        }
    }

    #[test]
    fn grid_is_cartesian_and_deduplicated() {
        let s = WalkForwardSettings {
            atr_mult_grid: vec![2.0, 2.5],
            rsi_max_grid: vec![70.0, 75.0],
            exit_period_grid: vec![10, 20],
            ..WalkForwardSettings::default()
        };
        assert_eq!(tune_grid(&s).len(), 8);
    }

    #[test]
    fn short_benchmark_runs_headline_and_stability() {
        let start = NaiveDate::from_ymd_opt(2023, 11, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 11, 1).unwrap();
        let warm = start - Duration::days(420);
        let bars: HashMap<String, Vec<crate::model::Bar>> = synthetic_universe(
            &["AAA".into(), "BBB".into(), "SPY".into(), "QQQ".into()],
            warm,
            end,
        );
        let data = PreparedData::new(
            &bars,
            &["AAA".into(), "BBB".into()],
            &["SPY".into(), "QQQ".into()],
        )
        .unwrap();
        let settings = WalkForwardSettings {
            atr_mult_grid: vec![2.0, 2.5],
            rsi_max_grid: vec![75.0],
            exit_period_grid: vec![20],
            ..WalkForwardSettings::default()
        };
        let report = run_benchmark(
            &data,
            &config(start, end),
            Horizon::Medium.params(),
            &settings,
            DataProvenance::synthetic(),
            true,
        )
        .unwrap();
        assert_eq!(report.headlines.len(), 1);
        assert!(report.headlines[0].metrics.final_equity > 0.0);
        assert!(report.walk_forward.is_some());
    }
}
