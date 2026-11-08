# Strategy Specification

This document describes exactly what the code computes. It is intentionally rule-based: there is no machine learning, no language model, no random search at runtime and no future-bar access.

## 1. Universe and data

- US stocks and ETFs available through Alpaca. Crypto, options and short selling are not implemented.
- Daily bars, sorted by exchange date, split/dividend adjusted by default (`adjustment=all`). Raw prices can be selected in configuration, but then corporate actions are not modelled.
- A signal is valid only after 220 daily bars, enough for EMA200 and the 40-day channel.
- Volume comes from the selected Alpaca feed. The free `iex` feed covers a subset of consolidated volume, so volume ratios are IEX-based unless `sip` is configured.

## 2. Indicators

All values at bar `i` use only bars `0..=i`.

| Indicator | Definition |
|---|---|
| EMA 50 / 200 | Exponential moving average, `k = 2/(n+1)` (`ta` crate) |
| Donchian 20 / 40 | Highest high and lowest low over the previous N bars, **current bar excluded** |
| RSI 14 | Wilder average gain/loss |
| ATR 14 | Wilder average of true range |
| ADX 14 | Wilder directional movement and DX smoothing |
| MACD | EMA12 - EMA26, 9-period signal |
| Stochastic %K | 14-period raw stochastic smoothed by 3 |
| CCI 20 | Typical price, SMA and mean absolute deviation |
| ATR% | `ATR14 / close * 100` |
| Volume ratio | `volume / SMA20(volume)` |
| EMA20 distance | `(close - EMA20) / ATR14` |

## 3. Horizon parameters

The three horizons use different channels, holding limits and thresholds. They are evaluated and backtested separately. The short horizon no longer exits after five sessions; winners are held until the channel or ATR trail exits, with a 252-session safety cap.

| Parameter | Short | Medium | Long |
|---|---:|---:|---:|
| Breakout channel | 20 | 20 | 40 |
| Confirmation channel | 40 | 40 | 40 |
| Exit channel | 10 | 20 | 20 |
| ATR stop multiple | 2.0 | 2.5 | 3.5 |
| Maximum hold | 252-session safety cap | 20 sessions | 126 sessions |
| RSI band | 50-78 | 50-75 | 50-75 |
| Buy score threshold | 62 | 68 | 72 |
| Minimum ATR% | 0.8 | 0.6 | 0.5 |
| Maximum ATR% | 7.0 | 8.0 | 9.0 |

Defaults live in `config/default.toml` under `[strategy.short|medium|long]`. Validation rejects inverted channels, invalid RSI ranges, negative ATR settings and similar mistakes.

## 4. TPI composite score

The score is a sum of five components, clamped to 0-100.

### Trend, maximum 35

| Condition | Points |
|---|---:|
| Close > EMA50 | 15 |
| EMA50 > EMA200 | 10 |
| Close > EMA200 | 5 |
| EMA50 10-bar slope > 0 | 5 |

### Breakout, maximum 25

| Horizon | Condition | Points |
|---|---|---:|
| Short | Close > prior 20-day high | 25 |
| Medium | Close > prior 20-day high | 15 |
| Medium | Close > midpoint of the 40-day channel | 10 |
| Long | Close > prior 40-day high | 25 |
| Long | No 40-day breakout but close > prior 20-day high | 10 |

### Momentum, maximum 20

| Condition | Points |
|---|---:|
| RSI14 inside the horizon band | 10 |
| RSI14 higher than 3 bars ago | 4 |
| MACD histogram > 0 | 6 |

### Confirmation, maximum 10

| Condition | Points |
|---|---:|
| ADX14 >= 20 | 4 |
| +DI > -DI | 3 |
| Volume ratio >= configured minimum (default 1.0) | 3 |

### Volatility filter, maximum 10

| Condition | Points |
|---|---:|
| ATR% inside the horizon band | 5 |
| Price distance from EMA20 in `[-1, 3]` ATR | 5 |

### Penalties

| Condition | Adjustment |
|---|---:|
| RSI > 80 | -12 |
| ATR% above the horizon ceiling | -10 |
| Price more than 3 ATR above EMA20 | -5 |
| Close below EMA200 | -10 |

Penalties are applied after the component sum, then the total is clamped.

## 5. Hard gates and actions

A **BUY** requires all of the following:

1. `close > EMA200` and `EMA50 > EMA200`.
2. Breakout gate:
   - Short: `close > prior 20-day high`.
   - Medium: `close > prior 20-day high` and `close > 40-day channel midpoint`.
   - Long: `close > prior 40-day high`.
3. `RSI in [rsi_min, rsi_max]`.
4. `ATR% in [min_atr_pct, max_atr_pct]`.
5. Consensus mode only: rating is Buy or Strong Buy.
6. Composite score >= the horizon threshold.

A **SELL** is emitted when any of these holds:

- Close below the prior `exit_period`-day low.
- Close below EMA200 while EMA50 is also below EMA200.
- Consensus mode: rating is Sell or Strong Sell and close is below EMA50.

SELL is an **exit or avoid** signal. This build is long-only and never opens a short position.

**HOLD** covers everything else, including a high score that fails a hard gate and a TPI setup vetoed by the consensus rating.

**Strength** is the composite score for BUY and HOLD, and `100 - score` for SELL. It is a rule-agreement measure, not a probability.

## 6. Consensus mode

Consensus requires the TPI setup and a rating of Buy or Strong Buy. The rating alone never triggers a buy. This reduces trades during mixed technical conditions. Compare mode runs both engines and presents both results; it does not merge them into one signal.

