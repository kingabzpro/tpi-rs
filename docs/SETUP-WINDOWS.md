# Windows 10/11 Installation and Setup

This guide covers a native Windows build. No Docker, WSL, Python or GPU is required.

## 1. Prerequisites

- Windows 10 or 11, x64.
- PowerShell or Windows Terminal.
- Internet access for crates, Alpaca data and Alpaca's paper API.
- About 1 GB of disk for the Rust toolchain and build artifacts, plus your data cache.

## 2. Install Rust

```powershell
winget install --id Rustlang.Rustup -e --accept-source-agreements --accept-package-agreements
```

Alternative: download `rustup-init.exe` from https://rustup.rs and accept the default MSVC toolchain.

Open a **new** terminal and verify:

```powershell
rustc --version
cargo --version
rustup show active-toolchain
```

Expected: `stable-x86_64-pc-windows-msvc`. Rust 1.88 or newer is required.

If `rustc` is not found, add `%USERPROFILE%\.cargo\bin` to `PATH` or log out and back in.

## 3. Install MSVC C++ Build Tools

The MSVC linker and Windows SDK are required. The Rust MSVC toolchain does not include them.

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e `
  --accept-source-agreements --accept-package-agreements `
  --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

This is a large download (often 2-4 GB). Wait for the command to finish, then open a new terminal.

Verify the workload:

```powershell
& "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe" `
  -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
  -property installationPath
```

You should see a Visual Studio installation path.

`link.exe not found` means the workload is missing or PATH was not refreshed. Restart the terminal, or open "x64 Native Tools Command Prompt for VS 2022" and build there.

## 4. Build

From the project directory:

```powershell
cd C:\Repository\GitHub\TPI-RS
cargo build --release
```

The first build compiles dependencies and takes about 1-2 minutes on a modern laptop. The result is:

```text
target\release\tpi.exe
```

Run the tests:

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
```

## 5. Alpaca credentials

1. Create or sign in to an Alpaca account.
2. In the Alpaca dashboard, generate **paper** API keys.
3. Copy the environment template:

```powershell
Copy-Item .env.example .env
notepad .env
```

Set:

```text
ALPACA_API_KEY=your_paper_key_id
ALPACA_SECRET_KEY=your_paper_secret
TPI_ALPACA_FEED=iex
TPI_ALLOW_LIVE=false
```

Notes:

- `iex` works on Alpaca's free data plan. `sip` requires a paid subscription. A 403 error usually means the feed is not entitled; rerun with `--feed iex`.
- Paper trading uses `https://paper-api.alpaca.markets`. Live uses `https://api.alpaca.markets` and is blocked by default.
- Never commit `.env`. It is ignored by git.

Alternatively set the variables in PowerShell:

```powershell
$env:ALPACA_API_KEY = "..."
$env:ALPACA_SECRET_KEY = "..."
$env:TPI_ALPACA_FEED = "iex"
```

## 6. Verify with doctor

```powershell
.\target\release\tpi.exe --doctor
```

Expected checks:

```text
[ OK ] config validation
[ OK ] directories
[ OK ] SQLite cache
[ OK ] credentials
[ OK ] Alpaca clock
[ OK ] Market calendar
[ OK ] Paper account
[ OK ] Historical bars (SPY)
Doctor finished: all checks passed.
```

Offline equivalent, with no credentials or network:

```powershell
.\target\release\tpi.exe --offline --doctor
```

## 7. First runs

```powershell
# Interactive prompts: ticker, horizon, mode, strategy, risk, capital
.\target\release\tpi.exe

# One ticker
.\target\release\tpi.exe --tickers AAPL --mode analyze

# Top 100 short-horizon scan
.\target\release\tpi.exe --top 100 --mode scan --horizon short --strategy consensus

# Dashboard
.\target\release\tpi.exe --tui --tickers AAPL,MSFT,NVDA --mode scan

# Paper trading one cycle without orders
.\target\release\tpi.exe --mode paper --once --dry-run
```

## 8. Data cache and reports

