//! Causal daily indicator series.
//!
//! Every value at index `i` is computed from bars `0..=i` only. The series is
//! precomputed once per symbol for speed; there is no future-bar access.
//! `ta` supplies EMA, SMA, MACD and CCI. Wilder-smoothed RSI, ATR, ADX and the
//! remaining oscillators/channels are implemented here to keep their exact
//! definitions visible and testable.

pub mod ratings;

use crate::model::{Bar, IndicatorSnapshot};
use ta::Next;
use ta::indicators::{
    CommodityChannelIndex, ExponentialMovingAverage, MovingAverageConvergenceDivergence,
    SimpleMovingAverage,
};

type TripleSeries = (Vec<Option<f64>>, Vec<Option<f64>>, Vec<Option<f64>>);

#[derive(Debug, Clone)]
pub struct Series {
    pub dates: Vec<chrono::NaiveDate>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    pub volume: Vec<f64>,
    pub ema10: Vec<Option<f64>>,
    pub ema13: Vec<Option<f64>>,
    pub ema20: Vec<Option<f64>>,
    pub ema30: Vec<Option<f64>>,
    pub ema50: Vec<Option<f64>>,
    pub ema100: Vec<Option<f64>>,
    pub ema200: Vec<Option<f64>>,
    pub rsi14: Vec<Option<f64>>,
    pub atr14: Vec<Option<f64>>,
    pub adx14: Vec<Option<f64>>,
    pub plus_di: Vec<Option<f64>>,
    pub minus_di: Vec<Option<f64>>,
    pub cci20: Vec<Option<f64>>,
    pub macd: Vec<Option<f64>>,
    pub macd_signal: Vec<Option<f64>>,
    pub macd_hist: Vec<Option<f64>>,
    pub stoch_k: Vec<Option<f64>>,
    pub stoch_rsi: Vec<Option<f64>>,
    pub williams_r: Vec<Option<f64>>,
    pub ao: Vec<Option<f64>>,
    pub momentum10: Vec<Option<f64>>,
    pub bbp: Vec<Option<f64>>,
    pub uo: Vec<Option<f64>>,
    pub don10_low: Vec<Option<f64>>,
    pub don20_high: Vec<Option<f64>>,
    pub don20_low: Vec<Option<f64>>,
    pub don40_high: Vec<Option<f64>>,
    pub don40_low: Vec<Option<f64>>,
    pub volume_sma20: Vec<Option<f64>>,
}

impl Series {
    pub fn len(&self) -> usize {
        self.close.len()
    }

    pub fn is_empty(&self) -> bool {
        self.close.is_empty()
    }

    pub fn compute(bars: &[Bar]) -> Self {
        let close: Vec<f64> = bars.iter().map(|b| b.close).collect();
        let high: Vec<f64> = bars.iter().map(|b| b.high).collect();
        let low: Vec<f64> = bars.iter().map(|b| b.low).collect();
        let open: Vec<f64> = bars.iter().map(|b| b.open).collect();
        let volume: Vec<f64> = bars.iter().map(|b| b.volume).collect();
        let dates: Vec<chrono::NaiveDate> = bars.iter().map(|b| b.date).collect();

        let ema10 = ta_ema(&close, 10);
        let ema13 = ta_ema(&close, 13);
        let ema20 = ta_ema(&close, 20);
        let ema30 = ta_ema(&close, 30);
        let ema50 = ta_ema(&close, 50);
        let ema100 = ta_ema(&close, 100);
        let ema200 = ta_ema(&close, 200);

        let rsi14 = rsi_wilder(&close, 14);
        let atr14 = atr_wilder(bars, 14);
        let (adx14, plus_di, minus_di) = adx_wilder(bars, 14);
        let cci20 = ta_cci(bars, 20);
        let (macd, macd_signal, macd_hist) = ta_macd(&close, 12, 26, 9);
        let stoch_k = stochastic_k(bars, 14, 3);
        let stoch_rsi = stoch_rsi(bars, 14, 3);
        let williams_r = williams_r(bars, 14);
        let ao = awesome_oscillator(bars, 5, 34);
        let momentum10 = momentum(&close, 10);
        let bbp = bull_bear_power(bars, 13);
        let uo = ultimate_oscillator(bars, 7, 14, 28);

        Self {
            don10_low: prior_rolling_min(&low, 10),
            don20_high: prior_rolling_max(&high, 20),
            don20_low: prior_rolling_min(&low, 20),
            don40_high: prior_rolling_max(&high, 40),
            don40_low: prior_rolling_min(&low, 40),
            volume_sma20: sma_series(&volume, 20),
            dates,
            open,
            high,
            low,
            close,
            volume,
            ema10,
            ema13,
            ema20,
            ema30,
            ema50,
            ema100,
            ema200,
            rsi14,
            atr14,
            adx14,
            plus_di,
            minus_di,
            cci20,
            macd,
            macd_signal,
            macd_hist,
            stoch_k,
            stoch_rsi,
            williams_r,
            ao,
            momentum10,
            bbp,
            uo,
        }
    }

