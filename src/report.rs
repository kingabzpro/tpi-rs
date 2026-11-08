//! Human-readable reports and CSV/JSON exports.

use crate::backtest::BenchmarkReport;
use crate::backtest::types::{BacktestResult, Metrics, WalkForwardResult};
use crate::error::{Error, IoContext, Result};
use crate::model::{Action, DataSource, Rating, Signal};
use crate::scan::ScanReport;
use chrono::Utc;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn timestamp_prefix() -> String {
    Utc::now().format("%Y%m%d-%H%M%S").to_string()
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).at_path(path)
}

// ---------------------------------------------------------------------------
// Console tables
// ---------------------------------------------------------------------------

pub fn print_scan(report: &ScanReport, detailed: bool) {
    println!();
    println!(
        "TPI-RS {} scan | horizon={} | strategy={} | risk={} | equity=${:.2}",
        report.provenance.source.label(),
        report.horizon.short_label(),
        report.strategy.label(),
        report.risk.label(),
        report.equity
    );
    if let Some(as_of) = report.as_of {
        println!(
            "As of {as_of} | data={} | feed={} | adjusted={} | {} symbols",
            report.provenance.source.label(),
            report.provenance.feed,
            report.provenance.adjusted,
            report.requested_symbols
        );
    }
    if report.provenance.source == DataSource::Synthetic {
        println!("WARNING: SYNTHETIC DEMO DATA. These are generated series, not market prices.");
    }
    println!(
        "{:<4} {:<7} {:<6} {:<6} {:<5} {:<12} {:>9} {:>6} {:>6} {:>9} {:>9} {:>6} {:>7}",
        "#",
        "SYMBOL",
        "STRAT",
        "ACTION",
        "STR",
        "RATING",
        "CLOSE",
        "RSI",
        "ATR%",
        "ENTRY",
        "STOP",
        "RISK%",
        "SCORE"
    );
    println!("{}", "-".repeat(111));
    for (idx, s) in report.signals.iter().enumerate() {
        println!(
            "{:<4} {:<7} {:<6} {:<6} {:>5.0} {:<12} {:>9.2} {:>6.1} {:>6.2} {:>9.2} {:>9.2} {:>6.2} {:>7.0}",
            idx + 1,
            s.symbol,
            s.strategy.short_label(),
            s.action.label(),
            s.strength,
            s.rating.label(),
            s.price,
            opt(s.indicators.rsi14),
            opt(s.indicators.atr_pct),
            s.entry,
            s.stop,
            s.stop_distance_pct,
            s.components.total()
        );
    }
    println!("{}", "-".repeat(111));
    let buys = report.buys().count();
    println!(
        "{} signals: {buys} BUY / {} HOLD / {} SELL",
        report.signals.len(),
        report
            .signals
            .iter()
            .filter(|s| s.action == Action::Hold)
            .count(),
        report
            .signals
            .iter()
            .filter(|s| s.action == Action::Sell)
            .count()
    );
    if report.strategy == crate::model::StrategyMode::Compare {
        for strat in [
            crate::model::StrategyMode::Tpi,
            crate::model::StrategyMode::Consensus,
        ] {
            let subset: Vec<&Signal> = report
                .signals
                .iter()
                .filter(|s| s.strategy == strat)
                .collect();
            let buys = subset.iter().filter(|s| s.action == Action::Buy).count();
            println!(
                "  {}: {} signals, {buys} BUY",
                strat.short_label(),
                subset.len()
            );
        }
    }
    if detailed {
        for s in &report.signals {
            print_signal_detail(s);
        }
    }
    for e in &report.errors {
        println!("ERROR {}: {}", e.symbol, e.message);
    }
    for w in &report.warnings {
        println!("NOTE: {w}");
    }
    println!(
        "\nSignals are rule-based conditions, not price forecasts. Analysis took {} ms.",
        report.elapsed_ms
    );
}

