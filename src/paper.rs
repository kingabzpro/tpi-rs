//! Paper-trading execution loop and shared broker session logic.
//!
//! Orders are market orders submitted to Alpaca. The engine uses the broker as
//! the source of truth: account, positions and open orders are re-read every
//! cycle, and order submissions are reconciled by `client_order_id` after an
//! ambiguous network failure. Live trading is gated in `live`.

use crate::ansi;
use crate::config::Config;
use crate::data::alpaca::{AlpacaClient, Order, OrderRequest, Position};
use crate::data::{MarketData, universe};
use crate::error::{Error, Result};
use crate::lockfile::InstanceLock;
use crate::model::{Action, Horizon, Mode, RiskProfile, StrategyMode};
use crate::scan::{self, AnalysisOptions};
use crate::tradelog::TradeLog;
use chrono::{DateTime, Duration, Utc};
use std::io::IsTerminal;
use std::path::PathBuf;
use tokio::time::{Duration as TokioDuration, sleep};

#[derive(Debug, Clone)]
pub struct TradeOptions {
    pub dry_run: bool,
    pub once: bool,
    pub wait_for_open: bool,
    pub flatten: bool,
    pub loop_secs: Option<u64>,
    pub top: Option<usize>,
    pub tickers: Vec<String>,
    pub i_understand_live_risk: bool,
}

pub async fn run_paper(
    cfg: &Config,
    horizon: Horizon,
    strategy: StrategyMode,
    risk: RiskProfile,
    capital_override: Option<f64>,
    options: &TradeOptions,
) -> Result<()> {
    run_session(
        cfg,
        true,
        horizon,
        strategy,
        risk,
        capital_override,
        options,
    )
    .await
}

