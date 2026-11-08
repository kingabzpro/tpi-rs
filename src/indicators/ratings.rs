//! TradingView-style five-level technical rating.
//!
//! The panel mirrors TradingView's layout: 15 moving-average votes and 11
//! oscillator votes, then a signed score of `(buy - sell) / total` mapped to
//! Strong Sell / Sell / Neutral / Buy / Strong Buy. It is an independent
//! consensus gauge, not a price forecast and not part of the TPI score unless
//! the user selects the consensus strategy.

use super::{Series, hma_at, ichimoku_base_at, sma_at, vwma_at};
use crate::model::{RatingDetail, RatingVote, Vote};

pub fn technical_rating(series: &Series, i: usize) -> RatingDetail {
    let price = match series.close.get(i) {
        Some(p) => *p,
        None => return RatingDetail::from_votes(Vec::new()),
    };
    let mut votes = Vec::with_capacity(26);

    // --- Moving averages (15) -------------------------------------------------
    for period in [10usize, 20, 30, 50, 100, 200] {
        let value = series.ema_at(i, period);
        votes.push(RatingVote {
            name: format!("EMA {period}"),
            vote: ma_vote(price, value),
            value,
        });
    }
    for period in [10usize, 20, 30, 50, 100, 200] {
        let value = sma_at(&series.close, i, period);
        votes.push(RatingVote {
            name: format!("SMA {period}"),
            vote: ma_vote(price, value),
            value,
        });
    }
    {
        let value = ichimoku_base_at(&series.high, &series.low, i);
        votes.push(RatingVote {
            name: "Ichimoku Base".to_string(),
            vote: ma_vote(price, value),
            value,
        });
    }
    {
        let value = vwma_at(&series.close, &series.volume, i, 20);
        votes.push(RatingVote {
            name: "VWMA 20".to_string(),
            vote: ma_vote(price, value),
            value,
        });
    }
    {
        let value = hma_at(&series.close, i, 20);
        votes.push(RatingVote {
            name: "HMA 20".to_string(),
            vote: ma_vote(price, value),
            value,
        });
    }

    // --- Oscillators (11) -----------------------------------------------------
    let rsi = series.rsi14.get(i).copied().flatten();
    votes.push(RatingVote {
        name: "RSI 14".to_string(),
        vote: band_vote(rsi, 30.0, 70.0, true),
        value: rsi,
    });

    let stoch = series.stoch_k.get(i).copied().flatten();
    votes.push(RatingVote {
        name: "Stochastic %K".to_string(),
        vote: band_vote(stoch, 20.0, 80.0, true),
        value: stoch,
    });

    let cci = series.cci20.get(i).copied().flatten();
    votes.push(RatingVote {
        name: "CCI 20".to_string(),
        vote: band_vote(cci, -100.0, 100.0, false),
        value: cci,
    });

    let (adx, pdi, mdi) = (
        series.adx14.get(i).copied().flatten(),
        series.plus_di.get(i).copied().flatten(),
        series.minus_di.get(i).copied().flatten(),
    );
    let adx_vote = match (adx, pdi, mdi) {
        (Some(a), Some(p), Some(m)) if a >= 20.0 => {
            if p > m {
                Vote::Buy
            } else if m > p {
                Vote::Sell
            } else {
                Vote::Neutral
            }
        }
        _ => Vote::Neutral,
    };
    votes.push(RatingVote {
        name: "ADX 14".to_string(),
        vote: adx_vote,
        value: adx,
    });

    let ao = series.ao.get(i).copied().flatten();
    votes.push(RatingVote {
        name: "Awesome Oscillator".to_string(),
        vote: sign_vote(ao),
        value: ao,
    });

    let mom = series.momentum10.get(i).copied().flatten();
    votes.push(RatingVote {
        name: "Momentum 10".to_string(),
        vote: sign_vote(mom),
        value: mom,
    });

    let macd = series.macd.get(i).copied().flatten();
    let signal = series.macd_signal.get(i).copied().flatten();
    let macd_vote = match (macd, signal) {
        (Some(m), Some(s)) if m > s => Vote::Buy,
        (Some(m), Some(s)) if m < s => Vote::Sell,
        _ => Vote::Neutral,
    };
    votes.push(RatingVote {
        name: "MACD 12/26/9".to_string(),
        vote: macd_vote,
        value: macd,
    });

    let stoch_rsi = series.stoch_rsi.get(i).copied().flatten();
    votes.push(RatingVote {
        name: "Stoch RSI 14".to_string(),
        vote: band_vote(stoch_rsi, 20.0, 80.0, true),
        value: stoch_rsi,
    });

    let williams = series.williams_r.get(i).copied().flatten();
    votes.push(RatingVote {
        name: "Williams %R 14".to_string(),
        vote: band_vote(williams, -80.0, -20.0, true),
        value: williams,
    });

    let bbp = series.bbp.get(i).copied().flatten();
    votes.push(RatingVote {
        name: "Bull/Bear Power 13".to_string(),
        vote: sign_vote(bbp),
        value: bbp,
    });

    let uo = series.uo.get(i).copied().flatten();
    votes.push(RatingVote {
        name: "Ultimate Oscillator".to_string(),
        vote: band_vote(uo, 30.0, 70.0, true),
        value: uo,
    });

    RatingDetail::from_votes(votes)
}

