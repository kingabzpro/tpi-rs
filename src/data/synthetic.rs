//! Deterministic synthetic daily bars for offline demos, tests and benchmarks.
//! These series are explicitly labelled as synthetic and are never presented as
//! market data.

use crate::model::Bar;
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use std::collections::HashMap;

pub fn business_days(start: NaiveDate, end: NaiveDate) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut d = start;
    while d <= end {
        if !matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
            out.push(d);
        }
        d += Duration::days(1);
    }
    out
}

pub fn synthetic_bars(symbol: &str, start: NaiveDate, end: NaiveDate) -> Vec<Bar> {
    let dates = business_days(start, end);
    if dates.is_empty() {
        return Vec::new();
    }
    let mut rng = SplitMix64::new(fnv1a(symbol));
    let base_price = 25.0 + (rng.next_f64() * 420.0);
    let base_volume = 500_000.0 + rng.next_f64() * 6_000_000.0;
    let daily_vol = 0.007 + rng.next_f64() * 0.018;
    let mut regime_drift = (rng.next_f64() - 0.45) * 0.0025;
    let mut close = base_price;
    let mut bars = Vec::with_capacity(dates.len());

    for (i, date) in dates.iter().enumerate() {
        if i % 55 == 0 && i > 0 {
            regime_drift = (rng.next_f64() - 0.45) * 0.0028;
        }
        let gaussian = rng.next_gaussian();
        let gap = rng.next_gaussian() * daily_vol * 0.25;
        let open = (close * (1.0 + gap)).max(1.0);
        let ret = regime_drift + gaussian * daily_vol;
        let next_close = (open * (1.0 + ret)).max(1.0);
        let body_high = open.max(next_close);
        let body_low = open.min(next_close);
        let wick = daily_vol * (0.3 + rng.next_f64() * 0.9);
        let high = body_high * (1.0 + wick);
        let low = (body_low * (1.0 - wick)).max(0.5);
        let volume =
            (base_volume * (0.55 + rng.next_f64() * 0.9) * (1.0 + (ret.abs() / daily_vol) * 0.35))
                .round();
        bars.push(Bar {
            date: *date,
            open: round2(open),
            high: round2(high.max(body_high + 0.01)),
            low: round2(low.min(body_low - 0.01).max(0.5)),
            close: round2(next_close),
            volume,
        });
        close = next_close;
    }
    bars
}

pub fn synthetic_universe(
    symbols: &[String],
    start: NaiveDate,
    end: NaiveDate,
) -> HashMap<String, Vec<Bar>> {
    symbols
        .iter()
        .map(|s| (s.clone(), synthetic_bars(s, start, end)))
        .collect()
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

struct SplitMix64 {
    state: u64,
    spare: Option<f64>,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.max(1),
            spare: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn next_gaussian(&mut self) -> f64 {
        if let Some(spare) = self.spare.take() {
            return spare;
        }
        let u1 = self.next_f64().max(1e-12);
        let u2 = self.next_f64();
        let radius = (-2.0 * u1.ln()).sqrt();
        let theta = std::f64::consts::TAU * u2;
        self.spare = Some(radius * theta.sin());
        radius * theta.cos()
    }
}

fn fnv1a(symbol: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in symbol.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_ohlc_valid() {
        let start = NaiveDate::from_ymd_opt(2023, 11, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let a = synthetic_bars("AAPL", start, end);
        let b = synthetic_bars("AAPL", start, end);
        assert_eq!(a, b);
        assert!(!a.is_empty());
        for bar in &a {
            assert!(bar.high >= bar.open.max(bar.close));
            assert!(bar.low <= bar.open.min(bar.close));
            assert!(bar.low > 0.0);
            assert!(bar.volume > 0.0);
        }
        assert!(
            !a.iter()
                .any(|b| matches!(b.date.weekday(), Weekday::Sat | Weekday::Sun))
        );
    }

    #[test]
    fn symbols_differ() {
        let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 3, 1).unwrap();
        let a = synthetic_bars("AAA", start, end);
        let b = synthetic_bars("BBB", start, end);
        assert_ne!(a[0].close, b[0].close);
    }
}
