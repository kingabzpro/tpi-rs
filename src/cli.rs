//! Command-line interface and interactive startup prompts.
//!
//! Every prompt has an equivalent flag so the same behaviour is scriptable.

use crate::config::Config;
use crate::data::universe::{self, parse_tickers};
use crate::error::{Error, Result};
use crate::model::{Horizon, Mode, RiskProfile, StrategyMode};
use chrono::NaiveDate;
use clap::{Parser, ValueEnum};
use dialoguer::{Input, Select, theme::ColorfulTheme};
use std::io::IsTerminal;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
}

/// Turn `tpi run AAPL`, `tpi backtest`, and similar commands into the flat
/// flag form the parser understands. Anything unrecognised is left untouched.
pub fn expand_command_args(args: Vec<String>) -> Vec<String> {
    if args.len() < 2 {
        return args;
    }
    let command = args[1].to_ascii_lowercase();
    let mut expanded = vec![args[0].clone()];
    let mut rest_start = 2;
    match command.as_str() {
        "run" | "scan" => {
            expanded.push("--mode".into());
            expanded.push("scan".into());
        }
        "analyze" | "analyse" => {
            expanded.push("--mode".into());
            expanded.push("analyze".into());
        }
        "backtest" | "bt" => {
            expanded.push("--mode".into());
            expanded.push("backtest".into());
        }
        "benchmark" | "bench" => {
            expanded.push("--mode".into());
            expanded.push("benchmark".into());
        }
        "paper" => {
            expanded.push("--mode".into());
            expanded.push("paper".into());
        }
        "live" => {
            expanded.push("--mode".into());
            expanded.push("live".into());
        }
        "tui" | "dashboard" => {
            expanded.push("--tui".into());
            let has_target = args[2..].iter().any(|value| {
                !value.starts_with('-')
                    || value == "--tickers"
                    || value.starts_with("--tickers=")
                    || value == "--top"
                    || value.starts_with("--top=")
                    || value == "--mode"
                    || value.starts_with("--mode=")
            });
            if !has_target {
                expanded.push("--mode".into());
                expanded.push("scan".into());
                expanded.push("--top".into());
                expanded.push("100".into());
            }
        }
        "doctor" => expanded.push("--doctor".into()),
        "perf" => expanded.push("--perf".into()),
        "help" => {
            expanded.push("--help".into());
            rest_start = args.len();
        }
        "version" => {
            expanded.push("--version".into());
            rest_start = args.len();
        }
        _ => return args,
    }

    // Leading non-flag tokens after the command are tickers or a universe.
    let mut positionals = Vec::new();
    let mut i = rest_start;
    while i < args.len() && !args[i].starts_with('-') {
        positionals.push(args[i].clone());
        i += 1;
    }
    if !positionals.is_empty() {
        expanded.push("--tickers".into());
        expanded.push(positionals.join(","));
    }
    expanded.extend(args[i..].iter().cloned());
    expanded
}

#[derive(Parser, Debug)]
#[command(
    name = "tpi",
    version,
    about = "A super fast algorithmic trading system for US stocks, built in Rust",
    after_help = "Commands:
  tpi                     interactive prompts
  tpi run [TICKERS]       scan tickers, or the Top 100 if none are given
  tpi analyze TICKERS     detailed analysis with explanations
  tpi scan [TICKERS]      ranked signal table
  tpi backtest [TICKERS]  next-open backtest
  tpi benchmark           three-year benchmark with validation
  tpi paper               paper trading
  tpi live                live trading, gated
  tpi tui [TICKERS]       terminal dashboard
  tpi doctor              setup and connectivity checks
  tpi perf                performance harness

Examples:
  tpi
  tpi run AAPL,MSFT --horizon short
  tpi scan --top 100 --strategy consensus
  tpi backtest --start 2023-11-01 --end 2026-11-01
  tpi benchmark --strategy compare
  tpi paper --once --dry-run

The flat flags still work: tpi --mode backtest.
Signals are rule-based conditions, not price forecasts."
)]
pub struct Cli {
    /// Ticker(s), comma separated. Use "top100" or "auto" for discovery.
    #[arg(long, value_delimiter = ',')]
    pub tickers: Vec<String>,

    /// Scan the top N most-active US stocks (N defaults to 100).
    #[arg(long, num_args = 0..=1, default_missing_value = "100")]
    pub top: Option<usize>,

    #[arg(long, value_enum)]
    pub horizon: Option<Horizon>,

    #[arg(long, value_enum)]
    pub mode: Option<Mode>,

    #[arg(long, value_enum)]
    pub strategy: Option<StrategyMode>,

    #[arg(long, value_enum)]
    pub risk: Option<RiskProfile>,

    /// Starting capital / analysis equity in USD.
    #[arg(long)]
    pub capital: Option<f64>,