    pub fn ema_at(&self, i: usize, period: usize) -> Option<f64> {
        let series = match period {
            10 => &self.ema10,
            13 => &self.ema13,
            20 => &self.ema20,
            30 => &self.ema30,
            50 => &self.ema50,
            100 => &self.ema100,
            200 => &self.ema200,
            _ => return None,
        };
        series.get(i).copied().flatten()
    }

    pub fn ema50_slope_pct(&self, i: usize, lookback: usize) -> Option<f64> {
        if i < lookback {
            return None;
        }
        let now = self.ema50.get(i).copied().flatten()?;
        let past = self.ema50.get(i - lookback).copied().flatten()?;
        if past.abs() < f64::EPSILON {
            return None;
        }
        Some((now - past) / past * 100.0)
    }

    pub fn atr_pct(&self, i: usize) -> Option<f64> {
        let atr = self.atr14.get(i).copied().flatten()?;
        let close = *self.close.get(i)?;
        (close > 0.0).then_some(atr / close * 100.0)
    }

    pub fn volume_ratio(&self, i: usize) -> Option<f64> {
        let avg = self.volume_sma20.get(i).copied().flatten()?;
        if avg <= 0.0 {
            return None;
        }
        Some(self.volume.get(i)? / avg)
    }

    pub fn don20_mid(&self, i: usize) -> Option<f64> {
        Some(
            (self.don20_high.get(i).copied().flatten()?
                + self.don20_low.get(i).copied().flatten()?)
                / 2.0,
        )
    }

    /// Prior N-day low channel used by exit rules.
    pub fn don_exit_low(&self, i: usize, period: usize) -> Option<f64> {
        match period {
            10 => self.don10_low.get(i).copied().flatten(),
            20 => self.don20_low.get(i).copied().flatten(),
            40 => self.don40_low.get(i).copied().flatten(),
            _ => None,
        }
    }

    pub fn dist_ema20_atr(&self, i: usize) -> Option<f64> {
        let ema = self.ema20.get(i).copied().flatten()?;
        let atr = self.atr14.get(i).copied().flatten()?;
        if atr <= 0.0 {
            return None;
        }
        Some((self.close.get(i)? - ema) / atr)
    }

    pub fn snapshot(&self, i: usize) -> IndicatorSnapshot {
        let close = self.close.get(i).copied().unwrap_or(0.0);
        IndicatorSnapshot {
            close,
            ema20: self.ema20.get(i).copied().flatten(),
            ema50: self.ema50.get(i).copied().flatten(),
            ema200: self.ema200.get(i).copied().flatten(),
            ema50_slope_pct: self.ema50_slope_pct(i, 10),
            rsi14: self.rsi14.get(i).copied().flatten(),
            atr14: self.atr14.get(i).copied().flatten(),
            atr_pct: self.atr_pct(i),
            adx14: self.adx14.get(i).copied().flatten(),
            plus_di: self.plus_di.get(i).copied().flatten(),
            minus_di: self.minus_di.get(i).copied().flatten(),
            donchian20_high: self.don20_high.get(i).copied().flatten(),
            donchian20_low: self.don20_low.get(i).copied().flatten(),
            donchian20_mid: self.don20_mid(i),
            donchian40_high: self.don40_high.get(i).copied().flatten(),
            donchian40_low: self.don40_low.get(i).copied().flatten(),
            macd: self.macd.get(i).copied().flatten(),
            macd_signal: self.macd_signal.get(i).copied().flatten(),
            macd_hist: self.macd_hist.get(i).copied().flatten(),
            stoch_k: self.stoch_k.get(i).copied().flatten(),
            cci20: self.cci20.get(i).copied().flatten(),
            volume_sma20: self.volume_sma20.get(i).copied().flatten(),
            volume_ratio: self.volume_ratio(i),
            dist_ema20_atr: self.dist_ema20_atr(i),
        }
    }
}