pub(crate) async fn run_session(
    cfg: &Config,
    paper: bool,
    horizon: Horizon,
    strategy: StrategyMode,
    risk: RiskProfile,
    capital_override: Option<f64>,
    options: &TradeOptions,
) -> Result<()> {
    if strategy == StrategyMode::Compare {
        return Err(Error::validation(
            "Compare mode is for analysis and backtests; choose TPI or Consensus for trading",
        ));
    }
    let client = AlpacaClient::new(&cfg.alpaca, paper)?;
    let label = if paper { "PAPER" } else { "LIVE" };
    let label_text = if paper {
        ansi::bold(&ansi::cyan("PAPER"))
    } else {
        ansi::bold(&ansi::red("LIVE"))
    };
    println!("TPI-RS {label_text} trading | {horizon} | {strategy} | risk {risk}");

    if options.flatten {
        let report = client.emergency_flatten().await?;
        println!(
            "Flatten complete: orders cancelled={}, positions closed={}, errors={}",
            report.orders_cancelled,
            report.closed.len(),
            report.errors.len()
        );
        for c in &report.closed {
            println!("  closed {c}");
        }
        for e in &report.errors {
            println!("  ERROR {e}");
        }
        return Ok(());
    }

    let kill_path = PathBuf::from(&cfg.trading.kill_switch_file);
    if kill_path.exists() {
        println!(
            "KILL switch {} exists. No orders will be submitted. Remove it to resume.",
            kill_path.display()
        );
        return Ok(());
    }

    let _lock = InstanceLock::acquire(PathBuf::from(&cfg.paths.logs_dir).join("paper.lock"))?;
    let log = TradeLog::open(cfg)?;
    log.append(&format!(
        "session start: {label} {horizon} {strategy} risk {risk}"
    ));
    println!("{}", ansi::dim(&format!("log: {}", log.path().display())));

    let clock = client.clock().await?;
    println!(
        "Market {} | now {} | next open {} | next close {}",
        if clock.is_open {
            ansi::green("OPEN")
        } else {
            ansi::dim("CLOSED")
        },
        clock.timestamp.format("%Y-%m-%d %H:%M UTC"),
        clock.next_open.format("%Y-%m-%d %H:%M UTC"),
        clock.next_close.format("%Y-%m-%d %H:%M UTC")
    );
    if !clock.is_open && options.wait_for_open {
        wait_for_open(&client).await?;
    }

    let market = MarketData::new(cfg, false, false)?;
    let retry_delay = TokioDuration::from_secs(30);
    loop {
        let cycle_clock = match client.clock().await {
            Ok(clock) => clock,
            Err(e) => {
                log.record(&format!("CYCLE ERROR clock: {e}; retrying in 30s"));
                sleep(retry_delay).await;
                continue;
            }
        };
        let market_is_open = cycle_clock.is_open;
        let today = cycle_clock.timestamp.date_naive();
        let as_of = if market_is_open {
            today - Duration::days(1)
        } else {
            today
        };
        let account = match client.account().await {
            Ok(account) => account,
            Err(e) => {
                log.record(&format!("CYCLE ERROR account: {e}; retrying in 30s"));
                sleep(retry_delay).await;
                continue;
            }
        };
        if account.account_blocked || account.trading_blocked {
            println!(
                "Account is blocked (account_blocked={}, trading_blocked={}); no orders.",
                account.account_blocked, account.trading_blocked
            );
            return Ok(());
        }
        let positions = match client.positions().await {
            Ok(positions) => positions,
            Err(e) => {
                log.record(&format!("CYCLE ERROR positions: {e}; retrying in 30s"));
                sleep(retry_delay).await;
                continue;
            }
        };
        let active_positions: Vec<&Position> =
            positions.iter().filter(|p| p.qty.abs() >= 1e-6).collect();
        let open_orders = match client.orders("open").await {
            Ok(orders) => orders,
            Err(e) => {
                log.record(&format!("CYCLE ERROR open orders: {e}; retrying in 30s"));
                sleep(retry_delay).await;
                continue;
            }
        };
        // Fail closed: without the order history the daily budget cannot be
        // checked, so entries are blocked instead of assuming a clean day.
        let (recent_orders, history_error) = match client.orders("all").await {
            Ok(orders) => (orders, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        print_account(label, &account, &positions, &open_orders, &recent_orders);

        // Held symbols are added to the scanned list, never substituted for it.
        // Substituting made the loop stop looking for new entries as soon as it
        // held anything, so it only ever saw its own positions.
        let mut symbols = universe::parse_tickers(&options.tickers);
        if symbols.is_empty() {
            let (resolved, warnings) = universe::resolve_symbols(
                Some(&client),
                &[],
                Some(options.top.unwrap_or(100)),
                false,
                &cfg.backtest.universe,
            )
            .await?;
            for w in warnings {
                println!("NOTE: {w}");
            }
            symbols = resolved;
        }
        for p in &active_positions {
            if !symbols.contains(&p.symbol) {
                symbols.push(p.symbol.clone());
            }
        }
        let equity = capital_override.unwrap_or(account.equity);
        let analysis = scan::analyze_market(
            &market,
            &symbols,
            as_of - Duration::days(500),
            as_of,
            &AnalysisOptions {
                horizon,
                strategy,
                risk,
                vol_target_pct: 0.0,
                risk_pct_override: None,
                equity,
                config: cfg,
                as_of: Some(as_of),
                stale_bar_days: cfg.backtest.stale_bar_days,
                cancel: None,
            },
            None,
        )
        .await;
        let analysis = match analysis {
            Ok(analysis) => analysis,
            Err(e) => {
                log.record(&format!("CYCLE ERROR scan: {e}; retrying in 30s"));
                sleep(retry_delay).await;
                continue;
            }
        };
        for w in &analysis.warnings {
            log.append(&format!("NOTE: {w}"));
        }
        if let Some(as_of) = analysis.as_of {
            println!("Signals as of completed session {as_of}");
        }
        let buys = analysis
            .signals
            .iter()
            .filter(|s| s.action == Action::Buy)
            .count();
        let sells = analysis
            .signals
            .iter()
            .filter(|s| s.action == Action::Sell)
            .count();
        log.append(&format!(
            "cycle as_of {as_of}: {} symbols, {buys} buy, {sells} sell, {} holdings, {} open orders",
            analysis.signals.len(),
            active_positions.len(),
            open_orders.len()
        ));

        let dry_run = options.dry_run || !market_is_open;
        if !market_is_open {
            println!(
                "Market closed: analysis only, no orders (use --wait-for-open to trade at the open)."
            );
        }

        let daily_loss_breached = account.daily_pnl_pct() <= -cfg.trading.max_daily_loss_pct.abs();
        if daily_loss_breached {
            println!(
                "RISK: daily P&L {:+.2}% breaches the {:.2}% loss limit; new entries blocked.",
                account.daily_pnl_pct(),
                cfg.trading.max_daily_loss_pct
            );
        }
        let kill_active = kill_path.exists();
        if kill_active {
            println!("KILL switch present; exiting.");
            return Ok(());
        }

        let mut exited_any = false;
        for pos in &active_positions {
            if open_orders.iter().any(|o| o.symbol == pos.symbol) {
                println!(
                    "{}: open order exists; skipping exit evaluation this cycle",
                    pos.symbol
                );
                continue;
            }
            let signal = analysis
                .signals
                .iter()
                .find(|s| s.symbol == pos.symbol && s.strategy == strategy);
            let current = pos.current_price.unwrap_or(pos.avg_entry_price);
            if is_dust(
                pos.qty_available.unwrap_or(pos.qty),
                current,
                cfg.trading.min_order_notional,
            ) {
                println!(
                    "{}",
                    ansi::dim(&format!(
                        "{}: holding is worth ${:.2}, below the ${:.2} minimum; no exit order",
                        pos.symbol,
                        pos.qty_available.unwrap_or(pos.qty).abs() * current,
                        cfg.trading.min_order_notional
                    ))
                );
                continue;
            }
            let stop_hit = signal
                .map(|s| s.stop > 0.0 && current <= s.stop)
                .unwrap_or(false);
            let sell_signal = signal.map(|s| s.action == Action::Sell).unwrap_or(false);
            if sell_signal || stop_hit {
                let reason = if stop_hit {
                    format!(
                        "current {:.2} at/below stop {:.2}",
                        current,
                        signal.map(|s| s.stop).unwrap_or(0.0)
                    )
                } else {
                    "strategy SELL signal".to_string()
                };
                let qty = pos.qty_available.unwrap_or(pos.qty).abs();
                if qty <= 0.0 {
                    continue;
                }
                let req = OrderRequest::market(&pos.symbol, qty, "sell");
                submit_and_report(
                    &client,
                    &req,
                    dry_run,
                    cfg,
                    &log,
                    &format!("exit {} ({reason})", pos.symbol),
                )
                .await?;
                exited_any = true;
            }
        }

        let can_enter = !exited_any
            && !daily_loss_breached
            && positions.len() < cfg.trading.max_open_positions
            && !open_orders
                .iter()
                .any(|o| o.side.eq_ignore_ascii_case("buy"));

        if can_enter {
            let today = Utc::now().date_naive();
            let (orders_today, notional_today) = daily_usage(&recent_orders, today);
            if let Some(error) = &history_error {
                println!(
                    "{}",
                    ansi::yellow(&format!(
                        "RISK: order history unavailable ({error}); no new entry this cycle."
                    ))
                );
            } else if let Some(reason) =
                daily_gate(orders_today, notional_today, cfg, account.equity)
            {
                println!("{}", ansi::yellow(&format!("RISK: {reason}")));
            } else if let Some(best) = analysis.strongest() {
                if best.stale {
                    println!(
                        "{}",
                        ansi::yellow(&format!(
                            "RISK: {} signal is stale (last bar {}); no new entry.",
                            best.symbol, best.as_of
                        ))
                    );
                } else if active_positions.iter().any(|p| p.symbol == best.symbol) {
                    println!(
                        "{}",
                        ansi::dim(&format!("{}: already held; no add-on", best.symbol))
                    );
                } else if let Some(order) = build_entry_order(cfg, best, &account).await? {
                    submit_and_report(
                        &client,
                        &order,
                        dry_run,
                        cfg,
                        &log,
                        &format!("entry {} strength {:.0}", best.symbol, best.strength),
                    )
                    .await?;
                }
            } else {
                println!("{}", ansi::dim("No qualified BUY candidate this cycle."));
            }
        }

        if dry_run {
            println!("{}", ansi::dim("DRY RUN: no orders were submitted."));
        }
        if options.once || !market_is_open {
            break;
        }
        let interval = options
            .loop_secs
            .unwrap_or(cfg.trading.loop_interval_secs)
            .max(15);
        println!("Sleeping {interval}s until the next cycle (Ctrl+C to stop).");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("Shutdown requested.");
                if cfg.trading.flatten_on_exit {
                    let confirm = paper || confirm_flatten();
                    if confirm {
                        let report = client.emergency_flatten().await?;
                        println!("Flatten complete: {} positions closed, {} errors", report.closed.len(), report.errors.len());
                    }
                }
                break;
            }
            _ = sleep(TokioDuration::from_secs(interval)) => {}
        }
        let clock = client.clock().await?;
        if !clock.is_open {
            println!("Market closed; stopping trading loop.");
            break;
        }
    }
    Ok(())
}

