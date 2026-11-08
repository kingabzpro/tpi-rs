# Changelog

All notable changes to TPI-RS are recorded here. The project follows semantic versioning.

## [0.1.1] - 2026-11-08

### Changed

- Renamed the executable from `tpi-rs` to `tpi`.
- Added natural commands: `tpi`, `tpi run`, `tpi analyze`, `tpi scan`, `tpi backtest`, `tpi benchmark`, `tpi paper`, `tpi live`, `tpi tui`, `tpi doctor`, `tpi perf`.
- Positional tickers work after a command, for example `tpi run AAPL,MSFT`.
- All previous flat flags still work, including `tpi --mode backtest`.
- Updated README, setup guide, benchmark commands, installer and release asset names for the new binary.
- Added six command-expansion tests and one end-to-end CLI command test. The suite now has 63 tests.

## [0.1.0] - 2026-11-08

First public release.

### Added

- TPI strategy with Donchian 40/20 channels, EMA 50/200 trend filter, RSI 14, ATR 14, ADX and volume confirmation, volatility gates, and ATR trailing stops.
- Separate short, medium and long horizon parameter sets.
- 26-vote technical rating with Strong Sell, Sell, Neutral, Buy and Strong Buy levels.
- Concurrent Top-100 scanning, single and multi-ticker analysis, and deterministic ranking.
- SQLite candle cache, Alpaca daily bars, discovery, calendar, account, positions and orders.
- Next-session backtest engine with spread, slippage, commissions, SEC/TAF fees, stop gaps and end-of-test liquidation.
- Three-year benchmark over six continuous six-month windows, with SPY, QQQ, equal-weight and cash comparisons.
- Walk-forward parameter selection, parameter stability tables and an untouched out-of-sample holdout.
- Alpaca paper trading by default, with gated live execution, kill switch, daily loss limits and emergency flatten.
- Ratatui dashboard with sortable signals, chart, rating panel, portfolio, backtests, recent trades, Horizons tab, progress and cancellation.
- CSV and JSON exports for scans, backtests, decisions, equity, trades, windows and benchmarks.
- Windows and Linux setup guides, architecture diagram, strategy specification, upstream audit and benchmark report.
- 56 tests covering indicators, properties, no-lookahead behavior, execution accounting, mock exchange responses, concurrency, safety gates and CLI workflows.
- Criterion benchmarks and a process-level performance harness.
- Prebuilt Windows and Linux release binaries published by GitHub Actions.

### Known limitations

- The default configuration underperformed SPY over 2023-11 to 2026-11, and all walk-forward out-of-sample folds were negative.
- One long position at a time, no shorting, no leverage, daily bars only.
- The static 50-stock universe carries survivorship bias, and the free IEX feed does not cover consolidated volume.
- The technical rating is a TradingView-style reconstruction and has not been validated against forward returns.
- Live exits are monitored client-side; there is no exchange-side bracket order in this release.