// ---------------------------------------------------------------------------
// ta-backed indicators
// ---------------------------------------------------------------------------

fn ta_ema(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut ind = ExponentialMovingAverage::new(period).expect("period > 0");
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let out = ind.next(*v);
            (i + 1 >= period).then_some(out)
        })
        .collect()
}

fn sma_series(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut ind = SimpleMovingAverage::new(period).expect("period > 0");
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let out = ind.next(*v);
            (i + 1 >= period).then_some(out)
        })
        .collect()
}

fn ta_cci(bars: &[Bar], period: usize) -> Vec<Option<f64>> {
    let mut ind = CommodityChannelIndex::new(period).expect("period > 0");
    bars.iter()
        .enumerate()
        .map(|(i, b)| {
            let out = ind.next(b);
            (i + 1 >= period).then_some(out)
        })
        .collect()
}

fn ta_macd(values: &[f64], fast: usize, slow: usize, signal: usize) -> TripleSeries {
    let mut ind = MovingAverageConvergenceDivergence::new(fast, slow, signal).expect("period > 0");
    let mut macd = Vec::with_capacity(values.len());
    let mut sig = Vec::with_capacity(values.len());
    let mut hist = Vec::with_capacity(values.len());
    let warmup = slow + signal;
    for (i, v) in values.iter().enumerate() {
        let out = ind.next(*v);
        if i + 1 >= warmup {
            macd.push(Some(out.macd));
            sig.push(Some(out.signal));
            hist.push(Some(out.histogram));
        } else {
            macd.push(None);
            sig.push(None);
            hist.push(None);
        }
    }
    (macd, sig, hist)
}

// ---------------------------------------------------------------------------
// Wilder smoothing family
// ---------------------------------------------------------------------------

fn rsi_wilder(close: &[f64], period: usize) -> Vec<Option<f64>> {
    let n = close.len();
    let mut out = vec![None; n];
    if n <= period || period == 0 {
        return out;
    }
    let mut avg_gain = 0.0;
    let mut avg_loss = 0.0;
    for i in 1..=period {
        let change = close[i] - close[i - 1];
        if change >= 0.0 {
            avg_gain += change;
        } else {
            avg_loss -= change;
        }
    }
    avg_gain /= period as f64;
    avg_loss /= period as f64;
    out[period] = Some(rsi_from_averages(avg_gain, avg_loss));
    for i in (period + 1)..n {
        let change = close[i] - close[i - 1];
        let (gain, loss) = if change >= 0.0 {
            (change, 0.0)
        } else {
            (0.0, -change)
        };
        avg_gain = (avg_gain * (period as f64 - 1.0) + gain) / period as f64;
        avg_loss = (avg_loss * (period as f64 - 1.0) + loss) / period as f64;
        out[i] = Some(rsi_from_averages(avg_gain, avg_loss));
    }
    out
}

fn rsi_from_averages(avg_gain: f64, avg_loss: f64) -> f64 {
    if avg_loss <= f64::EPSILON {
        100.0
    } else if avg_gain <= f64::EPSILON {
        0.0
    } else {
        100.0 - 100.0 / (1.0 + avg_gain / avg_loss)
    }
}

pub fn true_ranges(bars: &[Bar]) -> Vec<f64> {
    let mut tr = Vec::with_capacity(bars.len());
    for (i, b) in bars.iter().enumerate() {
        let value = if i == 0 {
            b.high - b.low
        } else {
            let prev = bars[i - 1].close;
            (b.high - b.low)
                .max((b.high - prev).abs())
                .max((b.low - prev).abs())
        };
        tr.push(value.max(0.0));
    }
    tr
}

fn atr_wilder(bars: &[Bar], period: usize) -> Vec<Option<f64>> {
    let tr = true_ranges(bars);
    wilder_average(&tr, period)
}