## 7. Five-level technical rating

The rating panel contains 26 votes.

### Moving averages, 15 votes

EMA 10, 20, 30, 50, 100, 200; SMA 10, 20, 30, 50, 100, 200; Ichimoku base; VWMA 20; HMA 20.

Each votes Buy when price is above the average, Sell when below, Neutral when equal or unavailable.

### Oscillators, 11 votes

| Oscillator | Buy | Sell |
|---|---|---|
| RSI 14 | <= 30 | >= 70 |
| Stochastic %K | <= 20 | >= 80 |
| CCI 20 | >= 100 | <= -100 |
| ADX 14 | ADX >= 20 and +DI > -DI | ADX >= 20 and -DI > +DI |
| Awesome Oscillator | > 0 | < 0 |
| Momentum 10 | > 0 | < 0 |
| MACD 12/26/9 | MACD > signal | MACD < signal |
| Stoch RSI 14 | <= 20 | >= 80 |
| Williams %R 14 | <= -80 | >= -20 |
| Bull/Bear Power 13 | > 0 | < 0 |
| Ultimate Oscillator | <= 30 | >= 70 |

`score = (buy_votes - sell_votes) / 26`.

| Score | Rating |
|---|---|
| >= 0.5 | Strong Buy |
| >= 0.1 | Buy |
| > -0.1 | Neutral |
| <= -0.1 | Sell |
| <= -0.5 | Strong Sell |

This is a TradingView-style reconstruction, not a copy of TradingView's proprietary calculations. Several thresholds and implementations (for example the ADX gate and Bull/Bear Power rule) are documented engineering choices, so do not expect exact vote-for-vote parity with a TradingView chart.

The rating is shown for every analysed ticker and is independent of the pure TPI signal.

## 8. Position sizing and risk

```
risk_budget   = equity * risk_per_trade_pct / 100
max_notional  = equity * max_position_pct / 100
shares        = min(
                  cash / entry,
                  max_notional / entry,
                  risk_budget / (entry - stop)
                )
```

The third term is the binding one in normal conditions: size is driven by the distance to the stop, not by a fixed notional allocation. Fractional shares are allowed in backtests and paper trading by default; whole-share mode floors the quantity.

| Profile | Risk per trade | Maximum position |
|---|---:|---:|
| Conservative | 0.5% of equity | 50% of equity |
| Balanced | 1.0% | 75% |
| Aggressive | 1.5% | 100% |

Initial stop: `entry - atr_stop_mult * ATR14`.
Reference target: `entry + 2 * atr_stop_mult * ATR14` (2R).
Trailing stop: `max(current_stop, highest_close_since_entry - atr_stop_mult * ATR14)`.

Backtests check the stop against the day's low. If the open gaps below the stop, the fill is at the open, which is worse than the stop. Live and paper trading monitor the stop every cycle using the current quote and the strategy's stop guidance; there is no exchange-side bracket order in this version. That is a documented gap risk, not a guarantee of stop execution.

## 9. Execution model

- Signal evaluated at the close of session T.
- Entry or exit filled at the open of session T+1.
- Buy fill: `open * (1 + (slippage_bps + spread_bps/2) / 10000)`.
- Sell fill: `price * (1 - (slippage_bps + spread_bps/2) / 10000)`.
- Commission default: 0 bps (Alpaca equity commission-free), configurable.
- SEC Section 31 fee on sells: 0.3 bps by default.
- FINRA TAF on sells: $0.000166 per share by default.
- One active stock position at a time, long only, no leverage.
- Missing bars defer an execution; a pending entry is dropped after three sessions without an open.
- The last session force-liquidates any open position at the close so the reported equity is realized.
- Splits and dividends are embedded in adjusted bars. There is no separate dividend cash ledger, which avoids double counting.

## 10. Backtest accounting

- Continuous portfolio across the six benchmark windows.
- Equity marked daily at the close.
- Only BUY candidates are eligible for entry; HOLD means cash and SELL means exit or stay flat.
- Candidate ranking: strength descending, then rating, then symbol for deterministic ties.
- Trade PnL includes fill slippage, half-spread, commissions and sell fees.
- Maximum drawdown is measured from the running equity peak, not from the initial capital only.
- CAGR uses calendar years. Sharpe and Sortino use 252 trading days and an optional configurable risk-free rate (default 0% to keep comparisons simple and transparent).
- Benchmark equity curves use the same fee model and the same session dates.

## 11. No look-ahead guarantees

- Indicator series are causal by construction.
- A test appends future bars and asserts that the earlier signal, RSI, ATR and rating are unchanged.
- A full-history backtest and a truncated-history backtest must produce identical equity and decisions before the cutoff. This is an automated integration test.
- Walk-forward parameter selection uses only the training window. The final six months are reserved as an untouched out-of-sample holdout.

## 12. What the signals are not

- Not price forecasts.
- Not recommendations to buy or sell.
- Not a claim that a past rating predicts a short-term move.
- Not validated for every market regime. The real-data benchmark underperformed buy-and-hold over 2023-2026 and out-of-sample folds were negative (see `BENCHMARKS.md`).

## 13. Tuning

All parameters are in `config/default.toml`. Keep changes versioned and rerun walk-forward plus the untouched OOS holdout. Do not tune on the holdout. The built-in grid search only touches training windows and is deliberately small; enlarging it increases multiple-comparison and overfitting risk.
