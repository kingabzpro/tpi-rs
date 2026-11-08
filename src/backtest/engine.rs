//! Next-session portfolio simulator.
//!
//! Execution model: a signal is evaluated on the close of day T and filled at
//! the open of day T+1 with half-spread, slippage and commissions. Stops are
//! checked against the day's low with gap-aware fills. One stock position at a
//! time, long only, no leverage. Daily equity is marked at the close.

use crate::backtest::metrics::{compute_metrics, compute_windows, relative_metrics};
use crate::backtest::types::{
    BacktestConfig, BacktestResult, ComparisonResult, DailyDecision, EquityPoint, FeeModel, Trade,
};
use crate::error::{Error, Result};
use crate::indicators::Series;
use crate::model::{Action, Bar, HorizonParams, Rating, Signal, StrategyMode};
use crate::strategy::risk::{plan_position, trailing_stop};
use crate::strategy::{EvalConfig, evaluate_fast};
use chrono::NaiveDate;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

pub struct PreparedData {
    pub universe: Vec<String>,
    pub benchmarks: Vec<String>,
    pub series: BTreeMap<String, Series>,
    pub trading_days: Vec<NaiveDate>,
    pub warnings: Vec<String>,
}

impl PreparedData {
    pub fn new(
        bars: &HashMap<String, Vec<Bar>>,
        universe: &[String],
        benchmarks: &[String],
    ) -> Result<Self> {
        let mut warnings = Vec::new();
        let mut series = BTreeMap::new();
        let mut all_symbols: Vec<String> = universe.to_vec();
        for b in benchmarks {
            if !all_symbols.contains(b) {
                all_symbols.push(b.clone());
            }
        }
        all_symbols.sort();
        all_symbols.dedup();
        for symbol in &all_symbols {
            let Some(mut bars) = bars.get(symbol).cloned() else {
                continue;
            };
            bars.retain(|b| b.is_finite());
            bars.sort_by_key(|b| b.date);
            bars.dedup_by_key(|b| b.date);
            if bars.len() < 2 {
                warnings.push(format!("{symbol}: fewer than 2 valid bars; skipped"));
                continue;
            }
            series.insert(symbol.clone(), Series::compute(&bars));
        }

        let tradeable: Vec<String> = universe
            .iter()
            .filter(|s| series.contains_key(*s))
            .cloned()
            .collect();
        if tradeable.is_empty() {
            return Err(Error::NoData(
                "no usable universe series for backtest".to_string(),
            ));
        }
        for s in universe {
            if !series.contains_key(s) {
                warnings.push(format!("{s}: no data; excluded from scanning"));
            }
        }
        let benchmark_available: Vec<String> = benchmarks
            .iter()
            .filter(|s| series.contains_key(*s))
            .cloned()
            .collect();
        for s in benchmarks {
            if !series.contains_key(s) {
                warnings.push(format!(
                    "benchmark {s} missing from loaded data; comparison omitted"
                ));
            }
        }

        let trading_days = benchmark_available
            .first()
            .and_then(|s| series.get(s))
            .map(|s| s.dates.clone())
            .unwrap_or_else(|| {
                let mut days: Vec<NaiveDate> = series
                    .values()
                    .flat_map(|s| s.dates.iter().copied())
                    .collect();
                days.sort_unstable();
                days.dedup();
                days
            });

        Ok(Self {
            universe: tradeable,
            benchmarks: benchmark_available,
            series,
            trading_days,
            warnings,
        })
    }

    pub fn series(&self, symbol: &str) -> Option<&Series> {
        self.series.get(symbol)
    }

    pub fn bar_index(&self, symbol: &str, date: NaiveDate) -> Option<usize> {
        let series = self.series.get(symbol)?;
        series.dates.binary_search(&date).ok()
    }
}

struct OpenPosition {
    symbol: String,
    shares: f64,
    entry_price: f64,
    entry_date: NaiveDate,
    entry_index: usize,
    stop: f64,
    highest_close: f64,
    last_close: f64,
    entry_fee: f64,
    implicit_entry: f64,
    entry_score: f64,
    entry_rating: Rating,
}

