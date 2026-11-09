//! Paper-trading execution loop and shared broker session logic.
//!
//! Orders are market orders submitted to Alpaca. The engine uses the broker as
//! the source of truth: account, positions and open orders are re-read every
//! cycle, and order submissions are reconciled by `client_order_id` after an
//! ambiguous network failure. Live trading is gated in `live`.

use crate::config::Config;
use crate::data::alpaca::{AlpacaClient, Order, OrderRequest, Position};
use crate::data::{MarketData, universe};
use crate::error::{Error, Result};
use crate::model::{Action, Horizon, Mode, RiskProfile, StrategyMode};
use crate::scan::{self, AnalysisOptions};
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
    println!("TPI-RS {label} trading | {horizon} | {strategy} | risk {risk}");

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

    let clock = client.clock().await?;
    println!(
        "Market {} | now {} | next open {} | next close {}",
        if clock.is_open { "OPEN" } else { "CLOSED" },
        clock.timestamp.format("%Y-%m-%d %H:%M UTC"),
        clock.next_open.format("%Y-%m-%d %H:%M UTC"),
        clock.next_close.format("%Y-%m-%d %H:%M UTC")
    );
    if !clock.is_open && options.wait_for_open {
        wait_for_open(&client).await?;
    }

    let market = MarketData::new(cfg, false, false)?;
    loop {
        let cycle_clock = client.clock().await?;
        let market_is_open = cycle_clock.is_open;
        let today = cycle_clock.timestamp.date_naive();
        let as_of = if market_is_open {
            today - Duration::days(1)
        } else {
            today
        };
        let account = client.account().await?;
        if account.account_blocked || account.trading_blocked {
            println!(
                "Account is blocked (account_blocked={}, trading_blocked={}); no orders.",
                account.account_blocked, account.trading_blocked
            );
            return Ok(());
        }
        let positions = client.positions().await?;
        let active_positions: Vec<&Position> =
            positions.iter().filter(|p| p.qty.abs() >= 1e-6).collect();
        let open_orders = client.orders("open").await?;
        print_account(label, &account, &positions, &open_orders);

        let mut symbols = universe::parse_tickers(&options.tickers);
        for p in &active_positions {
            if !symbols.contains(&p.symbol) {
                symbols.push(p.symbol.clone());
            }
        }
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
        .await?;
        for w in &analysis.warnings {
            println!("NOTE: {w}");
        }
        if let Some(as_of) = analysis.as_of {
            println!("Signals as of completed session {as_of}");
        }

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
            let today_orders = client.orders("all").await.unwrap_or_default();
            let today = Utc::now().date_naive();
            let trades_today = today_orders
                .iter()
                .filter(|o| {
                    o.submitted_at
                        .map(|t| t.date_naive() == today)
                        .unwrap_or(false)
                })
                .count();
            if trades_today >= cfg.trading.max_trades_per_day as usize {
                println!(
                    "RISK: {} orders already submitted today (limit {}); no new entry.",
                    trades_today, cfg.trading.max_trades_per_day
                );
            } else if let Some(best) = analysis.strongest() {
                if active_positions.iter().any(|p| p.symbol == best.symbol) {
                    println!("{}: already held; no add-on", best.symbol);
                } else if let Some(order) = build_entry_order(cfg, best, &account).await? {
                    submit_and_report(
                        &client,
                        &order,
                        dry_run,
                        cfg,
                        &format!("entry {} strength {:.0}", best.symbol, best.strength),
                    )
                    .await?;
                }
            } else {
                println!("No qualified BUY candidate this cycle.");
            }
        }

        if dry_run {
            println!("DRY RUN: no orders were submitted.");
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
    description: &str,
) -> Result<Option<Order>> {
    if dry_run {
        println!(
            "DRY RUN order: {} | {} {} qty={} notional={} cid={}",
            description,
            req.side,
            req.symbol,
            req.qty.clone().unwrap_or_else(|| "-".into()),
            req.notional.clone().unwrap_or_else(|| "-".into()),
            req.client_order_id
        );
        return Ok(None);
    }
    println!(
        "Submitting {description} | client_order_id {}",
        req.client_order_id
    );
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

fn print_account(
    label: &str,
    account: &crate::data::Account,
    positions: &[Position],
    open_orders: &[Order],
) {
    println!(
        "\n{label} account {} | status {} | equity ${:.2} | cash ${:.2} | buying power ${:.2} | day P&L {:+.2}% | PDT {} | day trades {}",
        account.account_number,
        account.status,
        account.equity,
        account.cash,
        account.available_cash(),
        account.daily_pnl_pct(),
        account.pattern_day_trader,
        account.daytrade_count
    );
    let active = positions.iter().filter(|p| p.qty.abs() >= 1e-6).count();
    if positions.is_empty() {
        println!("Positions: none");
    } else {
        println!(
            "Positions ({} active, {} dust):",
            active,
            positions.len().saturating_sub(active)
        );
        for p in positions {
            println!(
                "  {} qty {:.4} avg {:.2} current {:.2} value {:.2} unrealized {:.2}",
                p.symbol,
                p.qty,
                p.avg_entry_price,
                p.current_price.unwrap_or(0.0),
                p.market_value.unwrap_or(0.0),
                p.unrealized_pl.unwrap_or(0.0)
            );
        }
    }
    if !open_orders.is_empty() {
        println!("Open orders:");
        for o in open_orders {
            println!(
                "  {} {} {} qty {:?} filled {:.4} status {}",
                o.id, o.side, o.symbol, o.qty, o.filled_qty, o.status
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
