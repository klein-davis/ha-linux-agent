//! The long-running daemon: connects to MQTT, publishes HA discovery configs,
//! polls every readable entity on its own interval, and dispatches incoming
//! command-topic messages to the right entity's sink.

use crate::config::RawConfig;
use crate::discovery::{discovery_payload, topics_for};
use crate::entities::Entity;
use anyhow::{Context, Result};
use rumqttc::{AsyncClient, Event, LastWill, MqttOptions, Packet, QoS};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

pub async fn run_daemon(config: RawConfig) -> Result<()> {
    let device_id = config.device_id();
    let entities: Vec<Arc<Entity>> = config
        .build_entities()
        .context("failed to build entities from config")?
        .into_iter()
        .map(Arc::new)
        .collect();

    info!(device_id = %device_id, entity_count = entities.len(), "starting ha-linux-agent daemon");

    let availability_topic = format!("{}/{}/availability", config.mqtt.base_topic, device_id);

    let client_id = config
        .mqtt
        .client_id
        .clone()
        .unwrap_or_else(|| format!("ha-linux-agent-{device_id}"));
    let mut opts = MqttOptions::new(client_id, config.mqtt.host.clone(), config.mqtt.port);
    if let (Some(u), Some(p)) = (&config.mqtt.username, config.mqtt.resolved_password()?) {
        opts.set_credentials(u.clone(), p);
    }
    opts.set_last_will(LastWill::new(
        availability_topic.clone(),
        "offline",
        QoS::AtLeastOnce,
        true,
    ));
    opts.set_keep_alive(Duration::from_secs(30));
    // Clean sessions make the broker drop our subscriptions on every
    // reconnect, so a transient network blip silently leaves us connected
    // but deaf to command topics until the service is restarted. Keep the
    // session alive across reconnects instead -- client_id is stable per
    // host, so the broker can match it back up. The ConnAck handler below
    // still redoes subscriptions/announcements unconditionally regardless,
    // since broker-side state can be lost independent of this flag.
    opts.set_clean_session(false);

    let (client, mut eventloop) = AsyncClient::new(opts, 64);

    client
        .publish(&availability_topic, QoS::AtLeastOnce, true, "online")
        .await
        .context("failed to publish availability")?;

    let mut command_topics: HashMap<String, Arc<Entity>> = HashMap::new();
    for entity in &entities {
        let topics = topics_for(&config.mqtt, &device_id, entity);

        if config.mqtt.discovery {
            let payload =
                discovery_payload(&config.device, &device_id, &config.mqtt, entity, &topics);
            client
                .publish(
                    &topics.discovery,
                    QoS::AtLeastOnce,
                    true,
                    serde_json::to_vec(&payload)?,
                )
                .await
                .with_context(|| format!("failed to publish discovery for '{}'", entity.id))?;
        }

        if let Some(cmd_topic) = &topics.command {
            client
                .subscribe(cmd_topic, QoS::AtLeastOnce)
                .await
                .with_context(|| format!("failed to subscribe to '{cmd_topic}'"))?;
            command_topics.insert(cmd_topic.clone(), Arc::clone(entity));
        }
    }

    // One polling task per readable entity.
    for entity in entities.iter().filter(|e| e.is_readable()) {
        let entity = Arc::clone(entity);
        let client = client.clone();
        let mqtt_cfg = config.mqtt.clone();
        let device_id = device_id.clone();
        tokio::spawn(async move {
            let topics = topics_for(&mqtt_cfg, &device_id, &entity);
            let mut interval = tokio::time::interval(entity.interval);
            loop {
                interval.tick().await;
                match entity.source.as_ref().unwrap().read().await {
                    Ok(value) => {
                        let payload = value.as_mqtt_payload();
                        debug!(id = %entity.id, %payload, "publishing state");
                        if let Err(e) = client
                            .publish(&topics.state, QoS::AtLeastOnce, true, payload)
                            .await
                        {
                            error!(id = %entity.id, error = %e, "failed to publish state");
                        }
                    }
                    Err(e) => warn!(id = %entity.id, error = %e, "failed to read entity"),
                }
            }
        });
    }

    // Main event loop: dispatch incoming command-topic messages.
    loop {
        match eventloop.poll().await {
            // Treat every ConnAck -- first connect or reconnect -- as a cue
            // to (re)announce ourselves: resubscribe to command topics and
            // republish availability/state. Retained messages and session
            // state can both be lost server-side (broker restart, session
            // expiry) independent of what clean_session asked for, so
            // don't bother gating this on session_present -- just always
            // redo it; subscribing/publishing retained topics we're
            // already subscribed/current on is a harmless no-op.
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                info!(host = %config.mqtt.host, port = config.mqtt.port, "mqtt connected");

                for cmd_topic in command_topics.keys() {
                    if let Err(e) = client.subscribe(cmd_topic, QoS::AtLeastOnce).await {
                        error!(topic = %cmd_topic, error = %e, "failed to resubscribe after reconnect");
                    }
                }

                if let Err(e) = client
                    .publish(&availability_topic, QoS::AtLeastOnce, true, "online")
                    .await
                {
                    error!(error = %e, "failed to republish availability after reconnect");
                }

                for entity in entities.iter().filter(|e| e.is_readable()) {
                    let topics = topics_for(&config.mqtt, &device_id, entity);
                    match entity.source.as_ref().unwrap().read().await {
                        Ok(value) => {
                            if let Err(e) = client
                                .publish(&topics.state, QoS::AtLeastOnce, true, value.as_mqtt_payload())
                                .await
                            {
                                error!(id = %entity.id, error = %e, "failed to republish state after reconnect");
                            }
                        }
                        Err(e) => {
                            warn!(id = %entity.id, error = %e, "failed to read entity for post-reconnect republish")
                        }
                    }
                }
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let Some(entity) = command_topics.get(publish.topic.as_str()) else {
                    continue;
                };
                let payload = String::from_utf8_lossy(&publish.payload).to_string();
                let entity = Arc::clone(entity);
                let client = client.clone();
                let mqtt_cfg = config.mqtt.clone();
                let device_id = device_id.clone();
                tokio::spawn(async move {
                    info!(id = %entity.id, %payload, "received command");
                    if let Err(e) = entity.sink.as_ref().unwrap().write(&payload).await {
                        error!(id = %entity.id, error = %e, "command failed");
                        return;
                    }
                    if let Some(source) = &entity.source {
                        let topics = topics_for(&mqtt_cfg, &device_id, &entity);
                        match source.read().await {
                            Ok(value) => {
                                let _ = client
                                    .publish(
                                        &topics.state,
                                        QoS::AtLeastOnce,
                                        true,
                                        value.as_mqtt_payload(),
                                    )
                                    .await;
                            }
                            Err(e) => {
                                warn!(id = %entity.id, error = %e, "post-command read failed")
                            }
                        }
                    }
                });
            }
            Ok(_) => {}
            Err(e) => {
                error!(error = %e, "mqtt event loop error, retrying");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}