    /// TOML configuration file.
    #[arg(long, env = "TPI_CONFIG")]
    pub config: Option<PathBuf>,

    /// Force interactive prompts even when flags are present.
    #[arg(long)]
    pub interactive: bool,

    /// Never prompt; require flags and use documented defaults.
    #[arg(long)]
    pub no_interactive: bool,

    /// Launch the Ratatui dashboard.
    #[arg(long)]
    pub tui: bool,

    #[arg(long, value_enum, default_value = "table")]
    pub output: OutputFormat,

    /// Directory for CSV/JSON exports.
    #[arg(long)]
    pub export: Option<PathBuf>,

    /// Deterministic synthetic data: no network, no credentials. Clearly labelled.
    #[arg(long)]
    pub offline: bool,

    /// Ignore the cache TTL and refetch the requested range.
    #[arg(long)]
    pub refresh: bool,

    /// Disable the SQLite cache for this run.
    #[arg(long)]
    pub no_cache: bool,

    /// Override the Alpaca market-data feed (iex, sip, delayed_sip, otc, boats).
    #[arg(long)]
    pub feed: Option<String>,

    /// Backtest/benchmark start date (YYYY-MM-DD).
    #[arg(long)]
    pub start: Option<NaiveDate>,

    /// Backtest/benchmark end date (YYYY-MM-DD).
    #[arg(long)]
    pub end: Option<NaiveDate>,

    /// Backtest universe (comma separated); overrides config.
    #[arg(long, value_delimiter = ',')]
    pub universe: Vec<String>,

    /// Run one trading cycle and exit.
    #[arg(long)]
    pub once: bool,

    /// Do not submit orders; print the intended actions.
    #[arg(long)]
    pub dry_run: bool,

    /// Wait for the next market open before trading.
    #[arg(long)]
    pub wait_for_open: bool,

    /// Emergency: cancel all open orders and close all positions.
    #[arg(long)]
    pub flatten: bool,

    /// Trading loop interval in seconds.
    #[arg(long)]
    pub loop_secs: Option<u64>,

    /// Required acknowledgement for live trading.
    #[arg(long)]
    pub i_understand_live_risk: bool,

    /// Skip walk-forward, stability and OOS analysis in benchmark mode.
    #[arg(long)]
    pub no_walk_forward: bool,

    /// Named profile from the config file.
    #[arg(long)]
    pub profile: Option<String>,

    /// Check installation, credentials, cache, and API connectivity.
    #[arg(long)]
    pub doctor: bool,

    /// Run the synthetic performance harness (latency, throughput, memory, CPU).
    #[arg(long)]
    pub perf: bool,
}

#[derive(Debug, Clone)]
pub struct Selection {
    pub tickers: Vec<String>,
    pub top: Option<usize>,
    pub horizon: Horizon,
    pub mode: Mode,
    pub strategy: StrategyMode,
    pub risk: RiskProfile,
    pub capital: f64,
    pub universe: Vec<String>,
}

pub fn parse_date(value: &str, field: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map_err(|e| Error::validation(format!("{field} '{value}' is not YYYY-MM-DD: {e}")))
}

pub fn backtest_dates(cli: &Cli, cfg: &Config) -> Result<(NaiveDate, NaiveDate)> {
    let start = match cli.start {
        Some(d) => d,
        None => parse_date(&cfg.backtest.start, "backtest.start")?,
    };
    let end = match cli.end {
        Some(d) => d,
        None => parse_date(&cfg.backtest.end, "backtest.end")?,
    };
    if start >= end {
        return Err(Error::validation("start date must be before end date"));
    }
    Ok((start, end))
}

