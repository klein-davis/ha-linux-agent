//! Home Assistant MQTT discovery: topic naming and discovery-config payloads.
//! See <https://www.home-assistant.io/integrations/mqtt/#discovery-messages>.

use crate::config::{DeviceConfig, MqttConfig};
use crate::entities::{Component, Entity};
use serde_json::{json, Value as Json};

pub struct Topics {
    pub state: String,
    pub command: Option<String>,
    pub availability: String,
    pub discovery: String,
}

pub fn topics_for(mqtt: &MqttConfig, device_id: &str, entity: &Entity) -> Topics {
    let base = format!("{}/{}/{}", mqtt.base_topic, device_id, entity.id);
    Topics {
        state: format!("{base}/state"),
        command: entity.is_writable().then(|| format!("{base}/set")),
        availability: format!("{}/{}/availability", mqtt.base_topic, device_id),
        discovery: format!(
            "{}/{}/{}/{}/config",
            mqtt.discovery_prefix,
            entity.component.as_str(),
            device_id,
            entity.id
        ),
    }
}

pub fn discovery_payload(
    device: &DeviceConfig,
    device_id: &str,
    _mqtt: &MqttConfig,
    entity: &Entity,
    topics: &Topics,
) -> Json {
    let mut payload = json!({
        "name": entity.name,
        "unique_id": format!("{device_id}_{}", entity.id),
        "availability_topic": topics.availability,
        "payload_available": "online",
        "payload_not_available": "offline",
        "device": device_json(device, device_id),
    });

    if entity.is_readable() {
        payload["state_topic"] = json!(topics.state);
    }
    if let Some(cmd_topic) = &topics.command {
        payload["command_topic"] = json!(cmd_topic);
    }
    if let Some(dc) = &entity.device_class {
        payload["device_class"] = json!(dc);
    }
    if let Some(unit) = &entity.unit {
        payload["unit_of_measurement"] = json!(unit);
    }
    if let Some(sc) = &entity.state_class {
        payload["state_class"] = json!(sc);
    }
    if let Some(icon) = &entity.icon {
        payload["icon"] = json!(icon);
    }

    match entity.component {
        Component::Number => {
            if let Some(min) = entity.min {
                payload["min"] = json!(min);
            }
            if let Some(max) = entity.max {
                payload["max"] = json!(max);
            }
            payload["step"] = json!(entity.step.unwrap_or(1.0));
            payload["mode"] = json!("slider");
        }
        Component::Switch => {
            payload["payload_on"] = json!("ON");
            payload["payload_off"] = json!("OFF");
            if entity.is_readable() {
                payload["state_on"] = json!("ON");
                payload["state_off"] = json!("OFF");
            }
        }
        Component::Lock => {
            payload["payload_lock"] = json!("LOCK");
            payload["payload_unlock"] = json!("UNLOCK");
            payload["state_locked"] = json!("LOCKED");
            payload["state_unlocked"] = json!("UNLOCKED");
        }
        Component::BinarySensor => {
            payload["payload_on"] = json!("ON");
            payload["payload_off"] = json!("OFF");
        }
        Component::Button | Component::Sensor => {}
    }

    payload
}

fn device_json(device: &DeviceConfig, device_id: &str) -> Json {
    let mut d = json!({
        "identifiers": [device_id],
        "name": device.name,
    });
    if let Some(m) = &device.manufacturer {
        d["manufacturer"] = json!(m);
    }
    if let Some(m) = &device.model {
        d["model"] = json!(m);
    }
    if let Some(v) = &device.sw_version {
        d["sw_version"] = json!(v);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::Component;

    fn mqtt() -> MqttConfig {
        MqttConfig {
            host: "127.0.0.1".into(),
            port: 1883,
            username: None,
            password: None,
            password_file: None,
            discovery_prefix: "homeassistant".into(),
            base_topic: "ha-stat".into(),
            client_id: None,
            discovery: true,
        }
    }

    fn entity(id: &str, component: Component, writable: bool, readable: bool) -> Entity {
        Entity {
            id: id.to_string(),
            name: id.to_string(),
            component,
            device_class: None,
            unit: None,
            state_class: None,
            icon: None,
            interval: std::time::Duration::from_secs(5),
            min: None,
            max: None,
            step: None,
            source: readable.then(|| {
                Box::new(crate::custom::CommandSource {
                    command: "echo 1".into(),
                    kind: crate::custom::ValueKind::Number,
                }) as Box<dyn crate::entities::Source>
            }),
            sink: writable.then(|| {
                Box::new(crate::custom::FireAndForgetCommandSink {
                    command: "true".into(),
                }) as Box<dyn crate::entities::Sink>
            }),
        }
    }

    #[test]
    fn topics_include_command_only_when_writable() {
        let mqtt = mqtt();
        let ro = entity("cpu", Component::Sensor, false, true);
        let t = topics_for(&mqtt, "laptop", &ro);
        assert!(t.command.is_none());
        assert_eq!(t.state, "ha-stat/laptop/cpu/state");
        assert_eq!(t.discovery, "homeassistant/sensor/laptop/cpu/config");

        let rw = entity("screen", Component::Switch, true, false);
        let t2 = topics_for(&mqtt, "laptop", &rw);
        assert_eq!(t2.command.unwrap(), "ha-stat/laptop/screen/set");
    }

    #[test]
    fn switch_payload_has_on_off_mapping() {
        let mqtt = mqtt();
        let device = DeviceConfig {
            name: "Laptop".into(),
            ..Default::default()
        };
        let sw = entity("screen", Component::Switch, true, true);
        let topics = topics_for(&mqtt, "laptop", &sw);
        let payload = discovery_payload(&device, "laptop", &mqtt, &sw, &topics);
        assert_eq!(payload["payload_on"], "ON");
        assert_eq!(payload["payload_off"], "OFF");
        assert_eq!(payload["device"]["identifiers"][0], "laptop");
    }

    #[test]
    fn number_includes_min_max_step() {
        let mqtt = mqtt();
        let device = DeviceConfig {
            name: "Laptop".into(),
            ..Default::default()
        };
        let mut n = entity("brightness", Component::Number, true, true);
        n.min = Some(0.0);
        n.max = Some(100.0);
        let topics = topics_for(&mqtt, "laptop", &n);
        let payload = discovery_payload(&device, "laptop", &mqtt, &n, &topics);
        assert_eq!(payload["min"], 0.0);
        assert_eq!(payload["max"], 100.0);
        assert_eq!(payload["step"], 1.0);
    }
}
