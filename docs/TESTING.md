# Testing Guide

## 1. Commands

```powershell
cargo test
cargo test --lib
cargo test --test properties
cargo test --test mock_exchange
cargo test --test execution
cargo test --test concurrency_safety
cargo test --test exports_cli
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

The suite does not need Alpaca credentials or a network connection. All exchange interaction is against a local mock server, and synthetic bars are deterministic.

## 2. Test inventory

| Suite | Tests | Focus |
|---|---:|---|
| Library unit tests | 40 | Config validation, cache roundtrip, Alpaca parsing, ticker validation, synthetic determinism, indicator causality, rating mechanics, TPI gates, sizing, scan determinism, engine accounting, benchmark |
| `properties.rs` | 4 | Invariants over random series: bounded RSI, ATR, scores; rating fast/detailed agreement; stop below entry; risk/notional caps; future bars cannot change past signals |
| `execution.rs` | 3 | Next-open fills with costs, prior-session BUY requirement, fee arithmetic, final liquidation, full vs truncated no-lookahead, non-negative equity across horizons and risk profiles |
| `mock_exchange.rs` | 4 | Pagination, sorting, auth headers, account/positions parsing, order submission and client-id lookup, transient 500 retry, emergency flatten |
| `concurrency_safety.rs` | 5 | Six concurrent scans produce identical output; live config/ack/TTY/ceiling gates |
| `exports_cli.rs` | 7 | Scan CSV/JSON, backtest artifacts, benchmark per-leg artifacts, CLI offline JSON, doctor, missing ticker error |

Total: **63 tests**.

## 3. Property tests

The property suite generates 260-bar series with random per-bar returns and wicks. It asserts:

- RSI is inside 0-100 or absent.
- ATR is non-negative and finite.
- Stochastic is inside the valid band.
- `rating_fast` and the full 26-vote panel agree on the five-level rating.
- Score and strength are inside 0-100.
- Stop is at or below entry and target is at or above entry when ATR exists.
- Suggested notional never exceeds equity or the 75% position cap.
- Position sizing never exceeds the risk budget or notional cap, and whole-share mode returns integers.
- A truncated series and the full series produce identical actions, strengths and indicators at the truncation index.

These complement, rather than replace, hand-computed unit tests.

## 4. No-lookahead verification

Two independent checks:

1. **Property test:** random series, random truncation index, compare the full-series signal to the truncated-series signal. Action, strength, RSI, ATR and rating must match exactly.
2. **Integration test:** run the full backtest over the entire period, then run it again with all bars after a cutoff removed. Every equity point and every daily decision before the cutoff must match within 1e-6.

The backtest engine also fills at the next open by construction. A test asserts each entry price equals `buy_fill(open of entry date)` and that the prior session produced a BUY for that symbol.

## 5. Mock exchange

`tests/mock_exchange.rs` starts a local HTTP server on an ephemeral port. It parses request lines, headers and bodies, records them, and replies with controlled JSON. Covered behaviours:

- `GET /v2/stocks/bars` with `page_token` pagination, out-of-order bars and multiple symbols.
- Auth headers `APCA-API-KEY-ID` and `APCA-API-SECRET-KEY`.
- Transient 500 on a GET, then success. The client retries idempotent reads.
- Account decimal strings and available-cash priority.
- Positions parsing.
- Market order POST body and response.
- Lookup by `client_order_id`.
- Cancel-all (207) and close-position DELETE.
- Emergency flatten report.

The mock replies with `Connection: close` so each request is self-contained and tests do not depend on HTTP keep-alive.

## 6. Concurrency tests

Six Tokio tasks analyze the same 60-symbol universe concurrently, each using the shared `MarketData` facade and Rayon. They must produce byte-for-byte identical tuples of symbol, strategy, action, strength bits and rating. This exercises:

- Shared read-only config and market data.
- Rayon use from multiple runtime tasks.
- Deterministic ranking and tie-breaking.
- Cancellation-free scan path.

The cache itself is protected by a `Mutex`; the test runs with caching disabled for the synthetic path, and cache roundtrip is unit-tested separately.

## 7. Safety tests

- Live fails when `allow_live` is false.
- Live fails without `--i-understand-live-risk`.
- Live fails without an interactive terminal.
- Live fails unless `paper=false` is set in addition to `allow_live`.
- Live fails when hard ceilings are exceeded (for example an 80% per-order notional cap).
- Provider-side order ambiguity is handled by client-order-id reconciliation in the execution code. The mock covers the lookup endpoint; a live network failure is not simulated in the unit suite because it would require fault injection at the socket layer.

## 8. Manual verification performed

- `tpi doctor` against the real Alpaca paper account: clock, account, and SPY history passed.
- A real Top-100 scan completed in about 3.3 seconds with 100 symbols and correctly excluded newly listed symbols with insufficient warmup.
- A full real-data benchmark with walk-forward, stability and OOS completed and exported all artifacts.
- Paper mode `--once --dry-run` read the live paper account, positions and open orders, rendered the signals, and submitted nothing.
- The TUI was launched in a pseudo-terminal (winpty) and its ANSI stream was rendered with `pyte` to verify the signal table, detail pane, chart and footer. Quitting with `q` restored the terminal.

## 9. CI

`.github/workflows/ci.yml` runs on Windows and Ubuntu:

1. Install Linux TLS prerequisites.
2. `cargo fmt --check`
3. `cargo clippy --all-targets -- -D warnings`
4. `cargo test`
5. `cargo build --release`
6. `tpi perf` smoke run

No secrets are required. Live API tests are deliberately excluded from CI.

## 10. Reproducing failures

For a failing indicator or strategy test, the fastest loop is:

```powershell
cargo test --lib indicators -- --nocapture
cargo test --test properties -- --nocapture
```

For a failing mock or execution test:

```powershell
cargo test --test mock_exchange -- --nocapture
cargo test --test execution -- --nocapture
```

Rust's test harness labels panics with the assertion context. Property tests shrink failing inputs automatically and print the minimal case.
