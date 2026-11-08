//! Concurrency and live-trading safety tests.

use chrono::{Duration, NaiveDate};
use std::sync::Arc;
use tpi_rs::config::Config;
use tpi_rs::data::MarketData;
use tpi_rs::live;
use tpi_rs::model::{Horizon, RiskProfile, StrategyMode};
use tpi_rs::paper::TradeOptions;
use tpi_rs::scan::{self, AnalysisOptions};

fn trade_options(flag: bool) -> TradeOptions {
    TradeOptions {
        dry_run: true,
        once: true,
        wait_for_open: false,
        flatten: false,
        loop_secs: None,
        top: None,
        tickers: vec!["AAPL".into()],
        i_understand_live_risk: flag,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_offline_scans_are_deterministic() {
    let cfg = Arc::new(Config::default());
    let market = Arc::new(MarketData::new(&cfg, true, true).unwrap());
    let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let start = today - Duration::days(500);
    let symbols: Vec<String> = (0..60).map(|i| format!("C{i:02}")).collect();

    let mut handles = Vec::new();
    for _ in 0..6 {
        let market = market.clone();
        let cfg = cfg.clone();
        let symbols = symbols.clone();
        handles.push(tokio::spawn(async move {
            let options = AnalysisOptions {
                horizon: Horizon::Medium,
                strategy: StrategyMode::Compare,
                risk: RiskProfile::Balanced,
                equity: 10_000.0,
                config: &cfg,
                as_of: Some(today),
                stale_bar_days: 5,
                cancel: None,
            };
            let report = scan::analyze_market(&market, &symbols, start, today, &options, None)
                .await
                .unwrap();
            report
                .signals
                .iter()
                .map(|s| {
                    (
                        s.symbol.clone(),
                        s.strategy,
                        s.action,
                        s.strength.to_bits(),
                        s.rating,
                    )
                })
                .collect::<Vec<_>>()
        }));
    }
    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await.unwrap());
    }
    for other in results.iter().skip(1) {
        assert_eq!(results[0], *other, "concurrent scans diverged");
    }
    assert_eq!(results[0].len(), 60 * 2);
}

#[tokio::test]
async fn live_trading_requires_configuration_and_acknowledgement() {
    let cfg = Config::default();
    let err = live::run_live(
        &cfg,
        Horizon::Medium,
        StrategyMode::Tpi,
        RiskProfile::Balanced,
        Some(10_000.0),
        &trade_options(true),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, tpi_rs::Error::LiveTradingRefused(_)));

    let mut cfg2 = Config::default();
    cfg2.trading.allow_live = true;
    let err = live::run_live(
        &cfg2,
        Horizon::Medium,
        StrategyMode::Tpi,
        RiskProfile::Balanced,
        Some(10_000.0),
        &trade_options(false),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, tpi_rs::Error::LiveTradingRefused(_)));
}

#[tokio::test]
async fn live_requires_explicit_paper_false() {
    let mut cfg = Config::default();
    cfg.trading.allow_live = true;
    cfg.alpaca.key_id = "dummy".into();
    cfg.alpaca.secret_key = "dummy".into();
    let err = live::run_live(
        &cfg,
        Horizon::Medium,
        StrategyMode::Tpi,
        RiskProfile::Balanced,
        Some(10_000.0),
        &trade_options(true),
    )
    .await
    .unwrap_err();
    match err {
        tpi_rs::Error::LiveTradingRefused(message) => {
            assert!(message.contains("paper"));
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn live_trading_refuses_non_interactive_terminals() {
    let mut cfg = Config::default();
    cfg.trading.allow_live = true;
    cfg.trading.paper = false;
    cfg.alpaca.key_id = "dummy".into();
    cfg.alpaca.secret_key = "dummy".into();
    // The test harness has no TTY, so the typed confirmation gate must refuse
    // before any network client or order is created.
    let err = live::run_live(
        &cfg,
        Horizon::Medium,
        StrategyMode::Tpi,
        RiskProfile::Balanced,
        Some(10_000.0),
        &trade_options(true),
    )
    .await
    .unwrap_err();
    match err {
        tpi_rs::Error::LiveTradingRefused(message) => {
            assert!(message.contains("interactive"));
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn live_hard_risk_ceilings_are_enforced() {
    let mut cfg = Config::default();
    cfg.trading.allow_live = true;
    cfg.trading.paper = false;
    cfg.alpaca.key_id = "dummy".into();
    cfg.alpaca.secret_key = "dummy".into();
    cfg.trading.max_order_notional_pct = 80.0;
    let err = live::run_live(
        &cfg,
        Horizon::Medium,
        StrategyMode::Tpi,
        RiskProfile::Balanced,
        Some(10_000.0),
        &trade_options(true),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, tpi_rs::Error::LiveTradingRefused(_)));
}
