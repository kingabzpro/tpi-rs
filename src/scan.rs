//! Concurrent symbol analysis: one batched market-data load, then Rayon
//! evaluation of independent symbols. Results preserve input order before the
//! final deterministic ranking.

use crate::config::Config;
use crate::data::{LoadResult, MarketData};
use crate::error::Result;
use crate::indicators::Series;
use crate::model::{Action, Bar, DataProvenance, Horizon, RiskProfile, Signal, StrategyMode};
use crate::strategy::{EvalConfig, evaluate};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolError {
    pub symbol: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanReport {
    pub generated_at: DateTime<Utc>,
    pub horizon: Horizon,
    pub strategy: StrategyMode,
    pub risk: RiskProfile,
    pub equity: f64,
    pub as_of: Option<NaiveDate>,
    pub provenance: DataProvenance,
    pub requested_symbols: usize,
    pub signals: Vec<Signal>,
    pub errors: Vec<SymbolError>,
    pub warnings: Vec<String>,
    pub elapsed_ms: u128,
    #[serde(skip)]
    pub bars: HashMap<String, Vec<Bar>>,
}

impl ScanReport {
    pub fn buys(&self) -> impl Iterator<Item = &Signal> {
        self.signals.iter().filter(|s| s.action == Action::Buy)
    }

    pub fn strongest(&self) -> Option<&Signal> {
        self.signals
            .iter()
            .filter(|s| s.action == Action::Buy)
            .max_by(|a, b| {
                a.strength
                    .partial_cmp(&b.strength)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.symbol.cmp(&a.symbol))
            })
    }

    pub fn for_symbol(&self, symbol: &str) -> Vec<&Signal> {
        self.signals.iter().filter(|s| s.symbol == symbol).collect()
    }
}

pub struct AnalysisOptions<'a> {
    pub horizon: Horizon,
    pub strategy: StrategyMode,
    pub risk: RiskProfile,
    pub equity: f64,
    pub config: &'a Config,
    pub as_of: Option<NaiveDate>,
    pub stale_bar_days: i64,
    /// Cooperative cancellation checked per symbol and around the data load.
    pub cancel: Option<Arc<AtomicBool>>,
}

pub struct Progress<'a> {
    pub on_phase: &'a (dyn Fn(&str, usize, usize) + Send + Sync),
}

impl Progress<'_> {
    fn report(&self, phase: &str, done: usize, total: usize) {
        (self.on_phase)(phase, done, total);
    }
}