fn ma_vote(price: f64, value: Option<f64>) -> Vote {
    match value {
        Some(v) if price > v => Vote::Buy,
        Some(v) if price < v => Vote::Sell,
        _ => Vote::Neutral,
    }
}

/// For `low_is_buy=true`, values below `buy_below` vote Buy and above
/// `sell_above` vote Sell. For false (CCI), above `sell_above` is Buy and below
/// `buy_below` is Sell.
fn band_vote(value: Option<f64>, buy_below: f64, sell_above: f64, low_is_buy: bool) -> Vote {
    let Some(v) = value else {
        return Vote::Neutral;
    };
    if low_is_buy {
        if v <= buy_below {
            Vote::Buy
        } else if v >= sell_above {
            Vote::Sell
        } else {
            Vote::Neutral
        }
    } else if v >= sell_above {
        Vote::Buy
    } else if v <= buy_below {
        Vote::Sell
    } else {
        Vote::Neutral
    }
}

fn sign_vote(value: Option<f64>) -> Vote {
    match value {
        Some(v) if v > 0.0 => Vote::Buy,
        Some(v) if v < 0.0 => Vote::Sell,
        _ => Vote::Neutral,
    }
}

/// Allocation-light rating used by backtests and large parameter sweeps.
/// It applies exactly the same vote rules as [`technical_rating`] but only
/// returns the five-level result.
pub fn rating_fast(series: &Series, i: usize) -> crate::model::Rating {
    let Some(&price) = series.close.get(i) else {
        return crate::model::Rating::Neutral;
    };
    let mut buy = 0i32;
    let mut sell = 0i32;
    let mut count = |vote: Vote| match vote {
        Vote::Buy => buy += 1,
        Vote::Sell => sell += 1,
        Vote::Neutral => {}
    };

    for period in [10usize, 20, 30, 50, 100, 200] {
        count(ma_vote(price, series.ema_at(i, period)));
    }
    for period in [10usize, 20, 30, 50, 100, 200] {
        count(ma_vote(price, sma_at(&series.close, i, period)));
    }
    count(ma_vote(
        price,
        ichimoku_base_at(&series.high, &series.low, i),
    ));
    count(ma_vote(
        price,
        vwma_at(&series.close, &series.volume, i, 20),
    ));
    count(ma_vote(price, hma_at(&series.close, i, 20)));

    count(band_vote(
        series.rsi14.get(i).copied().flatten(),
        30.0,
        70.0,
        true,
    ));
    count(band_vote(
        series.stoch_k.get(i).copied().flatten(),
        20.0,
        80.0,
        true,
    ));
    count(band_vote(
        series.cci20.get(i).copied().flatten(),
        -100.0,
        100.0,
        false,
    ));
    let adx = series.adx14.get(i).copied().flatten();
    let pdi = series.plus_di.get(i).copied().flatten();
    let mdi = series.minus_di.get(i).copied().flatten();
    count(match (adx, pdi, mdi) {
        (Some(a), Some(p), Some(m)) if a >= 20.0 => {
            if p > m {
                Vote::Buy
            } else if m > p {
                Vote::Sell
            } else {
                Vote::Neutral
            }
        }
        _ => Vote::Neutral,
    });
    count(sign_vote(series.ao.get(i).copied().flatten()));
    count(sign_vote(series.momentum10.get(i).copied().flatten()));
    count(
        match (
            series.macd.get(i).copied().flatten(),
            series.macd_signal.get(i).copied().flatten(),
        ) {
            (Some(m), Some(s)) if m > s => Vote::Buy,
            (Some(m), Some(s)) if m < s => Vote::Sell,
            _ => Vote::Neutral,
        },
    );
    count(band_vote(
        series.stoch_rsi.get(i).copied().flatten(),
        20.0,
        80.0,
        true,
    ));
    count(band_vote(
        series.williams_r.get(i).copied().flatten(),
        -80.0,
        -20.0,
        true,
    ));
    count(sign_vote(series.bbp.get(i).copied().flatten()));
    count(band_vote(
        series.uo.get(i).copied().flatten(),
        30.0,
        70.0,
        true,
    ));

    let total = 26.0;
    crate::model::Rating::from_score((buy as f64 - sell as f64) / total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Bar;
    use chrono::NaiveDate;

    fn series(n: usize, step: f64) -> Vec<Bar> {
        (0..n)
            .map(|i| Bar {
                date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap() + chrono::Days::new(i as u64),
                open: 100.0 + i as f64 * step,
                high: 101.0 + i as f64 * step,
                low: 99.0 + i as f64 * step,
                close: 100.5 + i as f64 * step,
                volume: 1_000_000.0 + i as f64 * 1000.0,
            })
            .collect()
    }

    #[test]
    fn rating_has_26_votes_and_is_bounded() {
        let bars = series(300, 0.5);
        let s = Series::compute(&bars);
        let detail = technical_rating(&s, 299);
        assert_eq!(detail.votes.len(), 26);
        assert!((0..=26).contains(&(detail.buy_votes + detail.neutral_votes + detail.sell_votes)));
        assert!(detail.score >= -1.0 && detail.score <= 1.0);
    }

    #[test]
    fn uptrend_rates_buy_or_better() {
        let bars = series(300, 0.5);
        let s = Series::compute(&bars);
        let detail = technical_rating(&s, 299);
        assert!(detail.rating.rank() >= 1, "rating was {}", detail.rating);
    }
}