pub fn resolve(cli: &Cli, cfg: &Config) -> Result<Selection> {
    let profile = match &cli.profile {
        Some(name) => Some(
            cfg.profiles
                .get(name)
                .ok_or_else(|| {
                    Error::validation(format!(
                        "profile '{name}' not found; available: {}",
                        cfg.profiles.keys().cloned().collect::<Vec<_>>().join(", ")
                    ))
                })?
                .clone(),
        ),
        None => None,
    };
    let mut horizon = cli
        .horizon
        .or(profile.as_ref().and_then(|p| p.horizon))
        .unwrap_or(Horizon::Medium);
    let mut mode = cli
        .mode
        .or(profile.as_ref().and_then(|p| p.mode))
        .unwrap_or(Mode::Analyze);
    let mut strategy = cli
        .strategy
        .or(profile.as_ref().and_then(|p| p.strategy))
        .unwrap_or(StrategyMode::Tpi);
    let mut risk = cli
        .risk
        .or(profile.as_ref().and_then(|p| p.risk))
        .unwrap_or(RiskProfile::Balanced);
    let mut capital = cli
        .capital
        .or(profile.as_ref().and_then(|p| p.capital))
        .unwrap_or(cfg.backtest.initial_capital);
    let mut tickers = if cli.tickers.is_empty() {
        profile
            .as_ref()
            .map(|p| p.tickers.clone())
            .unwrap_or_default()
    } else {
        cli.tickers.clone()
    };
    let mut top = cli.top;
    let mut universe: Vec<String> = if cli.universe.is_empty() {
        profile
            .as_ref()
            .and_then(|p| p.universe.clone())
            .unwrap_or_else(|| cfg.backtest.universe.clone())
    } else {
        cli.universe.clone()
    };

    if universe::wants_discovery(&tickers) {
        top = Some(top.unwrap_or(100));
        tickers.retain(|t| !universe::wants_discovery(std::slice::from_ref(t)));
    }

    let tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let has_selection = cli.mode.is_some()
        || !cli.tickers.is_empty()
        || cli.top.is_some()
        || cli.horizon.is_some()
        || cli.strategy.is_some()
        || cli.risk.is_some()
        || cli.capital.is_some()
        || cli.start.is_some()
        || cli.end.is_some()
        || cli.doctor
        || cli.perf
        || cli.flatten;
    let interactive = cli.interactive || (tty && !cli.no_interactive && !has_selection);

    if interactive {
        println!(
            "TPI-RS {} | algorithmic trend/breakout trading",
            crate::VERSION
        );
        let default_tickers = if !tickers.is_empty() {
            tickers.join(",")
        } else if matches!(mode, Mode::Backtest | Mode::Benchmark) && !universe.is_empty() {
            universe.join(",")
        } else {
            "top100".to_string()
        };
        let input: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Ticker(s), comma separated, or top100/auto (backtest/benchmark: enter a universe; blank = config)")
            .default(default_tickers)
            .allow_empty(true)
            .interact_text()
            .map_err(prompt_err)?;

        let horizon_items = [
            "Short-term (1-5 day setup, trend exit)",
            "Medium (1-4 weeks)",
            "Long (1-6 months)",
        ];
        let h_idx = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Trading horizon")
            .items(horizon_items)
            .default(match horizon {
                Horizon::Short => 0,
                Horizon::Medium => 1,
                Horizon::Long => 2,
            })
            .interact()
            .map_err(prompt_err)?;
        horizon = match h_idx {
            0 => Horizon::Short,
            1 => Horizon::Medium,
            _ => Horizon::Long,
        };

        let mode_items = [
            "Analyze",
            "Scan",
            "Backtest",
            "3-Year Benchmark",
            "Paper Trading",
            "Live Trading",
        ];
        let mode_idx = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Mode")
            .items(mode_items)
            .default(match mode {
                Mode::Analyze => 0,
                Mode::Scan => 1,
                Mode::Backtest => 2,
                Mode::Benchmark => 3,
                Mode::Paper => 4,
                Mode::Live => 5,
            })
            .interact()
            .map_err(prompt_err)?;
        mode = match mode_idx {
            0 => Mode::Analyze,
            1 => Mode::Scan,
            2 => Mode::Backtest,
            3 => Mode::Benchmark,
            4 => Mode::Paper,
            _ => Mode::Live,
        };
        let parsed = parse_tickers(std::slice::from_ref(&input));
        if matches!(mode, Mode::Backtest | Mode::Benchmark) {
            if parsed.is_empty() || universe::wants_discovery(&parsed) {
                if !parsed.is_empty() {
                    println!(
                        "NOTE: backtest/benchmark use the configured universe; Top-N discovery is for scans"
                    );
                }
                tickers.clear();
            } else {
                universe = parsed;
                tickers.clear();
            }
            top = None;
        } else {
            tickers = parsed;
            top = None;
            if universe::wants_discovery(&tickers) {
                top = Some(100);
                tickers.clear();
            }
        }

        let strategy_items = ["TPI", "TPI + Technical Consensus", "Compare both"];
        let s_idx = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Strategy")
            .items(strategy_items)
            .default(match strategy {
                StrategyMode::Tpi => 0,
                StrategyMode::Consensus => 1,
                StrategyMode::Compare => 2,
            })
            .interact()
            .map_err(prompt_err)?;
        strategy = match s_idx {
            0 => StrategyMode::Tpi,
            1 => StrategyMode::Consensus,
            _ => StrategyMode::Compare,
        };

        let risk_items = ["Conservative", "Balanced", "Aggressive"];
        let r_idx = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Risk profile")
            .items(risk_items)
            .default(match risk {
                RiskProfile::Conservative => 0,
                RiskProfile::Balanced => 1,
                RiskProfile::Aggressive => 2,
            })
            .interact()
            .map_err(prompt_err)?;
        risk = match r_idx {
            0 => RiskProfile::Conservative,
            1 => RiskProfile::Balanced,
            _ => RiskProfile::Aggressive,
        };

        capital = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Starting capital (USD)")
            .default(capital)
            .validate_with(|v: &f64| {
                if *v > 0.0 {
                    Ok(())
                } else {
                    Err("capital must be positive")
                }
            })
            .interact()
            .map_err(prompt_err)?;

        println!(
            "\nSelected: mode={} horizon={} strategy={} risk={} capital=${:.2} {}",
            mode.label(),
            horizon.short_label(),
            strategy.label(),
            risk.label(),
            capital,
            if let Some(t) = top {
                format!("top {t}")
            } else if tickers.is_empty() {
                format!("universe {}", universe.len())
            } else {
                tickers.join(",")
            }
        );
    } else {
        if tickers.is_empty() && top.is_none() {
            match mode {
                Mode::Analyze => {
                    return Err(Error::validation(
                        "no tickers. Pass --tickers AAPL,MSFT, --top 100, or run tpi interactively",
                    ));
                }
                Mode::Scan | Mode::Paper | Mode::Live => {
                    top = Some(100);
                }
                Mode::Backtest | Mode::Benchmark => {
                    if universe.is_empty() {
                        return Err(Error::validation("backtest universe is empty"));
                    }
                }
            }
        }
    }

    if capital <= 0.0 || !capital.is_finite() {
        return Err(Error::validation("capital must be a positive number"));
    }
    if cli.tui && mode.is_trading() {
        return Err(Error::validation(
            "--tui is for analysis/backtest dashboards; paper/live trading runs in the console",
        ));
    }
    if cli.tui && cli.output == OutputFormat::Json {
        return Err(Error::validation(
            "--tui and --output json are mutually exclusive",
        ));
    }
    if matches!(mode, Mode::Backtest | Mode::Benchmark) {
        universe = universe::resolve_universe(&universe, &cfg.backtest.universe)?;
    }

    Ok(Selection {
        tickers,
        top,
        horizon,
        mode,
        strategy,
        risk,
        capital,
        universe,
    })
}