struct PendingEntry {
    symbol: String,
    original_stop: f64,
    atr: f64,
    score: f64,
    rating: Rating,
    attempts: u8,
}

pub fn run(
    data: &PreparedData,
    cfg: &BacktestConfig,
    params: HorizonParams,
) -> Result<BacktestResult> {
    run_with_progress(data, cfg, params, None)
}

/// Run with an optional progress/cancellation callback. The callback receives
/// (completed_days, total_days) and returns false to cancel.
pub fn run_with_progress(
    data: &PreparedData,
    cfg: &BacktestConfig,
    params: HorizonParams,
    progress: Option<&(dyn Fn(usize, usize) -> bool + Sync)>,
) -> Result<BacktestResult> {
    let started = Instant::now();
    if cfg.start >= cfg.end {
        return Err(Error::validation("backtest start must be before end"));
    }
    let days: Vec<NaiveDate> = data
        .trading_days
        .iter()
        .copied()
        .filter(|d| *d >= cfg.start && *d <= cfg.end)
        .collect();
    if days.len() < 2 {
        return Err(Error::NoData(format!(
            "no trading sessions between {} and {}",
            cfg.start, cfg.end
        )));
    }
    let benchmarks: HashSet<&str> = data.benchmarks.iter().map(|s| s.as_str()).collect();
    let candidates: Vec<&String> = data
        .universe
        .iter()
        .filter(|s| !benchmarks.contains(s.as_str()))
        .collect();

    let mut warnings = data.warnings.clone();
    let mut cash = cfg.initial_capital;
    let mut position: Option<OpenPosition> = None;
    let mut pending_exit: Option<String> = None;
    let mut pending_entry: Option<PendingEntry> = None;
    let mut trades: Vec<Trade> = Vec::new();
    let mut decisions: Vec<DailyDecision> = Vec::new();
    let mut equity_curve: Vec<EquityPoint> = Vec::with_capacity(days.len());
    let max_hold = cfg.max_hold_days_override.unwrap_or(params.max_hold_days);
    let mut peak = cfg.initial_capital;
    let eval_cfg_base = EvalConfig {
        horizon: cfg.horizon,
        strategy: if cfg.strategy == StrategyMode::Compare {
            StrategyMode::Tpi
        } else {
            cfg.strategy
        },
        risk: cfg.risk,
        equity: cfg.initial_capital,
        cash: cfg.initial_capital,
        allow_fractional: cfg.allow_fractional,
        require_rating_buy: true,
        volume_ratio_min: 1.0,
        min_history_bars: cfg.min_history_bars,
        long_only: true,
    };

    if let Some(p) = progress
        && !p(0, days.len())
    {
        return Err(Error::Cancelled);
    }
    for (day_idx, &day) in days.iter().enumerate() {
        if let Some(p) = progress
            && day_idx % 4 == 0
            && !p(day_idx, days.len())
        {
            return Err(Error::Cancelled);
        }
        // --------------------------------------------------------------
        // 1. Execute orders generated on the previous close at today's open.
        // --------------------------------------------------------------
        if pending_exit.is_some()
            && let Some(pos) = position.as_ref()
            && let Some(i) = data.bar_index(&pos.symbol, day)
        {
            let series = data.series(&pos.symbol).expect("series");
            let base = series.open[i];
            if base > 0.0 {
                let pos = position.take().expect("position");
                close_position(
                    pos,
                    day,
                    day_idx,
                    base,
                    "exit signal at open",
                    &cfg.fees,
                    &mut cash,
                    &mut trades,
                );
                pending_exit = None;
            }
        }

        if position.is_some() {
            // Entry cannot proceed unless the held position was actually exited.
            pending_entry = None;
        } else if let Some(mut pe) = pending_entry.take() {
            if let Some(i) = data.bar_index(&pe.symbol, day) {
                let series = data.series(&pe.symbol).expect("series");
                let open = series.open[i];
                let fill = cfg.fees.buy_fill(open);
                let stop = fill - params.atr_stop_mult * pe.atr;
                if open > 0.0 && fill > 0.0 && stop > 0.0 && stop < fill && fill > pe.original_stop
                {
                    let plan = plan_position(
                        cash,
                        cash,
                        fill,
                        stop,
                        cfg.risk.params(),
                        cfg.allow_fractional,
                    );
                    if plan.notional >= 10.0 {
                        let entry_fee = cfg.fees.buy_commission(plan.notional);
                        let mut shares = plan.shares;
                        let mut notional = shares * fill;
                        if notional + entry_fee > cash {
                            shares = (cash / (fill * (1.0 + cfg.fees.commission_bps / 10_000.0)))
                                .max(0.0);
                            if !cfg.allow_fractional {
                                shares = shares.floor();
                            }
                            notional = shares * fill;
                        }
                        let entry_fee = cfg.fees.buy_commission(notional);
                        if shares > 0.0 && notional + entry_fee <= cash + 1e-6 {
                            cash -= notional + entry_fee;
                            position = Some(OpenPosition {
                                symbol: pe.symbol.clone(),
                                shares,
                                entry_price: fill,
                                entry_date: day,
                                entry_index: day_idx,
                                stop,
                                highest_close: fill,
                                last_close: fill,
                                entry_fee,
                                implicit_entry: cfg.fees.implicit_cost(notional),
                                entry_score: pe.score,
                                entry_rating: pe.rating,
                            });
                        }
                    }
                }
            } else {
                pe.attempts += 1;
                if pe.attempts < 3 {
                    pending_entry = Some(pe);
                } else {
                    warnings.push(format!(
                        "Dropped deferred entry for {} after 3 sessions without an open",
                        pe.symbol
                    ));
                }
            }
        }

        // --------------------------------------------------------------
        // 2. Intraday protective stop (gap aware).
        // --------------------------------------------------------------
        if let Some(pos) = position.as_ref()
            && let Some(i) = data.bar_index(&pos.symbol, day)
        {
            let series = data.series(&pos.symbol).expect("series");
            if series.low[i] <= pos.stop {
                let base = if series.open[i] <= pos.stop {
                    series.open[i]
                } else {
                    pos.stop
                };
                let pos = position.take().expect("position");
                close_position(
                    pos,
                    day,
                    day_idx,
                    base,
                    "ATR trailing stop",
                    &cfg.fees,
                    &mut cash,
                    &mut trades,
                );
                pending_exit = None;
            }
        }

        // --------------------------------------------------------------
        // 3. Mark-to-market estimate used for sizing and decisions.
        // --------------------------------------------------------------
        let mut position_value = 0.0;
        if let Some(pos) = position.as_mut() {
            if let Some(i) = data.bar_index(&pos.symbol, day) {
                pos.last_close = data.series(&pos.symbol).expect("series").close[i];
            }
            position_value = pos.shares * pos.last_close;
        }
        let equity_now = cash + position_value;

        // --------------------------------------------------------------
        // 4. Evaluate the universe at the close.
        // --------------------------------------------------------------
        let mut signals: Vec<Signal> = Vec::with_capacity(candidates.len());
        let mut eval_cfg = eval_cfg_base;
        eval_cfg.equity = equity_now.max(0.0);
        eval_cfg.cash = cash.max(0.0);
        for symbol in &candidates {
            if let Some(signal) = signal_at(data, symbol, day, params, eval_cfg) {
                signals.push(signal);
            }
        }
        let buy_candidates: Vec<&Signal> =
            signals.iter().filter(|s| s.action == Action::Buy).collect();
        let best = buy_candidates
            .iter()
            .filter(|s| {
                position
                    .as_ref()
                    .map(|p| p.symbol != s.symbol)
                    .unwrap_or(true)
            })
            .max_by(|a, b| {
                a.strength
                    .partial_cmp(&b.strength)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.rating.rank().cmp(&a.rating.rank()))
                    .then_with(|| b.symbol.cmp(&a.symbol))
            })
            .copied();

        // --------------------------------------------------------------
        // 5. Trailing stop and exit rules for the held position.
        // --------------------------------------------------------------
        if let Some(pos) = position.as_mut() {
            if let Some(i) = data.bar_index(&pos.symbol, day) {
                let series = data.series(&pos.symbol).expect("series");
                pos.highest_close = pos.highest_close.max(series.close[i]);
                if let Some(atr) = series.atr14[i] {
                    pos.stop =
                        pos.stop
                            .max(trailing_stop(pos.highest_close, atr, params.atr_stop_mult));
                }
            }
            if pending_exit.is_none() {
                let held_signal = signals.iter().find(|s| s.symbol == pos.symbol);
                let mut reason = None;
                if let Some(sig) = held_signal
                    && sig.action == Action::Sell
                {
                    reason = Some("TPI/consensus exit signal at close".to_string());
                }
                if reason.is_none()
                    && let Some(i) = data.bar_index(&pos.symbol, day)
                {
                    let series = data.series(&pos.symbol).expect("series");
                    if let Some(exit_low) = series.don_exit_low(i, params.exit_period)
                        && series.close[i] < exit_low
                    {
                        reason = Some(format!(
                            "close below prior {}-day low {:.2}",
                            params.exit_period, exit_low
                        ));
                    }
                    if series.close[i] < pos.stop {
                        reason = Some(format!("close below trailing stop {:.2}", pos.stop));
                    }
                }
                let held_sessions = day_idx.saturating_sub(pos.entry_index) + 1;
                if reason.is_none() && held_sessions >= max_hold {
                    reason = Some(format!("max hold {max_hold} sessions reached"));
                }
                if reason.is_some() {
                    pending_exit = reason;
                }
            }
        }

        // --------------------------------------------------------------
        // 6. Rank a candidate for the next open when flat or exiting.
        // --------------------------------------------------------------
        let can_enter = position.is_none() || pending_exit.is_some();
        if can_enter && day_idx + 1 < days.len() {
            if let Some(best) = best {
                if let Some(atr) = best.indicators.atr14 {
                    pending_entry = Some(PendingEntry {
                        symbol: best.symbol.clone(),
                        original_stop: best.stop,
                        atr,
                        score: best.strength,
                        rating: best.rating,
                        attempts: 0,
                    });
                }
            } else {
                pending_entry = None;
            }
        } else if position.is_some() {
            pending_entry = None;
        }

        // --------------------------------------------------------------
        // 7. Force liquidation at the final close.
        // --------------------------------------------------------------
        let last_day = day_idx + 1 == days.len();
        if last_day {
            if let Some(pos) = position.take() {
                let base = pos.last_close;
                close_position(
                    pos,
                    day,
                    day_idx,
                    base,
                    "end of backtest",
                    &cfg.fees,
                    &mut cash,
                    &mut trades,
                );
            }
            pending_exit = None;
            pending_entry = None;
        }

        // --------------------------------------------------------------
        // 8. Daily decision and equity mark.
        // --------------------------------------------------------------
        let held_symbol = position.as_ref().map(|p| p.symbol.clone());
        let chosen = held_symbol
            .clone()
            .or_else(|| pending_entry.as_ref().map(|p| p.symbol.clone()));
        let action = if pending_exit.is_some() {
            Action::Sell
        } else if position.is_some() {
            Action::Hold
        } else if pending_entry.is_some() {
            Action::Buy
        } else {
            Action::Hold
        };
        let chosen_signal = chosen
            .as_ref()
            .and_then(|s| signals.iter().find(|sig| &sig.symbol == s));
        let note = if last_day {
            "final session: any open position was liquidated at the close".to_string()
        } else if let Some(reason) = &pending_exit {
            format!("exit scheduled for next open: {reason}")
        } else if let Some(pe) = &pending_entry {
            format!(
                "top-ranked BUY scheduled for next open (score {:.1}, {} candidates)",
                pe.score,
                buy_candidates.len()
            )
        } else if position.is_some() {
            "holding position; no exit trigger".to_string()
        } else {
            "cash; no qualified BUY candidate".to_string()
        };

        let pos_value = position
            .as_ref()
            .map(|p| p.shares * p.last_close)
            .unwrap_or(0.0);
        let equity = cash + pos_value;
        peak = peak.max(equity);
        let drawdown = if peak > 0.0 {
            (peak - equity) / peak * 100.0
        } else {
            0.0
        };
        decisions.push(DailyDecision {
            date: day,
            action,
            symbol: chosen,
            score: chosen_signal
                .map(|s| s.strength)
                .or_else(|| pending_entry.as_ref().map(|p| p.score)),
            rating: chosen_signal
                .map(|s| s.rating)
                .or_else(|| pending_entry.as_ref().map(|p| p.rating)),
            candidates: buy_candidates.len(),
            equity,
            note,
        });
        equity_curve.push(EquityPoint {
            date: day,
            equity,
            cash,
            position_value: pos_value,
            drawdown_pct: drawdown,
        });
    }

    if trades.is_empty() {
        warnings.push(
            "No trades were generated. This is a valid result, not an error; the strategy has a \
             strict trend + breakout gate."
                .to_string(),
        );
    }
    if pending_exit.is_some() {
        warnings.push("An exit signal remained unfilled at the end of the period".to_string());
    }

    let metrics = compute_metrics(
        &equity_curve,
        &trades,
        cfg.initial_capital,
        cfg.risk_free_rate_pct,
    );
    let windows = compute_windows(&equity_curve, &trades, cfg.start, cfg.end);
    let comparisons = build_comparisons(data, cfg, &days);
    let mut relative = Vec::new();
    for comparison in &comparisons {
        if comparison.label == "SPY" || comparison.label == "QQQ" {
            relative.push(relative_metrics(
                &equity_curve,
                &comparison.equity,
                &comparison.label,
                cfg.risk_free_rate_pct,
            ));
        }
    }

    Ok(BacktestResult {
        label: cfg.label.clone(),
        start: cfg.start,
        end: cfg.end,
        initial_capital: cfg.initial_capital,
        final_equity: metrics.final_equity,
        horizon: cfg.horizon,
        strategy: cfg.strategy,
        risk: cfg.risk,
        params,
        fees: cfg.fees,
        metrics,
        windows,
        equity: equity_curve,
        trades,
        decisions,
        comparisons,
        relative,
        warnings,
        data_note: format!(
            "Long-only, one position; signals at close, fills at next open; {:.1} bps spread, {:.1} bps slippage, {:.1} bps commission; adjusted daily bars; ATR trailing stop {:.1}x; max hold {} sessions",
            cfg.fees.spread_bps,
            cfg.fees.slippage_bps,
            cfg.fees.commission_bps,
            params.atr_stop_mult,
            max_hold
        ),
        universe_size: data.universe.len(),
        trading_days: days.len(),
        elapsed_ms: started.elapsed().as_millis(),
    })
}

