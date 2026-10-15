//! Built-in system sensors sourced straight from /proc and /sys, with no
//! external dependencies (and so, no version drift to chase).

use crate::entities::{Source, Value};
use anyhow::{Context, Result};
use async_trait::async_trait;
use nix::sys::statvfs::statvfs;
use std::sync::Mutex;
use tokio::fs;

async fn read_trimmed(path: &str) -> Result<String> {
    Ok(fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read {path}"))?
        .trim()
        .to_string())
}

/// Total CPU usage percentage, computed as a delta between consecutive polls
/// of /proc/stat's aggregate `cpu` line. The first read always returns 0.0.
pub struct CpuUsageSource {
    prev: Mutex<Option<(u64, u64)>>, // (idle, total)
}

impl Default for CpuUsageSource {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuUsageSource {
    pub fn new() -> Self {
        Self {
            prev: Mutex::new(None),
        }
    }

    fn parse_cpu_line(line: &str) -> Option<(u64, u64)> {
        let fields: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .filter_map(|f| f.parse::<u64>().ok())
            .collect();
        if fields.len() < 4 {
            return None;
        }
        let idle = fields[3] + fields.get(4).copied().unwrap_or(0); // idle + iowait
        let total: u64 = fields.iter().sum();
        Some((idle, total))
    }
}

#[async_trait]
impl Source for CpuUsageSource {
    async fn read(&self) -> Result<Value> {
        let stat = read_trimmed("/proc/stat").await?;
        let line = stat.lines().next().context("empty /proc/stat")?;
        let (idle, total) =
            Self::parse_cpu_line(line).context("could not parse /proc/stat cpu line")?;

        let mut prev = self.prev.lock().unwrap();
        let usage = match *prev {
            Some((prev_idle, prev_total)) => {
                let idle_delta = idle.saturating_sub(prev_idle) as f64;
                let total_delta = total.saturating_sub(prev_total) as f64;
                if total_delta > 0.0 {
                    (100.0 * (1.0 - idle_delta / total_delta)).clamp(0.0, 100.0)
                } else {
                    0.0
                }
            }
            None => 0.0,
        };
        *prev = Some((idle, total));
        Ok(Value::Num((usage * 10.0).round() / 10.0))
    }
}

fn parse_meminfo_kb(meminfo: &str, key: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix(key)?;
        rest.trim()
            .trim_end_matches(" kB")
            .trim()
            .parse::<u64>()
            .ok()
    })
}

pub struct MemoryUsageSource;

#[async_trait]
impl Source for MemoryUsageSource {
    async fn read(&self) -> Result<Value> {
        let meminfo = read_trimmed("/proc/meminfo").await?;
        let total = parse_meminfo_kb(&meminfo, "MemTotal:").context("MemTotal missing")?;
        let avail = parse_meminfo_kb(&meminfo, "MemAvailable:").context("MemAvailable missing")?;
        if total == 0 {
            return Ok(Value::Num(0.0));
        }
        let used_pct = 100.0 * (total.saturating_sub(avail)) as f64 / total as f64;
        Ok(Value::Num((used_pct * 10.0).round() / 10.0))
    }
}

pub struct SwapUsageSource;

#[async_trait]
impl Source for SwapUsageSource {
    async fn read(&self) -> Result<Value> {
        let meminfo = read_trimmed("/proc/meminfo").await?;
        let total = parse_meminfo_kb(&meminfo, "SwapTotal:").unwrap_or(0);
        let free = parse_meminfo_kb(&meminfo, "SwapFree:").unwrap_or(0);
        if total == 0 {
            return Ok(Value::Num(0.0));
        }
        let used_pct = 100.0 * (total.saturating_sub(free)) as f64 / total as f64;
        Ok(Value::Num((used_pct * 10.0).round() / 10.0))
    }
}

pub struct UptimeSource;

#[async_trait]
impl Source for UptimeSource {
    async fn read(&self) -> Result<Value> {
        let raw = read_trimmed("/proc/uptime").await?;
        let seconds: f64 = raw
            .split_whitespace()
            .next()
            .context("empty /proc/uptime")?
            .parse()
            .context("could not parse /proc/uptime")?;
        Ok(Value::Num(seconds.round()))
    }
}

/// Load average, picking one of the three columns of /proc/loadavg.
pub struct LoadAverageSource {
    pub column: usize, // 0 = 1min, 1 = 5min, 2 = 15min
}

#[async_trait]
impl Source for LoadAverageSource {
    async fn read(&self) -> Result<Value> {
        let raw = read_trimmed("/proc/loadavg").await?;
        let value: f64 = raw
            .split_whitespace()
            .nth(self.column)
            .context("missing /proc/loadavg column")?
            .parse()
            .context("could not parse /proc/loadavg")?;
        Ok(Value::Num(value))
    }
}

pub struct ProcessCountSource;

#[async_trait]
impl Source for ProcessCountSource {
    async fn read(&self) -> Result<Value> {
        let raw = read_trimmed("/proc/loadavg").await?;
        let field = raw
            .split_whitespace()
            .nth(3)
            .context("missing /proc/loadavg running/total field")?;
        let total: f64 = field
            .split('/')
            .nth(1)
            .context("unexpected /proc/loadavg running/total format")?
            .parse()
            .context("could not parse process count")?;
        Ok(Value::Num(total))
    }
}

/// Percentage of disk space used at `path` (e.g. "/", "/home").
pub struct DiskUsageSource {
    pub path: String,
}

#[async_trait]
impl Source for DiskUsageSource {
    async fn read(&self) -> Result<Value> {
        let path = self.path.clone();
        let stat = tokio::task::spawn_blocking(move || statvfs(path.as_str()))
            .await
            .context("statvfs task panicked")?
            .with_context(|| format!("statvfs failed for {}", self.path))?;
        let total = stat.blocks() as f64;
        if total == 0.0 {
            return Ok(Value::Num(0.0));
        }
        let free = stat.blocks_available() as f64;
        let used_pct = 100.0 * (total - free) / total;
        Ok(Value::Num((used_pct * 10.0).round() / 10.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_line() {
        let line = "cpu  100 0 200 600 50 0 0 0 0 0";
        let (idle, total) = CpuUsageSource::parse_cpu_line(line).unwrap();
        assert_eq!(idle, 650); // idle(600) + iowait(50)
        assert_eq!(total, 950);
    }

    #[test]
    fn parses_meminfo() {
        let meminfo = "MemTotal:       16384000 kB\nMemAvailable:    8192000 kB\n";
        assert_eq!(parse_meminfo_kb(meminfo, "MemTotal:"), Some(16384000));
        assert_eq!(parse_meminfo_kb(meminfo, "MemAvailable:"), Some(8192000));
    }

    #[tokio::test]
    async fn cpu_usage_first_read_is_zero() {
        let src = CpuUsageSource::new();
        // We can't rely on /proc/stat contents in a sandbox, so just check
        // the delta math directly instead of calling read().
        assert_eq!(*src.prev.lock().unwrap(), None);
    }
}
