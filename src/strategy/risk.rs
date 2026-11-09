use crate::model::RiskParams;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionPlan {
    pub shares: f64,
    pub notional: f64,
    pub risk_amount: f64,
    pub risk_per_share: f64,
    pub stop_distance_pct: f64,
}

impl PositionPlan {
    pub fn empty() -> Self {
        Self {
            shares: 0.0,
            notional: 0.0,
            risk_amount: 0.0,
            risk_per_share: 0.0,
            stop_distance_pct: 0.0,
        }
    }
}

/// Risk-based sizing:
/// `shares = min(cash / entry, equity * max_position_pct / entry,
///              equity * risk_per_trade_pct / (entry - stop))`.
/// The stop distance therefore drives size, not notional allocation.
///
/// With `risk.vol_target_pct > 0` the dollar risk budget is replaced by a
/// volatility target: `notional = equity * vol_target_pct / annualized ATR%`.
/// Per-trade loss then scales with stop distance instead of staying fixed.
pub fn plan_position(
    equity: f64,
    cash: f64,
    entry: f64,
    stop: f64,
    atr: f64,
    risk: RiskParams,
    allow_fractional: bool,
) -> PositionPlan {
    if !equity.is_finite()
        || !entry.is_finite()
        || !stop.is_finite()
        || equity <= 0.0
        || entry <= 0.0
        || stop <= 0.0
        || stop >= entry
    {
        return PositionPlan::empty();
    }
    let risk_per_share = entry - stop;
    let stop_distance_pct = risk_per_share / entry * 100.0;
    let max_notional = equity * (risk.max_position_pct / 100.0).clamp(0.0, 100.0);

    let max_shares = (max_notional / entry).min(cash.max(0.0) / entry);
    let risk_budget_shares = equity * (risk.risk_per_trade_pct / 100.0).max(0.0) / risk_per_share;
    // ATR% approximates daily range, so annualize it to compare with the target.
    let annual_vol_pct = if atr.is_finite() && atr > 0.0 {
        atr / entry * 100.0 * 252.0_f64.sqrt()
    } else {
        0.0
    };
    let vt_shares = if risk.vol_target_pct > 0.0 && annual_vol_pct > 0.0 {
        equity * risk.vol_target_pct / annual_vol_pct / entry
    } else {
        f64::INFINITY
    };

    let mut shares = if vt_shares.is_finite() && risk.vol_target_pct > 0.0 {
        vt_shares.min(max_shares)
    } else {
        risk_budget_shares.min(max_shares)
    };
    if !allow_fractional {
        shares = shares.floor();
    }
    if !shares.is_finite() || shares <= 0.0 {
        return PositionPlan {
            risk_per_share,
            stop_distance_pct,
            ..PositionPlan::empty()
        };
    }
    let notional = shares * entry;
    PositionPlan {
        shares,
        notional,
        risk_amount: shares * risk_per_share,
        risk_per_share,
        stop_distance_pct,
    }
}

/// Initial stop below entry by `atr_mult` ATRs.
pub fn initial_stop(entry: f64, atr: f64, atr_mult: f64) -> f64 {
    (entry - atr_mult * atr).max(0.01)
}

/// Chandelier-style trailing stop: highest close since entry minus `atr_mult` ATR.
pub fn trailing_stop(highest_close: f64, atr: f64, atr_mult: f64) -> f64 {
    (highest_close - atr_mult * atr).max(0.01)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn balanced() -> RiskParams {
        RiskParams {
            risk_per_trade_pct: 1.0,
            max_position_pct: 75.0,
            vol_target_pct: 0.0,
        }
    }

    #[test]
    fn risk_sizing_respects_risk_budget_and_cap() {
        let plan = plan_position(10_000.0, 10_000.0, 100.0, 95.0, 2.0, balanced(), true);
        // Risk budget $100 / $5 = 20 shares = $2,000, below the $7,500 cap.
        assert!((plan.shares - 20.0).abs() < 1e-9);
        assert!((plan.risk_amount - 100.0).abs() < 1e-9);
        assert!((plan.stop_distance_pct - 5.0).abs() < 1e-9);
    }

    #[test]
    fn notional_cap_binds_for_tight_stop() {
        let plan = plan_position(
            10_000.0,
            10_000.0,
            100.0,
            99.9,
            2.0,
            RiskParams {
                risk_per_trade_pct: 1.0,
                max_position_pct: 50.0,
                vol_target_pct: 0.0,
            },
            true,
        );
        assert!((plan.notional - 5_000.0).abs() < 1e-9);
    }

    #[test]
    fn invalid_stop_returns_empty() {
        let plan = plan_position(
            10_000.0,
            10_000.0,
            100.0,
            100.0,
            2.0,
            RiskParams {
                risk_per_trade_pct: 1.0,
                max_position_pct: 50.0,
                vol_target_pct: 0.0,
            },
            true,
        );
        assert_eq!(plan.shares, 0.0);
    }

    #[test]
    fn vol_target_sizes_from_annualized_atr() {
        // ATR 2% of price annualizes to about 31.7%, so a 10% target holds 31.5% notional.
        let plan = plan_position(
            10_000.0,
            10_000.0,
            100.0,
            95.0,
            2.0,
            RiskParams {
                risk_per_trade_pct: 1.0,
                max_position_pct: 100.0,
                vol_target_pct: 10.0,
            },
            true,
        );
        let expected = 10_000.0 * 10.0 / (2.0 * 252.0_f64.sqrt());
        assert!((plan.notional - expected).abs() < 1e-6);
        assert!(
            plan.notional > 20.0 * 100.0,
            "vol target deploys more than risk budget"
        );
        assert!(
            plan.risk_amount > 100.0,
            "per-trade risk scales with stop distance"
        );
    }
}
