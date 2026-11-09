//! TPI-inspired deterministic strategy.
//!
//! Core rules: Donchian 40/20 channels, EMA 50/200 trend filter, RSI 14
//! momentum band, ATR 14 volatility filter and stop, volume/ADX confirmation,
//! and a Chandelier-style ATR trailing stop. No ML, no randomness and no
//! future-bar access.

pub mod risk;

use crate::Result;
use crate::indicators::Series;
use crate::indicators::ratings::{rating_fast, technical_rating};
use crate::model::{
    Action, Horizon, HorizonParams, Rating, RatingDetail, RiskProfile, ScoreBreakdown, Signal,
    StrategyMode,
};
use crate::strategy::risk::{PositionPlan, initial_stop, plan_position};

#[derive(Debug, Clone, Copy)]
pub struct EvalConfig {
    pub horizon: Horizon,
    pub strategy: StrategyMode,
    pub risk: RiskProfile,
    pub equity: f64,
    pub cash: f64,
    pub allow_fractional: bool,
    pub require_rating_buy: bool,
    pub volume_ratio_min: f64,
    pub min_history_bars: usize,
    pub long_only: bool,
    /// Annualized volatility target for sizing, percent. Zero keeps risk-budget sizing.
    pub vol_target_pct: f64,
    /// Override for the equity percentage risked per trade.
    pub risk_pct_override: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Explain {
    Full,
    Fast,
}

/// Full evaluation with human-readable reasons and the complete rating panel.
pub fn evaluate(
    symbol: &str,
    series: &Series,
    i: usize,
    params: HorizonParams,
    cfg: EvalConfig,
) -> Result<Signal> {
    evaluate_inner(symbol, series, i, params, cfg, Explain::Full)
}

/// Allocation-light evaluation for backtests and grid searches. Behavioural
/// gates and scores are identical; explanation strings are omitted.
pub fn evaluate_fast(
    symbol: &str,
    series: &Series,
    i: usize,
    params: HorizonParams,
    cfg: EvalConfig,
) -> Result<Signal> {
    evaluate_inner(symbol, series, i, params, cfg, Explain::Fast)
}

fn evaluate_inner(
    symbol: &str,
    series: &Series,
    i: usize,
    params: HorizonParams,
    cfg: EvalConfig,
    explain: Explain,
) -> Result<Signal> {
    let close = *series
        .close
        .get(i)
        .ok_or_else(|| crate::Error::validation(format!("bar index {i} out of range")))?;
    let date = *series
        .dates
        .get(i)
        .ok_or_else(|| crate::Error::validation(format!("date index {i} out of range")))?;
    let rating_detail = if explain == Explain::Full {
        technical_rating(series, i)
    } else {
        RatingDetail::empty()
    };
    let rating = if explain == Explain::Full {
        rating_detail.rating
    } else {
        rating_fast(series, i)
    };
    let snapshot = series.snapshot(i);

    let minimum = cfg.min_history_bars.max(200);
    if i + 1 < minimum {
        return Ok(insufficient_history_signal(
            symbol,
            date,
            cfg,
            rating,
            rating_detail,
            close,
            minimum,
            i + 1,
            explain,
        ));
    }

    let (
        Some(ema50),
        Some(ema200),
        Some(rsi),
        Some(atr),
        Some(atr_pct),
        Some(don20_high),
        Some(_don20_low),
        Some(don40_high),
        Some(don40_low),
        Some(exit_low),
    ) = (
        snapshot.ema50,
        snapshot.ema200,
        snapshot.rsi14,
        snapshot.atr14,
        snapshot.atr_pct,
        snapshot.donchian20_high,
        snapshot.donchian20_low,
        snapshot.donchian40_high,
        snapshot.donchian40_low,
        series.don_exit_low(i, params.exit_period),
    )
    else {
        return Ok(insufficient_history_signal(
            symbol,
            date,
            cfg,
            rating,
            rating_detail,
            close,
            minimum,
            i + 1,
            explain,
        ));
    };

    let ema50_slope = snapshot.ema50_slope_pct.unwrap_or(0.0);
    let macd_hist = snapshot.macd_hist.unwrap_or(0.0);
    let adx = snapshot.adx14.unwrap_or(0.0);
    let plus_di = snapshot.plus_di.unwrap_or(0.0);
    let minus_di = snapshot.minus_di.unwrap_or(0.0);
    let volume_ratio = snapshot.volume_ratio.unwrap_or(0.0);
    let dist_ema20 = snapshot.dist_ema20_atr.unwrap_or(0.0);
    let rsi_prev = if i >= 3 {
        series.rsi14.get(i - 3).copied().flatten()
    } else {
        None
    };

    // --- Composite score ------------------------------------------------------
    let mut trend = 0.0;
    if close > ema50 {
        trend += 15.0;
    }
    if ema50 > ema200 {
        trend += 10.0;
    }
    if close > ema200 {
        trend += 5.0;
    }
    if ema50_slope > 0.0 {
        trend += 5.0;
    }

    let don20_break = close > don20_high;
    let don40_break = close > don40_high;
    let mut breakout = 0.0;
    match cfg.horizon {
        Horizon::Short => {
            if don20_break {
                breakout += 25.0;
            }
        }
        Horizon::Medium => {
            if don20_break {
                breakout += 15.0;
            }
            if close > (don40_high + don40_low) / 2.0 {
                breakout += 10.0;
            }
        }
        Horizon::Long => {
            if don40_break {
                breakout += 25.0;
            } else if don20_break {
                breakout += 10.0;
            }
        }
    }

    let mut momentum = 0.0;
    if (params.rsi_min..=params.rsi_max).contains(&rsi) {
        momentum += 10.0;
    }
    if rsi_prev.is_some_and(|p| rsi > p) {
        momentum += 4.0;
    }
    if macd_hist > 0.0 {
        momentum += 6.0;
    }

    let mut confirmation = 0.0;
    if adx >= 20.0 {
        confirmation += 4.0;
    }
    if plus_di > minus_di {
        confirmation += 3.0;
    }
    if volume_ratio >= cfg.volume_ratio_min {
        confirmation += 3.0;
    }

    let mut volatility = 0.0;
    if (params.min_atr_pct..=params.max_atr_pct).contains(&atr_pct) {
        volatility += 5.0;
    }
    if (-1.0..=3.0).contains(&dist_ema20) {
        volatility += 5.0;
    }

    let mut components = ScoreBreakdown {
        trend,
        breakout,
        momentum,
        confirmation,
        volatility,
    };
    let mut score = components.total();
    if rsi > 80.0 {
        score -= 12.0;
    }
    if atr_pct > params.max_atr_pct {
        score -= 10.0;
    }
    if dist_ema20 > 3.0 {
        score -= 5.0;
    }
    if close < ema200 {
        score -= 10.0;
    }
    score = score.clamp(0.0, 100.0);
    components.trend = components.trend.min(ScoreBreakdown::TREND_MAX);
    components.breakout = components.breakout.min(ScoreBreakdown::BREAKOUT_MAX);
    components.momentum = components.momentum.min(ScoreBreakdown::MOMENTUM_MAX);
    components.confirmation = components
        .confirmation
        .min(ScoreBreakdown::CONFIRMATION_MAX);
    components.volatility = components.volatility.min(ScoreBreakdown::VOLATILITY_MAX);

    // --- Hard gates -----------------------------------------------------------
    let trend_gate = close > ema200 && ema50 > ema200;
    let breakout_gate = match cfg.horizon {
        Horizon::Short => don20_break,
        Horizon::Medium => don20_break && close > (don40_high + don40_low) / 2.0,
        Horizon::Long => don40_break,
    };
    let rsi_gate = (params.rsi_min..=params.rsi_max).contains(&rsi);
    let vol_gate = (params.min_atr_pct..=params.max_atr_pct).contains(&atr_pct);
    let rating_gate = match cfg.strategy {
        StrategyMode::Consensus if cfg.require_rating_buy => rating.is_buy(),
        StrategyMode::Consensus => !rating.is_sell(),
        StrategyMode::Tpi | StrategyMode::Compare => true,
    };
    let tpi_gates = trend_gate && breakout_gate && rsi_gate && vol_gate;
    let setup_ok = tpi_gates && score >= params.buy_threshold;

    let sell_reason = if close < exit_low {
        Some(format!(
            "close {close:.2} below prior {}-day low {exit_low:.2}",
            params.exit_period
        ))
    } else if close < ema200 && ema50 < ema200 {
        Some("close and EMA50 are below EMA200 (bearish trend)".to_string())
    } else if cfg.strategy == StrategyMode::Consensus && rating.is_sell() && close < ema50 {
        Some(format!(
            "technical rating is {rating} with close below EMA50"
        ))
    } else {
        None
    };

    let action = if setup_ok && rating_gate {
        Action::Buy
    } else if sell_reason.is_some() {
        Action::Sell
    } else {
        Action::Hold
    };
    let strength = match action {
        Action::Buy => score,
        Action::Sell => 100.0 - score,
        Action::Hold => score,
    }
    .clamp(0.0, 100.0);

    let stop = initial_stop(close, atr, params.atr_stop_mult);
    let target = close + 2.0 * params.atr_stop_mult * atr;
    let risk_per_share = (close - stop).max(0.0);
    let stop_distance_pct = if close > 0.0 {
        risk_per_share / close * 100.0
    } else {
        0.0
    };
    let plan = if action == Action::Buy {
        plan_position(
            cfg.equity,
            cfg.cash,
            close,
            stop,
            atr,
            cfg.risk
                .params()
                .with_overrides(cfg.risk_pct_override, cfg.vol_target_pct),
            cfg.allow_fractional,
        )
    } else {
        PositionPlan::empty()
    };

    let (reasons, warnings) = if explain == Explain::Full {
        build_explanations(&ExplainCtx {
            close,
            ema50,
            ema200,
            ema50_slope,
            don20_high,
            don20_break,
            don40_break,
            don40_high,
            don40_low,
            exit_low,
            rsi,
            macd_hist,
            adx,
            plus_di,
            minus_di,
            volume_ratio,
            atr_pct,
            dist_ema20,
            rating,
            rating_detail: &rating_detail,
            action,
            score,
            tpi_gates,
            trend_gate,
            breakout_gate,
            rsi_gate,
            vol_gate,
            rating_gate,
            params,
            cfg,
            plan,
            stop,
            target,
            stop_distance_pct,
            sell_triggered: sell_reason.is_some(),
        })
    } else {
        (Vec::new(), Vec::new())
    };

    Ok(Signal {
        symbol: symbol.to_string(),
        as_of: date,
        horizon: cfg.horizon,
        strategy: cfg.strategy,
        action,
        strength,
        rating,
        rating_detail,
        price: close,
        indicators: snapshot,
        components,
        entry: close,
        stop,
        target,
        stop_distance_pct,
        risk_per_share,
        risk_amount: plan.risk_amount,
        suggested_shares: plan.shares,
        suggested_notional: plan.notional,
        reasons,
        warnings,
        stale: false,
    })
}

struct ExplainCtx<'a> {
    close: f64,
    ema50: f64,
    ema200: f64,
    ema50_slope: f64,
    don20_high: f64,
    don20_break: bool,
    don40_break: bool,
    don40_high: f64,
    don40_low: f64,
    exit_low: f64,
    rsi: f64,
    macd_hist: f64,
    adx: f64,
    plus_di: f64,
    minus_di: f64,
    volume_ratio: f64,
    atr_pct: f64,
    dist_ema20: f64,
    rating: Rating,
    rating_detail: &'a RatingDetail,
    action: Action,
    score: f64,
    tpi_gates: bool,
    trend_gate: bool,
    breakout_gate: bool,
    rsi_gate: bool,
    vol_gate: bool,
    rating_gate: bool,
    params: HorizonParams,
    cfg: EvalConfig,
    plan: PositionPlan,
    stop: f64,
    target: f64,
    stop_distance_pct: f64,
    sell_triggered: bool,
}

