# Changelog

All notable changes to TPI-RS are recorded here. The project follows semantic versioning.

## [Unreleased]

### Changed

- The daily entry budget now caps traded notional as well as order count: `max_trades_per_day` is 50 and the new `max_daily_notional_pct` caps a day at 100% of equity. Six dust sells worth $0.000189 previously exhausted the old 4-order allowance and blocked every entry for the rest of the day.
- Live trading raises its order ceiling from 20 to 50 per day to match the new default, and now also refuses a `max_daily_notional_pct` above 100.

- Paper and live console output is colourised when stdout is a terminal, and stays plain when piped or when `NO_COLOR` is set.
- The paper loop prints a holdings table with per-position entry, current price, value and unrealised P&L, and lists orders submitted today so fills are visible without leaving the console.

## [0.2.0] - 2026-11-09

### Added

- Picks tab in the TUI (key `8`): the top five buy candidates from the current scan and the top five exit candidates from the connected Alpaca portfolio. Held positions are evaluated with the same rules as the scan, in the same background job as the portfolio refresh. Read-only, no order submission.
- `R` in the TUI cycles the risk profile (conservative, balanced, aggressive), clears any `--risk-pct` override and reruns the current job.
- `--vol-target <pct>` for volatility-targeted position sizing (annualized position volatility, 0 = off) and `--risk-pct <pct>` to override the per-trade risk budget. Both default to existing behavior.
- `docs/BENCHMARKS.md` section 13 records the sizing experiment. Volatility targeting moved return and drawdown together and left Sharpe and Calmar flat, while a deployment-matched fixed-risk control produced identical Sharpe.

### Changed

- The README leads with the default-horizon comparison table against SPY, QQQ, the equal-weight universe and cash, plus the four sizing arms. The wins-first infographic and its short-horizon bullets are removed.

## [0.1.2] - 2026-11-08

### Changed

- Removed the five-session time stop on the short horizon. The short horizon now exits on its channel, ATR trail or SELL signal, with a 252-session safety cap. This improved the real-data short-horizon backtest from +9.73% and Sharpe 0.56 to +25.24% and Sharpe 1.09, with drawdown rising from 5.55% to 6.35%.
- Medium and long horizons keep their existing maximum holding periods. Removing the medium time stop worsened results.
- Updated strategy docs, benchmark tables and the bundled short-horizon artifact.

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
