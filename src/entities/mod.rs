use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;

/// A value produced by a [`Source`] or accepted by a [`Sink`].
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Num(f64),
    Text(String),
}

impl Value {
    /// Render as the MQTT payload Home Assistant expects for this value.
    pub fn as_mqtt_payload(&self) -> String {
        match self {
            Value::Bool(b) => {
                if *b {
                    "ON".to_string()
                } else {
                    "OFF".to_string()
                }
            }
            Value::Num(n) => {
                if n.fract() == 0.0 {
                    format!("{}", *n as i64)
                } else {
                    format!("{n}")
                }
            }
            Value::Text(s) => s.clone(),
        }
    }
}

/// The Home Assistant MQTT component an entity is published as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    Sensor,
    BinarySensor,
    Number,
    Switch,
    Lock,
    Button,
}

impl Component {
    pub fn as_str(&self) -> &'static str {
        match self {
            Component::Sensor => "sensor",
            Component::BinarySensor => "binary_sensor",
            Component::Number => "number",
            Component::Switch => "switch",
            Component::Lock => "lock",
            Component::Button => "button",
        }
    }
}

/// Something that can be polled for a current value.
#[async_trait]
pub trait Source: Send + Sync {
    async fn read(&self) -> Result<Value>;
}

/// Something that can be commanded to a new value from an incoming MQTT payload.
#[async_trait]
pub trait Sink: Send + Sync {
    async fn write(&self, payload: &str) -> Result<()>;
}

/// A fully resolved, runnable entity: metadata plus optional read/write behavior.
pub struct Entity {
    pub id: String,
    pub name: String,
    pub component: Component,
    pub device_class: Option<String>,
    pub unit: Option<String>,
    pub state_class: Option<String>,
    pub icon: Option<String>,
    pub interval: Duration,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub source: Option<Box<dyn Source>>,
    pub sink: Option<Box<dyn Sink>>,
}

impl Entity {
    pub fn is_readable(&self) -> bool {
        self.source.is_some()
    }

    pub fn is_writable(&self) -> bool {
        self.sink.is_some()
    }
}
