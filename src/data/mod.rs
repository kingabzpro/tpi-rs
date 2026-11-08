//! Market-data facade: Alpaca fetch, cache policy and synthetic fallback.

pub mod alpaca;
pub mod cache;
pub mod synthetic;
pub mod universe;

pub use alpaca::{
    Account, AlpacaClient, CalendarDay, FlattenReport, MarketClock, Order, OrderRequest, Position,
};

use crate::config::{AlpacaConfig, CacheSettings, Config};
use crate::error::{Error, Result};
use crate::model::{Bar, DataProvenance, DataSource};
use cache::CandleCache;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub struct MarketData {
    client: Option<AlpacaClient>,
    cache: Option<Arc<CandleCache>>,
    alpaca_cfg: AlpacaConfig,
    cache_cfg: CacheSettings,
    offline: bool,
    refresh: bool,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LoadResult {
    pub bars: HashMap<String, Vec<Bar>>,
    pub provenance: DataProvenance,
    pub warnings: Vec<String>,
    pub missing: Vec<String>,
}

impl MarketData {
    pub fn new(cfg: &Config, offline: bool, refresh: bool) -> Result<Self> {
        let mut warnings = Vec::new();
        let client = if offline {
            None
        } else if cfg.alpaca.has_credentials() {
            Some(AlpacaClient::new(&cfg.alpaca, true)?)
        } else {
            None
        };
        let cache = if cfg.cache.enabled {
            match CandleCache::open(&cfg.cache_path()) {
                Ok(c) => Some(Arc::new(c)),
                Err(e) => {
                    warnings.push(format!(
                        "SQLite cache disabled at {}: {e}",
                        cfg.cache_path().display()
                    ));
                    None
                }
            }
        } else {
            None
        };
        if !offline && client.is_none() && cache.is_none() {
            return Err(Error::MissingCredentials);
        }
        Ok(Self {
            client,
            cache,
            alpaca_cfg: cfg.alpaca.clone(),
            cache_cfg: cfg.cache.clone(),
            offline,
            refresh: refresh || cfg.cache.refresh,
            warnings,
        })
    }

    pub fn offline(&self) -> bool {
        self.offline
    }

    pub fn client(&self) -> Option<&AlpacaClient> {
        self.client.as_ref()
    }

    pub fn cache(&self) -> Option<&Arc<CandleCache>> {
        self.cache.as_ref()
    }

    pub async fn load(
        &self,
        symbols: &[String],
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<LoadResult> {
        if start > end {
            return Err(Error::validation(format!(
                "start {start} is after end {end}"
            )));
        }
        if self.offline {
            let bars = synthetic::synthetic_universe(symbols, start, end);
            return Ok(LoadResult {
                bars,
                provenance: DataProvenance::synthetic(),
                warnings: self.warnings.clone(),
                missing: Vec::new(),
            });
        }

        let mut warnings = self.warnings.clone();
        let mut fetched_bars: HashMap<String, Vec<Bar>> = HashMap::new();
        let mut fetched_any = false;
        let mut used_cache = false;

        let cache = self.cache.as_ref();
        let ttl_expired = match cache {
            Some(c) => match c.last_refresh()? {
                Some(t) => {
                    Utc::now().signed_duration_since(t)
                        > Duration::hours(self.cache_cfg.ttl_hours.max(1))
                }
                None => true,
            },
            None => true,
        };
        let refresh_all = self.refresh || ttl_expired;

        let mut fetch_symbols: Vec<String> = Vec::new();
        let mut fetch_start = start;
        if let Some(cache) = cache {
            if refresh_all {
                fetch_symbols = symbols.to_vec();
            } else {
                let mut tail_start: Option<NaiveDate> = None;
                let mut needs_full = false;
                for symbol in symbols {
                    match cache.coverage(symbol)? {
                        None => {
                            fetch_symbols.push(symbol.clone());
                            needs_full = true;
                        }
                        Some(cov) => {
                            if cov.min > start {
                                fetch_symbols.push(symbol.clone());
                                needs_full = true;
                            } else if cov.max < end {
                                fetch_symbols.push(symbol.clone());
                                let tail = cov.max + Duration::days(1) - Duration::days(5);
                                tail_start = Some(match tail_start {
                                    Some(t) => t.min(tail),
                                    None => tail,
                                });
                            }
                        }
                    }
                }
                if needs_full {
                    fetch_start = start;
                } else if let Some(t) = tail_start {
                    fetch_start = t.max(start);
                }
            }
            if !fetch_symbols.is_empty() {
                used_cache = symbols
                    .iter()
                    .any(|s| !fetch_symbols.iter().any(|f| f == s));
            }
        } else {
            fetch_symbols = symbols.to_vec();
        }

        if !fetch_symbols.is_empty() {
            if let Some(client) = &self.client {
                match client.daily_bars(&fetch_symbols, fetch_start, end).await {
                    Ok(map) => {
                        if let Some(cache) = cache {
                            for (symbol, bars) in &map {
                                cache.upsert(symbol, bars)?;
                            }
                            cache.mark_refreshed(Utc::now())?;
                        }
                        fetched_any = true;
                        for (symbol, bars) in map {
                            fetched_bars.insert(symbol, bars);
                        }
                        let returned: HashSet<&String> = fetched_bars.keys().collect();
                        let missing_fetch: Vec<&String> = fetch_symbols
                            .iter()
                            .filter(|s| !returned.contains(s))
                            .collect();
                        if !missing_fetch.is_empty() {
                            warnings.push(format!(
                                "Alpaca returned no bars for {} symbol(s): {}",
                                missing_fetch.len(),
                                missing_fetch
                                    .iter()
                                    .take(8)
                                    .map(|s| s.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                        }
                    }
                    Err(e) => {
                        let fully_cached = cache
                            .map(|c| {
                                symbols.iter().all(|s| {
                                    c.coverage(s)
                                        .ok()
                                        .flatten()
                                        .is_some_and(|cov| cov.min <= start && cov.max >= end)
                                })
                            })
                            .unwrap_or(false);
                        if fully_cached {
                            warnings.push(format!(
                                "Alpaca fetch failed ({e}); using cached bars for the requested range"
                            ));
                            used_cache = true;
                        } else {
                            return Err(e);
                        }
                    }
                }
            } else {
                let fully_cached = cache
                    .map(|c| {
                        symbols.iter().all(|s| {
                            c.coverage(s)
                                .ok()
                                .flatten()
                                .is_some_and(|cov| cov.min <= start && cov.max >= end)
                        })
                    })
                    .unwrap_or(false);
                if fully_cached {
                    warnings.push(
                        "No Alpaca credentials; using fully cached data for the requested range"
                            .to_string(),
                    );
                    used_cache = true;
                } else {
                    return Err(Error::MissingCredentials);
                }
            }
        }

        let mut bars = if let Some(cache) = cache {
            cache.load(symbols, start, end)?
        } else {
            HashMap::new()
        };
        if cache.is_none() {
            for (symbol, series) in &fetched_bars {
                bars.insert(symbol.clone(), series.clone());
            }
        }

        let mut missing = Vec::new();
        for symbol in symbols {
            match bars.get(symbol) {
                Some(series) if !series.is_empty() => {}
                _ => missing.push(symbol.clone()),
            }
        }
        if !missing.is_empty() {
            warnings.push(format!(
                "No usable market data for {} of {} symbols",
                missing.len(),
                symbols.len()
            ));
        }
        if bars.values().all(|b| b.is_empty()) {
            return Err(Error::NoData(format!(
                "no bars for {} symbols between {start} and {end}",
                symbols.len()
            )));
        }
        if fetched_any && bars.keys().any(|s| !fetched_bars.contains_key(s)) {
            used_cache = true;
        }
        let source = match (fetched_any, used_cache) {
            (true, true) => DataSource::Mixed,
            (true, false) => DataSource::Alpaca,
            (false, _) => DataSource::Cache,
        };
        let mut provenance =
            DataProvenance::alpaca(&self.alpaca_cfg.feed, self.alpaca_cfg.adjustment == "all");
        provenance.source = source;
        if source == DataSource::Cache {
            provenance.note = format!(
                "SQLite cache ({} bars), adjustment={}; no fetch this run",
                cache.map(|c| c.total_bars().unwrap_or(0)).unwrap_or(0),
                self.alpaca_cfg.adjustment
            );
        }
        if source == DataSource::Mixed {
            provenance.note.push_str("; mixed cache and fresh bars");
        }

        Ok(LoadResult {
            bars,
            provenance,
            warnings,
            missing,
        })
    }

    pub fn fetched_at(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