pub fn print_signal_detail(s: &Signal) {
    println!(
        "\n{} {} | strength {:.0}/100 | rating {}",
        s.symbol, s.action, s.strength, s.rating
    );
    println!(
        "  as of {} | horizon {} | strategy {} | close {:.2}",
        s.as_of,
        s.horizon.short_label(),
        s.strategy.label(),
        s.price
    );
    if s.stale {
        println!("  STALE QUOTE: last bar is not the latest completed session");
    }
    println!(
        "  EMA50 {:.2} | EMA200 {:.2} | Donchian20 {:.2}/{:.2} | Donchian40 {:.2}/{:.2}",
        opt(s.indicators.ema50),
        opt(s.indicators.ema200),
        opt(s.indicators.donchian20_high),
        opt(s.indicators.donchian20_low),
        opt(s.indicators.donchian40_high),
        opt(s.indicators.donchian40_low),
    );
    println!(
        "  RSI14 {:.1} | ATR14 {:.2} ({:.2}%) | ADX14 {:.1} | volume {:.2}x",
        opt(s.indicators.rsi14),
        opt(s.indicators.atr14),
        opt(s.indicators.atr_pct),
        opt(s.indicators.adx14),
        opt(s.indicators.volume_ratio),
    );
    println!(
        "  Score: total {:.0}/100 = trend {:.0}/35 + breakout {:.0}/25 + momentum {:.0}/20 + confirm {:.0}/10 + volatility {:.0}/10",
        s.components.total(),
        s.components.trend,
        s.components.breakout,
        s.components.momentum,
        s.components.confirmation,
        s.components.volatility
    );
    println!(
        "  Plan: entry ref {:.2} | stop {:.2} ({:.2}% risk) | 2R target {:.2} | shares {:.4} | notional ${:.2} | risk ${:.2}",
        s.entry,
        s.stop,
        s.stop_distance_pct,
        s.target,
        s.suggested_shares,
        s.suggested_notional,
        s.risk_amount
    );
    println!(
        "  Technical panel: {} buy / {} neutral / {} sell (MA {} / {} / {}, osc {} / {} / {})",
        s.rating_detail.buy_votes,
        s.rating_detail.neutral_votes,
        s.rating_detail.sell_votes,
        s.rating_detail.ma_buy,
        s.rating_detail.ma_neutral,
        s.rating_detail.ma_sell,
        s.rating_detail.osc_buy,
        s.rating_detail.osc_neutral,
        s.rating_detail.osc_sell
    );
    println!("  Why:");
    for r in &s.reasons {
        println!("   - {r}");
    }
    if !s.warnings.is_empty() {
        println!("  Warnings:");
        for w in &s.warnings {
            println!("   - {w}");
        }
    }
}

pub fn print_backtest(result: &BacktestResult, show_trades: bool) {
    println!();
    println!("=== {} ===", result.label);
    if result.label.contains("SYNTHETIC DEMO") {
        println!("WARNING: SYNTHETIC DEMO DATA. These are generated series, not market prices.");
    }
    println!(
        "Period {} .. {} | {} sessions | universe {} | horizon {} | strategy {} | risk {}",
        result.start,
        result.end,
        result.trading_days,
        result.universe_size,
        result.horizon.short_label(),
        result.strategy.label(),
        result.risk.label()
    );
    println!(
        "Capital ${:.2} -> ${:.2} | total {:+.2}% | CAGR {:+.2}% | Sharpe {:.2} | Sortino {:.2} | max DD {:.2}% | Calmar {:.2}",
        result.initial_capital,
        result.metrics.final_equity,
        result.metrics.total_return_pct,
        result.metrics.cagr_pct,
        result.metrics.sharpe,
        result.metrics.sortino,
        result.metrics.max_drawdown_pct,
        result.metrics.calmar
    );
    println!(
        "Vol {:.2}% | exposure {:.0}% | trades {} (win {:.1}%, PF {:.2}, avg win {:+.2}%, avg loss {:+.2}%, avg trade {:+.2}%) | fees ${:.2}",
        result.metrics.annual_volatility_pct,
        result.metrics.exposure_pct,
        result.metrics.trades,
        result.metrics.win_rate_pct,
        result.metrics.profit_factor,
        result.metrics.avg_win_pct,
        result.metrics.avg_loss_pct,
        result.metrics.avg_trade_pct,
        result.metrics.total_fees
    );
    println!("Rules: {}", result.data_note);
    print_windows(result);
    print_comparisons(result);

    if !result.relative.is_empty() {
        println!("\nRelative to benchmarks:");
        for r in &result.relative {
            println!(
                "  {}: alpha {:+.2}%/yr | beta {:.2} | corr {:.2} | tracking error {:.2}%",
                r.benchmark, r.alpha_annual_pct, r.beta, r.correlation, r.tracking_error_pct
            );
        }
    }

    if show_trades {
        println!("\nTrades:");
        if result.trades.is_empty() {
            println!("  (none)");
        } else {
            println!(
                "  {:<7} {:<11} {:<11} {:>9} {:>9} {:>9} {:>9} {:>7} {:<20}",
                "SYMBOL", "ENTRY", "EXIT", "ENTRY$", "EXIT$", "SHARES", "NET P&L", "RET%", "REASON"
            );
            for t in result.trades.iter().take(30) {
                println!(
                    "  {:<7} {:<11} {:<11} {:>9.2} {:>9.2} {:>9.4} {:>9.2} {:>7.2} {:<20}",
                    t.symbol,
                    t.entry_date,
                    t.exit_date,
                    t.entry_price,
                    t.exit_price,
                    t.shares,
                    t.net_pnl,
                    t.net_return_pct,
                    trim_reason(&t.exit_reason)
                );
            }
            if result.trades.len() > 30 {
                println!(
                    "  ... {} more trades in the CSV/JSON export",
                    result.trades.len() - 30
                );
            }
        }
    }
    for w in &result.warnings {
        println!("NOTE: {w}");
    }
    println!("Computed in {} ms.", result.elapsed_ms);
}