fn wilder_average(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let n = values.len();
    let mut out = vec![None; n];
    if n < period || period == 0 {
        return out;
    }
    let sum: f64 = values[..period].iter().sum();
    let mut avg = sum / period as f64;
    out[period - 1] = Some(avg);
    for i in period..n {
        avg = (avg * (period as f64 - 1.0) + values[i]) / period as f64;
        out[i] = Some(avg);
    }
    out
}

fn adx_wilder(bars: &[Bar], period: usize) -> TripleSeries {
    let n = bars.len();
    let mut plus_dm = vec![0.0; n];
    let mut minus_dm = vec![0.0; n];
    for i in 1..n {
        let up = bars[i].high - bars[i - 1].high;
        let down = bars[i - 1].low - bars[i].low;
        if up > down && up > 0.0 {
            plus_dm[i] = up;
        }
        if down > up && down > 0.0 {
            minus_dm[i] = down;
        }
    }
    let tr = true_ranges(bars);
    let tr_s = wilder_average(&tr, period);
    let plus_s = wilder_average(&plus_dm, period);
    let minus_s = wilder_average(&minus_dm, period);

    let mut plus_di = vec![None; n];
    let mut minus_di = vec![None; n];
    let mut dx = vec![0.0; n];
    let mut dx_valid = vec![false; n];
    for i in 0..n {
        if let (Some(trv), Some(p), Some(m)) = (tr_s[i], plus_s[i], minus_s[i])
            && trv > 0.0
        {
            let pdi = 100.0 * p / trv;
            let mdi = 100.0 * m / trv;
            plus_di[i] = Some(pdi);
            minus_di[i] = Some(mdi);
            let denom = pdi + mdi;
            if denom > 0.0 {
                dx[i] = 100.0 * (pdi - mdi).abs() / denom;
                dx_valid[i] = true;
            }
        }
    }

    let mut adx = vec![None; n];
    let mut collected = Vec::new();
    for i in 0..n {
        if dx_valid[i] {
            collected.push((i, dx[i]));
        }
    }
    if collected.len() >= period {
        let first_avg: f64 =
            collected[..period].iter().map(|(_, v)| *v).sum::<f64>() / period as f64;
        let first_idx = collected[period - 1].0;
        adx[first_idx] = Some(first_avg);
        let mut prev = first_avg;
        for (idx, value) in collected.iter().skip(period) {
            prev = (prev * (period as f64 - 1.0) + value) / period as f64;
            adx[*idx] = Some(prev);
        }
    }

    (adx, plus_di, minus_di)
}

// ---------------------------------------------------------------------------
// Other oscillators and channels
// ---------------------------------------------------------------------------

fn stochastic_k(bars: &[Bar], period: usize, smooth: usize) -> Vec<Option<f64>> {
    let n = bars.len();
    let mut raw = vec![0.0; n];
    let mut valid = vec![false; n];
    for i in (period - 1)..n {
        let hh = bars[i + 1 - period..=i]
            .iter()
            .map(|b| b.high)
            .fold(f64::NEG_INFINITY, f64::max);
        let ll = bars[i + 1 - period..=i]
            .iter()
            .map(|b| b.low)
            .fold(f64::INFINITY, f64::min);
        raw[i] = if (hh - ll).abs() < f64::EPSILON {
            50.0
        } else {
            (bars[i].close - ll) / (hh - ll) * 100.0
        };
        valid[i] = true;
    }
    smooth_valid(&raw, &valid, smooth)
}

fn stoch_rsi(bars: &[Bar], period: usize, smooth: usize) -> Vec<Option<f64>> {
    let rsi = rsi_wilder(&bars.iter().map(|b| b.close).collect::<Vec<_>>(), period);
    let n = rsi.len();
    let mut raw = vec![0.0; n];
    let mut valid = vec![false; n];
    for i in (period * 2 - 1)..n {
        let window = &rsi[i + 1 - period..=i];
        if window.iter().any(|v| v.is_none()) {
            continue;
        }
        let values: Vec<f64> = window.iter().map(|v| v.unwrap()).collect();
        let min = values.iter().fold(f64::INFINITY, |a, b| a.min(*b));
        let max = values.iter().fold(f64::NEG_INFINITY, |a, b| a.max(*b));
        raw[i] = if (max - min).abs() < f64::EPSILON {
            50.0
        } else {
            (values[values.len() - 1] - min) / (max - min) * 100.0
        };
        valid[i] = true;
    }
    smooth_valid(&raw, &valid, smooth)
}

