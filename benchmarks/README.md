# Benchmark Artifacts

Recorded on 2026-11-08 from this repository's release build.

| File | Description |
|---|---|
| `benchmark-console.txt` | Full console output of `--mode benchmark --strategy compare` |
| `benchmark-full.json` | Complete benchmark report: both strategy legs, comparisons, equity, trades, decisions, walk-forward, stability, OOS |
| `benchmark-summary.csv` | Window returns for the strategy legs and SPY/QQQ/buy-and-hold/cash |
| `walkforward.csv` | Four rolling 12-month train / 6-month test folds |
| `stability.csv` | 18 parameter sets x six window returns, in-sample descriptive table |
| `perf.json` | Performance harness result: latency, throughput, working set, CPU, binary size |
| `backtest-short.json` | Separate short-horizon backtest |
| `backtest-medium.json` | Separate medium-horizon backtest |
| `backtest-long.json` | Separate long-horizon backtest |

Market data: Alpaca daily bars, `adjustment=all`, `feed=iex`, 752 sessions, 2023-11-01 to 2026-11-01, fixed 50-stock universe. Costs: 0 bps commission, 2 bps spread, 5 bps slippage, 0.3 bps SEC fee, $0.000166/share FINRA TAF. Fills at the next open.

**These are research results, not investment advice and not a forecast.** The universe is static (survivorship and selection bias), IEX volume is partial, the strategy is long-only and holds at most one position, and the walk-forward out-of-sample folds were negative. See `docs/BENCHMARKS.md` for the full interpretation and limitations.

Reproduce:

```powershell
.\target\release\tpi-rs.exe --mode benchmark --strategy compare --export reports
.\target\release\tpi-rs.exe --perf
```