fn print_windows(result: &BacktestResult) {
    println!("\nSix-month windows (continuous portfolio):");
    println!(
        "  {:<25} {:>9} {:>9} {:>9} {:>9} {:>7} {:>7}",
        "WINDOW", "RETURN", "SPY", "QQQ", "MAX DD", "TRADES", "WIN%"
    );
    for (idx, w) in result.windows.iter().enumerate() {
        let spy = result
            .comparisons
            .iter()
            .find(|c| c.label == "SPY")
            .and_then(|c| c.windows.get(idx))
            .map(|x| x.return_pct)
            .unwrap_or(0.0);
        let qqq = result
            .comparisons
            .iter()
            .find(|c| c.label == "QQQ")
            .and_then(|c| c.windows.get(idx))
            .map(|x| x.return_pct)
            .unwrap_or(0.0);
        println!(
            "  {:<25} {:>8.2}% {:>8.2}% {:>8.2}% {:>8.2}% {:>7} {:>7.1}",
            w.label, w.return_pct, spy, qqq, w.max_drawdown_pct, w.trades, w.win_rate_pct
        );
    }
}

fn print_comparisons(result: &BacktestResult) {
    if result.comparisons.is_empty() {
        return;
    }
    println!("\nComparisons over the same period:");
    println!(
        "  {:<18} {:>10} {:>10} {:>8} {:>8} {:>9} {:>9}",
        "STRATEGY", "FINAL", "RETURN", "CAGR", "SHARPE", "MAX DD", "VOL"
    );
    println!(
        "  {:<18} {:>10.2} {:>9.2}% {:>7.2}% {:>8.2} {:>8.2}% {:>8.2}%",
        "TPI portfolio",
        result.metrics.final_equity,
        result.metrics.total_return_pct,
        result.metrics.cagr_pct,
        result.metrics.sharpe,
        result.metrics.max_drawdown_pct,
        result.metrics.annual_volatility_pct
    );
    for c in &result.comparisons {
        println!(
            "  {:<18} {:>10.2} {:>9.2}% {:>7.2}% {:>8.2} {:>8.2}% {:>8.2}%",
            c.label,
            c.metrics.final_equity,
            c.metrics.total_return_pct,
            c.metrics.cagr_pct,
            c.metrics.sharpe,
            c.metrics.max_drawdown_pct,
            c.metrics.annual_volatility_pct
        );
    }
}

