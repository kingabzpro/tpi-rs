# Research Review and Improvement Plan

> [!NOTE]
> No TPI-specific PDF was supplied with this task. I reviewed the TPI strategy brief used to build TPI-RS, the repository's own benchmark, and the closest directly relevant public paper in full: Wilcox and Crittenden (2005), "Does Trend Following Work on Stocks?" If you meant a different paper, share it and I will revise this page against that text.

## The short version

The indicator math is not the main problem. Portfolio construction is.

TPI-RS has a reasonable trend/breakout entry, but it puts at most one stock on at a time, risks 1% of equity per trade, and usually deploys only a small slice of capital because the stop is far away. In the 2023-2026 test it was in a position on 90% of sessions but finished with 4.7% annualized volatility and a 5.15% maximum drawdown. SPY and QQQ took 15-20% volatility and returned far more.

That does not mean the strategy would beat the index with more leverage. It means the current result tells us more about exposure and breadth than about the breakout rule. Any serious improvement has to address portfolio construction, validation quality and data bias, not just re-tune the ATR multiple.

## What the stock paper actually found

Wilcox and Crittenden tested a long-only trend system on 24,000+ US securities from 1983 to 2004. Their database included delisted companies, adjusted for corporate actions, applied price and liquidity filters at each point in time, and charged commissions and slippage. That is a much cleaner test than a current-index universe.

Their entry rule was simple: buy at the next open after a close at an all-time high. Their exit was a 10 ATR trailing stop. They also applied portfolio-level risk limits. The hypothetical 1991-2004 portfolio results, net of estimated costs, were:

| Metric | Trend system | S&P 500 total return |
|---|---:|---:|
| Annual compounded return | 19.3% | 12.0% |
| Annual standard deviation | 15.6% | 14.4% |
| Maximum drawdown | -20.8% | -44.7% |

The authors are explicit that "portfolio level money management is absolutely essential" and that position sizing, scaling in and out, total open risk constraints and stop discipline matter as much as the entry signal.

That paper is useful here for three reasons:

1. It shows stock trend following can have positive expectancy when tested with delistings, costs and liquidity filters. Our static 50-stock universe does not meet that bar.
2. It used a wide trailing stop and no fixed time stop. TPI-RS uses a 2.5 ATR stop and a 20-session cap on the medium horizon, which may cut winners early.
3. It held many positions and managed total portfolio risk. TPI-RS holds one. That is the largest structural difference between the two designs.

The 19.3% is hypothetical, not a promise, and it covers a different period and rule set. But the design lesson is directly relevant to the underperformance in our report.

## What the research supports

| Idea | Evidence | What it means here |
|---|---|---|
| Time-series momentum exists across many assets | Moskowitz, Ooi and Pedersen (2012) find persistent 1-12 month own-return predictability across 58 futures markets | The core breakout/trend premise is not nonsense, but the evidence is mostly diversified futures, not one US stock at a time |
| Trend following is robust over long samples | Hurst, Ooi and Pedersen (2017) document over a century of trend returns, with crisis alpha | A three-year bull-market sample is too short to judge the approach, but it is also not evidence of an edge |
| Volatility scaling improves risk-adjusted outcomes | Moreira and Muir (2017) show that scaling exposure down when volatility is high improves Sharpe ratios | Fixed 1% risk per trade is not the same as targeting portfolio volatility. Exposure should be managed at the portfolio level |
| Breakout systems need wide diversification | Wilcox and Crittenden held a large book and still called portfolio risk management essential; Turtle rules and Clenow spread risk across many markets | One position makes the outcome depend on a handful of stock-specific events |
| Backtest overfitting is the default failure mode | Bailey, Borwein, Lopez de Prado and Zhu (2014); Harvey, Liu and Zhu (2016) | An 18-combination grid over 54 trades is thin. The negative OOS folds are a warning, not noise to ignore |
| Composites can reduce single-indicator noise | TradingView-style technical ratings combine many moving-average and oscillator votes | The rating is a useful summary, but it is not validated by this project and is highly correlated with price trend |

## Where this implementation diverges

