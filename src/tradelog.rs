//! Append-only trade log for the paper and live loops.
//!
//! One file per UTC day under `paths.logs_dir`. Anything written here is also
//! printed, so the console and the file never disagree.

use crate::config::Config;
use crate::error::{Error, Result};
use chrono::Utc;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

pub struct TradeLog {
    path: PathBuf,
}

impl TradeLog {
    pub fn open(cfg: &Config) -> Result<Self> {
        let dir = PathBuf::from(&cfg.paths.logs_dir);
        std::fs::create_dir_all(&dir).map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })?;
        let path = dir.join(format!("paper-{}.log", Utc::now().format("%Y%m%d")));
        Ok(Self { path })
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Write one line and echo it, so a detached run still leaves a record.
    pub fn record(&self, line: &str) {
        println!("{line}");
        self.append(line);
    }

    pub fn append(&self, line: &str) {
        let stamped = format!("{} {line}\n", Utc::now().format("%H:%M:%S"));
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = file.write_all(stamped.as_bytes());
        }
    }
}