fn signal_at(
    data: &PreparedData,
    symbol: &str,
    day: NaiveDate,
    params: HorizonParams,
    cfg: EvalConfig,
) -> Option<Signal> {
    let series = data.series.get(symbol)?;
    let i = series.dates.binary_search(&day).ok()?;
    evaluate_fast(symbol, series, i, params, cfg).ok()
}

#[allow(clippy::too_many_arguments)]
fn close_position(
    pos: OpenPosition,
    exit_date: NaiveDate,
    day_idx: usize,
    base_price: f64,
    reason: &str,
    fees: &FeeModel,
    cash: &mut f64,
    trades: &mut Vec<Trade>,
) {
    let exit_fill = fees.sell_fill(base_price);
    if exit_fill <= 0.0 {
        return;
    }
    let exit_notional = pos.shares * exit_fill;
    let exit_fees = fees.sell_fees(exit_notional, pos.shares);
    *cash += exit_notional - exit_fees;
    let gross = pos.shares * (exit_fill - pos.entry_price);
    let net = gross - pos.entry_fee - exit_fees;
    let cost_basis = pos.shares * pos.entry_price + pos.entry_fee;
    let hold_days = day_idx.saturating_sub(pos.entry_index) + 1;
    trades.push(Trade {
        symbol: pos.symbol,
        entry_date: pos.entry_date,
        exit_date,
        entry_price: pos.entry_price,
        exit_price: exit_fill,
        shares: pos.shares,
        entry_notional: pos.shares * pos.entry_price,
        gross_pnl: gross,
        fees: pos.entry_fee + exit_fees,
        implicit_costs: pos.implicit_entry + fees.implicit_cost(exit_notional),
        net_pnl: net,
        net_return_pct: if cost_basis > 0.0 {
            net / cost_basis * 100.0
        } else {
            0.0
        },
        hold_days,
        entry_stop: pos.stop,
        exit_reason: reason.to_string(),
        entry_score: pos.entry_score,
        entry_rating: pos.entry_rating,
    });
}