async fn build_entry_order(
    cfg: &Config,
    signal: &crate::model::Signal,
    account: &crate::data::Account,
) -> Result<Option<OrderRequest>> {
    let equity = account.equity.max(0.0);
    let cash = account.available_cash();
    let cap_pct = cfg.trading.max_order_notional_pct.clamp(0.0, 100.0) / 100.0;
    let cap = (equity * cap_pct)
        .min(cfg.trading.max_single_order_notional.max(0.0))
        .min(cash);
    let desired = signal.suggested_notional.min(cap);
    let min = cfg.trading.min_order_notional.max(1.0);
    if desired < min {
        println!(
            "{}: planned notional ${:.2} is below the ${:.2} minimum (equity ${:.2}, cash ${:.2})",
            signal.symbol, desired, min, equity, cash
        );
        return Ok(None);
    }
    let order = if cfg.trading.allow_fractional {
        OrderRequest::notional(&signal.symbol, desired, "buy")
    } else {
        let price = signal.price.max(0.01);
        let qty = (desired / price).floor();
        if qty < 1.0 {
            println!(
                "{}: less than one whole share fits the risk cap",
                signal.symbol
            );
            return Ok(None);
        }
        OrderRequest::market(&signal.symbol, qty, "buy")
    };
    println!(
        "Plan BUY {} notional ${:.2} | signal {:.2} stop {:.2} ({:.2}% risk) | account equity ${:.2} cash ${:.2}",
        signal.symbol, desired, signal.price, signal.stop, signal.stop_distance_pct, equity, cash
    );
    Ok(Some(order))
}