fn smooth_valid(raw: &[f64], valid: &[bool], period: usize) -> Vec<Option<f64>> {
    let n = raw.len();
    let mut out = vec![None; n];
    for i in 0..n {
        if i + 1 >= period && (i + 1 - period..=i).all(|j| valid[j]) {
            let avg = raw[i + 1 - period..=i].iter().sum::<f64>() / period as f64;
            out[i] = Some(avg);
        }
    }
    out
}

fn williams_r(bars: &[Bar], period: usize) -> Vec<Option<f64>> {
    let n = bars.len();
    let mut out = vec![None; n];
    for i in (period - 1)..n {
        let hh = bars[i + 1 - period..=i]
            .iter()
            .map(|b| b.high)
            .fold(f64::NEG_INFINITY, f64::max);
        let ll = bars[i + 1 - period..=i]
            .iter()
            .map(|b| b.low)
            .fold(f64::INFINITY, f64::min);
        out[i] = if (hh - ll).abs() < f64::EPSILON {
            Some(-50.0)
        } else {
            Some((hh - bars[i].close) / (hh - ll) * -100.0)
        };
    }
    out
}

fn awesome_oscillator(bars: &[Bar], fast: usize, slow: usize) -> Vec<Option<f64>> {
    let medians: Vec<f64> = bars.iter().map(|b| b.median_price()).collect();
    let fast_sma = sma_series(&medians, fast);
    let slow_sma = sma_series(&medians, slow);
    fast_sma
        .iter()
        .zip(slow_sma.iter())
        .map(|(f, s)| match (f, s) {
            (Some(f), Some(s)) => Some(f - s),
            _ => None,
        })
        .collect()
}

fn momentum(close: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; close.len()];
    for i in period..close.len() {
        out[i] = Some(close[i] - close[i - period]);
    }
    out
}

fn bull_bear_power(bars: &[Bar], period: usize) -> Vec<Option<f64>> {
    let close: Vec<f64> = bars.iter().map(|b| b.close).collect();
    let ema = ta_ema(&close, period);
    bars.iter()
        .zip(ema.iter())
        .map(|(b, e)| e.map(|e| (b.high - e) - (e - b.low)))
        .collect()
}

