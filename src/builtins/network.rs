//! Reachability checks: ping a target from every network interface whose
//! name matches a regex, and report success/failure as a binary_sensor.
//!
//! Useful for "is my router/phone/NAS actually reachable over wifi vs.
//! ethernet vs. a specific VPN interface" style checks.

use crate::entities::{Source, Value};
use anyhow::{Context, Result};
use async_trait::async_trait;
use regex::Regex;
use tokio::fs;
use tokio::process::Command;

#[derive(Debug, Clone, Copy)]
pub enum SuccessMode {
    /// At least one ping, from any matching interface, got a reply.
    Any,
    /// Every single ping sent, across every matching interface, got a reply.
    All,
    /// At least N total replies were received across all matching interfaces.
    AtLeast(u32),
}

impl std::str::FromStr for SuccessMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "any" => Ok(SuccessMode::Any),
            "all" => Ok(SuccessMode::All),
            other => {
                if let Some(n) = other.strip_prefix("atleast:") {
                    Ok(SuccessMode::AtLeast(
                        n.parse().context("atleast:N must be a number")?,
                    ))
                } else {
                    anyhow::bail!(
                        "invalid success_mode '{s}', expected 'any', 'all', or 'atleast:N'"
                    )
                }
            }
        }
    }
}

pub struct PingCheckSource {
    pub interface_regex: Regex,
    pub target: String,
    pub count: u32,
    pub timeout_secs: u32,
    pub mode: SuccessMode,
}

async fn matching_interfaces(regex: &Regex) -> Result<Vec<String>> {
    let mut entries = fs::read_dir("/sys/class/net")
        .await
        .context("failed to list /sys/class/net")?;
    let mut matches = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        if regex.is_match(&name) {
            matches.push(name);
        }
    }
    Ok(matches)
}

/// Runs `ping -I <iface> -c <count> -W <timeout> <target>` and returns the
/// number of replies received (0 if the interface can't send at all).
async fn ping_from(iface: &str, target: &str, count: u32, timeout_secs: u32) -> u32 {
    let output = Command::new("ping")
        .arg("-I")
        .arg(iface)
        .arg("-c")
        .arg(count.to_string())
        .arg("-W")
        .arg(timeout_secs.to_string())
        .arg(target)
        .output()
        .await;

    let Ok(output) = output else { return 0 };
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_received(&stdout)
}

fn parse_received(ping_output: &str) -> u32 {
    let re = Regex::new(r"(\d+) (?:packets )?received").unwrap();
    ping_output
        .lines()
        .find_map(|line| re.captures(line))
        .and_then(|c| c.get(1).unwrap().as_str().parse().ok())
        .unwrap_or(0)
}

#[async_trait]
impl Source for PingCheckSource {
    async fn read(&self) -> Result<Value> {
        let interfaces = matching_interfaces(&self.interface_regex).await?;
        if interfaces.is_empty() {
            return Ok(Value::Bool(false));
        }

        let mut total_sent = 0u32;
        let mut total_received = 0u32;
        for iface in &interfaces {
            total_sent += self.count;
            total_received += ping_from(iface, &self.target, self.count, self.timeout_secs).await;
        }

        let success = match self.mode {
            SuccessMode::Any => total_received >= 1,
            SuccessMode::All => total_received == total_sent,
            SuccessMode::AtLeast(n) => total_received >= n,
        };
        Ok(Value::Bool(success))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_linux_ping_summary() {
        let output = "PING 1.1.1.1: 3 packets transmitted, 3 received, 0% packet loss\n";
        assert_eq!(parse_received(output), 3);
    }

    #[test]
    fn parses_zero_received() {
        let output = "3 packets transmitted, 0 received, 100% packet loss\n";
        assert_eq!(parse_received(output), 0);
    }

    #[test]
    fn success_mode_parses() {
        assert!(matches!(
            "any".parse::<SuccessMode>().unwrap(),
            SuccessMode::Any
        ));
        assert!(matches!(
            "all".parse::<SuccessMode>().unwrap(),
            SuccessMode::All
        ));
        assert!(matches!(
            "atleast:2".parse::<SuccessMode>().unwrap(),
            SuccessMode::AtLeast(2)
        ));
        assert!("bogus".parse::<SuccessMode>().is_err());
    }
}