async fn submit_and_report(
    client: &AlpacaClient,
    req: &OrderRequest,
    dry_run: bool,
    cfg: &Config,
    log: &TradeLog,
    description: &str,
) -> Result<Option<Order>> {
    if dry_run {
        log.record(&format!(
            "DRY RUN order: {description} | {} {} qty={} notional={} cid={}",
            req.side,
            req.symbol,
            req.qty.clone().unwrap_or_else(|| "-".into()),
            req.notional.clone().unwrap_or_else(|| "-".into()),
            req.client_order_id
        ));
        return Ok(None);
    }
    log.record(&format!(
        "SUBMIT {description} | {} {} qty={} notional={} cid={}",
        req.side,
        req.symbol,
        req.qty.clone().unwrap_or_else(|| "-".into()),
        req.notional.clone().unwrap_or_else(|| "-".into()),
        req.client_order_id
    ));
    let mut order = match client.submit_order(req).await {
        Ok(o) => o,
        Err(Error::Network { source, .. }) => {
            println!("Ambiguous submission ({source}); reconciling by client_order_id");
            let mut found = None;
            for _ in 0..5 {
                sleep(TokioDuration::from_secs(cfg.trading.poll_secs.max(1))).await;
                match client.order_by_client_id(&req.client_order_id).await? {
                    Some(o) => {
                        found = Some(o);
                        break;
                    }
                    None => continue,
                }
            }
            found.ok_or_else(|| Error::Api {
                status: 0,
                path: "/v2/orders".into(),
                message: format!(
                    "order submission ambiguous and {} not found; manual reconciliation required",
                    req.client_order_id
                ),
            })?
        }
        Err(e) => return Err(e),
    };
    let deadline = tokio::time::Instant::now()
        + TokioDuration::from_secs(cfg.trading.fill_timeout_secs.max(1));
    loop {
        println!(
            "  order {} {} {} | status={} filled={}/{} avg={}",
            order.id,
            order.side,
            order.symbol,
            order.status,
            order.filled_qty,
            order
                .qty
                .map(|q| format!("{q:.4}"))
                .unwrap_or_else(|| "?".into()),
            order
                .filled_avg_price
                .map(|p| format!("{p:.4}"))
                .unwrap_or_else(|| "-".into())
        );
        if order.is_terminal() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            println!(
                "  still {} after {}s; leaving the order with the broker (reconciled next cycle)",
                order.status, cfg.trading.fill_timeout_secs
            );
            break;
        }
        sleep(TokioDuration::from_secs(cfg.trading.poll_secs.max(1))).await;
        match client.order_by_client_id(&req.client_order_id).await? {
            Some(o) => order = o,
            None => break,
        }
    }
    if order.status == "partially_filled" {
        println!(
            "  PARTIAL FILL: {} of {} shares filled; remaining quantity stays at the broker",
            order.filled_qty,
            order
                .qty
                .map(|q| format!("{q:.4}"))
                .unwrap_or_else(|| "?".into())
        );
    }
    Ok(Some(order))
}