fn ultimate_oscillator(bars: &[Bar], p1: usize, p2: usize, p3: usize) -> Vec<Option<f64>> {
    let n = bars.len();
    let mut out = vec![None; n];
    if n < 2 {
        return out;
    }
    let mut buying_pressure = vec![0.0; n];
    let mut tr = vec![0.0; n];
    for i in 1..n {
        let prev_close = bars[i - 1].close;
        let true_low = bars[i].low.min(prev_close);
        let true_high = bars[i].high.max(prev_close);
        buying_pressure[i] = bars[i].close - true_low;
        tr[i] = (true_high - true_low).max(0.0);
    }
    let max_period = p3;
    for i in 0..n {
        if i + 1 < max_period {
            continue;
        }
        let avg = |period: usize| -> Option<f64> {
            let start = i + 1 - period;
            let bp: f64 = buying_pressure[start..=i].iter().sum();
            let tr_sum: f64 = tr[start..=i].iter().sum();
            (tr_sum > 0.0).then_some(bp / tr_sum)
        };
        if let (Some(a1), Some(a2), Some(a3)) = (avg(p1), avg(p2), avg(p3)) {
            out[i] = Some(100.0 * (4.0 * a1 + 2.0 * a2 + a3) / 7.0);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Simple causal helpers
// ---------------------------------------------------------------------------

pub fn prior_rolling_max(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let n = values.len();
    let mut out = vec![None; n];
    for i in 0..n {
        if i < period {
            continue;
        }
        out[i] = Some(
            values[i - period..i]
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max),
        );
    }
    out
}

pub fn prior_rolling_min(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let n = values.len();
    let mut out = vec![None; n];
    for i in 0..n {
        if i < period {
            continue;
        }
        out[i] = Some(
            values[i - period..i]
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min),
        );
    }
    out
}

/// Simple moving average at `i` over `period` values, computed from the slice.
pub fn sma_at(values: &[f64], i: usize, period: usize) -> Option<f64> {
    if period == 0 || i + 1 < period {
        return None;
    }
    let sum: f64 = values[i + 1 - period..=i].iter().sum();
    Some(sum / period as f64)
}

/// Weighted moving average at `i`; most recent value has the largest weight.
pub fn wma_at(values: &[f64], i: usize, period: usize) -> Option<f64> {
    if period == 0 || i + 1 < period {
        return None;
    }
    let slice = &values[i + 1 - period..=i];
    let mut weighted = 0.0;
    let mut denom = 0.0;
    for (k, v) in slice.iter().enumerate() {
        let weight = (k + 1) as f64;
        weighted += v * weight;
        denom += weight;
    }
    (denom > 0.0).then_some(weighted / denom)
}

/// Hull moving average: WMA(2*WMA(n/2) - WMA(n), sqrt(n)).
pub fn hma_at(values: &[f64], i: usize, period: usize) -> Option<f64> {
    let half = period / 2;
    let root = (period as f64).sqrt().round() as usize;
    let mut transformed = Vec::with_capacity(period);
    let start = i.saturating_sub(period + root);
    for j in start..=i {
        let half_avg = wma_at(values, j, half)?;
        let full_avg = wma_at(values, j, period)?;
        transformed.push(2.0 * half_avg - full_avg);
    }
    if transformed.len() < root {
        return None;
    }
    wma_at(&transformed, transformed.len() - 1, root)
}

/// Volume-weighted moving average at `i`.
pub fn vwma_at(close: &[f64], volume: &[f64], i: usize, period: usize) -> Option<f64> {
    if period == 0 || i + 1 < period {
        return None;
    }
    let mut pv = 0.0;
    let mut vol = 0.0;
    for j in i + 1 - period..=i {
        pv += close[j] * volume[j];
        vol += volume[j];
    }
    (vol > 0.0).then_some(pv / vol)
}

/// Ichimoku base line (9, 26, 52, 26): (max9+min9+max26+min26)/4.
pub fn ichimoku_base_at(high: &[f64], low: &[f64], i: usize) -> Option<f64> {
    if i + 1 < 26 {
        return None;
    }
    let max9 = high[i + 1 - 9..=i]
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let min9 = low[i + 1 - 9..=i]
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let max26 = high[i + 1 - 26..=i]
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let min26 = low[i + 1 - 26..=i]
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    Some((max9 + min9 + max26 + min26) / 4.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn bar(date: i32, close: f64) -> Bar {
        Bar {
            date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap() + chrono::Days::new(date as u64),
            open: close,
            high: close + 1.0,
            low: close - 1.0,
            close,
            volume: 1000.0,
        }
    }

    #[test]
    fn donchian_excludes_current_bar() {
        let highs: Vec<f64> = (1..=10).map(|v| v as f64).collect();
        let d = prior_rolling_max(&highs, 3);
        assert_eq!(d[2], None, "only two prior bars are available");
        assert_eq!(d[3], Some(3.0));
        assert_eq!(d[9], Some(9.0));
    }

    #[test]
    fn constant_series_rsi_is_100() {
        let closes = vec![10.0; 40];
        let rsi = rsi_wilder(&closes, 14);
        assert_eq!(rsi[14], Some(100.0));
    }

    #[test]
    fn atr_is_positive_and_causal() {
        let bars: Vec<Bar> = (0..60).map(|i| bar(i, 100.0 + i as f64)).collect();
        let series = Series::compute(&bars);
        assert!(series.atr14[59].unwrap() > 0.0);
        // Truncating future bars must not change the past value.
        let shorter = Series::compute(&bars[..40]);
        assert_eq!(shorter.atr14[39], series.atr14[39]);
        assert_eq!(shorter.rsi14[39], series.rsi14[39]);
    }

    #[test]
    fn ema_series_matches_ta() {
        let values: Vec<f64> = (0..50).map(|i| 10.0 + i as f64 * 0.5).collect();
        let s = Series::compute(
            &values
                .iter()
                .enumerate()
                .map(|(i, c)| bar(i as i32, *c))
                .collect::<Vec<_>>(),
        );
        assert!(s.ema20[49].unwrap() > values[49] - 6.0);
        assert!(s.ema20[49].unwrap() < values[49]);
    }
}
