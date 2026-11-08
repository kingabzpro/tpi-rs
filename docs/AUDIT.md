# Upstream Audit and Reuse Decisions

## 1. Target

- Repository: `https://github.com/Zuytan/rustrade`
- Audited commit: `54524e9f6513bd42fa1fa8a8416884e10b3f6bd2`
- Version: `0.99.33`
- License: MIT

## 2. Size and shape

| Metric | Upstream |
|---|---:|
| Rust source files | 289 |
| Rust lines | 59,425 |
| Direct dependency declarations | 65 |
| UI | egui/eframe desktop |
| Strategies | SMC, order flow, Z-score reversion, statistical momentum, SNN surrogate, regime adaptive, ensemble |
| Venues | Binance, Alpaca, OANDA |
| Services | Axum API, Prometheus metrics, SQLx persistence |
| Extras | News, VADER sentiment, i18n, desktop settings |

The repository is a mature multi-agent desktop platform. Most of its complexity is unrelated to a deterministic TPI strategy, and its modules are tightly coupled through application services, configuration, persistence and agent state. Copying a subset would have dragged in SQLx, egui, Axum, Prometheus, Binance and sentiment dependencies, which conflicts with the requirement for a lightweight, auditable system.

TPI-RS is therefore a **clean-room lightweight successor** built against the same operational realities. No upstream source files were copied into this crate. The audit identified which behaviours are worth preserving and which failure modes must not be repeated.

## 3. Reusable inventory

| Area | Upstream location | Decision |
|---|---|---|
| Alpaca auth headers and base URLs | `src/infrastructure/alpaca/common.rs`, `market_data/historical.rs`, `execution.rs` | Reused as API knowledge; new minimal client |
| Daily bars pagination | `market_data/historical.rs:110-253` | Reimplemented with `page_token`, chunking and adjusted bars |
| Account and position parsing | `execution.rs:67-215,254-274` | Reimplemented; string decimals normalized |
| Order submit/status/cancel endpoints | `execution.rs:304-408,416-690` | Reimplemented; client order ids and polling |
| Retry policy | `infrastructure/core/http_client_factory.rs`, `order_submission.rs` | Adopted: retry idempotent GETs only, reconcile writes |
| Order reconciliation | `order_reconciler.rs`, `order_monitor.rs` | Adopted as a principle; broker remains authoritative |
| Risk sizing and trailing stops | `application/risk_management/sizing_engine.rs`, `trailing_stops.rs` | Redesigned around stop distance; new implementation |
| Metrics concepts | `domain/performance/metrics.rs` | Reimplemented: CAGR, Sharpe, Sortino, drawdown, alpha/beta |
| Fee/slippage concepts | `domain/trading/fee_model.rs`, `simulator/engine.rs` | New explicit fee model with spread, slippage, SEC/TAF |
| Indicator wrappers | `monitoring/strategy/feature_engineering_service.rs` | Replaced with a causal `Series` and a larger rating panel |
| Alpaca test fixtures | few status-map unit tests, no recorded JSON | New local HTTP mock with pagination, auth, retries and orders |

## 4. Upstream failure modes found during the audit

These were found by reading the upstream code and are the reasons several behaviours were deliberately redesigned.

1. **Look-ahead fills.** Backtests filled at the signal bar's close (`candle_pipeline.rs:308` into `simulator/engine.rs:118-156`). TPI-RS signals at close T and fills at the open of T+1. This is covered by an execution test and a full-vs-truncated-history test.
2. **Timestamp unit mismatch.** Daily closes were stored in milliseconds while metrics divided trade timestamps by 1000 and the period by 86,400 (`simulator/engine.rs:192,462`, `metrics.rs:125,157-158`). TPI-RS uses `NaiveDate` for sessions and `DateTime<Utc>` only for wall-clock metadata.
3. **Trade PnL excluded commissions.** Reporting popped LIFO while calling itself FIFO and ignored fees (`reporting.rs:243-257,304-339`), and the engine set fees to zero (`engine.rs:367-399`). TPI-RS records explicit fees, implicit costs and net PnL; tests assert `net = gross - fees`.
4. **Walk-forward did not fit parameters.** `engine.rs:214-240` never performed parameter selection, and `run_walk_forward` hardcoded `"1Min"` (`engine.rs:162`). TPI-RS runs a real grid search on each training window and evaluates the selected set on the following test window.
5. **Order status paths were inconsistent.** `get_today_orders` marked every order `Filled` and used milliseconds while `get_open_orders` used seconds (`execution.rs:458-460` vs `571-573`); both lost limit/stop prices. TPI-RS maps Alpaca status strings verbatim and polls by client order id.
6. **Crypto detection by substring.** `symbol.contains("USD")` misclassifies stock tickers (`execution.rs:336`). TPI-RS is US-equity only and has no substring asset detection.
7. **Unbounded cache and poller.** `historical.rs:230-240` had an unbounded bar cache and `execution.rs:67-215` spawned a permanent portfolio poller with no shutdown. TPI-RS uses a bounded SQLite cache with TTL and a single explicit execution loop with Ctrl+C handling.
8. **Risk confused notional allocation with stop risk.** The upstream sizing treated `risk_per_trade_percent` as allocation and allowed very large risk at high scores (`risk_appetite.rs:68-98`). TPI-RS sizes from `risk_budget / stop_distance` and caps notional separately.
9. **PDT and session handling.** The PDT check treated any held position as a day trade (`pdt_validator.rs:64-67`), and the daily reset used UTC midnight (`session_manager.rs:184-205`). TPI-RS uses the Alpaca clock/calendar and keeps position and order state at the broker. TPI-RS does not implement a full PDT engine; paper accounts are governed by Alpaca, and live trading is gated and capped.
10. **Double slippage.** Simulation applied fee-model slippage plus a separate volatility slippage (`simulator/engine.rs:118-133`, `mock/execution_service.rs:96-102`). TPI-RS applies one explicit fill adjustment.

## 5. Removed components

- ML, SNN surrogate, training binaries, regime and ensemble models.
- News ingestion, RSS and VADER sentiment.
- Binance and OANDA venues, crypto scanners and crypto order paths.
- egui/eframe desktop UI, i18n, settings persistence.
- Axum server, Prometheus registry, Grafana assets.
- SQLx persistence and the agent-oriented bootstrap.
- Bundled grid configs and optimization binaries.

## 6. New components

- Causal `Series` with 40+ indicator arrays and an allocation-light rating path.
- TPI score, hard gates, horizon parameter sets, explanations and stop-distance sizing.
- Deterministic next-open backtest engine with one position, stop gaps and final liquidation.
- Six-window benchmark, walk-forward selection, parameter stability and untouched OOS.
- Concurrent scan with bounded fetch, token bucket, cache and cancellation.
- SQLite candle cache with coverage and TTL.
- Alpaca paper/live execution with risk checks, reconciliation and flatten.
- Ratatui dashboard, CLI prompts, doctor, CSV/JSON exports and a performance harness.
- 56 tests including property, mock-exchange, no-lookahead, concurrency and safety tests.

## 7. License and attribution

The upstream repository is MIT licensed. TPI-RS is a new MIT crate. It retains no upstream source code, but it deliberately follows publicly documented Alpaca REST semantics and general operational patterns identified in the audit. The upstream project is credited as the audit reference.