| Area | What TPI-RS does now | Why it matters | Suggested change |
|---|---|---|---|
| Breadth | One stock at a time | A single name, sector or earnings event dominates results; the stock paper held a broad book and stressed total open risk | Allow 3-5 concurrent positions with equal risk and sector/correlation caps |
| Volatility targeting | Fixed 1% equity risk per trade | Realized portfolio vol was 4.7%, far below equity indices | Target 8-12% annualized portfolio vol, with a hard cap and a drawdown brake |
| Candidate ranking | Composite score, then rating, then symbol | Does not select the strongest relative-strength names | Add 12-1 cross-sectional momentum and dollar-volume liquidity to the ranking |
| Regime | Per-stock EMA50/200 only | Trend systems bleed in choppy markets | Add a market regime filter: SPY above 200-day, breadth, realized-vol percentile |
| Exit | 20-session maximum hold and a 2.5 ATR stop | The stock trend paper used a 10 ATR trailing stop with no fixed time stop; a tight stop and time cap cut the right tail | Test 5-10 ATR trailing stops and no fixed hold; rely on channel and ATR exits |
| Pyramiding | None | Research systems often add to winners | Allow one or two 0.5R adds while the trend remains valid |
| Parameter selection | 18-combination grid, four folds | Multiple testing can produce a flattering in-sample winner | Use purged walk-forward, nested selection and Deflated Sharpe |
| Universe | Static 50 large caps | Survivorship and selection bias | Point-in-time universe including delistings, with SIP data |
| Costs | 2 bps spread, 5 bps slippage | Illiquid Top-100 names can be worse | Stress at 2x and 5x costs; add a capacity estimate |
| Rating validation | No forward-return study | The consensus leg improved one window; that is not validation | Measure rank IC by horizon and drop votes that add no information |
| Live stops | Monitored each cycle, no exchange bracket | Gap risk is real | Use exchange-side stop orders where fractional rules allow |

## What I would do next, in order

### P0: make the evidence trustworthy

1. **Measure capital deployment, not just time in market.** Add average invested notional divided by equity, average position count and turnover to the metrics. The current "90% exposure" line is time in market and hides the real exposure.
2. **Add a statistical significance gate.** Report a block-bootstrap confidence interval for Sharpe and a Deflated Sharpe Ratio that accounts for the 18 trials. If the lower bound is below zero, stop tuning.
3. **Use a point-in-time universe.** Include delisted names and historical index membership. The current 50-stock list was chosen with hindsight.
4. **Stress costs.** Rerun with 2x and 5x spread/slippage. If the edge disappears, it was never an edge.
5. **Keep the untouched holdout untouched.** No parameter decisions from the final six months until a new forward period is recorded.

### P1: fix portfolio construction

6. **Hold 3-5 positions.** Equal risk per position, maximum 25-30% initial notional each, sector cap 40%, pairwise correlation cap.
7. **Target portfolio volatility.** Scale gross exposure toward 8-12% annualized using a 20-60 day realized-vol estimate, with a 2x leverage ceiling and a drawdown throttle.
8. **Rank cross-sectionally.** Require a top-quintile 12-1 momentum rank in addition to the breakout. This is the simplest way to stop buying the weakest breakout in the scan.
9. **Add a market regime overlay.** Reduce or disable new entries when SPY is below its 200-day average, breadth is deteriorating, or realized volatility is in the top decile.

### P2: improve the signal and exits

10. **Remove the medium-horizon time stop first.** Test 20, 40 and no maximum hold. If the time stop is hurting the right tail, this is a one-parameter fix.
11. **Ensemble breakout lengths.** Run 20/40/55-day channels and 2.0/2.5/3.0 ATR exits in parallel, then require a majority or weighted vote. This reduces single-parameter sensitivity.
12. **Evaluate pyramiding.** Add 0.5R on a new 10-day high while the initial stop is intact. This is a Turtle-style idea, not a new invention.
13. **Validate the rating.** Compute forward 1/5/20/60-day return IC for the 26 votes and for each component. Remove votes that are redundant with trend or add no incremental information.
14. **Test a pullback entry.** A breakout that immediately mean-reverts is common. Compare breakout entries with a 20-EMA pullback entry inside an established uptrend.

### P3: prove it forward

15. Run a six-month paper-trading period after any change, with a fixed config, before considering capital.
16. Report results against SPY at matched volatility, not only raw return. Beating SPY with 5% volatility is a different claim from beating it with 15%.
17. Publish negative results. The current report already does this; keep that habit.

## Acceptance criteria for any future change

These are gates, not promises:

- Positive net return after 2x cost stress.
- Out-of-sample Sharpe above 0.8 across at least two non-overlapping test windows.
- Maximum drawdown below 15% at the chosen volatility target.
- Deflated Sharpe probability above 0.95 given the number of trials.
- Positive in at least two of three regimes: trending up, choppy, trending down.
- No single stock or sector contributes more than 35% of total PnL.
- Rating votes and strategy gates survive a component ablation study.

If a change fails these, it does not go into the default configuration.

## What not to do

- Do not add ML or an LLM to rescue a thin sample. That increases overfitting without improving the evidence.
- Do not tune on the untouched 2026 holdout.
- Do not claim index-beating performance from a three-year bull market.
- Do not increase leverage without volatility targeting and a drawdown brake.
- Do not keep a parameter because it won one window. Keep it because it survives costs, regimes and out-of-sample tests.

