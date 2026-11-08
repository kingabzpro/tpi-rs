use crate::backtest::types::{EquityPoint, Metrics, RelativeMetrics, Trade, WindowResult};
use chrono::{Months, NaiveDate};

pub fn daily_returns(equity: &[EquityPoint]) -> Vec<f64> {
    let mut out = Vec::with_capacity(equity.len().saturating_sub(1));
    for pair in equity.windows(2) {
        if pair[0].equity > 0.0 {
            let r = pair[1].equity / pair[0].equity - 1.0;
            if r.is_finite() {
                out.push(r);
            }
        }
    }
    out
}

pub fn compute_metrics(
    equity: &[EquityPoint],
    trades: &[Trade],
    initial_capital: f64,
    risk_free_rate_pct: f64,
) -> Metrics {
    let mut m = Metrics {
        initial_equity: initial_capital,
        final_equity: equity.last().map(|p| p.equity).unwrap_or(initial_capital),
        ..Metrics::default()
    };
    if equity.is_empty() || initial_capital <= 0.0 {
        return m;
    }
    m.total_return_pct = (m.final_equity / initial_capital - 1.0) * 100.0;
    let returns = daily_returns(equity);
    let rf_daily = (1.0 + risk_free_rate_pct / 100.0).powf(1.0 / 252.0) - 1.0;

    if returns.len() >= 2 {
        let n = returns.len() as f64;
        let mean = returns.iter().sum::<f64>() / n;
        let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
        let std = variance.sqrt();
        m.annual_volatility_pct = std * 252f64.sqrt() * 100.0;
        if std > 1e-12 {
            m.sharpe = (mean - rf_daily) / std * 252f64.sqrt();
        }
        let downside = (returns
            .iter()
            .map(|r| (r - rf_daily).min(0.0).powi(2))
            .sum::<f64>()
            / n)
            .sqrt();
        if downside > 1e-12 {
            m.sortino = (mean - rf_daily) / downside * 252f64.sqrt();
        }
        m.best_day_pct = returns.iter().copied().fold(f64::NEG_INFINITY, f64::max) * 100.0;
        m.worst_day_pct = returns.iter().copied().fold(f64::INFINITY, f64::min) * 100.0;
    }

    // Drawdown and exposure.
    let mut peak: f64 = initial_capital;
    let mut max_dd: f64 = 0.0;
    let mut exposed = 0usize;
    for point in equity {
        peak = peak.max(point.equity);
        if peak > 0.0 {
            max_dd = max_dd.max((peak - point.equity) / peak);
        }
        if point.position_value > 0.0 {
            exposed += 1;
        }
    }
    m.max_drawdown_pct = max_dd * 100.0;
    m.exposure_pct = exposed as f64 / equity.len() as f64 * 100.0;

    let first_date = equity.first().map(|p| p.date);
    let last_date = equity.last().map(|p| p.date);
    if let (Some(first), Some(last)) = (first_date, last_date) {
        m.period_days = (last - first).num_days().max(1);
        let years = m.period_days as f64 / 365.25;
        if years > 0.0 && m.final_equity > 0.0 {
            m.cagr_pct = ((m.final_equity / initial_capital).powf(1.0 / years) - 1.0) * 100.0;
        }
    }
    if m.max_drawdown_pct > 1e-9 {
        m.calmar = m.cagr_pct / m.max_drawdown_pct;
    }

    m.trades = trades.len();
    m.total_fees = trades.iter().map(|t| t.fees).sum();
    let mut gross_win = 0.0;
    let mut gross_loss = 0.0;
    let mut win_pcts = Vec::new();
    let mut loss_pcts = Vec::new();
    for t in trades {
        if t.net_pnl >= 0.0 {
            m.winning_trades += 1;
            gross_win += t.net_pnl;
            win_pcts.push(t.net_return_pct);
        } else {
            m.losing_trades += 1;
            gross_loss += -t.net_pnl;
            loss_pcts.push(t.net_return_pct);
        }
    }
    if m.trades > 0 {
        m.win_rate_pct = m.winning_trades as f64 / m.trades as f64 * 100.0;
        m.profit_factor = if gross_loss > 1e-9 {
            gross_win / gross_loss
        } else if gross_win > 0.0 {
            999.0
        } else {
            0.0
        };
        m.avg_win_pct = mean(&win_pcts);
        m.avg_loss_pct = mean(&loss_pcts);
        m.avg_trade_pct = mean(&trades.iter().map(|t| t.net_return_pct).collect::<Vec<_>>());
    }
    m
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

/// Return for [start, end], using the last equity point before `start` as the
/// base so continuous-portfolio windows do not lose the overnight gap.
pub fn window_return_pct(equity: &[EquityPoint], start: NaiveDate, end: NaiveDate) -> f64 {
    let base = equity
        .iter()
        .rev()
        .find(|p| p.date < start)
        .map(|p| p.equity)
        .or_else(|| equity.iter().find(|p| p.date >= start).map(|p| p.equity));
    let final_eq = equity
        .iter()
        .rev()
        .find(|p| p.date <= end)
        .map(|p| p.equity);
    match (base, final_eq) {
        (Some(b), Some(f)) if b > 0.0 => (f / b - 1.0) * 100.0,
        _ => 0.0,
    }
}

pub fn compute_windows(
    equity: &[EquityPoint],
    trades: &[Trade],
    start: NaiveDate,
    end: NaiveDate,
) -> Vec<WindowResult> {
    six_month_windows(start, end)
        .into_iter()
        .map(|(label, w_start, w_end)| {
            let slice: Vec<EquityPoint> = equity
                .iter()
                .filter(|p| p.date >= w_start && p.date <= w_end)
                .cloned()
                .collect();
            let mut peak: f64 = slice.first().map(|p| p.equity).unwrap_or(0.0);
            let mut max_dd: f64 = 0.0;
            for p in &slice {
                peak = peak.max(p.equity);
                if peak > 0.0 {
                    max_dd = max_dd.max((peak - p.equity) / peak);
                }
            }
            let window_trades: Vec<&Trade> = trades
                .iter()
                .filter(|t| t.exit_date >= w_start && t.exit_date <= w_end)
                .collect();
            let wins = window_trades.iter().filter(|t| t.net_pnl >= 0.0).count();
            WindowResult {
                label,
                start: w_start,
                end: w_end,
                return_pct: window_return_pct(equity, w_start, w_end),
                max_drawdown_pct: max_dd * 100.0,
                trades: window_trades.len(),
                win_rate_pct: if window_trades.is_empty() {
                    0.0
                } else {
                    wins as f64 / window_trades.len() as f64 * 100.0
                },
                final_equity: slice.last().map(|p| p.equity).unwrap_or(0.0),
            }
        })
        .collect()
}

pub fn six_month_windows(start: NaiveDate, end: NaiveDate) -> Vec<(String, NaiveDate, NaiveDate)> {
    let mut out = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let next = cursor
            .checked_add_months(Months::new(6))
            .unwrap_or(end)
            .min(end);
        out.push((format!("{} .. {}", cursor, next), cursor, next));
        cursor = next;
        if out.len() >= 12 {
            break;
        }
    }
    if out.is_empty() {
        out.push((format!("{start} .. {end}"), start, end));
    }
    out
}