fn prompt_err(e: dialoguer::Error) -> Error {
    Error::validation(format!("interactive prompt failed: {e}"))
}

pub fn init_logging(tui: bool) {
    if tui {
        // Keep the alternate screen free of log output.
        return;
    }
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::expand_command_args;

    fn expand(args: &[&str]) -> Vec<String> {
        expand_command_args(args.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn expands_run_with_positional_tickers() {
        let out = expand(&["tpi", "run", "AAPL", "MSFT", "--horizon", "short"]);
        assert_eq!(
            out,
            vec![
                "tpi",
                "--mode",
                "scan",
                "--tickers",
                "AAPL,MSFT",
                "--horizon",
                "short"
            ]
        );
    }

    #[test]
    fn expands_analyze_and_flags() {
        let out = expand(&["tpi", "analyze", "AAPL", "--offline"]);
        assert_eq!(
            out,
            vec!["tpi", "--mode", "analyze", "--tickers", "AAPL", "--offline"]
        );
    }

    #[test]
    fn expands_backtest_and_benchmark() {
        assert_eq!(
            expand(&["tpi", "backtest", "--start", "2024-01-01"]),
            vec!["tpi", "--mode", "backtest", "--start", "2024-01-01"]
        );
        assert_eq!(
            expand(&["tpi", "bench"]),
            vec!["tpi", "--mode", "benchmark"]
        );
    }

    #[test]
    fn expands_tui_default_target() {
        assert_eq!(
            expand(&["tpi", "tui"]),
            vec!["tpi", "--tui", "--mode", "scan", "--top", "100"]
        );
        assert_eq!(
            expand(&["tpi", "tui", "--offline"]),
            vec![
                "tpi",
                "--tui",
                "--mode",
                "scan",
                "--top",
                "100",
                "--offline"
            ]
        );
        assert_eq!(
            expand(&["tpi", "tui", "AAPL"]),
            vec!["tpi", "--tui", "--tickers", "AAPL"]
        );
    }

    #[test]
    fn expands_doctor_perf_and_help() {
        assert_eq!(
            expand(&["tpi", "doctor", "--offline"]),
            vec!["tpi", "--doctor", "--offline"]
        );
        assert_eq!(expand(&["tpi", "perf"]), vec!["tpi", "--perf"]);
        assert_eq!(expand(&["tpi", "help"]), vec!["tpi", "--help"]);
    }

    #[test]
    fn leaves_flat_invocations_alone() {
        let flat: Vec<String> = ["tpi", "--offline", "--mode", "scan"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(expand_command_args(flat.clone()), flat);
        assert_eq!(expand_command_args(vec!["tpi".into()]), vec!["tpi"]);
    }
}