fn build_comparisons(
    data: &PreparedData,
    cfg: &BacktestConfig,
    days: &[NaiveDate],
) -> Vec<ComparisonResult> {
    let mut out = Vec::new();
    for symbol in &data.benchmarks {
        if let Some(series) = data.series.get(symbol) {
            let equity = buy_hold_curve(series, days, cfg.initial_capital, &cfg.fees);
            let metrics =
                compute_metrics(&equity, &[], cfg.initial_capital, cfg.risk_free_rate_pct);
            let windows = compute_windows(&equity, &[], cfg.start, cfg.end);
            out.push(ComparisonResult {
                label: symbol.clone(),
                equity,
                metrics,
                windows,
            });
        }
    }
    let universe_curve = equal_weight_curve(data, days, cfg.initial_capital, &cfg.fees);
    let metrics = compute_metrics(
        &universe_curve,
        &[],
        cfg.initial_capital,
        cfg.risk_free_rate_pct,
    );
    let windows = compute_windows(&universe_curve, &[], cfg.start, cfg.end);
    out.push(ComparisonResult {
        label: "Buy&Hold Universe".to_string(),
        equity: universe_curve,
        metrics,
        windows,
    });
    let cash_curve = cash_curve(days, cfg.initial_capital);
    out.push(ComparisonResult {
        label: "Cash".to_string(),
        equity: cash_curve.clone(),
        metrics: compute_metrics(&cash_curve, &[], cfg.initial_capital, 0.0),
        windows: compute_windows(&cash_curve, &[], cfg.start, cfg.end),
    });
    out
}