fn build_explanations(c: &ExplainCtx<'_>) -> (Vec<String>, Vec<String>) {
    let mut reasons = Vec::with_capacity(10);
    reasons.push(format!(
        "Trend: close {:.2} vs EMA50 {:.2} / EMA200 {:.2}; EMA50 slope {:+.2}% over 10 bars",
        c.close, c.ema50, c.ema200, c.ema50_slope
    ));
    reasons.push(if c.don20_break {
        format!(
            "Breakout: close {:.2} is above the prior 20-day high {:.2}{}",
            c.close,
            c.don20_high,
            if c.don40_break {
                format!(" and prior 40-day high {:.2}", c.don40_high)
            } else {
                String::new()
            }
        )
    } else {
        format!(
            "No breakout: close {:.2} vs prior 20-day high {:.2} / 40-day high {:.2}",
            c.close, c.don20_high, c.don40_high
        )
    });
    reasons.push(format!(
        "Momentum: RSI14 {:.1} (band {:.0}-{:.0}); MACD histogram {:+.3}",
        c.rsi, c.params.rsi_min, c.params.rsi_max, c.macd_hist
    ));
    reasons.push(format!(
        "Confirmation: ADX14 {:.1}, +DI {:.1} vs -DI {:.1}, volume {:.2}x 20-day average",
        c.adx, c.plus_di, c.minus_di, c.volume_ratio
    ));
    reasons.push(format!(
        "Volatility: ATR14 {:.2}% of price (filter {:.1}-{:.1}%)",
        c.atr_pct, c.params.min_atr_pct, c.params.max_atr_pct
    ));
    reasons.push(format!(
        "Technical rating: {} ({}, score {:+.2})",
        c.rating,
        rating_breakdown(c.rating_detail),
        c.rating_detail.score
    ));
    reasons.push(format!(
        "Entry reference {:.2}; stop {:.2} ({:.2}% risk); 2R target {:.2}",
        c.close, c.stop, c.stop_distance_pct, c.target
    ));
    reasons.push(format!(
        "Exit rules: close below prior {}-day low {:.2}, {:.1}x ATR trailing stop, or {} sessions",
        c.params.exit_period, c.exit_low, c.params.atr_stop_mult, c.params.max_hold_days
    ));
    if c.action == Action::Buy {
        reasons.push(format!(
            "Size: {:.4} shares (${:.2}); risk ${:.2} = {:.2}% of ${:.0} equity",
            c.plan.shares,
            c.plan.notional,
            c.plan.risk_amount,
            c.cfg.risk.params().risk_per_trade_pct,
            c.cfg.equity
        ));
    }

    let mut warnings = Vec::with_capacity(8);
    if !c.trend_gate {
        warnings.push(format!(
            "No BUY: trend filter requires close > EMA200 and EMA50 > EMA200 (close {:.2}, EMA50 {:.2}, EMA200 {:.2})",
            c.close, c.ema50, c.ema200
        ));
    }
    if !c.breakout_gate {
        warnings.push(match c.cfg.horizon {
            Horizon::Short => format!(
                "No BUY: {}-day Donchian breakout not confirmed (needs close > {:.2})",
                c.params.breakout_period, c.don20_high
            ),
            Horizon::Medium => format!(
                "No BUY: 20-day breakout and 40-day channel confirmation required (close {:.2}, 40-day mid {:.2})",
                c.close,
                (c.don40_high + c.don40_low) / 2.0
            ),
            Horizon::Long => format!(
                "No BUY: {}-day Donchian breakout not confirmed (needs close > {:.2})",
                c.params.breakout_period, c.don40_high
            ),
        });
    }
    if !c.rsi_gate {
        warnings.push(format!(
            "No BUY: RSI14 {:.1} outside {:.0}-{:.0}",
            c.rsi, c.params.rsi_min, c.params.rsi_max
        ));
    }
    if !c.vol_gate {
        warnings.push(format!(
            "No BUY: ATR14 {:.2}% outside {:.1}-{:.1}%",
            c.atr_pct, c.params.min_atr_pct, c.params.max_atr_pct
        ));
    }
    if c.tpi_gates && !c.rating_gate {
        warnings.push(format!(
            "No BUY: TPI setup qualifies but technical rating is {}, below the consensus Buy gate",
            c.rating
        ));
    }
    if c.score < c.params.buy_threshold && c.tpi_gates && c.rating_gate {
        warnings.push(format!(
            "No BUY: composite score {:.1} below threshold {:.1}",
            c.score, c.params.buy_threshold
        ));
    }
    if c.rsi > 80.0 {
        warnings.push("Caution: RSI above 80 (overbought); momentum can reverse".to_string());
    }
    if c.atr_pct > c.params.max_atr_pct {
        warnings.push("Caution: volatility above the strategy ceiling".to_string());
    }
    if c.dist_ema20 > 3.0 {
        warnings.push("Caution: price is more than 3 ATR above EMA20 (extended)".to_string());
    }
    if c.volume_ratio < 0.8 {
        warnings.push(format!(
            "Caution: volume {:.2}x average, weak participation",
            c.volume_ratio
        ));
    }
    if c.cfg.long_only && c.action == Action::Sell {
        warnings.push(
            "SELL is an exit/flat signal: this build does not short individual stocks".to_string(),
        );
    }
    if c.sell_triggered && c.cfg.strategy == StrategyMode::Consensus {
        warnings.push("Exit signal triggered by TPI and/or rating deterioration".to_string());
    }
    warnings.push(
        "Signals describe rule-based conditions, not a forecast of future prices".to_string(),
    );
    (reasons, warnings)
}