pub fn print_benchmark(report: &BenchmarkReport) {
    println!();
    println!("================ TPI-RS THREE-YEAR BENCHMARK ================");
    println!(
        "Generated {} | data={} | feed={} | adjusted={} | universe {}",
        report.generated_at.format("%Y-%m-%d %H:%M UTC"),
        report.provenance.source.label(),
        report.provenance.feed,
        report.provenance.adjusted,
        report.universe_size
    );
    if report.provenance.source == DataSource::Synthetic {
        println!("WARNING: SYNTHETIC DEMO DATA. Results are not market performance.");
    }
    for headline in &report.headlines {
        print_backtest(headline, false);
    }
    if let Some(wf) = &report.walk_forward {
        print_walk_forward(wf);
    }
    for w in &report.warnings {
        println!("NOTE: {w}");
    }
    println!("Benchmark finished in {} ms.", report.elapsed_ms);
}

pub fn print_walk_forward(wf: &WalkForwardResult) {
    println!("\nWalk-forward (parameters fitted only on past training data):");
    println!(
        "  {:<4} {:<23} {:<23} {:<25} {:>10} {:>10} {:>10} {:>8}",
        "FOLD", "TRAIN", "TEST", "SELECTED", "IS SHARPE", "OOS RET%", "OOS SHARPE", "OOS DD%"
    );
    for (idx, f) in wf.folds.iter().enumerate() {
        println!(
            "  {:<4} {:<23} {:<23} {:<25} {:>10.2} {:>9.2}% {:>10.2} {:>8.2}",
            idx + 1,
            format!("{} .. {}", f.train_start, f.train_end),
            format!("{} .. {}", f.test_start, f.test_end),
            f.selected.to_string(),
            f.in_sample.sharpe,
            f.out_of_sample.total_return_pct,
            f.out_of_sample.sharpe,
            f.out_of_sample.max_drawdown_pct
        );
    }
    if let Some(oos) = &wf.oos {
        println!(
            "\nUntouched OOS: train {} .. {}, test {} .. {} | selected {} | IS Sharpe {:.2} | OOS return {:+.2}% | OOS Sharpe {:.2} | OOS DD {:.2}% | OOS trades {}",
            oos.train_start,
            oos.train_end,
            oos.test_start,
            oos.test_end,
            oos.selected,
            oos.in_sample.sharpe,
            oos.out_of_sample.total_return_pct,
            oos.out_of_sample.sharpe,
            oos.out_of_sample.max_drawdown_pct,
            oos.out_of_sample.trades
        );
    }
    if !wf.stability.is_empty() {
        println!("\nParameter stability across six-month windows (top 10):");
        println!(
            "  {:<30} {:>10} {:>10} {:>10} {:>10} {:>8}",
            "PARAMS", "MEAN%", "STDEV%", "WORST%", "BEST%", "POS/ALL"
        );
        for row in wf.stability.iter().take(10) {
            println!(
                "  {:<30} {:>9.2}% {:>9.2}% {:>9.2}% {:>9.2}% {:>4}/{:<3}",
                row.params.to_string(),
                row.mean_return_pct,
                row.stdev_return_pct,
                row.worst_window_pct,
                row.best_window_pct,
                row.positive_windows,
                row.window_returns_pct.len()
            );
        }
    }
    for w in &wf.warnings {
        println!("NOTE: {w}");
    }
}

pub fn print_metrics_row(label: &str, m: &Metrics) {
    println!(
        "{:<20} final {:>10.2} return {:>8.2}% sharpe {:>6.2} maxDD {:>7.2}% trades {:>4}",
        label, m.final_equity, m.total_return_pct, m.sharpe, m.max_drawdown_pct, m.trades
    );
}

// ---------------------------------------------------------------------------
// Exports
// ---------------------------------------------------------------------------

