//! TPI-RS: a lightweight, deterministic Rust trading system.
//!
//! The crate is organised in layers:
//! - `model` / `config`: plain domain types and configuration
//! - `indicators`: causal indicator series and TradingView-style ratings
//! - `strategy`: TPI entry/exit scoring and risk sizing
//! - `data`: Alpaca REST access, SQLite cache, synthetic data
//! - `backtest`: next-session portfolio simulation and metrics
//! - `paper` / `live`: broker execution with safety gates
//! - `tui`: terminal dashboard
//! - `report`: human, CSV and JSON output

pub mod backtest;
pub mod cli;
pub mod config;
pub mod data;
pub mod error;
pub mod indicators;
pub mod live;
pub mod model;
pub mod paper;
pub mod perf;
pub mod perf_harness;
pub mod report;
pub mod scan;
pub mod strategy;
pub mod tui;

pub use error::{Error, Result};

/// Crate version, also exposed by `tpi --version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