#[allow(clippy::too_many_arguments)]
fn insufficient_history_signal(
    symbol: &str,
    date: chrono::NaiveDate,
    cfg: EvalConfig,
    rating: Rating,
    rating_detail: RatingDetail,
    close: f64,
    needed: usize,
    have: usize,
    explain: Explain,
) -> Signal {
    let (reasons, warnings) = if explain == Explain::Full {
        (
            vec![format!(
                "Insufficient history: {have} bars available, {needed} required for EMA200/Donchian warmup"
            )],
            vec!["No signal produced until the warmup completes".to_string()],
        )
    } else {
        (Vec::new(), Vec::new())
    };
    Signal {
        symbol: symbol.to_string(),
        as_of: date,
        horizon: cfg.horizon,
        strategy: cfg.strategy,
        action: Action::Hold,
        strength: 0.0,
        rating,
        rating_detail,
        price: close,
        indicators: crate::model::IndicatorSnapshot::empty(close),
        components: ScoreBreakdown::default(),
        entry: close,
        stop: 0.0,
        target: 0.0,
        stop_distance_pct: 0.0,
        risk_per_share: 0.0,
        risk_amount: 0.0,
        suggested_shares: 0.0,
        suggested_notional: 0.0,
        reasons,
        warnings,
        stale: false,
    }
}