pub fn buy_hold_curve(
    series: &Series,
    days: &[NaiveDate],
    capital: f64,
    fees: &FeeModel,
) -> Vec<EquityPoint> {
    let Some(first_pos) = days
        .iter()
        .position(|d| series.dates.binary_search(d).is_ok())
    else {
        return cash_curve(days, capital);
    };
    let first_date = days[first_pos];
    let entry_i = series
        .dates
        .binary_search(&first_date)
        .expect("first day exists");
    let fill = fees.buy_fill(series.open[entry_i]);
    if fill <= 0.0 {
        return cash_curve(days, capital);
    }
    let shares = capital / (fill * (1.0 + fees.commission_bps / 10_000.0));
    let notional = shares * fill;
    let mut cash = capital - notional - fees.buy_commission(notional);
    let mut last_close = fill;
    let mut points = Vec::with_capacity(days.len());
    let mut peak = capital;
    for day in days {
        if let Ok(i) = series.dates.binary_search(day) {
            last_close = series.close[i];
        }
        let equity = cash + shares * last_close;
        peak = peak.max(equity);
        points.push(EquityPoint {
            date: *day,
            equity,
            cash,
            position_value: shares * last_close,
            drawdown_pct: if peak > 0.0 {
                (peak - equity) / peak * 100.0
            } else {
                0.0
            },
        });
    }
    if let Some(last) = points.last_mut() {
        let exit_notional = shares * fees.sell_fill(last_close);
        let exit_fees = fees.sell_fees(exit_notional, shares);
        cash += exit_notional - exit_fees;
        last.equity = cash;
        last.cash = cash;
        last.position_value = 0.0;
    }
    points
}

