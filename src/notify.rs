//! HA's MQTT `notify` entity: receives a title/message from Home Assistant
//! (via `notify.<name>` service calls) and hands it to the system's
//! notification daemon (dunst, mako, etc. via `notify-send`/libnotify), or
//! to a fully custom command if you want something else.
//!
//! The Jinja `command_template` we set in our own discovery payload (see
//! `discovery.rs`) asks Home Assistant to send `{"title": ..., "message":
//! ...}` as the MQTT payload, so `parse_payload` below always has both
//! fields available (title may be empty).

use crate::entities::Sink;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;

#[derive(Debug, Deserialize)]
struct NotifyPayload {
    #[serde(default)]
    title: String,
    #[serde(default)]
    message: String,
}

/// Parses an incoming notify command_topic payload into (title, message).
/// Accepts `{"title": "...", "message": "..."}` (what our command_template
/// produces); falls back to treating the whole payload as the message if
/// it isn't that exact JSON shape, so a plain `mosquitto_pub -m "hello"`
/// still works for manual testing.
pub fn parse_payload(payload: &str) -> (String, String) {
    match serde_json::from_str::<NotifyPayload>(payload) {
        Ok(p) => (p.title, p.message),
        Err(_) => (String::new(), payload.trim().to_string()),
    }
}

/// Delivers via `notify-send` (argv only, never a shell) by default, or via
/// a fully custom `{title}`/`{message}` command template if you need to
/// route somewhere `notify-send` can't reach.
pub struct NotifySink {
    pub app_name: Option<String>,
    pub urgency: Option<String>,
    pub expire_ms: Option<u32>,
    pub command: Option<String>,
}

#[async_trait]
impl Sink for NotifySink {
    async fn write(&self, payload: &str) -> Result<()> {
        let (title, message) = parse_payload(payload);

        if let Some(template) = &self.command {
            let command = template
                .replace("{title}", &title)
                .replace("{message}", &message);
            let output = Command::new("sh")
                .arg("-c")
                .arg(&command)
                .output()
                .await
                .with_context(|| format!("failed to spawn command: {command}"))?;
            if !output.status.success() {
                anyhow::bail!(
                    "notify command `{command}` exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            return Ok(());
        }

        let mut cmd = Command::new("notify-send");
        if let Some(app_name) = &self.app_name {
            cmd.arg("-a").arg(app_name);
        }
        if let Some(urgency) = &self.urgency {
            cmd.arg("-u").arg(urgency);
        }
        if let Some(expire_ms) = self.expire_ms {
            cmd.arg("-t").arg(expire_ms.to_string());
        }
        cmd.arg(&title).arg(&message);

        let output = cmd.output().await.context("failed to spawn notify-send")?;
        if !output.status.success() {
            anyhow::bail!(
                "notify-send exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_title_and_message_json() {
        let (title, message) = parse_payload(r#"{"title": "Alert", "message": "Door open"}"#);
        assert_eq!(title, "Alert");
        assert_eq!(message, "Door open");
    }

    #[test]
    fn parses_message_only_json() {
        let (title, message) = parse_payload(r#"{"message": "Just this"}"#);
        assert_eq!(title, "");
        assert_eq!(message, "Just this");
    }

    #[test]
    fn falls_back_to_plain_text() {
        let (title, message) = parse_payload("plain text notification");
        assert_eq!(title, "");
        assert_eq!(message, "plain text notification");
    }
}
