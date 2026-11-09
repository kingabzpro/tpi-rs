use chrono::{Duration, NaiveDate, Utc};
use clap::Parser;
use std::io::Write;
use tpi_rs::backtest::engine::PreparedData;
use tpi_rs::backtest::run_benchmark;
use tpi_rs::backtest::types::{BacktestConfig, BacktestResult, FeeModel};
use tpi_rs::cli::{self, Cli, OutputFormat, Selection};
use tpi_rs::config::{self, Config};
use tpi_rs::data::MarketData;
use tpi_rs::data::universe;
use tpi_rs::error::{Error, Result};
use tpi_rs::live;
use tpi_rs::model::{Mode, StrategyMode};
use tpi_rs::paper::{self, TradeOptions};
use tpi_rs::perf;
use tpi_rs::report;
use tpi_rs::scan;
use tpi_rs::tui;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        report::print_error(&e);
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    config::load_dotenv();
    let args = cli::expand_command_args(std::env::args().collect());
    let cli = Cli::parse_from(args);
    cli::init_logging(cli.tui);
    let mut cfg = Config::load(cli.config.as_deref())?;
    if let Some(feed) = &cli.feed {
        cfg.alpaca.feed = feed.clone();
    }
    if cli.no_cache {
        cfg.cache.enabled = false;
    }
    if cli.refresh {
        cfg.cache.refresh = true;
    }
    cfg.normalise();
    cfg.validate()?;
    cfg.ensure_directories()?;

    if cli.doctor {
        return doctor(&cfg, cli.offline).await;
    }
    if cli.perf {
        return tpi_rs::perf_harness::run(&cfg);
    }

    let selection = cli::resolve(&cli, &cfg)?;
    let market = MarketData::new(&cfg, cli.offline, cli.refresh)?;
    let (start, end) = if matches!(selection.mode, Mode::Backtest | Mode::Benchmark) {
        cli::backtest_dates(&cli, &cfg)?
    } else {
        let e = Utc::now().date_naive();
        (e - Duration::days(500), e)
    };

    if cli.tui {
        return tui::run(tui::TuiRequest {
            cfg,
            selection,
            market,
            start,
            end,
            offline: cli.offline,
            refresh: cli.refresh,
            run_validation: !cli.no_walk_forward,
            export: cli.export,
        })
        .await;
    }

    match selection.mode {
        Mode::Analyze | Mode::Scan => run_scan(&cfg, &market, &selection, &cli).await?,
        Mode::Backtest => run_backtest(&cfg, &market, &selection, &cli, start, end).await?,
        Mode::Benchmark => run_benchmark_mode(&cfg, &market, &selection, &cli, start, end).await?,
        Mode::Paper => {
            let options = trade_options(&cli, &selection);
            paper::run_paper(
                &cfg,
                selection.horizon,
                selection.strategy,
                selection.risk,
                None,
                &options,
            )
            .await?;
        }
        Mode::Live => {
            let options = trade_options(&cli, &selection);
            live::run_live(
                &cfg,
                selection.horizon,
                selection.strategy,
                selection.risk,
                None,
                &options,
            )
            .await?;
        }
    }
    Ok(())
}

fn trade_options(cli: &Cli, selection: &Selection) -> TradeOptions {
    TradeOptions {
        dry_run: cli.dry_run,
        once: cli.once,
        wait_for_open: cli.wait_for_open,
        flatten: cli.flatten,
        loop_secs: cli.loop_secs,
        top: selection.top,
        tickers: selection.tickers.clone(),
        i_understand_live_risk: cli.i_understand_live_risk,
    }
}

async fn analysis_symbols(
    market: &MarketData,
    cfg: &Config,
    selection: &Selection,
) -> Result<(Vec<String>, Vec<String>)> {
    if selection.top.is_some() {
        return universe::resolve_symbols(
            market.client(),
            &[],
            selection.top,
            market.offline(),
            &cfg.backtest.universe,
        )
        .await;
    }
    universe::resolve_symbols(
        market.client(),
        &selection.tickers,
        None,
        market.offline(),
        &cfg.backtest.universe,
    )
    .await
}