pub fn equal_weight_curve(
    data: &PreparedData,
    days: &[NaiveDate],
    capital: f64,
    fees: &FeeModel,
) -> Vec<EquityPoint> {
    let symbols: Vec<&String> = data
        .universe
        .iter()
        .filter(|s| data.series.contains_key(*s))
        .collect();
    if symbols.is_empty() {
        return cash_curve(days, capital);
    }
    let per_symbol = capital / symbols.len() as f64;
    struct Holding {
        symbol: String,
        shares: f64,
        last_close: f64,
        entered: bool,
    }
    let mut holdings: Vec<Holding> = symbols
        .iter()
        .map(|s| Holding {
            symbol: (*s).clone(),
            shares: 0.0,
            last_close: 0.0,
            entered: false,
        })
        .collect();
    let mut cash = capital;
    let mut points = Vec::with_capacity(days.len());
    let mut peak = capital;
    for day in days {
        for holding in holdings.iter_mut() {
            let series = data.series(&holding.symbol).expect("series");
            if let Ok(i) = series.dates.binary_search(day) {
                holding.last_close = series.close[i];
                if !holding.entered {
                    let fill = fees.buy_fill(series.open[i]);
                    if fill > 0.0 {
                        let shares = per_symbol / (fill * (1.0 + fees.commission_bps / 10_000.0));
                        let notional = shares * fill;
                        cash -= notional + fees.buy_commission(notional);
                        holding.shares = shares;
                        holding.entered = true;
                    }
                }
            }
        }
        let position_value: f64 = holdings
            .iter()
            .filter(|h| h.entered)
            .map(|h| h.shares * h.last_close)
            .sum();
        let equity = cash + position_value;
        peak = peak.max(equity);
        points.push(EquityPoint {
            date: *day,
            equity,
            cash,
            position_value,
            drawdown_pct: if peak > 0.0 {
                (peak - equity) / peak * 100.0
            } else {
                0.0
            },
        });
    }
    if let Some(last) = points.last_mut() {
        for holding in &holdings {
            if holding.entered && holding.last_close > 0.0 {
                let exit_notional = holding.shares * fees.sell_fill(holding.last_close);
                cash += exit_notional - fees.sell_fees(exit_notional, holding.shares);
            }
        }
        last.equity = cash;
        last.cash = cash;
        last.position_value = 0.0;
    }
    points
}

