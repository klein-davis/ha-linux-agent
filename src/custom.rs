//! Generic, user-configured command-driven entities: run a shell command to
//! read a value, and/or run a shell command to apply a written value.

use crate::entities::{Sink, Source, Value};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use tokio::process::Command;

async fn run_shell(command: &str) -> Result<std::process::Output> {
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .await
        .with_context(|| format!("failed to spawn command: {command}"))
}

async fn run_shell_checked(command: &str) -> Result<String> {
    let output = run_shell(command).await?;
    if !output.status.success() {
        return Err(anyhow!(
            "command `{command}` exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Reads a value by running a shell command and parsing its trimmed stdout.
pub struct CommandSource {
    pub command: String,
    pub kind: ValueKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Text,
    Number,
    /// stdout is treated as truthy unless it is empty, "0", "false", "off" (case-insensitive).
    Bool,
}

#[async_trait]
impl Source for CommandSource {
    async fn read(&self) -> Result<Value> {
        let out = run_shell_checked(&self.command).await?;
        Ok(match self.kind {
            ValueKind::Text => Value::Text(out),
            ValueKind::Number => Value::Num(out.parse::<f64>().with_context(|| {
                format!("command `{}` did not print a number: `{out}`", self.command)
            })?),
            ValueKind::Bool => {
                let truthy = !matches!(out.to_lowercase().as_str(), "" | "0" | "false" | "off");
                Value::Bool(truthy)
            }
        })
    }
}

/// Writes an arbitrary value by substituting `{value}` into a template command.
pub struct TemplateCommandSink {
    pub template: String,
}

#[async_trait]
impl Sink for TemplateCommandSink {
    async fn write(&self, payload: &str) -> Result<()> {
        let command = self.template.replace("{value}", payload);
        let output = run_shell(&command).await?;
        if !output.status.success() {
            return Err(anyhow!(
                "command `{command}` exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }
}

/// Writes a boolean by running one of two fixed commands (on/off, lock/unlock, ...).
pub struct TwoStateCommandSink {
    pub on_command: String,
    pub off_command: String,
    /// Payload strings (uppercased) that mean "on"/true, vs "off"/false.
    pub on_payloads: Vec<String>,
}

#[async_trait]
impl Sink for TwoStateCommandSink {
    async fn write(&self, payload: &str) -> Result<()> {
        let is_on = self
            .on_payloads
            .iter()
            .any(|p| p.eq_ignore_ascii_case(payload));
        let command = if is_on {
            &self.on_command
        } else {
            &self.off_command
        };
        run_shell_checked(command).await.map(|_| ())
    }
}

/// Reads a lock's state by running a command and mapping its truthy/falsy
/// stdout to the "LOCKED"/"UNLOCKED" strings Home Assistant's lock entity expects.
pub struct LockStateCommandSource {
    pub command: String,
}

fn parse_locked(raw: &str) -> bool {
    let normalized = raw.trim().to_lowercase();
    !matches!(
        normalized.as_str(),
        "" | "0" | "false" | "off" | "unlocked" | "unlock"
    )
}

#[async_trait]
impl Source for LockStateCommandSource {
    async fn read(&self) -> Result<Value> {
        let out = run_shell_checked(&self.command).await?;
        let state = if parse_locked(&out) {
            "LOCKED"
        } else {
            "UNLOCKED"
        };
        Ok(Value::Text(state.to_string()))
    }
}

/// Write-only command, e.g. for an HA button entity. Ignores the payload.
pub struct FireAndForgetCommandSink {
    pub command: String,
}

#[async_trait]
impl Sink for FireAndForgetCommandSink {
    async fn write(&self, _payload: &str) -> Result<()> {
        run_shell_checked(&self.command).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_locked_truthy_falsy() {
        assert!(parse_locked("locked"));
        assert!(parse_locked("1"));
        assert!(parse_locked("true"));
        assert!(!parse_locked("unlocked"));
        assert!(!parse_locked("0"));
        assert!(!parse_locked(""));
        assert!(!parse_locked("false"));
    }

    #[tokio::test]
    async fn command_source_number() {
        let src = CommandSource {
            command: "echo 42".to_string(),
            kind: ValueKind::Number,
        };
        assert_eq!(src.read().await.unwrap(), Value::Num(42.0));
    }

    #[tokio::test]
    async fn command_source_bool_truthy() {
        let src = CommandSource {
            command: "echo on".to_string(),
            kind: ValueKind::Bool,
        };
        assert_eq!(src.read().await.unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn command_source_bool_falsy_on_empty() {
        let src = CommandSource {
            command: "true".to_string(),
            kind: ValueKind::Bool,
        };
        assert_eq!(src.read().await.unwrap(), Value::Bool(false));
    }

    #[tokio::test]
    async fn command_source_propagates_failure() {
        let src = CommandSource {
            command: "exit 1".to_string(),
            kind: ValueKind::Text,
        };
        assert!(src.read().await.is_err());
    }

    #[tokio::test]
    async fn template_sink_substitutes_value() {
        let dir = tempfile::tempdir().unwrap();
        let out_file = dir.path().join("out.txt");
        let sink = TemplateCommandSink {
            template: format!("echo {{value}} > {}", out_file.display()),
        };
        sink.write("55").await.unwrap();
        let content = std::fs::read_to_string(&out_file).unwrap();
        assert_eq!(content.trim(), "55");
    }

    #[tokio::test]
    async fn two_state_sink_picks_on_or_off() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker.txt");
        let sink = TwoStateCommandSink {
            on_command: format!("echo on > {}", marker.display()),
            off_command: format!("echo off > {}", marker.display()),
            on_payloads: vec!["ON".to_string()],
        };
        sink.write("ON").await.unwrap();
        assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "on");
        sink.write("OFF").await.unwrap();
        assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "off");
    }
}
