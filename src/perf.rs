//! Portable process metrics for benchmark reports: working set/RSS, peak
//! memory, CPU time and wall time. No external crates beyond the OS APIs.

use std::time::Instant;

#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessStats {
    pub working_set_mb: f64,
    pub peak_working_set_mb: f64,
    pub cpu_seconds: f64,
    pub wall_seconds: f64,
}

impl ProcessStats {
    pub fn cpu_percent(&self) -> f64 {
        if self.wall_seconds > 0.0 {
            self.cpu_seconds / self.wall_seconds * 100.0
        } else {
            0.0
        }
    }
}

pub struct Stopwatch {
    start: Instant,
    cpu_start: f64,
    wall_start: f64,
}

impl Stopwatch {
    pub fn start() -> Self {
        let s = snapshot();
        Self {
            start: Instant::now(),
            cpu_start: s.cpu_seconds,
            wall_start: s.wall_seconds,
        }
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    pub fn finish(&self) -> ProcessStats {
        let now = snapshot();
        ProcessStats {
            working_set_mb: now.working_set_mb,
            peak_working_set_mb: now.peak_working_set_mb,
            cpu_seconds: (now.cpu_seconds - self.cpu_start).max(0.0),
            wall_seconds: (now.wall_seconds - self.wall_start).max(self.elapsed_secs()),
        }
    }
}

#[cfg(windows)]
pub fn snapshot() -> ProcessStats {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    unsafe {
        let process = GetCurrentProcess();
        let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let ok = GetProcessMemoryInfo(
            process,
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        );
        let (working_set, peak) = if ok != 0 {
            (
                counters.WorkingSetSize as f64 / (1024.0 * 1024.0),
                counters.PeakWorkingSetSize as f64 / (1024.0 * 1024.0),
            )
        } else {
            (0.0, 0.0)
        };

        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let ok = GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user);
        let cpu_seconds = if ok != 0 {
            (filetime_to_u64(kernel) + filetime_to_u64(user)) as f64 / 10_000_000.0
        } else {
            0.0
        };

        ProcessStats {
            working_set_mb: working_set,
            peak_working_set_mb: peak,
            wall_seconds: uptime_seconds(),
            cpu_seconds,
        }
    }
}

#[cfg(windows)]
fn filetime_to_u64(ft: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

#[cfg(windows)]
fn uptime_seconds() -> f64 {
    use std::sync::OnceLock;
    use std::time::SystemTime;
    static START: OnceLock<SystemTime> = OnceLock::new();
    let start = START.get_or_init(SystemTime::now);
    SystemTime::now()
        .duration_since(*start)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(target_os = "linux")]
pub fn snapshot() -> ProcessStats {
    let mut working_set_mb = 0.0;
    let mut peak_working_set_mb = 0.0;
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                working_set_mb = parse_kb(rest);
            } else if let Some(rest) = line.strip_prefix("VmHWM:") {
                peak_working_set_mb = parse_kb(rest);
            }
        }
    }
    let cpu_seconds = std::fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|stat| {
            let close = stat.rfind(')')?;
            let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
            // After comm: state=0, ... utime=11, stime=12
            let utime: f64 = fields.get(11)?.parse().ok()?;
            let stime: f64 = fields.get(12)?.parse().ok()?;
            Some((utime + stime) / 100.0)
        })
        .unwrap_or(0.0);
    ProcessStats {
        working_set_mb,
        peak_working_set_mb,
        cpu_seconds,
        wall_seconds: uptime_seconds_linux(),
    }
}

#[cfg(target_os = "linux")]
fn parse_kb(rest: &str) -> f64 {
    rest.split_whitespace()
        .next()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|kb| kb / 1024.0)
        .unwrap_or(0.0)
}

#[cfg(target_os = "linux")]
fn uptime_seconds_linux() -> f64 {
    use std::sync::OnceLock;
    use std::time::SystemTime;
    static START: OnceLock<SystemTime> = OnceLock::new();
    let start = START.get_or_init(SystemTime::now);
    SystemTime::now()
        .duration_since(*start)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn snapshot() -> ProcessStats {
    ProcessStats::default()
}