## Where the changes belong in this codebase

| Change | File or module |
|---|---|
| Average exposure, Deflated Sharpe, bootstrap | `src/backtest/metrics.rs` |
| Multiple positions, portfolio heat, vol target | `src/backtest/engine.rs`, `src/strategy/risk.rs` |
| Cross-sectional momentum ranking | `src/scan.rs`, `src/backtest/engine.rs` |
| Market regime overlay | new `src/strategy/regime.rs` |
| Point-in-time universe and delistings | `src/data/universe.rs`, data layer |
| Cost stress | `src/backtest/types.rs`, CLI/config |
| Rating IC and vote ablation | `src/indicators/ratings.rs`, a new research binary or test |
| Exchange-side stop orders | `src/paper.rs`, `src/data/alpaca.rs` |

## References

- Wilcox, C., and Crittenden, E. (2005). *Does Trend Following Work on Stocks?* Blackstar Funds. [PDF](https://www.cis.upenn.edu/~mkearns/finread/trend.pdf)
- Moskowitz, T., Ooi, Y. H., and Pedersen, L. H. (2012). *Time Series Momentum*. Journal of Financial Economics 104(2), 228-250. [SSRN 2089463](https://papers.ssrn.com/sol3/papers.cfm?abstract_id=2089463)
- Hurst, B., Ooi, Y. H., and Pedersen, L. H. (2017). *A Century of Evidence on Trend-Following Investing*. Journal of Portfolio Management 44(1). [SSRN 2993026](https://papers.ssrn.com/sol3/papers.cfm?abstract_id=2993026)
- Moreira, A., and Muir, T. (2017). *Volatility-Managed Portfolios*. Journal of Finance 72(4). [NBER w22208](https://www.nber.org/papers/w22208)
- Bailey, D., Borwein, J., Lopez de Prado, M., and Zhu, Q. (2014). *Pseudo-Mathematics and Financial Charlatanism: The Effects of Backtest Overfitting on Out-of-Sample Performance*. Notices of the AMS 61(5). [PDF](https://www.ams.org/notices/201405/rnoti-p458.pdf)
- Harvey, C., Liu, Y., and Zhu, H. (2016). *... and the Cross-Section of Expected Returns*. Review of Financial Studies 29(1), 5-68. [DOI](https://doi.org/10.1093/rfs/hhv059)
- Faith, C. (2007). *Way of the Turtle*. McGraw-Hill. The original Turtle breakout, N-unit sizing and pyramiding rules.
- Clenow, A. (2013). *Following the Trend*. Wiley. Diversified trend-following portfolio construction.
- TradingView. *Technical Ratings*. [Methodology overview](https://www.tradingview.com/support/solutions/43000502040-technical-ratings/)

## Tested and rejected: native ridge forecast gate

Before adding a larger model, I tested a dependency-free ridge regression on engineered features. It predicted the forward return matching each horizon, was retrained every 21 calendar days on expanding history, and rejected a BUY when the prediction was zero or negative.

The setup was causal. Training rows were included only when the target date was on or before the prediction date. The data was the same real Alpaca IEX adjusted set used in [BENCHMARKS.md](BENCHMARKS.md), 2023-11-01 to 2026-11-01, 50-stock universe.

| Horizon | Baseline return | Gated return | Baseline Sharpe | Gated Sharpe | Baseline max DD | Gated max DD | Baseline trades | Gated trades |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Short | +9.73% | +8.67% | 0.56 | 0.51 | 5.55% | 5.46% | 150 | 147 |
| Medium | +9.13% | +6.21% | 0.65 | 0.45 | 5.15% | 7.14% | 54 | 57 |
| Long | +0.05% | +0.05% | 0.02 | 0.02 | 8.09% | 8.09% | 27 | 27 |

The gate rejected about 9% of short and medium BUY signals. Those rejected trades were not worse than the accepted ones on average. The medium drawdown increased. The long model predicted positive returns for every signal, so it never acted as a filter.

**Decision:** rejected. The branch was deleted, the experiment module and binary were removed, no dependencies were added, and no model weights were stored. The idea is worth revisiting only with better features, a longer point-in-time dataset and a different validation design. A small model that mostly repeats momentum already present in TPI does not add value.

## Current status

None of the strategy changes above are implemented in the default configuration yet. The published benchmark is the honest baseline: TPI +9.13%, TPI + Consensus +14.15%, SPY +86.67% over 2023-11-01 to 2026-11-01, with negative walk-forward OOS folds. See [BENCHMARKS.md](BENCHMARKS.md) for the full tables.