pub fn export_scan(dir: &Path, report: &ScanReport) -> Result<Vec<PathBuf>> {
    ensure_dir(dir)?;
    let prefix = timestamp_prefix();
    let json_path = dir.join(format!("scan-{prefix}.json"));
    write_json(&json_path, report)?;
    let csv_path = dir.join(format!("signals-{prefix}.csv"));
    let mut writer = csv::Writer::from_path(&csv_path).map_err(Error::from)?;
    writer
        .write_record([
            "symbol",
            "as_of",
            "horizon",
            "strategy",
            "action",
            "strength",
            "rating",
            "rating_score",
            "close",
            "ema50",
            "ema200",
            "rsi14",
            "atr14",
            "atr_pct",
            "adx14",
            "donchian20_high",
            "donchian20_low",
            "donchian40_high",
            "donchian40_low",
            "entry",
            "stop",
            "target",
            "stop_distance_pct",
            "suggested_shares",
            "suggested_notional",
            "risk_amount",
            "trend_score",
            "breakout_score",
            "momentum_score",
            "confirmation_score",
            "volatility_score",
            "score_total",
            "stale",
        ])
        .map_err(Error::from)?;
    for s in &report.signals {
        writer
            .write_record([
                s.symbol.clone(),
                s.as_of.to_string(),
                s.horizon.short_label().to_string(),
                s.strategy.label().to_string(),
                s.action.label().to_string(),
                format!("{:.2}", s.strength),
                s.rating.label().to_string(),
                format!("{:.4}", s.rating_detail.score),
                format!("{:.4}", s.price),
                opt_s(s.indicators.ema50),
                opt_s(s.indicators.ema200),
                opt_s(s.indicators.rsi14),
                opt_s(s.indicators.atr14),
                opt_s(s.indicators.atr_pct),
                opt_s(s.indicators.adx14),
                opt_s(s.indicators.donchian20_high),
                opt_s(s.indicators.donchian20_low),
                opt_s(s.indicators.donchian40_high),
                opt_s(s.indicators.donchian40_low),
                format!("{:.4}", s.entry),
                format!("{:.4}", s.stop),
                format!("{:.4}", s.target),
                format!("{:.4}", s.stop_distance_pct),
                format!("{:.6}", s.suggested_shares),
                format!("{:.2}", s.suggested_notional),
                format!("{:.2}", s.risk_amount),
                format!("{:.1}", s.components.trend),
                format!("{:.1}", s.components.breakout),
                format!("{:.1}", s.components.momentum),
                format!("{:.1}", s.components.confirmation),
                format!("{:.1}", s.components.volatility),
                format!("{:.1}", s.components.total()),
                s.stale.to_string(),
            ])
            .map_err(Error::from)?;
    }
    writer.flush().map_err(Error::from)?;
    Ok(vec![json_path, csv_path])
}

pub fn export_backtest(dir: &Path, prefix: &str, result: &BacktestResult) -> Result<Vec<PathBuf>> {
    ensure_dir(dir)?;
    let mut paths = Vec::new();
    let json_path = dir.join(format!("backtest-{prefix}.json"));
    write_json(&json_path, result)?;
    paths.push(json_path);

    let trades_path = dir.join(format!("trades-{prefix}.csv"));
    let mut w = csv::Writer::from_path(&trades_path).map_err(Error::from)?;
    w.write_record([
        "symbol",
        "entry_date",
        "exit_date",
        "entry_price",
        "exit_price",
        "shares",
        "entry_notional",
        "gross_pnl",
        "fees",
        "implicit_costs",
        "net_pnl",
        "net_return_pct",
        "hold_days",
        "entry_score",
        "entry_rating",
        "exit_reason",
    ])
    .map_err(Error::from)?;
    for t in &result.trades {
        w.write_record([
            t.symbol.clone(),
            t.entry_date.to_string(),
            t.exit_date.to_string(),
            format!("{:.4}", t.entry_price),
            format!("{:.4}", t.exit_price),
            format!("{:.6}", t.shares),
            format!("{:.2}", t.entry_notional),
            format!("{:.2}", t.gross_pnl),
            format!("{:.2}", t.fees),
            format!("{:.2}", t.implicit_costs),
            format!("{:.2}", t.net_pnl),
            format!("{:.4}", t.net_return_pct),
            t.hold_days.to_string(),
            format!("{:.1}", t.entry_score),
            t.entry_rating.label().to_string(),
            t.exit_reason.clone(),
        ])
        .map_err(Error::from)?;
    }
    w.flush().map_err(Error::from)?;
    paths.push(trades_path);

    let equity_path = dir.join(format!("equity-{prefix}.csv"));
    let mut w = csv::Writer::from_path(&equity_path).map_err(Error::from)?;
    w.write_record(["date", "equity", "cash", "position_value", "drawdown_pct"])
        .map_err(Error::from)?;
    for p in &result.equity {
        w.write_record([
            p.date.to_string(),
            format!("{:.2}", p.equity),
            format!("{:.2}", p.cash),
            format!("{:.2}", p.position_value),
            format!("{:.4}", p.drawdown_pct),
        ])
        .map_err(Error::from)?;
    }
    w.flush().map_err(Error::from)?;
    paths.push(equity_path);

    let decisions_path = dir.join(format!("decisions-{prefix}.csv"));
    let mut w = csv::Writer::from_path(&decisions_path).map_err(Error::from)?;
    w.write_record([
        "date",
        "action",
        "symbol",
        "score",
        "rating",
        "candidates",
        "equity",
        "note",
    ])
    .map_err(Error::from)?;
    for d in &result.decisions {
        w.write_record([
            d.date.to_string(),
            d.action.label().to_string(),
            d.symbol.clone().unwrap_or_default(),
            d.score.map(|v| format!("{v:.1}")).unwrap_or_default(),
            d.rating.map(|r| r.label().to_string()).unwrap_or_default(),
            d.candidates.to_string(),
            format!("{:.2}", d.equity),
            d.note.clone(),
        ])
        .map_err(Error::from)?;
    }
    w.flush().map_err(Error::from)?;
    paths.push(decisions_path);

    let windows_path = dir.join(format!("windows-{prefix}.csv"));
    let mut w = csv::Writer::from_path(&windows_path).map_err(Error::from)?;
    w.write_record([
        "label",
        "start",
        "end",
        "return_pct",
        "max_drawdown_pct",
        "trades",
        "win_rate_pct",
        "final_equity",
    ])
    .map_err(Error::from)?;
    for win in &result.windows {
        w.write_record([
            win.label.clone(),
            win.start.to_string(),
            win.end.to_string(),
            format!("{:.4}", win.return_pct),
            format!("{:.4}", win.max_drawdown_pct),
            win.trades.to_string(),
            format!("{:.2}", win.win_rate_pct),
            format!("{:.2}", win.final_equity),
        ])
        .map_err(Error::from)?;
    }
    w.flush().map_err(Error::from)?;
    paths.push(windows_path);
    Ok(paths)
}

