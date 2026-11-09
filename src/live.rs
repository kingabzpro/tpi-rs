//! Live trading gate. Live orders are refused unless configuration, an
//! explicit CLI acknowledgement and an interactive typed confirmation all
//! agree. Paper trading never passes through this module.

use crate::config::Config;
use crate::error::{Error, Result};
use crate::model::{Horizon, RiskProfile, StrategyMode};
use crate::paper::{self, TradeOptions};
use std::io::{IsTerminal, Write};

pub async fn run_live(
    cfg: &Config,
    horizon: Horizon,
    strategy: StrategyMode,
    risk: RiskProfile,
    capital_override: Option<f64>,
    options: &TradeOptions,
) -> Result<()> {
    preflight(cfg, options)?;
    println!();
    println!("==========================================================");
    println!("                 LIVE TRADING - REAL MONEY");
    println!("==========================================================");
    println!("Account URL : {}", cfg.alpaca.live_url);
    println!("Strategy    : {strategy}, {horizon}, risk {risk}");
    println!(
        "Limits      : order cap {:.1}% equity, daily loss {:.1}%, max trades {}/day, max positions {}",
        cfg.trading.max_order_notional_pct,
        cfg.trading.max_daily_loss_pct,
        cfg.trading.max_trades_per_day,
        cfg.trading.max_open_positions
    );
    println!("Stop trading: press Ctrl+C; emergency flatten with --flatten.");
    println!();

    if options.flatten {
        require_phrase(
            "FLATTEN",
            "type FLATTEN to cancel all orders and close all positions",
        )?;
    } else {
        require_phrase("LIVE", "type LIVE to start submitting real orders")?;
    }
    paper::run_session(
        cfg,
        false,
        horizon,
        strategy,
        risk,
        capital_override,
        options,
    )
    .await
}

fn preflight(cfg: &Config, options: &TradeOptions) -> Result<()> {
    if !cfg.trading.allow_live {
        return Err(Error::LiveTradingRefused(
            "trading.allow_live is false; set it in config or TPI_ALLOW_LIVE=1".to_string(),
        ));
    }
    if cfg.trading.paper {
        return Err(Error::LiveTradingRefused(
            "trading.paper is true; set it to false as an explicit live acknowledgement"
                .to_string(),
        ));
    }
    if !options.i_understand_live_risk {
        return Err(Error::LiveTradingRefused(
            "the --i-understand-live-risk flag is required".to_string(),
        ));
    }
    if !cfg.alpaca.has_credentials() {
        return Err(Error::MissingCredentials);
    }
    // Hard safety ceilings independent of the config file.
    if cfg.trading.max_order_notional_pct > 50.0 {
        return Err(Error::LiveTradingRefused(
            "max_order_notional_pct above 50% is not allowed for live trading".to_string(),
        ));
    }
    if cfg.trading.max_daily_loss_pct > 10.0 {
        return Err(Error::LiveTradingRefused(
            "max_daily_loss_pct above 10% is not allowed for live trading".to_string(),
        ));
    }
    if cfg.trading.max_open_positions > 5 {
        return Err(Error::LiveTradingRefused(
            "max_open_positions above 5 is not allowed for live trading".to_string(),
        ));
    }
    if cfg.trading.max_trades_per_day > 50 {
        return Err(Error::LiveTradingRefused(
            "max_trades_per_day above 50 is not allowed for live trading".to_string(),
        ));
    }
    if cfg.trading.max_daily_notional_pct > 100.0 {
        return Err(Error::LiveTradingRefused(
            "max_daily_notional_pct above 100 is not allowed for live trading".to_string(),
        ));
    }
    if !std::io::stdin().is_terminal() {
        return Err(Error::LiveTradingRefused(
            "live trading requires an interactive terminal for typed confirmation".to_string(),
        ));
    }
    Ok(())
}

fn require_phrase(expected: &str, prompt: &str) -> Result<()> {
    print!("{prompt}: ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| Error::Io {
            path: "<stdin>".into(),
            source: e,
        })?;
    if line.trim() == expected {
        Ok(())
    } else {
        Err(Error::LiveTradingRefused(format!(
            "confirmation phrase did not match '{expected}'; no live orders were sent"
        )))
    }
}
