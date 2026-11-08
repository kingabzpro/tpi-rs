<div align="center">

# TPI-RS

**Super fast algorithmic trading for US stocks, built entirely in Rust.**

[![CI](https://github.com/kingabzpro/tpi-rs/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/kingabzpro/tpi-rs/actions/workflows/ci.yml) [![Latest release](https://img.shields.io/badge/release-latest-blue)](https://github.com/kingabzpro/tpi-rs/releases/latest) ![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-blue) ![Rust](https://img.shields.io/badge/rust-1.88%2B-orange) ![Tests](https://img.shields.io/badge/tests-63%20passing-brightgreen)

</div>

<p align="center">
  <a href="docs/screenshots/tui.png">
    <img src="docs/screenshots/tui.png" alt="TPI-RS Windows terminal dashboard with ranked signals, rating and trade plan" width="920">
  </a>
</p>

<p align="center"><sub>Captured on Windows 11. The dashboard (<code>tpi tui</code>): ranked signals, 26-vote rating, trade plan, chart, portfolio and backtests.</sub></p>

TPI-RS is a Windows-native algorithmic trading system for US stocks. It scans for Donchian breakouts, confirms them with EMA trend, RSI, ATR and volume filters, adds a 26-vote technical rating, then backtests or trades the result through Alpaca. No Python, Docker, WSL, GPU or ML model.

It is fast and small: scan math for 100 symbols takes 0.05 ms, a 50-symbol, 3-year backtest computes in 33 ms, and 100 symbols fit in 98 MB. It is not a market-beating strategy: SPY still wins on return, and the results below say so plainly.

## Recent results vs SPY

<p align="center">
  <a href="docs/screenshots/comparison.png">
    <img src="docs/screenshots/comparison.png" alt="TPI-RS short and medium returns and drawdowns versus SPY and QQQ" width="900">
  </a>
</p>

Measured on Windows 11. Same $10,000, same period (November 2023 to November 2026), real Alpaca IEX adjusted bars, costs included.

| Portfolio | Total return | CAGR | Sharpe | Max drawdown |
|---|---:|---:|---:|---:|
| **TPI short** | **+25.24%** | **+7.80%** | **1.09** | **6.35%** |
| TPI medium | +9.13% | +2.96% | 0.65 | 5.15% |
| TPI medium + consensus | +14.15% | +4.52% | 0.87 | 4.47% |
| SPY | +86.67% | +23.17% | 1.47 | 18.74% |
| QQQ | +99.45% | +25.92% | 1.21 | 22.72% |

**SPY still wins on return.** The recent exit-policy change lifted the short horizon to +25.24% while keeping its max drawdown at roughly one third of SPY's. The medium default stayed at +9.13%; removing the time stop hurt that horizon. In the medium-horizon walk-forward, all four out-of-sample folds were negative, and the untouched final six months returned -0.66%.

### Last 3 months

| Portfolio | Return | Max drawdown |
|---|---:|---:|
| TPI short | -0.86% | 3.18% |
| TPI medium | -1.41% | 2.28% |
| TPI long | -2.01% | 3.23% |
| SPY | +1.38% | 3.06% |
| QQQ | +0.43% | 9.15% |

2026-07-31 to 2026-10-30, from the same daily equity curves. All three TPI horizons were negative over this stretch; SPY and QQQ were positive. The latest full six-month window is in [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

**Recent experiment:** a hard 1.2x volume gate lifted the short horizon to +33.47% in-sample, but 1.1x and 1.3x both underperformed. It was rejected as threshold-sensitive. Details in [docs/RESEARCH.md](docs/RESEARCH.md).

> [!WARNING]
> This is research software, not investment advice. It underperformed SPY over the sample. Live trading is disabled by default and requires deliberate confirmations. Paper trade first.

<details>
<summary><strong>Console output</strong>: real short-horizon backtest</summary>
<br>

![TPI-RS short-horizon backtest output on Windows 11](docs/screenshots/backtest.png)

</details>

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

The hot path is a precomputed causal series per symbol, a Rayon scan over the universe, and a single reuse of that series across every backtest day and parameter set. There is no garbage collector, interpreter or model inference in the loop. Full numbers: [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

## What is inside

- **Entry:** close above EMA200, EMA50 above EMA200, Donchian 40/20 breakout, RSI and ATR inside the horizon band, composite score above threshold.
- **Exit:** ATR trailing stop, channel exit, 252-session safety cap.
- **Rating:** 26 votes across 15 moving averages and 11 oscillators.
- **Execution:** next-open fills, spread, slippage, SEC/TAF fees, Alpaca paper by default, gated live trading.
- **Validation:** six-month walk-forward windows, parameter stability, untouched out-of-sample holdout.

Rules and per-horizon parameters: [docs/STRATEGY.md](docs/STRATEGY.md).

## Run it on Windows

No Rust, Cargo, Python or build tools required.

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

## Build from source

Rust 1.88 or newer and the MSVC C++ Build Tools on Windows. Step-by-step setup: [docs/SETUP-WINDOWS.md](docs/SETUP-WINDOWS.md).

```powershell
git clone https://github.com/kingabzpro/tpi-rs.git
cd tpi-rs
cargo build --release
.\target\release\tpi.exe --doctor
```

## Learn more

- [Strategy rules and parameters](docs/STRATEGY.md)
- [Benchmark report and limitations](docs/BENCHMARKS.md)
- [Architecture and concurrency](docs/ARCHITECTURE.md)
- [Research review and rejected experiments](docs/RESEARCH.md)
- [Windows setup and troubleshooting](docs/SETUP-WINDOWS.md)
- [Testing guide](docs/TESTING.md)
- [Changelog](CHANGELOG.md)

Paper trade first. The live path needs explicit flags, a typed confirmation, hard risk ceilings and a kill switch.
