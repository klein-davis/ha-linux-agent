//! TOML configuration: parsing, defaults, and turning the raw declarative
//! config into runnable [`Entity`] values.

use crate::builtins::{battery, brightness, network, system};
use crate::custom::{
    self, CommandSource, FireAndForgetCommandSink, TemplateCommandSink, TwoStateCommandSink,
    ValueKind,
};
use crate::entities::{Component, Entity};
use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

fn default_true() -> bool {
    true
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_port() -> u16 {
    1883
}
fn default_discovery_prefix() -> String {
    "homeassistant".to_string()
}
fn default_base_topic() -> String {
    "ha-stat".to_string()
}
fn default_interval() -> String {
    "30s".to_string()
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct DeviceConfig {
    pub name: String,
    pub id: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub sw_version: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MqttConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub username: Option<String>,
    /// Plaintext password. Prefer `password_file` when the config itself is
    /// managed somewhere world-readable (e.g. the Nix store) -- see
    /// `resolved_password`.
    pub password: Option<String>,
    /// Path to a file whose (trimmed) contents are the MQTT password, read
    /// at startup. Use this instead of `password` when `settings.mqtt` is
    /// rendered into the Nix store (as the ha-stat home-manager module
    /// does), so the secret itself can live outside the store (sops-nix,
    /// agenix, or just a manually-deployed file with tight permissions).
    pub password_file: Option<String>,
    #[serde(default = "default_discovery_prefix")]
    pub discovery_prefix: String,
    #[serde(default = "default_base_topic")]
    pub base_topic: String,
    pub client_id: Option<String>,
    /// Whether to publish retained HA MQTT discovery configs on startup.
    #[serde(default = "default_true")]
    pub discovery: bool,
}

impl MqttConfig {
    /// Resolves the effective password: the plaintext `password` if set,
    /// otherwise the trimmed contents of `password_file`, otherwise `None`.
    pub fn resolved_password(&self) -> Result<Option<String>> {
        if let Some(p) = &self.password {
            return Ok(Some(p.clone()));
        }
        if let Some(path) = &self.password_file {
            let contents = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read mqtt.password_file '{path}'"))?;
            return Ok(Some(contents.trim().to_string()));
        }
        Ok(None)
    }
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            username: None,
            password: None,
            password_file: None,
            discovery_prefix: default_discovery_prefix(),
            base_topic: default_base_topic(),
            client_id: None,
            discovery: true,
        }
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
struct CommonFields {
    id: String,
    name: Option<String>,
    #[serde(default = "default_interval")]
    interval: String,
    device_class: Option<String>,
    unit: Option<String>,
    state_class: Option<String>,
    icon: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RawSensor {
    #[serde(flatten)]
    common: CommonFields,
    builtin: Option<String>,
    command: Option<String>,
    /// "number" (default) or "text" — how to interpret a custom command's stdout.
    value_type: Option<String>,
    /// For builtin = "disk_usage".
    path: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RawBinarySensor {
    #[serde(flatten)]
    common: CommonFields,
    builtin: Option<String>,
    command: Option<String>,
    // ping params
    interface_regex: Option<String>,
    target: Option<String>,
    count: Option<u32>,
    timeout_secs: Option<u32>,
    success_mode: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RawNumber {
    #[serde(flatten)]
    common: CommonFields,
    builtin: Option<String>,
    read_command: Option<String>,
    set_command: Option<String>,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    /// For builtin = "brightness": backlight device name override.
    device: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RawSwitch {
    #[serde(flatten)]
    common: CommonFields,
    read_command: Option<String>,
    on_command: String,
    off_command: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RawLock {
    #[serde(flatten)]
    common: CommonFields,
    state_command: Option<String>,
    lock_command: String,
    unlock_command: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RawButton {
    #[serde(flatten)]
    common: CommonFields,
    command: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct RawConfig {
    pub device: DeviceConfig,
    #[serde(default)]
    pub mqtt: MqttConfig,
    #[serde(default)]
    pub sensor: Vec<RawSensor>,
    #[serde(default)]
    pub binary_sensor: Vec<RawBinarySensor>,
    #[serde(default)]
    pub number: Vec<RawNumber>,
    #[serde(default)]
    pub switch: Vec<RawSwitch>,
    #[serde(default)]
    pub lock: Vec<RawLock>,
    #[serde(default)]
    pub button: Vec<RawButton>,
}

fn humanize_id(id: &str) -> String {
    id.replace(['_', '-'], " ")
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_interval(s: &str) -> Result<Duration> {
    humantime::parse_duration(s).with_context(|| format!("invalid interval '{s}'"))
}

impl RawConfig {
    pub fn from_toml_str(raw: &str) -> Result<Self> {
        toml::from_str(raw).context("failed to parse TOML config")
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        Self::from_toml_str(&raw)
    }

    /// Build the runnable entity list from the declarative config.
    pub fn build_entities(&self) -> Result<Vec<Entity>> {
        let mut entities = Vec::new();
        for s in &self.sensor {
            entities.push(build_sensor(s)?);
        }
        for b in &self.binary_sensor {
            entities.push(build_binary_sensor(b)?);
        }
        for n in &self.number {
            entities.push(build_number(n)?);
        }
        for sw in &self.switch {
            entities.push(build_switch(sw)?);
        }
        for l in &self.lock {
            entities.push(build_lock(l)?);
        }
        for btn in &self.button {
            entities.push(build_button(btn)?);
        }
        if entities.is_empty() {
            bail!("config declares no entities (no [[sensor]], [[switch]], ... sections)");
        }
        Ok(entities)
    }

    pub fn device_id(&self) -> String {
        self.device
            .id
            .clone()
            .unwrap_or_else(|| slug::slugify(&self.device.name))
    }
}

fn base(common: &CommonFields, component: Component) -> Result<Entity> {
    Ok(Entity {
        id: common.id.clone(),
        name: common
            .name
            .clone()
            .unwrap_or_else(|| humanize_id(&common.id)),
        component,
        device_class: common.device_class.clone(),
        unit: common.unit.clone(),
        state_class: common.state_class.clone(),
        icon: common.icon.clone(),
        interval: parse_interval(&common.interval)?,
        min: None,
        max: None,
        step: None,
        source: None,
        sink: None,
    })
}

fn build_sensor(raw: &RawSensor) -> Result<Entity> {
    let mut entity = base(&raw.common, Component::Sensor)?;
    entity.source = Some(match (&raw.builtin, &raw.command) {
        (Some(name), None) => sensor_builtin_source(name, raw)?,
        (None, Some(cmd)) => {
            let kind = match raw.value_type.as_deref() {
                Some("text") => ValueKind::Text,
                Some("number") | None => ValueKind::Number,
                Some(other) => bail!("sensor '{}': unknown value_type '{other}'", raw.common.id),
            };
            Box::new(CommandSource {
                command: cmd.clone(),
                kind,
            })
        }
        (Some(_), Some(_)) => bail!(
            "sensor '{}': set only one of builtin/command",
            raw.common.id
        ),
        (None, None) => bail!("sensor '{}': must set builtin or command", raw.common.id),
    });
    Ok(entity)
}

fn sensor_builtin_source(name: &str, raw: &RawSensor) -> Result<Box<dyn crate::entities::Source>> {
    Ok(match name {
        "cpu_usage" => Box::new(system::CpuUsageSource::new()),
        "memory_usage" => Box::new(system::MemoryUsageSource),
        "swap_usage" => Box::new(system::SwapUsageSource),
        "uptime" => Box::new(system::UptimeSource),
        "load1" => Box::new(system::LoadAverageSource { column: 0 }),
        "load5" => Box::new(system::LoadAverageSource { column: 1 }),
        "load15" => Box::new(system::LoadAverageSource { column: 2 }),
        "process_count" => Box::new(system::ProcessCountSource),
        "battery_percent" => Box::new(battery::BatteryPercentSource),
        "disk_usage" => {
            let path = raw.path.clone().with_context(|| {
                format!(
                    "sensor '{}': builtin disk_usage requires 'path'",
                    raw.common.id
                )
            })?;
            Box::new(system::DiskUsageSource { path })
        }
        "brightness" => Box::new(brightness::BrightnessController { device: None }),
        other => bail!("sensor '{}': unknown builtin '{other}'", raw.common.id),
    })
}

fn build_binary_sensor(raw: &RawBinarySensor) -> Result<Entity> {
    let mut entity = base(&raw.common, Component::BinarySensor)?;
    entity.source = Some(match (&raw.builtin, &raw.command) {
        (Some(name), None) => binary_sensor_builtin_source(name, raw)?,
        (None, Some(cmd)) => Box::new(CommandSource {
            command: cmd.clone(),
            kind: ValueKind::Bool,
        }),
        (Some(_), Some(_)) => bail!(
            "binary_sensor '{}': set only one of builtin/command",
            raw.common.id
        ),
        (None, None) => bail!(
            "binary_sensor '{}': must set builtin or command",
            raw.common.id
        ),
    });
    Ok(entity)
}

fn binary_sensor_builtin_source(
    name: &str,
    raw: &RawBinarySensor,
) -> Result<Box<dyn crate::entities::Source>> {
    Ok(match name {
        "charging" => Box::new(battery::ChargingSource),
        "ac_connected" => Box::new(battery::AcConnectedSource),
        "ping" => {
            let id = &raw.common.id;
            let pattern = raw.interface_regex.clone().with_context(|| {
                format!("binary_sensor '{id}': builtin ping requires 'interface_regex'")
            })?;
            let regex = Regex::new(&pattern).with_context(|| {
                format!("binary_sensor '{id}': invalid interface_regex '{pattern}'")
            })?;
            let target = raw
                .target
                .clone()
                .with_context(|| format!("binary_sensor '{id}': builtin ping requires 'target'"))?;
            let mode = raw
                .success_mode
                .as_deref()
                .unwrap_or("any")
                .parse()
                .with_context(|| format!("binary_sensor '{id}': invalid success_mode"))?;
            Box::new(network::PingCheckSource {
                interface_regex: regex,
                target,
                count: raw.count.unwrap_or(1),
                timeout_secs: raw.timeout_secs.unwrap_or(2),
                mode,
            })
        }
        other => bail!(
            "binary_sensor '{}': unknown builtin '{other}'",
            raw.common.id
        ),
    })
}

fn build_number(raw: &RawNumber) -> Result<Entity> {
    let mut entity = base(&raw.common, Component::Number)?;
    entity.min = raw.min;
    entity.max = raw.max;
    entity.step = raw.step;

    match raw.builtin.as_deref() {
        Some("brightness") => {
            let ctrl = brightness::BrightnessController {
                device: raw.device.clone(),
            };
            let ctrl_sink = brightness::BrightnessController {
                device: raw.device.clone(),
            };
            entity.source = Some(Box::new(ctrl));
            entity.sink = Some(Box::new(ctrl_sink));
            entity.min.get_or_insert(0.0);
            entity.max.get_or_insert(100.0);
        }
        Some(other) => bail!("number '{}': unknown builtin '{other}'", raw.common.id),
        None => {
            if let Some(cmd) = &raw.read_command {
                entity.source = Some(Box::new(CommandSource {
                    command: cmd.clone(),
                    kind: ValueKind::Number,
                }));
            }
            if let Some(cmd) = &raw.set_command {
                entity.sink = Some(Box::new(TemplateCommandSink {
                    template: cmd.clone(),
                }));
            }
            if entity.source.is_none() && entity.sink.is_none() {
                bail!(
                    "number '{}': must set builtin, or at least one of read_command/set_command",
                    raw.common.id
                );
            }
        }
    }
    Ok(entity)
}

fn build_switch(raw: &RawSwitch) -> Result<Entity> {
    let mut entity = base(&raw.common, Component::Switch)?;
    if let Some(cmd) = &raw.read_command {
        entity.source = Some(Box::new(CommandSource {
            command: cmd.clone(),
            kind: ValueKind::Bool,
        }));
    }
    entity.sink = Some(Box::new(TwoStateCommandSink {
        on_command: raw.on_command.clone(),
        off_command: raw.off_command.clone(),
        on_payloads: vec!["ON".to_string(), "TRUE".to_string(), "1".to_string()],
    }));
    Ok(entity)
}

fn build_lock(raw: &RawLock) -> Result<Entity> {
    let mut entity = base(&raw.common, Component::Lock)?;
    if let Some(cmd) = &raw.state_command {
        entity.source = Some(Box::new(custom::LockStateCommandSource {
            command: cmd.clone(),
        }));
    }
    entity.sink = Some(Box::new(TwoStateCommandSink {
        on_command: raw.lock_command.clone(),
        off_command: raw.unlock_command.clone(),
        on_payloads: vec!["LOCK".to_string()],
    }));
    Ok(entity)
}

fn build_button(raw: &RawButton) -> Result<Entity> {
    let mut entity = base(&raw.common, Component::Button)?;
    entity.sink = Some(Box::new(FireAndForgetCommandSink {
        command: raw.command.clone(),
    }));
    Ok(entity)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[device]
name = "My Laptop"

[mqtt]
host = "127.0.0.1"

[[sensor]]
id = "cpu"
builtin = "cpu_usage"

[[sensor]]
id = "custom_temp"
command = "cat /tmp/temp"
interval = "10s"

[[binary_sensor]]
id = "router"
builtin = "ping"
interface_regex = "^wl.*"
target = "1.1.1.1"
success_mode = "any"

[[number]]
id = "brightness"
builtin = "brightness"

[[switch]]
id = "screen"
on_command = "echo on"
off_command = "echo off"

[[lock]]
id = "session"
lock_command = "echo lock"
unlock_command = "echo unlock"

[[button]]
id = "reboot"
command = "echo reboot"
"#;

    #[test]
    fn parses_full_sample_and_builds_entities() {
        let cfg = RawConfig::from_toml_str(SAMPLE).unwrap();
        assert_eq!(cfg.device.name, "My Laptop");
        assert_eq!(cfg.device_id(), "my-laptop");
        let entities = cfg.build_entities().unwrap();
        assert_eq!(entities.len(), 7);
        let screen = entities.iter().find(|e| e.id == "screen").unwrap();
        assert!(screen.is_writable());
        assert!(!screen.is_readable());
    }

    #[test]
    fn humanizes_ids() {
        assert_eq!(humanize_id("cpu_usage"), "Cpu Usage");
        assert_eq!(humanize_id("router-reachable"), "Router Reachable");
    }

    #[test]
    fn rejects_sensor_with_no_source() {
        let toml = r#"
[device]
name = "x"
[[sensor]]
id = "bad"
"#;
        let cfg = RawConfig::from_toml_str(toml).unwrap();
        assert!(cfg.build_entities().is_err());
    }

    #[test]
    fn rejects_empty_config() {
        let toml = r#"
[device]
name = "x"
"#;
        let cfg = RawConfig::from_toml_str(toml).unwrap();
        assert!(cfg.build_entities().is_err());
    }

    #[test]
    fn resolved_password_prefers_plaintext() {
        let mqtt = MqttConfig {
            password: Some("plain".into()),
            password_file: Some("/nonexistent/should-not-be-read".into()),
            ..MqttConfig::default()
        };
        assert_eq!(mqtt.resolved_password().unwrap(), Some("plain".into()));
    }

    #[test]
    fn resolved_password_reads_file_when_no_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mqtt-password");
        std::fs::write(&path, "from-file\n").unwrap();
        let mqtt = MqttConfig {
            password_file: Some(path.to_string_lossy().to_string()),
            ..MqttConfig::default()
        };
        assert_eq!(mqtt.resolved_password().unwrap(), Some("from-file".into()));
    }

    #[test]
    fn resolved_password_none_when_neither_set() {
        let mqtt = MqttConfig::default();
        assert_eq!(mqtt.resolved_password().unwrap(), None);
    }
}
