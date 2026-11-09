//! Single-instance guard for the trading loops.
//!
//! Two processes trading the same Alpaca account each see an empty book and can
//! both enter, which broke the one-position rule in practice. The lock file
//! holds the owning PID and is treated as stale once it is older than
//! `STALE_AFTER`, so a killed process does not block tomorrow's run.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const STALE_AFTER: Duration = Duration::from_secs(15 * 60);

#[derive(Debug)]
pub struct InstanceLock {
    path: PathBuf,
}

impl InstanceLock {
    /// Take the lock or explain who holds it.
    pub fn acquire(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        if path.exists() && !is_stale(&path) {
            let owner = std::fs::read_to_string(&path).unwrap_or_default();
            return Err(Error::validation(format!(
                "another trading process holds {} ({}); stop it, or delete the file if it was killed",
                path.display(),
                owner.trim()
            )));
        }
        std::fs::write(&path, format!("pid {}\n", std::process::id())).map_err(|source| {
            Error::Io {
                path: path.clone(),
                source,
            }
        })?;
        Ok(Self { path })
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn is_stale(path: &Path) -> bool {
    match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(modified) => SystemTime::now()
            .duration_since(modified)
            .map(|age| age > STALE_AFTER)
            .unwrap_or(true),
        // Unreadable metadata is not evidence that the lock is live.
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tpi-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn second_acquire_is_refused() {
        let dir = temp_dir("second");
        let path = dir.join("paper.lock");
        let first = InstanceLock::acquire(&path).expect("first lock");
        let err = InstanceLock::acquire(&path).expect_err("second must fail");
        assert!(err.to_string().contains("another trading process"), "{err}");
        drop(first);
        assert!(!path.exists(), "lock removed on drop");
        InstanceLock::acquire(&path).expect("lock is reusable after release");
    }

    #[test]
    fn stale_lock_is_taken_over() {
        let dir = temp_dir("stale");
        let path = dir.join("paper.lock");
        std::fs::write(&path, "pid 999999\n").expect("write");
        let old = SystemTime::now() - Duration::from_secs(60 * 60);
        set_modified(&path, old);
        InstanceLock::acquire(&path).expect("stale lock is not blocking");
    }

    fn set_modified(path: &Path, when: SystemTime) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open");
        file.set_modified(when).expect("set mtime");
    }
}
