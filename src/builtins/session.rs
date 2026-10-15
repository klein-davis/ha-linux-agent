//! The logged-in session's status (active / idle / locked), via systemd-logind.
//!
//! Suspend itself isn't reported from in here -- the machine is asleep, so
//! nothing is running to report it. Instead this entity is meant to be kept
//! wired to `mqtt.availability` (unlike most other sensors, which you'd
//! typically turn availability off for): going "unavailable" in HA *is*
//! the asleep/disconnected signal, while everything else keeps showing its
//! last value instead of gapping.

use crate::custom::run_shell_checked;
use crate::entities::{Source, Value};
use anyhow::Result;
use async_trait::async_trait;

fn parse_session_prop(output: &str, key: &str) -> Option<bool> {
    output.lines().find_map(|line| {
        let value = line.strip_prefix(key)?.strip_prefix('=')?;
        Some(value.trim().eq_ignore_ascii_case("yes"))
    })
}

fn derive_status(idle: bool, locked: bool) -> &'static str {
    if locked {
        "locked"
    } else if idle {
        "idle"
    } else {
        "active"
    }
}

pub struct SessionStatusSource;

#[async_trait]
impl Source for SessionStatusSource {
    async fn read(&self) -> Result<Value> {
        let session_id =
            run_shell_checked("loginctl show-seat seat0 -p ActiveSession --value").await?;
        if session_id.is_empty() {
            return Ok(Value::Text("unknown".to_string()));
        }

        let props = run_shell_checked(&format!(
            "loginctl show-session {session_id} -p IdleHint -p LockedHint"
        ))
        .await?;
        let idle = parse_session_prop(&props, "IdleHint").unwrap_or(false);
        let locked = parse_session_prop(&props, "LockedHint").unwrap_or(false);

        Ok(Value::Text(derive_status(idle, locked).to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_idle_and_locked_hints_regardless_of_order() {
        let out = "LockedHint=no\nIdleHint=yes\n";
        assert_eq!(parse_session_prop(out, "IdleHint"), Some(true));
        assert_eq!(parse_session_prop(out, "LockedHint"), Some(false));
    }

    #[test]
    fn locked_takes_priority_over_idle() {
        assert_eq!(derive_status(true, true), "locked");
        assert_eq!(derive_status(true, false), "idle");
        assert_eq!(derive_status(false, false), "active");
    }
}