/// A position too small to be worth an order. Selling it costs a ticket and
/// pollutes the daily budget, so the loop leaves it for `--flatten`.
fn is_dust(qty: f64, price: f64, min_notional: f64) -> bool {
    qty.abs() * price.abs() < min_notional
}

/// Orders the account already sent today. The entry gate uses both the count and
/// the notional, because dust cleanups can use up a count-only allowance.
fn daily_usage(orders: &[Order], today: chrono::NaiveDate) -> (usize, f64) {
    let today_orders: Vec<&Order> = orders
        .iter()
        .filter(|o| {
            o.submitted_at
                .map(|t| t.date_naive() == today)
                .unwrap_or(false)
        })
        .collect();
    let notional = today_orders
        .iter()
        .map(|o| {
            let filled = o.filled_qty * o.filled_avg_price.unwrap_or(0.0);
            o.notional.unwrap_or(0.0).max(filled).max(0.0)
        })
        .sum();
    (today_orders.len(), notional)
}

/// `None` when a new entry fits the daily budget, otherwise the reason it does not.
fn daily_gate(count: usize, notional: f64, cfg: &Config, equity: f64) -> Option<String> {
    let max_orders = cfg.trading.max_trades_per_day as usize;
    if count >= max_orders {
        return Some(format!(
            "{count} orders already submitted today (limit {max_orders}); no new entry."
        ));
    }
    let cap = equity.max(0.0) * (cfg.trading.max_daily_notional_pct / 100.0);
    if cap > 0.0 && notional >= cap {
        return Some(format!(
            "${notional:.2} traded today (limit ${cap:.2}); no new entry."
        ));
    }
    None
}