pub fn cash_curve(days: &[NaiveDate], capital: f64) -> Vec<EquityPoint> {
    days.iter()
        .map(|d| EquityPoint {
            date: *d,
            equity: capital,
            cash: capital,
            position_value: 0.0,
            drawdown_pct: 0.0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::synthetic::synthetic_universe;
    use crate::model::{Horizon, RiskProfile};
    use chrono::Duration;

    fn cfg(start: NaiveDate, end: NaiveDate) -> BacktestConfig {
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
            label: "test".to_string(),
        }
    }

    #[test]
    fn runs_on_synthetic_and_never_negative_equity() {
        let start = NaiveDate::from_ymd_opt(2023, 11, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
        let mut symbols = vec!["AAA".to_string(), "BBB".to_string(), "CCC".to_string()];
        let warm = start - Duration::days(420);
        let mut bars = synthetic_universe(&symbols, warm, end);
        symbols.push("SPY".to_string());
        for s in ["SPY", "QQQ"] {
            let b = synthetic_universe(&[s.to_string()], warm, end);
            bars.extend(b);
        }
        let data = PreparedData::new(&bars, &symbols[..3], &["SPY".into(), "QQQ".into()]).unwrap();
        let result = run(&data, &cfg(start, end), Horizon::Medium.params()).unwrap();
        assert!(!result.equity.is_empty());
        assert!(result.equity.iter().all(|p| p.equity >= -1e-9));
        assert_eq!(result.comparisons.len(), 4);
        assert!(result.metrics.final_equity > 0.0);
    }
}
