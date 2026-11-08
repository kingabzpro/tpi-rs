//! SQLite candle cache.
//!
//! Adjusted daily bars are stored as absolute values keyed by symbol/date. A
//! cache TTL controls when a symbol's requested range is refetched so corporate
//! action adjustments do not silently mix with stale rows.

use crate::error::{Error, Result};
use crate::model::Bar;
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct CandleCache {
    conn: Mutex<Connection>,
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coverage {
    pub min: NaiveDate,
    pub max: NaiveDate,
    pub count: usize,
}

impl CandleCache {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let conn = Connection::open(path).map_err(|e| Error::Cache(e.to_string()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| Error::Cache(e.to_string()))?;
        let _: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .unwrap_or_else(|_| "wal".to_string());
        conn.execute_batch(
            "PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS bars (
                 symbol TEXT NOT NULL,
                 date TEXT NOT NULL,
                 open REAL NOT NULL,
                 high REAL NOT NULL,
                 low REAL NOT NULL,
                 close REAL NOT NULL,
                 volume REAL NOT NULL,
                 PRIMARY KEY (symbol, date)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )
        .map_err(|e| Error::Cache(e.to_string()))?;
        Ok(Self {
            conn: Mutex::new(conn),
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| Error::Cache("cache mutex poisoned".to_string()))
    }

    pub fn load(
        &self,
        symbols: &[String],
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<HashMap<String, Vec<Bar>>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT date, open, high, low, close, volume
                 FROM bars WHERE symbol = ?1 AND date >= ?2 AND date <= ?3
                 ORDER BY date",
            )
            .map_err(|e| Error::Cache(e.to_string()))?;
        let mut out = HashMap::new();
        for symbol in symbols {
            let rows = stmt
                .query_map(params![symbol, start.to_string(), end.to_string()], |row| {
                    let date: String = row.get(0)?;
                    Ok((
                        date,
                        Bar {
                            date: NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
                            open: row.get(1)?,
                            high: row.get(2)?,
                            low: row.get(3)?,
                            close: row.get(4)?,
                            volume: row.get(5)?,
                        },
                    ))
                })
                .map_err(|e| Error::Cache(e.to_string()))?;
            let mut bars = Vec::new();
            for row in rows {
                let (date, mut bar) = row.map_err(|e| Error::Cache(e.to_string()))?;
                if let Ok(parsed) = NaiveDate::parse_from_str(&date, "%Y-%m-%d") {
                    bar.date = parsed;
                    bars.push(bar);
                }
            }
            if !bars.is_empty() {
                out.insert(symbol.clone(), bars);
            }
        }
        Ok(out)
    }

    pub fn coverage(&self, symbol: &str) -> Result<Option<Coverage>> {
        let conn = self.lock()?;
        let row: Option<(Option<String>, Option<String>, i64)> = conn
            .query_row(
                "SELECT MIN(date), MAX(date), COUNT(*) FROM bars WHERE symbol = ?1",
                params![symbol],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|e| Error::Cache(e.to_string()))?;
        let Some((Some(min), Some(max), count)) = row else {
            return Ok(None);
        };
        let min = NaiveDate::parse_from_str(&min, "%Y-%m-%d").ok();
        let max = NaiveDate::parse_from_str(&max, "%Y-%m-%d").ok();
        match (min, max) {
            (Some(min), Some(max)) => Ok(Some(Coverage {
                min,
                max,
                count: count.max(0) as usize,
            })),
            _ => Ok(None),
        }
    }

    pub fn upsert(&self, symbol: &str, bars: &[Bar]) -> Result<usize> {
        if bars.is_empty() {
            return Ok(0);
        }
        let mut conn = self.lock()?;
        let tx = conn
            .transaction()
            .map_err(|e| Error::Cache(e.to_string()))?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO bars (symbol, date, open, high, low, close, volume)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(symbol, date) DO UPDATE SET
                         open=excluded.open, high=excluded.high, low=excluded.low,
                         close=excluded.close, volume=excluded.volume",
                )
                .map_err(|e| Error::Cache(e.to_string()))?;
            for bar in bars {
                stmt.execute(params![
                    symbol,
                    bar.date.to_string(),
                    bar.open,
                    bar.high,
                    bar.low,
                    bar.close,
                    bar.volume
                ])
                .map_err(|e| Error::Cache(e.to_string()))?;
            }
        }
        tx.commit().map_err(|e| Error::Cache(e.to_string()))?;
        Ok(bars.len())
    }

    pub fn last_refresh(&self) -> Result<Option<DateTime<Utc>>> {
        let conn = self.lock()?;
        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_refresh'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| Error::Cache(e.to_string()))?;
        Ok(value.and_then(|v| {
            DateTime::parse_from_rfc3339(&v)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        }))
    }

    pub fn mark_refreshed(&self, when: DateTime<Utc>) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('last_refresh', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![when.to_rfc3339()],
        )
        .map_err(|e| Error::Cache(e.to_string()))?;
        Ok(())
    }

    pub fn total_bars(&self) -> Result<usize> {
        let conn = self.lock()?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM bars", [], |row| row.get(0))
            .map_err(|e| Error::Cache(e.to_string()))?;
        Ok(count.max(0) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(date: NaiveDate, close: f64) -> Bar {
        Bar {
            date,
            open: close,
            high: close + 1.0,
            low: close - 1.0,
            close,
            volume: 1000.0,
        }
    }

    #[test]
    fn roundtrip_and_coverage() {
        let dir = tempfile::tempdir().unwrap();
        let cache = CandleCache::open(&dir.path().join("t.db")).unwrap();
        let d1 = NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2024, 1, 3).unwrap();
        cache
            .upsert("AAPL", &[bar(d1, 100.0), bar(d2, 101.0)])
            .unwrap();
        let cov = cache.coverage("AAPL").unwrap().unwrap();
        assert_eq!(cov.min, d1);
        assert_eq!(cov.max, d2);
        assert_eq!(cov.count, 2);
        let loaded = cache.load(&["AAPL".into()], d1, d2).unwrap();
        assert_eq!(loaded["AAPL"].len(), 2);
        // Idempotent upsert.
        cache.upsert("AAPL", &[bar(d2, 102.0)]).unwrap();
        let loaded = cache.load(&["AAPL".into()], d1, d2).unwrap();
        assert_eq!(loaded["AAPL"][1].close, 102.0);
        assert_eq!(cache.total_bars().unwrap(), 2);
    }
}