/// Fractional and dust quantities are real, so keep precision where it matters.
fn qty_text(qty: f64) -> String {
    if qty.abs() >= 1.0 {
        format!("{qty:.2}")
    } else if qty.abs() >= 0.01 {
        format!("{qty:.4}")
    } else {
        format!("{qty:.7}")
    }
}

fn print_account(
    label: &str,
    account: &crate::data::Account,
    positions: &[Position],
    open_orders: &[Order],
    recent_orders: &[Order],
) {
    let day_pnl = account.daily_pnl_pct();
    let equity = format!("${:.2}", account.equity);
    let cash = format!("${:.2}", account.cash);
    let buying_power = format!("${:.2}", account.available_cash());
    let pnl = format!("{day_pnl:+.2}%");
    println!(
        "\n{} {} | equity {} | cash {} | buying power {} | day P&L {} | PDT {} | day trades {}",
        ansi::bold(label),
        account.account_number,
        ansi::bold(&equity),
        cash,
        buying_power,
        ansi::signed(day_pnl, &pnl),
        account.pattern_day_trader,
        account.daytrade_count
    );

    let active: Vec<&Position> = positions.iter().filter(|p| p.qty.abs() >= 1e-6).collect();
    if active.is_empty() {
        println!(
            "{}",
            ansi::dim("Holdings: none (flat, nothing open right now)")
        );
    } else {
        let mut unrealized = 0.0;
        println!("Holdings ({})", active.len());
        println!(
            "  {:<7}{:>10}{:>10}{:>10}{:>11}{:>12}{:>9}",
            "SYMBOL", "QTY", "ENTRY", "CURRENT", "VALUE", "UNREAL P&L", "PCT"
        );
        for p in &active {
            let current = p.current_price.unwrap_or(0.0);
            let value = p.market_value.unwrap_or(p.qty * current);
            let pl = p
                .unrealized_pl
                .unwrap_or((current - p.avg_entry_price) * p.qty);
            let pct = p.unrealized_plpc.unwrap_or(0.0) * 100.0;
            unrealized += pl;
            println!(
                "  {:<7}{:>10}{:>10.2}{:>10.2}{:>11.2}{:>12}{:>9}",
                p.symbol,
                qty_text(p.qty),
                p.avg_entry_price,
                current,
                value,
                ansi::signed(pl, &format!("{pl:+.2}")),
                ansi::signed(pl, &format!("{pct:+.1}%"))
            );
        }
        println!(
            "  {:<7}{:>10}{:>10}{:>10}{:>11}{:>12}",
            "",
            "",
            "",
            "",
            ansi::dim("total"),
            ansi::signed(unrealized, &format!("{unrealized:+.2}"))
        );
    }

    let today = Utc::now().date_naive();
    let mut ordered: Vec<&Order> = recent_orders.iter().collect();
    ordered.sort_by_key(|o| std::cmp::Reverse(o.submitted_at));
    let mut shown: Vec<&Order> = ordered
        .iter()
        .copied()
        .filter(|o| {
            o.submitted_at
                .map(|t| t.date_naive() == today)
                .unwrap_or(false)
        })
        .collect();
    let heading = "Orders today";
    let shown_heading = if shown.is_empty() {
        shown = ordered.iter().copied().take(3).collect();
        "Last orders"
    } else {
        heading
    };
    if shown.is_empty() {
        println!("{}", ansi::dim("Orders: none"));
    } else {
        println!("{shown_heading} ({})", shown.len());
        for o in &shown {
            let when = o
                .submitted_at
                .map(|t| t.format("%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "--".to_string());
            let size = if o.qty.unwrap_or(0.0) > 0.0 {
                format!("qty {}", qty_text(o.qty.unwrap_or(0.0)))
            } else if let Some(notional) = o.notional {
                format!("$ {notional:.2}")
            } else {
                "-".to_string()
            };
            let fill = if o.filled_qty > 0.0 {
                format!(
                    "filled {} @ {}",
                    qty_text(o.filled_qty),
                    o.filled_avg_price
                        .map(|p| format!("{p:.2}"))
                        .unwrap_or_else(|| "-".to_string())
                )
            } else {
                "unfilled".to_string()
            };
            let side = if o.side.eq_ignore_ascii_case("buy") {
                ansi::green("BUY ")
            } else {
                ansi::red("SELL")
            };
            println!(
                "  {} {} {:<7} {:>12} {} {}",
                when,
                side,
                o.symbol,
                size,
                fill,
                ansi::dim(&o.status)
            );
        }
    }

    if !open_orders.is_empty() {
        println!("Open orders ({})", open_orders.len());
        for o in open_orders {
            println!(
                "  {} {} {} qty {} filled {:.4} status {}",
                o.id,
                o.side,
                o.symbol,
                o.qty
                    .map(|q| format!("{q:.2}"))
                    .unwrap_or_else(|| "-".to_string()),
                o.filled_qty,
                o.status
            );
        }
    }
}

async fn wait_for_open(client: &AlpacaClient) -> Result<()> {
    loop {
        let clock = client.clock().await?;
        if clock.is_open {
            return Ok(());
        }
        let now: DateTime<Utc> = Utc::now();
        let wait = (clock.next_open - now).num_seconds().clamp(30, 300) as u64;
        println!(
            "Market closed. Waiting until {} (sleeping {}s)...",
            clock.next_open.format("%Y-%m-%d %H:%M UTC"),
            wait
        );
        sleep(TokioDuration::from_secs(wait)).await;
    }
}

fn confirm_flatten() -> bool {
    if !std::io::stdin().is_terminal() {
        eprintln!("Refusing automatic live flatten without an interactive terminal");
        return false;
    }
    println!(
        "EMERGENCY FLATTEN: this cancels all orders and sells all positions. Type FLATTEN to confirm:"
    );
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    line.trim() == "FLATTEN"
}

pub fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Paper => "paper",
        Mode::Live => "live",
        _ => "analysis",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(max_orders: u32, notional_pct: f64) -> Config {
        let mut cfg = Config::default();
        cfg.trading.max_trades_per_day = max_orders;
        cfg.trading.max_daily_notional_pct = notional_pct;
        cfg
    }

    #[test]
    fn dust_is_left_alone() {
        // 0.0000007 shares of a $4 stock is not worth an order ticket.
        assert!(is_dust(0.000_000_7, 4.10, 10.0));
        assert!(is_dust(0.5, 12.0, 10.0));
        // A normal position still exits.
        assert!(!is_dust(20.0, 95.67, 10.0));
        assert!(!is_dust(1.0, 1053.46, 10.0));
    }

    #[test]
    fn dust_orders_do_not_consume_the_entry_budget() {
        let cfg = cfg_with(50, 100.0);
        // The six dust sells that used to block every entry.
        assert_eq!(daily_gate(6, 0.000_189, &cfg, 96_642.33), None);
    }

    #[test]
    fn entry_is_blocked_when_the_order_count_is_reached() {
        let cfg = cfg_with(4, 100.0);
        let reason = daily_gate(4, 0.0, &cfg, 10_000.0).expect("blocked");
        assert!(reason.contains("limit 4"), "{reason}");
    }

    #[test]
    fn entry_is_blocked_when_the_notional_ceiling_is_reached() {
        let cfg = cfg_with(50, 100.0);
        let reason = daily_gate(2, 10_000.0, &cfg, 10_000.0).expect("blocked");
        assert!(reason.contains("traded today"), "{reason}");
        // Half the ceiling still trades.
        assert_eq!(daily_gate(2, 5_000.0, &cfg, 10_000.0), None);
    }
}
