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
pub fn plan_position(
    equity: f64,
    cash: f64,
    entry: f64,
    stop: f64,
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
    let risk_budget = equity * (risk.risk_per_trade_pct / 100.0).max(0.0);
    let max_notional = equity * (risk.max_position_pct / 100.0).clamp(0.0, 100.0);

    let mut shares = (risk_budget / risk_per_share)
        .min(max_notional / entry)
        .min(cash.max(0.0) / entry);
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

    #[test]
    fn risk_sizing_respects_risk_budget_and_cap() {
        let plan = plan_position(
            10_000.0,
            10_000.0,
            100.0,
            95.0,
            RiskParams {
                risk_per_trade_pct: 1.0,
                max_position_pct: 75.0,
            },
            true,
        );
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
            RiskParams {
                risk_per_trade_pct: 1.0,
                max_position_pct: 50.0,
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
            RiskParams {
                risk_per_trade_pct: 1.0,
                max_position_pct: 50.0,
            },
            true,
        );
        assert_eq!(plan.shares, 0.0);
    }
}
