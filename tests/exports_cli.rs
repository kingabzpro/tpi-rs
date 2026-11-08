//! Export and CLI end-to-end tests.

use chrono::Duration;
use std::process::Command;
use tpi_rs::backtest::engine::{self, PreparedData};
use tpi_rs::backtest::run_benchmark;
use tpi_rs::backtest::types::{BacktestConfig, FeeModel};
use tpi_rs::config::Config;
use tpi_rs::data::synthetic::synthetic_universe;
use tpi_rs::model::{Horizon, RiskProfile, StrategyMode};
use tpi_rs::scan::{self, AnalysisOptions};

#[test]
fn scan_exports_json_and_csv() {
    let cfg = Config::default();
    let market = tpi_rs::data::MarketData::new(&cfg, true, true).unwrap();
    let end = chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let symbols = vec!["AAA".to_string(), "BBB".to_string()];
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let report = rt
        .block_on(scan::analyze_market(
            &market,
            &symbols,
            end - Duration::days(500),
            end,
            &AnalysisOptions {
                horizon: Horizon::Medium,
                strategy: StrategyMode::Consensus,
                risk: RiskProfile::Balanced,
                equity: 10_000.0,
                config: &cfg,
                as_of: Some(end),
                stale_bar_days: 5,
                cancel: None,
            },
            None,
        ))
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let paths = tpi_rs::report::export_scan(dir.path(), &report).unwrap();
    assert_eq!(paths.len(), 2);
    assert!(
        paths
            .iter()
            .all(|p| p.exists() && p.metadata().unwrap().len() > 0)
    );
    let json_path = paths
        .iter()
        .find(|p| p.extension().unwrap() == "json")
        .unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(json_path).unwrap()).unwrap();
    assert_eq!(value["signals"].as_array().unwrap().len(), 2);
    assert!(value.get("bars").is_none() || value["bars"].is_null());
    let csv_path = paths
        .iter()
        .find(|p| p.extension().unwrap() == "csv")
        .unwrap();
    let text = std::fs::read_to_string(csv_path).unwrap();
    assert_eq!(text.lines().count(), 3); // header + two signals
}

#[test]
fn backtest_exports_all_artifacts() {
    let end = chrono::NaiveDate::from_ymd_opt(2024, 6, 1).unwrap();
    let start = chrono::NaiveDate::from_ymd_opt(2023, 11, 1).unwrap();
    let warm = start - Duration::days(450);
    let symbols = vec!["AAA".to_string(), "BBB".to_string(), "CCC".to_string()];
    let mut all = symbols.clone();
    all.push("SPY".into());
    all.push("QQQ".into());
    let bars = synthetic_universe(&all, warm, end);
    let data = PreparedData::new(&bars, &symbols, &["SPY".into(), "QQQ".into()]).unwrap();
    let config = BacktestConfig {
        start,
        end,
        initial_capital: 10_000.0,
        horizon: Horizon::Medium,
        strategy: StrategyMode::Tpi,
        risk: RiskProfile::Balanced,
        fees: FeeModel {
            commission_bps: 0.0,
            spread_bps: 2.0,
            slippage_bps: 5.0,
            sec_fee_bps: 0.3,
            finra_taf_per_share: 0.000166,
        },
        allow_fractional: true,
        risk_free_rate_pct: 0.0,
        stale_bar_days: 5,
        min_history_bars: 220,
        max_hold_days_override: None,
        label: "export-test".into(),
    };
    let result = engine::run(&data, &config, Horizon::Medium.params()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let paths = tpi_rs::report::export_backtest(dir.path(), "test", &result).unwrap();
    assert_eq!(paths.len(), 5);
    for path in &paths {
        assert!(path.exists(), "missing {}", path.display());
        assert!(path.metadata().unwrap().len() > 0);
    }
    let equity_csv = paths
        .iter()
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("equity-")
        })
        .unwrap();
    let lines = std::fs::read_to_string(equity_csv).unwrap().lines().count();
    assert_eq!(lines, result.equity.len() + 1);
}

#[test]
fn benchmark_export_includes_per_leg_artifacts() {
    let end = chrono::NaiveDate::from_ymd_opt(2024, 11, 1).unwrap();
    let start = chrono::NaiveDate::from_ymd_opt(2023, 11, 1).unwrap();
    let warm = start - Duration::days(450);
    let symbols = vec!["AAA".to_string(), "BBB".to_string()];
    let mut all = symbols.clone();
    all.push("SPY".into());
    all.push("QQQ".into());
    let bars = synthetic_universe(&all, warm, end);
    let data = PreparedData::new(&bars, &symbols, &["SPY".into(), "QQQ".into()]).unwrap();
    let config = BacktestConfig {
        start,
        end,
        initial_capital: 10_000.0,
        horizon: Horizon::Medium,
        strategy: StrategyMode::Compare,
        risk: RiskProfile::Balanced,
        fees: FeeModel {
            commission_bps: 0.0,
            spread_bps: 2.0,
            slippage_bps: 5.0,
            sec_fee_bps: 0.3,
            finra_taf_per_share: 0.000166,
        },
        allow_fractional: true,
        risk_free_rate_pct: 0.0,
        stale_bar_days: 5,
        min_history_bars: 220,
        max_hold_days_override: None,
        label: "benchmark-export".into(),
    };
    let report = run_benchmark(
        &data,
        &config,
        Horizon::Medium.params(),
        &tpi_rs::config::WalkForwardSettings::default(),
        tpi_rs::model::DataProvenance::synthetic(),
        false,
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let paths = tpi_rs::report::export_benchmark(dir.path(), &report).unwrap();
    assert!(
        paths.len() >= 12,
        "expected per-leg artifacts, got {}",
        paths.len()
    );
    assert!(paths.iter().any(|p| {
        p.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("equity-benchmark-0")
    }));
    assert!(paths.iter().any(|p| {
        p.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("trades-benchmark-1")
    }));
    assert!(paths.iter().any(|p| {
        p.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("benchmark-summary")
    }));
}

#[test]
fn cli_offline_scan_emits_valid_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_tpi-rs"))
        .args([
            "--offline",
            "--tickers",
            "AAPL,MSFT",
            "--mode",
            "scan",
            "--strategy",
            "compare",
            "--horizon",
            "short",
            "--output",
            "json",
        ])
        .output()
        .expect("failed to run tpi-rs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is not JSON");
    assert_eq!(value["signals"].as_array().unwrap().len(), 4);
    assert_eq!(value["provenance"]["source"], "synthetic");
    assert_eq!(value["horizon"], "short");
    assert_eq!(value["strategy"], "compare");
}

#[test]
fn cli_offline_doctor_succeeds() {
    let output = Command::new(env!("CARGO_BIN_EXE_tpi-rs"))
        .args(["--offline", "--doctor"])
        .output()
        .expect("failed to run tpi-rs");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Doctor finished"));
    assert!(stdout.contains("synthetic"));
}

#[test]
fn cli_without_tickers_fails_clearly() {
    let output = Command::new(env!("CARGO_BIN_EXE_tpi-rs"))
        .args(["--offline", "--mode", "analyze", "--no-interactive"])
        .output()
        .expect("failed to run tpi-rs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no tickers") || stderr.contains("tickers"),
        "stderr was: {stderr}"
    );
}