- Cache: `data/tpi_cache.db` (SQLite, WAL mode). Created automatically.
- Reports and exports: `reports/`.
- Configuration: `config/default.toml`; user-level override at `%USERPROFILE%\.tpi-rs\config.toml`.
- Kill switch: create a file named `KILL` in the working directory to stop new trading orders; remove it to resume.

To force a full refresh (for example after a split, dividend adjustment or cache suspicion):

```powershell
.\target\release\tpi.exe --mode scan --tickers AAPL --refresh
```

To disable the cache for one run:

```powershell
.\target\release\tpi.exe --mode benchmark --no-cache
```

## 9. Linux build

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev
rustup toolchain install stable
cargo build --release
cargo test
```

The only platform-specific code is the process memory/CPU shim in `src/perf.rs` (Windows and Linux implementations; zero values elsewhere). macOS builds are expected to work with the fallback shim but are not primary targets. The HTTP client uses the platform TLS stack (`native-tls`); on Linux this is OpenSSL, hence `libssl-dev`.

## 10. Troubleshooting

### `link.exe not found`

- Install the MSVC Build Tools workload from section 3.
- Open a new terminal.
- Confirm with `where.exe link` from the x64 Native Tools prompt.
- Do not use the `link.exe` that ships with Git for Windows.

### `error: linker 'link.exe' not found` only in one IDE

The IDE was started before Build Tools was installed. Restart it.

### `MissingCredentials`

`ALPACA_API_KEY` and `ALPACA_SECRET_KEY` are absent. Copy `.env.example` to `.env`, or export the variables. `--offline` works without credentials.

### Alpaca returns 403

The requested market-data feed is not in your subscription. Use `--feed iex`. SIP historical and real-time data require a paid plan.

### Alpaca returns 429

The client honors `Retry-After` and retries idempotent GETs. If it persists, lower `alpaca.rate_limit_per_min` in the config. The default is 180 requests per minute, below Alpaca's 200/min limit.

### No data for a ticker

A symbol may be newly listed, halted, delisted or not covered by the selected feed. TPI-RS excludes symbols with fewer than 220 daily bars from scans and reports them as errors instead of emitting a meaningless signal.

### Cache seems stale or inconsistent

Run with `--refresh`. If the file is corrupt, stop the program and delete `data/tpi_cache.db`; it will be rebuilt.

### Dashboard renders incorrectly

Use Windows Terminal or a current console host with Unicode and truecolor. Legacy consoles with non-UTF-8 code pages may render box characters incorrectly. Use `--output json` or console modes if the terminal is limited.

### Live trading is refused

This is intentional. Live needs all of:

1. `allow_live = true` in config or `TPI_ALLOW_LIVE=1`.
2. `paper = false` in config, an explicit acknowledgement separate from `allow_live`.
3. `--i-understand-live-risk`.
4. An interactive terminal where you type `LIVE`.
5. Passing hard ceilings: order notional <= 50% equity, daily loss <= 10%, positions <= 5, trades/day <= 20.
6. Credentials and an active account.

### SmartScreen warns about the executable

The binary is unsigned. For local use, choose "More info" then "Run anyway", or build it yourself with `cargo build --release`. Sign the executable if you distribute it.

### PowerShell cannot find the executable

Use the explicit path `.\target\release\tpi.exe`, or add `target\release` to your `PATH`.

## 11. Environment variable reference

| Variable | Purpose |
|---|---|
| `ALPACA_API_KEY` | Alpaca key id |
| `ALPACA_SECRET_KEY` | Alpaca secret |
| `TPI_ALPACA_FEED` | `iex`, `sip`, `delayed_sip`, `otc`, `boats` |
| `TPI_DATA_URL` | Override market-data base URL |
| `TPI_PAPER_URL` | Override paper trading base URL |
| `TPI_LIVE_URL` | Override live trading base URL |
| `TPI_ALLOW_LIVE` | `1`/`true` enables the live preflight gate |
| `TPI_CACHE_PATH` | SQLite cache path |
| `TPI_CACHE_TTL_HOURS` | Cache refresh TTL |
| `TPI_REPORTS_DIR` | Export directory |
| `TPI_CONFIG` | Configuration file path |
| `RUST_LOG` | Console log filter (console modes) |
