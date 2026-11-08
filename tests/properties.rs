//! Property-based invariants for indicators, ratings, evaluation and sizing.

use chrono::{Days, NaiveDate};
use proptest::prelude::*;
use tpi_rs::indicators::Series;
use tpi_rs::indicators::ratings::{rating_fast, technical_rating};
use tpi_rs::model::{Action, Bar, Horizon, RiskProfile, StrategyMode};
use tpi_rs::strategy::risk::plan_position;
use tpi_rs::strategy::{EvalConfig, evaluate_fast};

fn make_bars(returns: &[f64], wick: f64) -> Vec<Bar> {
    let start = NaiveDate::from_ymd_opt(2023, 1, 2).unwrap();
    let mut close = 100.0f64;
    returns
        .iter()
        .enumerate()
        .map(|(i, r)| {
            close = (close * (1.0 + r.clamp(-0.15, 0.15))).max(1.0);
            let open = (close * (1.0 - r * 0.2)).max(0.5);
            let high = open.max(close) * (1.0 + wick.abs().min(0.05));
            let low = (open.min(close) * (1.0 - wick.abs().min(0.05))).max(0.4);
            Bar {
                date: start + Days::new(i as u64),
                open,
                high,
                low,
                close,
                volume: 500_000.0 + (i as f64 * 1000.0),
            }
        })
        .collect()
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

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    #[test]
    fn indicators_are_bounded_and_ratings_match(
        returns in prop::collection::vec(-0.06f64..0.06, 260),
        wick in 0.001f64..0.03,
    ) {
        let bars = make_bars(&returns, wick);
        let series = Series::compute(&bars);
        for i in 0..series.len() {
            if let Some(rsi) = series.rsi14[i] {
                prop_assert!((0.0..=100.0).contains(&rsi), "RSI out of range: {rsi}");
            }
            if let Some(atr) = series.atr14[i] {
                prop_assert!(atr >= 0.0 && atr.is_finite());
            }
            if let Some(stoch) = series.stoch_k[i] {
                prop_assert!((-0.001..=100.001).contains(&stoch));
            }
        }
        let detail = technical_rating(&series, series.len() - 1);
        let fast = rating_fast(&series, series.len() - 1);
        prop_assert_eq!(detail.rating, fast, "fast/detailed rating diverged");
        prop_assert!(detail.score >= -1.0 && detail.score <= 1.0);
    }

    #[test]
    fn evaluation_is_bounded_and_stop_below_entry(
        returns in prop::collection::vec(-0.05f64..0.05, 260),
        wick in 0.001f64..0.03,
    ) {
        let bars = make_bars(&returns, wick);
        let series = Series::compute(&bars);
        let signal = evaluate_fast(
            "PROP",
            &series,
            bars.len() - 1,
            Horizon::Medium.params(),
            eval_cfg(),
        ).unwrap();
        prop_assert!(matches!(signal.action, Action::Buy | Action::Hold | Action::Sell));
        prop_assert!((0.0..=100.0).contains(&signal.strength));
        if signal.indicators.atr14.is_some() {
            prop_assert!(signal.stop <= signal.entry);
            prop_assert!(signal.target >= signal.entry);
        }
        prop_assert!(signal.suggested_notional <= 10_000.0 + 1e-6);
        prop_assert!(signal.suggested_notional <= 7_500.0 + 1e-6);
        prop_assert!(signal.suggested_notional >= 0.0);
    }

    #[test]
    fn future_bars_do_not_change_past_evaluation(
        returns in prop::collection::vec(-0.04f64..0.04, 260),
        split in 230usize..255,
        wick in 0.001f64..0.02,
    ) {
        let bars = make_bars(&returns, wick);
        let full = Series::compute(&bars);
        let short_bars = &bars[..split];
        let truncated = Series::compute(short_bars);
        let i = split - 1;
        let a = evaluate_fast(
            "PROP",
            &full,
            i,
            Horizon::Medium.params(),
            eval_cfg(),
        ).unwrap();
        let b = evaluate_fast(
            "PROP",
            &truncated,
            i,
            Horizon::Medium.params(),
            eval_cfg(),
        ).unwrap();
        prop_assert_eq!(a.action, b.action);
        prop_assert!((a.strength - b.strength).abs() < 1e-9);
        prop_assert_eq!(a.indicators.rsi14, b.indicators.rsi14);
        prop_assert_eq!(a.indicators.atr14, b.indicators.atr14);
        prop_assert_eq!(a.rating, b.rating);
    }

    #[test]
    fn position_sizing_never_exceeds_budget(
        equity in 1000.0f64..1_000_000.0,
        entry in 1.0f64..1000.0,
        stop_pct in 0.1f64..30.0,
        risk_pct in 0.1f64..5.0,
        max_pos in 5.0f64..100.0,
        fractional in any::<bool>(),
    ) {
        let stop = entry * (1.0 - stop_pct / 100.0);
        let plan = plan_position(
            equity,
            equity,
            entry,
            stop,
            tpi_rs::model::RiskParams {
                risk_per_trade_pct: risk_pct,
                max_position_pct: max_pos,
            },
            fractional,
        );
        prop_assert!(plan.shares >= 0.0);
        prop_assert!(plan.notional <= equity * max_pos / 100.0 + 1e-6);
        prop_assert!(plan.risk_amount <= equity * risk_pct / 100.0 + 1e-6);
        if !fractional {
            prop_assert!((plan.shares - plan.shares.round()).abs() < 1e-9);
        }
    }
}