/// Analyse symbols over [start, end]. The caller chooses a start early enough
/// for the EMA200/Donchian warmup (recommended: 500 calendar days).
pub async fn analyze_market(
    market: &MarketData,
    symbols: &[String],
    start: NaiveDate,
    end: NaiveDate,
    options: &AnalysisOptions<'_>,
    progress: Option<&Progress<'_>>,
) -> Result<ScanReport> {
    let started = std::time::Instant::now();
    if cancelled(options) {
        return Err(crate::Error::Cancelled);
    }
    if let Some(p) = progress {
        p.report("market data", 0, symbols.len());
    }
    let LoadResult {
        bars,
        provenance,
        warnings,
        missing,
    } = market.load(symbols, start, end).await?;
    if cancelled(options) {
        return Err(crate::Error::Cancelled);
    }
    if let Some(p) = progress {
        p.report("indicator evaluation", 0, symbols.len());
    }

    let mut bars = bars;
    if let Some(cutoff) = options.as_of {
        for series in bars.values_mut() {
            series.retain(|b| b.date <= cutoff);
        }
    }
    // Report the last actual completed bar date, not a weekend/holiday label.
    let as_of = determine_as_of(&bars);

    let mut warnings = warnings;
    if !missing.is_empty() {
        warnings.push(format!(
            "Skipped {} symbols without data: {}",
            missing.len(),
            missing
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let counter = AtomicUsize::new(0);
    let total = symbols.len();
    let strategies: Vec<StrategyMode> = match options.strategy {
        StrategyMode::Compare => vec![StrategyMode::Tpi, StrategyMode::Consensus],
        other => vec![other],
    };
    let results: Vec<(String, Result<Vec<Signal>>)> = symbols
        .par_iter()
        .map(|symbol| {
            let result = if cancelled(options) {
                Err(crate::Error::Cancelled)
            } else {
                evaluate_symbol(symbol, bars.get(symbol), options, &strategies, as_of)
            };
            let done = counter.fetch_add(1, Ordering::Relaxed) + 1;
            if let Some(p) = progress {
                p.report("indicator evaluation", done, total);
            }
            (symbol.clone(), result)
        })
        .collect();

    if cancelled(options) {
        return Err(crate::Error::Cancelled);
    }
    let mut signals = Vec::new();
    let mut errors = Vec::new();
    for (symbol, result) in results {
        match result {
            Ok(mut sigs) => signals.append(&mut sigs),
            Err(crate::Error::Cancelled) => return Err(crate::Error::Cancelled),
            Err(e) => errors.push(SymbolError {
                symbol,
                message: e.to_string(),
            }),
        }
    }
    sort_signals(&mut signals);

    Ok(ScanReport {
        generated_at: Utc::now(),
        horizon: options.horizon,
        strategy: options.strategy,
        risk: options.risk,
        equity: options.equity,
        as_of,
        provenance,
        requested_symbols: symbols.len(),
        signals,
        errors,
        warnings,
        elapsed_ms: started.elapsed().as_millis(),
        bars,
    })
}

fn cancelled(options: &AnalysisOptions<'_>) -> bool {
    options
        .cancel
        .as_ref()
        .map(|c| c.load(Ordering::Relaxed))
        .unwrap_or(false)
}

fn evaluate_symbol(
    symbol: &str,
    bars: Option<&Vec<Bar>>,
    options: &AnalysisOptions<'_>,
    strategies: &[StrategyMode],
    as_of: Option<NaiveDate>,
) -> Result<Vec<Signal>> {
    let bars =
        bars.ok_or_else(|| crate::Error::NoData(format!("no bars returned for {symbol}")))?;
    let needed = options.config.strategy.min_history_bars.max(200);
    if bars.len() < needed {
        return Err(crate::Error::InsufficientData {
            symbol: symbol.to_string(),
            needed,
            have: bars.len(),
        });
    }
    let series = Series::compute(bars);
    let i = bars.len() - 1;
    let params = options.config.horizon_params(options.horizon);
    let stale = as_of
        .map(|d| (d - bars[i].date).num_days() > options.stale_bar_days)
        .unwrap_or(false);
    let mut out = Vec::with_capacity(strategies.len());
    for strategy in strategies {
        let eval = EvalConfig {
            horizon: options.horizon,
            strategy: *strategy,
            risk: options.risk,
            equity: options.equity,
            cash: options.equity,
            allow_fractional: options.config.backtest.allow_fractional,
            require_rating_buy: options.config.strategy.require_rating_buy,
            volume_ratio_min: options.config.strategy.volume_ratio_min,
            min_history_bars: options.config.strategy.min_history_bars,
            long_only: options.config.strategy.long_only,
        };
        let mut signal = evaluate(symbol, &series, i, params, eval)?;
        signal.stale = stale;
        if stale {
            signal
                .warnings
                .push(format!("Last bar is {}; quote may be stale", signal.as_of));
        }
        out.push(signal);
    }
    Ok(out)
}

fn determine_as_of(bars: &HashMap<String, Vec<Bar>>) -> Option<NaiveDate> {
    bars.values().filter_map(|s| s.last().map(|b| b.date)).max()
}

fn sort_signals(signals: &mut [Signal]) {
    signals.sort_by(|a, b| {
        action_rank(a.action)
            .cmp(&action_rank(b.action))
            .then_with(|| {
                b.strength
                    .partial_cmp(&a.strength)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| b.rating.rank().cmp(&a.rating.rank()))
            .then_with(|| a.symbol.cmp(&b.symbol))
    });
}

fn action_rank(action: Action) -> u8 {
    match action {
        Action::Buy => 0,
        Action::Hold => 1,
        Action::Sell => 2,
    }
}

/// The last completed session date. If the market is open, today's partial
/// daily bar is excluded.
pub async fn completed_session_date(market: &MarketData) -> Option<NaiveDate> {
    if let Some(client) = market.client()
        && let Ok(clock) = client.clock().await
    {
        let today = clock.timestamp.date_naive();
        if clock.is_open {
            return Some(today - Duration::days(1));
        }
        return Some(today);
    }
    Some(Utc::now().date_naive())
}

/// Convenience wrapper for single/multi-ticker analysis with a 500-day warmup.
pub async fn analyze_symbols(
    market: &MarketData,
    symbols: &[String],
    options: &AnalysisOptions<'_>,
    progress: Option<&Progress<'_>>,
) -> Result<ScanReport> {
    let end = options.as_of.unwrap_or_else(|| Utc::now().date_naive());
    let start = end - Duration::days(500);
    analyze_market(market, symbols, start, end, options, progress).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::data::synthetic::synthetic_universe;

    #[tokio::test]
    async fn offline_scan_is_deterministic_and_sorted() {
        let cfg = Config::default();
        let market = MarketData::new(&cfg, true, true).unwrap();
        let symbols = vec!["AAA".to_string(), "BBB".to_string(), "CCC".to_string()];
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let start = today - Duration::days(500);
        // Validate the synthetic universe shape separately; the scan itself
        // uses market.load, so this checks the offline path end to end.
        let syn = synthetic_universe(&symbols, start, today);
        assert!(syn.values().all(|b| b.len() > 300));
        let options = AnalysisOptions {
            horizon: Horizon::Medium,
            strategy: StrategyMode::Tpi,
            risk: RiskProfile::Balanced,
            equity: 10_000.0,
            config: &cfg,
            as_of: Some(today),
            stale_bar_days: 5,
            cancel: None,
        };
        let report = analyze_market(&market, &symbols, start, today, &options, None)
            .await
            .unwrap();
        assert_eq!(report.signals.len(), 3);
        let report2 = analyze_market(&market, &symbols, start, today, &options, None)
            .await
            .unwrap();
        assert_eq!(
            report.signals.iter().map(|s| &s.symbol).collect::<Vec<_>>(),
            report2
                .signals
                .iter()
                .map(|s| &s.symbol)
                .collect::<Vec<_>>()
        );
    }
}
