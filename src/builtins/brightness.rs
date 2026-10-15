//! Screen backlight brightness, exposed to Home Assistant as a `number`
//! (0-100%) so it shows up as a dimmer-style slider.

use crate::entities::{Sink, Source, Value};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;

async fn resolve_device(configured: &Option<String>) -> Result<PathBuf> {
    let base = PathBuf::from("/sys/class/backlight");
    if let Some(name) = configured {
        let path = base.join(name);
        if path.is_dir() {
            return Ok(path);
        }
        bail!(
            "backlight device '{name}' not found under {}",
            base.display()
        );
    }
    let mut entries = fs::read_dir(&base)
        .await
        .with_context(|| format!("no backlight devices under {}", base.display()))?;
    if let Some(entry) = entries.next_entry().await? {
        return Ok(entry.path());
    }
    bail!("no backlight devices found under {}", base.display())
}

async fn read_u64(path: &PathBuf) -> Result<u64> {
    let raw = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    raw.trim()
        .parse()
        .with_context(|| format!("not a number in {}", path.display()))
}

/// Shared by the brightness sensor (read) and number (read/write) entities.
/// `device` is the optional backlight name under /sys/class/backlight
/// (e.g. "intel_backlight"); if unset, the first device found is used.
pub struct BrightnessController {
    pub device: Option<String>,
}

#[async_trait]
impl Source for BrightnessController {
    async fn read(&self) -> Result<Value> {
        let dir = resolve_device(&self.device).await?;
        let current = read_u64(&dir.join("brightness")).await?;
        let max = read_u64(&dir.join("max_brightness")).await?;
        if max == 0 {
            return Ok(Value::Num(0.0));
        }
        let percent = 100.0 * current as f64 / max as f64;
        Ok(Value::Num((percent * 10.0).round() / 10.0))
    }
}

#[async_trait]
impl Sink for BrightnessController {
    async fn write(&self, payload: &str) -> Result<()> {
        let percent: f64 = payload
            .trim()
            .parse()
            .with_context(|| format!("brightness payload '{payload}' is not a number"))?;
        let percent = percent.clamp(0.0, 100.0);
        let dir = resolve_device(&self.device).await?;
        let max = read_u64(&dir.join("max_brightness")).await?;
        let raw = ((percent / 100.0) * max as f64).round() as u64;
        fs::write(dir.join("brightness"), raw.to_string())
            .await
            .with_context(|| format!("failed to write brightness to {}", dir.display()))?;
        Ok(())
    }
}
