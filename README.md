<div align="center">

# TPI-RS

**Super fast algorithmic trading for US stocks, built entirely in Rust.**

[![CI](https://github.com/kingabzpro/tpi-rs/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/kingabzpro/tpi-rs/actions/workflows/ci.yml) [![Latest release](https://img.shields.io/badge/release-latest-blue)](https://github.com/kingabzpro/tpi-rs/releases/latest) ![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-blue) ![Rust](https://img.shields.io/badge/rust-1.88%2B-orange) ![Tests](https://img.shields.io/badge/tests-64%20passing-brightgreen)

</div>

<p align="center">
  <a href="docs/screenshots/tui.png">
    <img src="docs/screenshots/tui.png" alt="TPI-RS Windows terminal dashboard with ranked signals, rating and trade plan" width="920">
  </a>
</p>

<p align="center"><sub>Captured on Windows 11. The dashboard (<code>tpi tui</code>): ranked signals, 26-vote rating, trade plan, chart, portfolio and backtests.</sub></p>

TPI-RS is a Windows-native algorithmic trading system for US stocks. It scans for Donchian breakouts, confirms them with EMA trend, RSI, ATR and volume filters, adds a 26-vote technical rating, then backtests or trades the result through Alpaca. No Python, Docker, WSL, GPU or ML model.

## Results against SPY and QQQ

50 US stocks, Alpaca IEX adjusted bars, next-open fills, costs included, November 2023 to November 2026. This is the only comparison table in this README.

| Arm | Final | Total | CAGR | Sharpe | Sortino | Max DD | Vol | Calmar |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| TPI | $10,913 | +9.13% | 2.96% | 0.65 | 0.97 | 5.15% | 4.71% | 0.58 |
| SPY | $18,667 | +86.67% | 23.17% | 1.47 | 2.19 | 18.74% | 14.84% | 1.24 |
| QQQ | $19,945 | +99.45% | 25.92% | 1.21 | 1.78 | 22.72% | 20.45% | 1.14 |
| Equal-weight universe | $19,228 | +92.28% | 24.39% | 1.53 | 2.29 | 18.96% | 14.90% | 1.29 |
| Cash | $10,000 | 0.00% | 0.00% | 0.00 | 0.00 | 0.00% | 0.00% | 0.00 |

Read it straight. Over this window the strategy lost to buy-and-hold and its worst drawdown was about a third of SPY's, because it sat in cash most of the time: a 1% risk budget against a 2.5 ATR stop keeps only about 17.6% of equity deployed on average. The low drawdown is real. The outperformance is not, and more risk did not fix it: volatility targeting and higher fixed risk both moved return and drawdown together and left Sharpe at 0.64. Full tables, windows and walk-forward: [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

## Speed and memory

| Workload | Result |
|---|---:|
| Scan math, 100 symbols | 0.05 ms |
| Indicators, 100 symbols x 1,105 bars | 15.9 ms parallel, 25.3 ms single-thread |
| Backtest, 50 symbols x 783 sessions | 32.9 ms, about 1.19M symbol-days/s |
| Cold Top-100 scan over Alpaca | about 3.3 s, network-bound |
| Idle process | 8 MB |
| 100 symbols loaded | 98 MB |
| Windows binary | 6 MB |
| Repeatable scans | six concurrent scans byte-identical |

The hot path is a precomputed causal series per symbol, a Rayon scan over the universe, and a single reuse of that series across every backtest day and parameter set. There is no garbage collector, interpreter or model inference in the loop. Measured with `tpi perf`, which uses deterministic synthetic bars.

## Run it on Windows

No Rust or Cargo required.

[![Download for Windows](https://img.shields.io/badge/download-Windows%2010%2F11-blue)](https://github.com/kingabzpro/tpi-rs/releases/latest)

The release includes the Windows zip and a matching `.sha256` file. The zip contains `tpi.exe`, `config/default.toml`, `.env.example`, and `README.md`. Extract it and try the offline demo:

```powershell
.\tpi.exe --offline --tickers AAPL,MSFT,NVDA --mode scan --strategy compare
```

For Alpaca data, copy `.env.example` to `.env`, add your paper keys, then run `.\tpi.exe doctor`. The Windows installer verifies the checksum and installs to `%LOCALAPPDATA%\TPI-RS`: [scripts/install-windows.ps1](scripts/install-windows.ps1).

## Commands

| Command | What it does |
|---|---|
| `tpi` | Interactive ticker, horizon, mode, strategy, risk and capital prompts |
| `tpi run AAPL,MSFT,NVDA` | Scan a ticker list |
| `tpi run` | Scan the Top 100 most-active US stocks |
| `tpi analyze AAPL` | One ticker in detail |
| `tpi backtest --horizon short` | Backtest one horizon |
| `tpi benchmark` | Three-year walk-forward benchmark |
| `tpi tui` | Terminal dashboard |
| `tpi paper --once --dry-run` | Paper order check |
| `tpi doctor` | Environment and API checks |

Every prompt has an equivalent flag, so scripts never need to answer questions. `--output json` works for scans, backtests and benchmarks.

## Learn more

- [Benchmark report, full tables and limitations](docs/BENCHMARKS.md)
- [Strategy rules and parameters](docs/STRATEGY.md)
- [Architecture and concurrency](docs/ARCHITECTURE.md)
- [Research review and rejected experiments](docs/RESEARCH.md)
- [Windows setup and troubleshooting](docs/SETUP-WINDOWS.md)
- [Testing guide](docs/TESTING.md)
- [Changelog](CHANGELOG.md)

> [!WARNING]
> Research software, not investment advice. Live trading is disabled by default and requires deliberate confirmations. Paper trade first, and read the walk-forward section of [docs/BENCHMARKS.md](docs/BENCHMARKS.md) before risking money.

<details>
<summary><strong>Console output</strong>: real short-horizon backtest</summary>
<br>

![TPI-RS short-horizon backtest output on Windows 11](docs/screenshots/backtest.png)

</details>
