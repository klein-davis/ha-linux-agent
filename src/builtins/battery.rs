//! Battery percent / charging state, read from /sys/class/power_supply.

use crate::entities::{Source, Value};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;

async fn find_power_supply(prefixes: &[&str]) -> Option<PathBuf> {
    let mut entries = fs::read_dir("/sys/class/power_supply").await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if prefixes.iter().any(|p| name.starts_with(p)) {
            return Some(entry.path());
        }
    }
    None
}

async fn read_trimmed(path: &std::path::Path) -> Result<String> {
    Ok(fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?
        .trim()
        .to_string())
}

pub struct BatteryPercentSource;

#[async_trait]
impl Source for BatteryPercentSource {
    async fn read(&self) -> Result<Value> {
        let battery = find_power_supply(&["BAT"])
            .await
            .context("no BAT* device found under /sys/class/power_supply")?;
        let raw = read_trimmed(&battery.join("capacity")).await?;
        let percent: f64 = raw.parse().context("could not parse battery capacity")?;
        Ok(Value::Num(percent))
    }
}

pub struct ChargingSource;

#[async_trait]
impl Source for ChargingSource {
    async fn read(&self) -> Result<Value> {
        let battery = find_power_supply(&["BAT"])
            .await
            .context("no BAT* device found under /sys/class/power_supply")?;
        let status = read_trimmed(&battery.join("status")).await?;
        Ok(Value::Bool(status.eq_ignore_ascii_case("Charging")))
    }
}

/// Whether any Mains/USB AC adapter reports itself online.
pub struct AcConnectedSource;

#[async_trait]
impl Source for AcConnectedSource {
    async fn read(&self) -> Result<Value> {
        let adapter = find_power_supply(&["AC", "ADP", "ACAD"]).await;
        let Some(adapter) = adapter else {
            bail!("no AC/ADP* device found under /sys/class/power_supply");
        };
        let raw = read_trimmed(&adapter.join("online")).await?;
        Ok(Value::Bool(raw.trim() == "1"))
    }
}