pub fn export_benchmark(dir: &Path, report: &BenchmarkReport) -> Result<Vec<PathBuf>> {
    ensure_dir(dir)?;
    let prefix = timestamp_prefix();
    let json_path = dir.join(format!("benchmark-{prefix}.json"));
    write_json(&json_path, report)?;
    let mut paths = vec![json_path];

    // Full per-leg artifacts: trades, equity, decisions and windows.
    for (idx, headline) in report.headlines.iter().enumerate() {
        let leg = sanitize_name(&headline.label);
        let leg_prefix = format!("benchmark-{idx}-{leg}-{prefix}");
        paths.extend(export_backtest(dir, &leg_prefix, headline)?);
    }

    let summary_path = dir.join(format!("benchmark-summary-{prefix}.csv"));
    let mut w = csv::Writer::from_path(&summary_path).map_err(Error::from)?;
    w.write_record([
        "leg",
        "label",
        "window",
        "start",
        "end",
        "return_pct",
        "max_drawdown_pct",
        "trades",
        "win_rate_pct",
    ])
    .map_err(Error::from)?;
    for headline in &report.headlines {
        for win in &headline.windows {
            w.write_record([
                headline.label.clone(),
                "TPI".to_string(),
                win.label.clone(),
                win.start.to_string(),
                win.end.to_string(),
                format!("{:.4}", win.return_pct),
                format!("{:.4}", win.max_drawdown_pct),
                win.trades.to_string(),
                format!("{:.2}", win.win_rate_pct),
            ])
            .map_err(Error::from)?;
        }
        for comparison in &headline.comparisons {
            for win in &comparison.windows {
                w.write_record([
                    headline.label.clone(),
                    comparison.label.clone(),
                    win.label.clone(),
                    win.start.to_string(),
                    win.end.to_string(),
                    format!("{:.4}", win.return_pct),
                    format!("{:.4}", win.max_drawdown_pct),
                    win.trades.to_string(),
                    format!("{:.2}", win.win_rate_pct),
                ])
                .map_err(Error::from)?;
            }
        }
    }
    w.flush().map_err(Error::from)?;
    paths.push(summary_path);

    if let Some(wf) = &report.walk_forward {
        let wf_path = dir.join(format!("walkforward-{prefix}.csv"));
        let mut w = csv::Writer::from_path(&wf_path).map_err(Error::from)?;
        w.write_record([
            "fold",
            "train_start",
            "train_end",
            "test_start",
            "test_end",
            "atr_mult",
            "rsi_max",
            "exit_period",
            "is_sharpe",
            "is_return_pct",
            "oos_return_pct",
            "oos_sharpe",
            "oos_max_dd_pct",
            "oos_trades",
        ])
        .map_err(Error::from)?;
        for (i, f) in wf.folds.iter().enumerate() {
            w.write_record([
                (i + 1).to_string(),
                f.train_start.to_string(),
                f.train_end.to_string(),
                f.test_start.to_string(),
                f.test_end.to_string(),
                format!("{:.2}", f.selected.atr_mult),
                format!("{:.0}", f.selected.rsi_max),
                f.selected.exit_period.to_string(),
                format!("{:.4}", f.in_sample.sharpe),
                format!("{:.4}", f.in_sample.total_return_pct),
                format!("{:.4}", f.out_of_sample.total_return_pct),
                format!("{:.4}", f.out_of_sample.sharpe),
                format!("{:.4}", f.out_of_sample.max_drawdown_pct),
                f.out_of_sample.trades.to_string(),
            ])
            .map_err(Error::from)?;
        }
        w.flush().map_err(Error::from)?;
        paths.push(wf_path);

        let stab_path = dir.join(format!("stability-{prefix}.csv"));
        let mut w = csv::Writer::from_path(&stab_path).map_err(Error::from)?;
        let mut header = vec![
            "atr_mult".to_string(),
            "rsi_max".to_string(),
            "exit_period".to_string(),
            "mean_return_pct".to_string(),
            "stdev_return_pct".to_string(),
            "worst_window_pct".to_string(),
            "best_window_pct".to_string(),
            "positive_windows".to_string(),
        ];
        for i in 0..wf
            .stability
            .first()
            .map(|r| r.window_returns_pct.len())
            .unwrap_or(0)
        {
            header.push(format!("window_{}_return_pct", i + 1));
        }
        w.write_record(&header).map_err(Error::from)?;
        for row in &wf.stability {
            let mut record = vec![
                format!("{:.2}", row.params.atr_mult),
                format!("{:.0}", row.params.rsi_max),
                row.params.exit_period.to_string(),
                format!("{:.4}", row.mean_return_pct),
                format!("{:.4}", row.stdev_return_pct),
                format!("{:.4}", row.worst_window_pct),
                format!("{:.4}", row.best_window_pct),
                row.positive_windows.to_string(),
            ];
            for r in &row.window_returns_pct {
                record.push(format!("{r:.4}"));
            }
            w.write_record(&record).map_err(Error::from)?;
        }
        w.flush().map_err(Error::from)?;
        paths.push(stab_path);
    }
    Ok(paths)
}

pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let file = std::fs::File::create(path).at_path(path)?;
    let mut writer = std::io::BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.write_all(b"\n").at_path(path)?;
    writer.flush().at_path(path)?;
    Ok(())
}

fn opt(value: Option<f64>) -> f64 {
    value.unwrap_or(0.0)
}

fn opt_s(value: Option<f64>) -> String {
    value.map(|v| format!("{v:.4}")).unwrap_or_default()
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect()
}

fn trim_reason(reason: &str) -> String {
    let mut out: String = reason.chars().take(17).collect();
    if reason.chars().count() > 17 {
        out.push_str("...");
    }
    out
}

pub fn rating_rank(rating: Rating) -> i8 {
    rating.rank()
}

pub fn action_label(action: Action) -> &'static str {
    action.label()
}

pub fn print_doctor_check(name: &str, ok: bool, detail: &str) {
    println!(
        "[{}] {:<28} {}",
        if ok { " OK " } else { "FAIL" },
        name,
        detail
    );
}

pub fn print_error(err: &Error) {
    eprintln!("ERROR: {err}");
    if let Error::MissingCredentials = err {
        eprintln!("Hint: copy .env.example to .env and fill ALPACA_API_KEY / ALPACA_SECRET_KEY.");
        eprintln!("Or run offline with --offline for a synthetic demo.");
    }
}
