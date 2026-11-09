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

| | Total | Sharpe | Max DD |
|---|---:|---:|---:|
| TPI | +9.13% | 0.65 | 5.15% |
| SPY | +86.67% | 1.47 | 18.74% |
| QQQ | +99.45% | 1.21 | 22.72% |

2023-11 to 2026-11, 50 US stocks, costs and next-open fills included. The strategy wins on drawdown and loses on return, and no sizing setting closes it: at 4.0% risk it runs more volatility than SPY for less than half the return. Every sizing arm and the walk-forward: [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

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