/// Alpha/beta versus a benchmark, using aligned daily equity curves.
pub fn relative_metrics(
    equity: &[EquityPoint],
    benchmark: &[EquityPoint],
    benchmark_name: &str,
    risk_free_rate_pct: f64,
) -> RelativeMetrics {
    let mut map: std::collections::HashMap<NaiveDate, f64> = std::collections::HashMap::new();
    for p in benchmark {
        map.insert(p.date, p.equity);
    }
    let aligned: Vec<(f64, f64)> = equity
        .windows(2)
        .filter_map(|pair| {
            let b0 = *map.get(&pair[0].date)?;
            let b1 = *map.get(&pair[1].date)?;
            if pair[0].equity <= 0.0 || b0 <= 0.0 {
                return None;
            }
            Some((pair[1].equity / pair[0].equity - 1.0, b1 / b0 - 1.0))
        })
        .collect();
    if aligned.len() < 3 {
        return RelativeMetrics {
            benchmark: benchmark_name.to_string(),
            alpha_annual_pct: 0.0,
            beta: 0.0,
            correlation: 0.0,
            tracking_error_pct: 0.0,
        };
    }
    let n = aligned.len() as f64;
    let mean_p = aligned.iter().map(|(p, _)| *p).sum::<f64>() / n;
    let mean_b = aligned.iter().map(|(_, b)| *b).sum::<f64>() / n;
    let var_b = aligned
        .iter()
        .map(|(_, b)| (b - mean_b).powi(2))
        .sum::<f64>()
        / (n - 1.0);
    let cov = aligned
        .iter()
        .map(|(p, b)| (p - mean_p) * (b - mean_b))
        .sum::<f64>()
        / (n - 1.0);
    let beta = if var_b > 1e-15 { cov / var_b } else { 0.0 };
    let rf_daily = (1.0 + risk_free_rate_pct / 100.0).powf(1.0 / 252.0) - 1.0;
    let alpha = (mean_p - rf_daily - beta * (mean_b - rf_daily)) * 252.0 * 100.0;
    let std_p = (aligned
        .iter()
        .map(|(p, _)| (p - mean_p).powi(2))
        .sum::<f64>()
        / (n - 1.0))
        .sqrt();
    let std_b = var_b.sqrt();
    let corr = if std_p > 1e-15 && std_b > 1e-15 {
        cov / (std_p * std_b)
    } else {
        0.0
    };
    let tracking = (aligned
        .iter()
        .map(|(p, b)| (p - b - (mean_p - mean_b)).powi(2))
        .sum::<f64>()
        / (n - 1.0))
        .sqrt()
        * 252f64.sqrt()
        * 100.0;
    RelativeMetrics {
        benchmark: benchmark_name.to_string(),
        alpha_annual_pct: alpha,
        beta,
        correlation: corr,
        tracking_error_pct: tracking,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn equity(values: &[f64]) -> Vec<EquityPoint> {
        let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        values
            .iter()
            .enumerate()
            .map(|(i, &v)| EquityPoint {
                date: start + chrono::Days::new(i as u64),
                equity: v,
                cash: v,
                position_value: 0.0,
                drawdown_pct: 0.0,
            })
            .collect()
    }

    #[test]
    fn metrics_basic() {
        let e = equity(&[10_000.0, 10_100.0, 9_900.0, 10_500.0]);
        let m = compute_metrics(&e, &[], 10_000.0, 0.0);
        assert!((m.total_return_pct - 5.0).abs() < 1e-9);
        assert!((m.max_drawdown_pct - (200.0 / 10_100.0 * 100.0)).abs() < 1e-9);
        assert!(m.cagr_pct > 0.0);
        assert_eq!(m.trades, 0);
    }

    #[test]
    fn window_return_uses_prior_equity() {
        let e = equity(&[100.0, 110.0, 121.0, 133.1]);
        let start = e[1].date;
        let end = e[3].date;
        let r = window_return_pct(&e, start, end);
        // Base is the last close before the window (100), so the window
        // includes the overnight gap from day 0 to day 1.
        assert!((r - 33.1).abs() < 1e-9);
    }

    #[test]
    fn six_month_count() {
        let start = NaiveDate::from_ymd_opt(2023, 11, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let w = six_month_windows(start, end);
        assert_eq!(w.len(), 6);
        assert_eq!(w[0].1, start);
        assert_eq!(w[5].2, end);
    }

    #[test]
    fn relative_metrics_of_self_is_beta_one() {
        let e = equity(&[100.0, 101.0, 102.5, 101.8, 103.0]);
        let rel = relative_metrics(&e, &e, "SPY", 0.0);
        assert!((rel.beta - 1.0).abs() < 1e-9);
        assert!((rel.correlation - 1.0).abs() < 1e-9);
        assert!(rel.alpha_annual_pct.abs() < 1e-9);
    }
}