pub fn rating_breakdown(detail: &RatingDetail) -> String {
    format!(
        "{} buy / {} neutral / {} sell",
        detail.buy_votes, detail.neutral_votes, detail.sell_votes
    )
}

/// Text used by reports and the TUI.
pub fn exit_policy(params: HorizonParams) -> String {
    format!(
        "Close below prior {}-day low; {:.1}x ATR trailing stop; max hold {} sessions",
        params.exit_period, params.atr_stop_mult, params.max_hold_days
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Bar;
    use chrono::NaiveDate;

    fn bars_with_breakout() -> Vec<Bar> {
        let mut bars: Vec<Bar> = Vec::new();
        let start = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let mut last_close = 80.0;
        for i in 0..260 {
            // Mild uptrend with a small 7-bar wave, then a 1.2-point breakout
            // above the prior 20-day high. This keeps RSI ~65 and price < 3 ATR
            // above EMA20, unlike a single parabolic jump.
            let base = 80.0 + 0.05 * i as f64;
            let wave = if i % 7 < 3 { 0.5 } else { -0.5 };
            let price = if i == 259 {
                last_close + 1.2
            } else {
                base + wave
            };
            let open = price - 0.05;
            let high = price + 0.3;
            let low = price - 0.35;
            bars.push(Bar {
                date: start + chrono::Days::new(i as u64),
                open,
                high,
                low,
                close: price,
                volume: if i == 259 { 3_000_000.0 } else { 1_000_000.0 },
            });
            last_close = price;
        }
        bars
    }

    fn cfg() -> EvalConfig {
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
            vol_target_pct: 0.0,
            risk_pct_override: None,
        }
    }

    #[test]
    fn breakout_in_uptrend_is_buy_with_stop_below_entry() {
        let bars = bars_with_breakout();
        let series = Series::compute(&bars);
        let signal = evaluate(
            "TEST",
            &series,
            bars.len() - 1,
            Horizon::Medium.params(),
            cfg(),
        )
        .unwrap();
        assert_eq!(signal.action, Action::Buy, "{:?}", signal.warnings);
        assert!(signal.stop < signal.entry);
        assert!(signal.target > signal.entry);
        assert!(signal.suggested_notional <= 10_000.0);
        assert!(signal.suggested_notional <= 7_500.0 + 1e-6);
        assert!((0.0..=100.0).contains(&signal.strength));
    }

    #[test]
    fn fast_and_full_agree_on_action() {
        let bars = bars_with_breakout();
        let series = Series::compute(&bars);
        let i = bars.len() - 1;
        let full = evaluate("TEST", &series, i, Horizon::Medium.params(), cfg()).unwrap();
        let fast = evaluate_fast("TEST", &series, i, Horizon::Medium.params(), cfg()).unwrap();
        assert_eq!(full.action, fast.action);
        assert!((full.strength - fast.strength).abs() < 1e-9);
        assert!(fast.reasons.is_empty());
    }

    #[test]
    fn no_lookahead_when_future_bars_are_appended() {
        let bars = bars_with_breakout();
        let series = Series::compute(&bars);
        let i = 250;
        let a = evaluate("TEST", &series, i, Horizon::Medium.params(), cfg()).unwrap();
        let truncated = &bars[..=i];
        let series2 = Series::compute(truncated);
        let b = evaluate("TEST", &series2, i, Horizon::Medium.params(), cfg()).unwrap();
        assert_eq!(a.action, b.action);
        assert_eq!(a.strength, b.strength);
        assert_eq!(a.indicators.rsi14, b.indicators.rsi14);
    }

    #[test]
    fn insufficient_history_holds() {
        let bars = bars_with_breakout()[..50].to_vec();
        let series = Series::compute(&bars);
        let signal = evaluate(
            "TEST",
            &series,
            bars.len() - 1,
            Horizon::Medium.params(),
            cfg(),
        )
        .unwrap();
        assert_eq!(signal.action, Action::Hold);
        assert_eq!(signal.stop, 0.0);
    }

    #[test]
    fn consensus_gate_can_veto_tpi() {
        let bars = bars_with_breakout();
        let series = Series::compute(&bars);
        let mut c = cfg();
        c.strategy = StrategyMode::Consensus;
        let signal =
            evaluate("TEST", &series, bars.len() - 1, Horizon::Medium.params(), c).unwrap();
        if !signal.rating.is_buy() {
            assert_eq!(signal.action, Action::Hold);
        }
    }
}