async fn run_scan(
    cfg: &Config,
    market: &MarketData,
    selection: &Selection,
    cli: &Cli,
) -> Result<()> {
    let (symbols, resolution_warnings) = analysis_symbols(market, cfg, selection).await?;
    if symbols.is_empty() {
        return Err(Error::validation("no symbols to analyze"));
    }
    let as_of = scan::completed_session_date(market).await;
    let end = as_of.unwrap_or_else(|| Utc::now().date_naive());
    let report = scan::analyze_market(
        market,
        &symbols,
        end - Duration::days(500),
        end,
        &scan::AnalysisOptions {
            horizon: selection.horizon,
            strategy: selection.strategy,
            risk: selection.risk,
            vol_target_pct: selection.vol_target_pct,
            risk_pct_override: selection.risk_pct_override,
            equity: selection.capital,
            config: cfg,
            as_of,
            stale_bar_days: cfg.backtest.stale_bar_days,
            cancel: None,
        },
        None,
    )
    .await?;
    for w in resolution_warnings {
        eprintln!("NOTE: {w}");
    }
    match cli.output {
        OutputFormat::Json => {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            serde_json::to_writer_pretty(&mut lock, &report)?;
            writeln!(lock)?;
            for w in &report.warnings {
                eprintln!("NOTE: {w}");
            }
        }
        OutputFormat::Table => report::print_scan(&report, selection.mode == Mode::Analyze),
    }
    if let Some(dir) = &cli.export {
        let paths = report::export_scan(dir, &report)?;
        println!(
            "Exported: {}",
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

async fn run_backtest(
    cfg: &Config,
    market: &MarketData,
    selection: &Selection,
    cli: &Cli,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<()> {
    let universe = effective_universe(selection)?;
    let (data, warnings) = load_prepared(cfg, market, &universe, start, end).await?;
    for w in &warnings {
        eprintln!("NOTE: {w}");
    }
    let mut base = backtest_config(cfg, selection, start, end);
    if cli.offline {
        base.label = format!("[SYNTHETIC DEMO] {}", base.label);
    }
    let params = cfg.horizon_params(selection.horizon);
    let mut results: Vec<BacktestResult> = Vec::new();
    match selection.strategy {
        StrategyMode::Compare => {
            for strategy in [StrategyMode::Tpi, StrategyMode::Consensus] {
                let mut c = base.clone();
                c.strategy = strategy;
                c.label = format!("{} [{}]", c.label, strategy.label());
                results.push(tpi_rs::backtest::engine::run(&data, &c, params)?);
            }
        }
        strategy => {
            let mut c = base.clone();
            c.strategy = strategy;
            results.push(tpi_rs::backtest::engine::run(&data, &c, params)?);
        }
    }
    match cli.output {
        OutputFormat::Json => {
            let value = if results.len() == 1 {
                serde_json::to_value(&results[0])?
            } else {
                serde_json::json!({ "tpi": &results[0], "consensus": &results[1] })
            };
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            serde_json::to_writer_pretty(&mut lock, &value)?;
            writeln!(lock)?;
        }
        OutputFormat::Table => {
            for result in &results {
                report::print_backtest(result, true);
                println!();
            }
            if results.len() == 2 {
                println!("Strategy comparison:");
                report::print_metrics_row("TPI", &results[0].metrics);
                report::print_metrics_row("TPI+Consensus", &results[1].metrics);
            }
        }
    }
    if let Some(dir) = &cli.export {
        for result in &results {
            let prefix = format!("{}-{}", sanitize(&result.label), report::timestamp_prefix());
            let paths = report::export_backtest(dir, &prefix, result)?;
            println!("Exported {} files to {}", paths.len(), dir.display());
        }
    }
    Ok(())
}

async fn run_benchmark_mode(
    cfg: &Config,
    market: &MarketData,
    selection: &Selection,
    cli: &Cli,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<()> {
    let universe = effective_universe(selection)?;
    let (data, load_result) =
        load_prepared_with_provenance(cfg, market, &universe, start, end).await?;
    for w in &load_result.warnings {
        eprintln!("NOTE: {w}");
    }
    let mut base = backtest_config(cfg, selection, start, end);
    if cli.offline {
        base.label = format!("[SYNTHETIC DEMO] {}", base.label);
    }
    let params = cfg.horizon_params(selection.horizon);
    println!("Running headline, walk-forward and stability analysis...");
    let report = run_benchmark(
        &data,
        &base,
        params,
        &cfg.backtest.walk_forward,
        load_result.provenance,
        !cli.no_walk_forward,
    )?;
    match cli.output {
        OutputFormat::Json => {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            serde_json::to_writer_pretty(&mut lock, &report)?;
            writeln!(lock)?;
        }
        OutputFormat::Table => report::print_benchmark(&report),
    }
    if let Some(dir) = &cli.export {
        let paths = report::export_benchmark(dir, &report)?;
        println!("Exported {} files to {}", paths.len(), dir.display());
    }
    Ok(())
}

fn effective_universe(selection: &Selection) -> Result<Vec<String>> {
    let list = if !selection.tickers.is_empty() {
        universe::parse_tickers(&selection.tickers)
    } else {
        selection.universe.clone()
    };
    if list.is_empty() {
        return Err(Error::validation("backtest universe is empty"));
    }
    Ok(list)
}

fn backtest_config(
    cfg: &Config,
    selection: &Selection,
    start: NaiveDate,
    end: NaiveDate,
) -> BacktestConfig {
    BacktestConfig {
        start,
        end,
        initial_capital: selection.capital,
        horizon: selection.horizon,
        strategy: selection.strategy,
        risk: selection.risk,
        vol_target_pct: selection.vol_target_pct,
        risk_pct_override: selection.risk_pct_override,
        fees: FeeModel {
            commission_bps: cfg.backtest.commission_bps,
            spread_bps: cfg.backtest.spread_bps,
            slippage_bps: cfg.backtest.slippage_bps,
            sec_fee_bps: cfg.backtest.sec_fee_bps,
            finra_taf_per_share: cfg.backtest.finra_taf_per_share,
        },
        allow_fractional: cfg.backtest.allow_fractional,
        risk_free_rate_pct: cfg.strategy.risk_free_rate_pct,
        stale_bar_days: cfg.backtest.stale_bar_days,
        min_history_bars: cfg.strategy.min_history_bars,
        max_hold_days_override: cfg.backtest.max_hold_days_override,
        label: format!("TPI {} {}..{}", selection.horizon.short_label(), start, end),
    }
}

async fn load_prepared(
    cfg: &Config,
    market: &MarketData,
    universe: &[String],
    start: NaiveDate,
    end: NaiveDate,
) -> Result<(PreparedData, Vec<String>)> {
    let (data, load) = load_prepared_with_provenance(cfg, market, universe, start, end).await?;
    Ok((data, load.warnings))
}

async fn load_prepared_with_provenance(
    cfg: &Config,
    market: &MarketData,
    universe: &[String],
    start: NaiveDate,
    end: NaiveDate,
) -> Result<(PreparedData, tpi_rs::data::LoadResult)> {
    let benchmarks = cfg.backtest.benchmark_symbols.clone();
    let mut symbols = universe.to_vec();
    for b in &benchmarks {
        if !symbols.contains(b) {
            symbols.push(b.clone());
        }
    }
    let warm_days = ((cfg.backtest.warmup_trading_days.max(220) as i64) * 2).max(450);
    let warm_start = start - Duration::days(warm_days);
    let load = market.load(&symbols, warm_start, end).await?;
    let data = PreparedData::new(&load.bars, universe, &benchmarks)?;
    Ok((data, load))
}

fn sanitize(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

async fn doctor(cfg: &Config, offline: bool) -> Result<()> {
    let mut critical_ok = true;
    println!("TPI-RS {} doctor", tpi_rs::VERSION);
    println!("{}", "-".repeat(72));
    report::print_doctor_check("config validation", true, "loaded and valid");
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    let stats = perf::snapshot();
    println!(
        "Rayon/CPU threads: {}, process working set {:.1} MB, peak {:.1} MB",
        threads, stats.working_set_mb, stats.peak_working_set_mb
    );
    match cfg.ensure_directories() {
        Ok(()) => report::print_doctor_check(
            "directories",
            true,
            &format!(
                "cache={} reports={}",
                cfg.cache_path().display(),
                cfg.report_dir().display()
            ),
        ),
        Err(e) => {
            critical_ok = false;
            report::print_doctor_check("directories", false, &e.to_string());
        }
    }
    if cfg.cache.enabled {
        match tpi_rs::data::cache::CandleCache::open(&cfg.cache_path()) {
            Ok(cache) => report::print_doctor_check(
                "SQLite cache",
                true,
                &format!(
                    "{} ({} bars)",
                    cfg.cache_path().display(),
                    cache.total_bars().unwrap_or(0)
                ),
            ),
            Err(e) => {
                critical_ok = false;
                report::print_doctor_check("SQLite cache", false, &e.to_string());
            }
        }
    } else {
        report::print_doctor_check("SQLite cache", true, "disabled by configuration/--no-cache");
    }
    println!(
        "Live trading: {}",
        if cfg.trading.allow_live {
            "ENABLED"
        } else {
            "disabled (safe default)"
        }
    );

    if offline {
        report::print_doctor_check("credentials", true, "skipped (--offline synthetic mode)");
        report::print_doctor_check("Alpaca connectivity", true, "skipped (--offline)");
        println!("\nDoctor finished. Offline mode is ready.");
        return Ok(());
    }

    if !cfg.alpaca.has_credentials() {
        report::print_doctor_check(
            "credentials",
            false,
            "ALPACA_API_KEY / ALPACA_SECRET_KEY missing; copy .env.example to .env",
        );
        println!("\nDoctor found blocking problems and network checks were skipped.");
        return Err(Error::MissingCredentials);
    }
    report::print_doctor_check(
        "credentials",
        true,
        &format!(
            "key {}... configured (paper URL {})",
            cfg.alpaca.key_id.chars().take(4).collect::<String>(),
            cfg.alpaca.paper_url
        ),
    );

    let client = tpi_rs::data::AlpacaClient::new(&cfg.alpaca, true)?;
    match client.clock().await {
        Ok(clock) => report::print_doctor_check(
            "Alpaca clock",
            true,
            &format!(
                "market {}; next open {}",
                if clock.is_open { "open" } else { "closed" },
                clock.next_open.format("%Y-%m-%d %H:%M UTC")
            ),
        ),
        Err(e) => {
            critical_ok = false;
            report::print_doctor_check("Alpaca clock", false, &e.to_string());
        }
    }
    let today = Utc::now().date_naive();
    match client.calendar(today, today + Duration::days(7)).await {
        Ok(days) => report::print_doctor_check(
            "Market calendar",
            true,
            &format!("{} sessions in the next 7 days", days.len()),
        ),
        Err(e) => {
            critical_ok = false;
            report::print_doctor_check("Market calendar", false, &e.to_string());
        }
    }
    match client.account().await {
        Ok(account) => report::print_doctor_check(
            "Paper account",
            true,
            &format!(
                "equity ${:.2}, cash ${:.2}, status {}",
                account.equity, account.cash, account.status
            ),
        ),
        Err(e) => {
            critical_ok = false;
            report::print_doctor_check("Paper account", false, &e.to_string());
        }
    }
    match client
        .daily_bars(&["SPY".to_string()], today - Duration::days(12), today)
        .await
    {
        Ok(bars) => {
            let count = bars.get("SPY").map(|b| b.len()).unwrap_or(0);
            if count == 0 {
                critical_ok = false;
                report::print_doctor_check(
                    "Historical bars (SPY)",
                    false,
                    "no bars returned; check the feed setting (iex is free)",
                );
            } else {
                report::print_doctor_check(
                    "Historical bars (SPY)",
                    true,
                    &format!("{count} daily bars, feed={}", cfg.alpaca.feed),
                );
            }
        }
        Err(e) => {
            critical_ok = false;
            report::print_doctor_check("Historical bars (SPY)", false, &e.to_string());
        }
    }
    println!("{}", "-".repeat(72));
    if critical_ok {
        println!("Doctor finished: all checks passed. Run `tpi` or `tpi --mode benchmark`.");
        Ok(())
    } else {
        Err(Error::config(
            "doctor found blocking problems; fix the FAIL lines above",
        ))
    }
}
