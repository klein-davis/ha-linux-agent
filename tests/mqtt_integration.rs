//! End-to-end test against a real MQTT broker. Ignored by default since it
//! needs a broker running; the devenv shell provides one via mosquitto.
//!
//! Run it with:
//!   devenv up -d            # starts mosquitto on 127.0.0.1:1883
//!   cargo test --test mqtt_integration -- --ignored
//!
//! Override HASTAT_TEST_MQTT_HOST/PORT to point at a different broker.

use hastat::config::RawConfig;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use std::time::Duration;
use tokio::time::timeout;

fn broker() -> (String, u16) {
    let host = std::env::var("HASTAT_TEST_MQTT_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = std::env::var("HASTAT_TEST_MQTT_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(1883);
    (host, port)
}

fn test_config(mqtt_host: &str, mqtt_port: u16, base_topic: &str) -> RawConfig {
    let toml = format!(
        r#"
[device]
name = "Integration Test Laptop"
id = "it-laptop"

[mqtt]
host = "{mqtt_host}"
port = {mqtt_port}
base_topic = "{base_topic}"

[[sensor]]
id = "answer"
command = "echo 42"
interval = "1s"

[[switch]]
id = "flag"
read_command = "cat /tmp/hastat-test-flag 2>/dev/null || echo 0"
on_command = "echo on > /tmp/hastat-test-flag"
off_command = "echo off > /tmp/hastat-test-flag"
"#
    );
    RawConfig::from_toml_str(&toml).expect("test config must parse")
}

async fn connect_test_client(host: &str, port: u16) -> (AsyncClient, rumqttc::EventLoop) {
    let client_id = format!("hastat-test-{}", std::process::id());
    let opts = MqttOptions::new(client_id, host, port);
    AsyncClient::new(opts, 32)
}

#[tokio::test]
#[ignore = "requires a live MQTT broker; see module docs"]
async fn daemon_publishes_discovery_and_state_and_handles_commands() {
    let (host, port) = broker();
    let base_topic = format!("hastat-test-{}", std::process::id());
    let _ = std::fs::remove_file("/tmp/hastat-test-flag");

    let config = test_config(&host, port, &base_topic);
    tokio::spawn(hastat::mqtt::run_daemon(config));

    let (client, mut eventloop) = connect_test_client(&host, port).await;
    client
        .subscribe("homeassistant/#", QoS::AtLeastOnce)
        .await
        .unwrap();
    client
        .subscribe(format!("{base_topic}/#"), QoS::AtLeastOnce)
        .await
        .unwrap();

    // rumqttc only actually flushes network I/O (both incoming and
    // outgoing) while something is polling the eventloop, so drive it
    // continuously in the background for the rest of the test.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            if let Ok(Event::Incoming(Packet::Publish(p))) = eventloop.poll().await {
                let payload = String::from_utf8_lossy(&p.payload).to_string();
                if tx.send((p.topic, payload)).is_err() {
                    return;
                }
            }
        }
    });

    let mut saw_discovery = false;
    let mut saw_sensor_state = false;
    let mut saw_availability_online = false;

    let result = timeout(Duration::from_secs(15), async {
        while let Some((topic, payload)) = rx.recv().await {
            if topic.starts_with("homeassistant/sensor/it-laptop/answer/config") {
                saw_discovery = true;
            }
            if topic == format!("{base_topic}/it-laptop/answer/state") && payload == "42" {
                saw_sensor_state = true;
            }
            if topic == format!("{base_topic}/it-laptop/availability") && payload == "online" {
                saw_availability_online = true;
            }
            if saw_discovery && saw_sensor_state && saw_availability_online {
                break;
            }
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "timed out waiting for daemon to publish expected messages"
    );
    assert!(
        saw_discovery,
        "never saw HA discovery config for the sensor"
    );
    assert!(saw_sensor_state, "never saw sensor state == 42");
    assert!(saw_availability_online, "never saw availability == online");

    // Now exercise the write path: command the switch on, and expect its
    // on_command side effect (writing /tmp/hastat-test-flag) to happen.
    client
        .publish(
            format!("{base_topic}/it-laptop/flag/set"),
            QoS::AtLeastOnce,
            false,
            "ON",
        )
        .await
        .unwrap();

    let flag_result = timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(content) = tokio::fs::read_to_string("/tmp/hastat-test-flag").await {
                if content.trim() == "on" {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    assert!(
        flag_result.is_ok(),
        "switch command never ran its on_command"
    );
    let _ = std::fs::remove_file("/tmp/hastat-test-flag");
}
