# ha-linux-agent

Expose your laptop (or any Linux box) to Home Assistant over MQTT: CPU,
memory, battery, brightness, reachability checks, and anything else you can
express as a shell command — all grouped under one HA device, with full
MQTT auto-discovery.

- **Builtins** for common system stats (cpu/memory/swap usage, uptime, load
  averages, process count, disk usage, battery percent/charging/AC status,
  backlight brightness) need zero shell commands.
- **Custom command entities** let you wire up literally anything: run a
  command to read a value, run (up to two) commands to write one. Perfect
  for screen on/off via `hyprctl`, volume via `wpctl`, or locking your
  session via noctalia/hyprlock/swaylock.
- **One MQTT device** — every entity shows up grouped under a single device
  in Home Assistant, named whatever you like.
- **Every HA entity shape**: read-only sensors/binary_sensors, read-write
  numbers (dimmer-style sliders) and switches, locks, and write-only
  buttons/notify entities.
- **HA → your notification daemon**: call `notify.<id>` from an HA
  automation with a title/message and it lands as a desktop notification
  (dunst/mako/etc. via `notify-send`) on your machine.

## Quick start

```console
$ nix run github:yourname/ha-linux-agent -- check --config ./examples/laptop.toml
$ nix run github:yourname/ha-linux-agent -- run --config ./examples/laptop.toml
```

Or try a single value with no config file at all:

```console
$ nix run github:yourname/ha-linux-agent -- oneshot --builtin cpu_usage
3.4
$ nix run github:yourname/ha-linux-agent -- oneshot --command "cat /sys/class/thermal/thermal_zone0/temp"
```

## Installing

ha-linux-agent needs to run as *you*, inside your logged-in session (it talks to
your Wayland compositor, your D-Bus session, and relies on logind's
session-scoped permissions for things like backlight brightness). That
makes it a home-manager concern, not a NixOS system service.

### Recommended: the home-manager module

```nix
{
  inputs.ha-linux-agent.url = "github:yourname/ha-linux-agent";

  # in your home-manager config:
  imports = [ inputs.ha-linux-agent.homeManagerModules.default ];

  services.ha-linux-agent = {
    enable = true;
    settings = {
      device.name = "My Laptop";
      mqtt = {
        host = "homeassistant.example.lan";
        username = "ha-linux-agent";
        # NEVER put `password` here -- `settings` is rendered into the
        # world-readable Nix store. Point at a secret deployed outside the
        # store instead (sops-nix, agenix, a manually-placed file, ...):
        password_file = "/run/secrets/ha-linux-agent-mqtt-password";
      };
      number = [
        {
          id = "brightness";
          read_command = "brightnessctl -m | cut -d, -f4 | tr -d '%'";
          set_command = "brightnessctl set {value}%";
          min = 0;
          max = 100;
        }
      ];
      sensor = [
        { id = "battery_percent"; builtin = "battery_percent"; unit = "%"; device_class = "battery"; }
        { id = "cpu_usage"; builtin = "cpu_usage"; unit = "%"; interval = "5s"; }
      ];
      binary_sensor = [
        { id = "charging"; builtin = "charging"; device_class = "battery_charging"; }
      ];
    };
  };
}
```

This wires up `systemd.user.services.ha-linux-agent` for you, pointed at a config
file generated from `settings` via `pkgs.formats.toml`. Apply the usual
way (`home-manager switch`), then:

```console
$ systemctl --user status ha-linux-agent
$ journalctl --user -u ha-linux-agent -f
```

**Reloading after a config change**: there's no live-reload — edit
`settings`, run `home-manager switch` (which regenerates the TOML and
restarts the unit because the `ExecStart` path, which embeds the config's
store hash, changed), and that's it. No manual `systemctl restart` needed.

### Alternative: plain package + your own service

If you don't want the module (e.g. not using home-manager, or you'd
rather hand-manage the TOML file):

```nix
{
  inputs.ha-linux-agent.url = "github:yourname/ha-linux-agent";
  home.packages = [ inputs.ha-linux-agent.packages.${pkgs.system}.default ];
  xdg.configFile."ha-linux-agent/config.toml".source = ./ha-linux-agent-config.toml;

  systemd.user.services.ha-linux-agent = {
    Unit.Description = "Home Assistant MQTT bridge";
    Install.WantedBy = [ "default.target" ];
    Service = {
      ExecStart = "${inputs.ha-linux-agent.packages.${pkgs.system}.default}/bin/ha-linux-agent run --config %h/.config/ha-linux-agent/config.toml";
      Restart = "on-failure";
    };
  };
}
```

Here, since the config is a plain file (not generated from `settings`),
editing it does need an explicit `systemctl --user restart ha-linux-agent` to
take effect.

### As a dev shell (no install)

```console
$ direnv allow        # or: nix develop
$ cargo run -- run --config ./dev.toml
```

`direnv`/`devenv` give you `cargo`, `rustc`, `clippy`, `rustfmt`, and a
local `mosquitto` broker for testing (see [Testing](#testing)).

## Configuration

A config is one TOML file: a `[device]`, an `[mqtt]` section, and any
number of `[[sensor]]`, `[[binary_sensor]]`, `[[number]]`, `[[switch]]`,
`[[lock]]`, and `[[button]]` entity blocks.

```toml
[device]
name = "My Laptop"          # shows up in HA; also used to derive the slug id
# id = "my-laptop"          # optional: override the device/topic slug

[mqtt]
host = "127.0.0.1"
port = 1883
# username = "..."
# password = "..."           # plaintext; fine in a plain file you keep out of git
# password_file = "/run/secrets/ha-linux-agent-mqtt-password"  # preferred if this
                                                           # TOML ends up somewhere
                                                           # world-readable (e.g. the
                                                           # Nix store, via the
                                                           # home-manager module's
                                                           # `settings`)
base_topic = "ha-linux-agent"               # default
discovery_prefix = "homeassistant"  # default
```

Every entity shares these fields:

| field         | meaning                                              |
|---------------|-------------------------------------------------------|
| `id`          | required; used in topics and the HA unique_id         |
| `name`        | optional; defaults to a humanized `id`                |
| `interval`    | poll period for readable entities, e.g. `"5s"`, `"2m"` (default `30s`) |
| `device_class`, `unit`, `state_class`, `icon` | passed straight through to HA discovery |

### `[[sensor]]` — read-only

```toml
[[sensor]]
id = "cpu"
builtin = "cpu_usage"       # percent, 0-100
unit = "%"

[[sensor]]
id = "room_temp"
command = "cat /sys/class/thermal/thermal_zone0/temp"
value_type = "number"       # number (default) | text
unit = "°C"
interval = "10s"
```

Parameterless builtins: `cpu_usage`, `memory_usage`, `swap_usage`,
`uptime`, `load1`, `load5`, `load15`, `process_count`, `battery_percent`,
`brightness` (read-only here).

Parameterized builtin: `disk_usage` (needs `path = "/"`).

### `[[binary_sensor]]` — read-only boolean

```toml
[[binary_sensor]]
id = "charging"
builtin = "charging"

[[binary_sensor]]
id = "on_ac"
builtin = "ac_connected"

# Ping a target from every interface matching a regex, and report success.
[[binary_sensor]]
id = "router_reachable"
builtin = "ping"
interface_regex = "^wl.*"   # match wifi interfaces; try "^(wl|en).*" for wifi+ethernet
target = "192.168.1.1"
count = 3                   # pings per matching interface (default 1)
timeout_secs = 2             # default 2
success_mode = "any"        # "any" | "all" | "atleast:N" (default "any")
device_class = "connectivity"
```

Or a custom command (truthy stdout → ON, falsy/empty/"0"/"false"/"off" → OFF):

```toml
[[binary_sensor]]
id = "vpn_up"
command = "ip link show tun0 up 2>/dev/null"
device_class = "connectivity"
```

### `[[number]]` — read-write numeric (renders as a slider/dimmer in HA)

```toml
[[number]]
id = "brightness"
builtin = "brightness"      # 0-100%; auto-detects /sys/class/backlight/*
# device = "intel_backlight" # optional: pick a specific backlight device
min = 0
max = 100
step = 5
```

Or fully custom (e.g. volume via PipeWire):

```toml
[[number]]
id = "volume"
read_command = "wpctl get-volume @DEFAULT_AUDIO_SINK@ | grep -oP '[\\d.]+' | awk '{print $1*100}'"
set_command = "wpctl set-volume @DEFAULT_AUDIO_SINK@ {value}%"
min = 0
max = 100
step = 1
```

`{value}` in `set_command` is substituted with HA's requested number.

### `[[switch]]` — read-write boolean

```toml
# Screen on/off via Hyprland DPMS.
[[switch]]
id = "screen"
read_command = "hyprctl monitors -j | jq -e '.[0].dpmsStatus' >/dev/null && echo on || echo off"
on_command = "hyprctl dispatch dpms on"
off_command = "hyprctl dispatch dpms off"
```

Omit `read_command` for a write-only switch (HA will just show the last
commanded state as optimistic feedback).

### `[[lock]]` — read-write lock (HA lock entity: LOCKED/UNLOCKED)

This is the generic mechanism behind "control noctalia (or any other
screen locker) from Home Assistant" — fill in whatever actually locks and
unlocks your session:

```toml
[[lock]]
id = "session"
# Replace these with whatever your locker actually exposes. If it's a
# plain CLI (noctalia-shell, swaylock, hyprlock, loginctl), this is just
# the command that does the thing; if it only exposes an IPC socket, wrap
# `socat`/`qs ipc call ...`/whatever in the command.
lock_command = "loginctl lock-session"
unlock_command = "loginctl unlock-session"
state_command = "loginctl show-session self -p LockedHint --value"
```

`state_command`'s stdout is truthy/falsy just like a binary_sensor
(`"yes"`/`"1"`/anything non-empty-and-non-"no" → LOCKED).

### `[[button]]` — write-only (HA button entity; fires a command, no state)

```toml
[[button]]
id = "reboot"
command = "systemctl reboot"
device_class = "restart"
```

### `[[notify]]` — write-only (HA `notify` entity: title + rich-ish text)

Shows up in HA as `notify.<id>`, callable from automations/scripts/the
Developer Tools "Call Service" UI with a title and message:

```yaml
action: notify.send_message
target:
  entity_id: notify.alert
data:
  title: "Build failed"
  message: "ci/deploy.yml failed on main"
```

```toml
[[notify]]
id = "alert"
app_name = "Home Assistant"   # optional; notify-send -a
urgency = "critical"          # optional; low | normal | critical
expire_ms = 5000               # optional; notify-send -t
```

By default this calls `notify-send <title> <message>` directly via argv
(no shell involved, so whatever HA sends can't break out into a shell
command) — works with dunst, mako, or anything else providing a
freedesktop notification daemon. We set our own `command_template` in
the HA discovery payload so HA always sends structured
`{"title": ..., "message": ...}` JSON, regardless of how the automation
call filled those fields in.

If you need something other than `notify-send` (a custom script, piping
into a different daemon, ...), set `command` instead — `{title}` and
`{message}` get substituted into it and the whole thing runs through a
shell, same trust model as `set_command`/`on_command` elsewhere in this
config (only point it at something you trust the content of):

```toml
[[notify]]
id = "alert"
command = "my-notify-script '{title}' '{message}'"
```

## CLI

- `ha-linux-agent run --config <path>` — the daemon: connects to MQTT, publishes
  discovery, polls every readable entity, and handles incoming commands.
- `ha-linux-agent check --config <path>` — parses the config and lists every
  entity it would create, without touching MQTT. Good for CI and for
  `devenv`'s dev loop.
- `ha-linux-agent oneshot [--builtin NAME | --command CMD] [--publish-topic TOPIC --host .. --port ..]`
  — a single ad-hoc read, for scratch use with no config file. Prints the
  value, or publishes it once if `--publish-topic` is given.

## Testing

`cargo test` runs all unit tests (pure logic: TOML parsing, discovery
payload shape, command-source parsing, etc. — no network, no root needed).

The one end-to-end test, `tests/mqtt_integration.rs`, spins up the real
daemon against a real broker and watches real MQTT traffic. It's
`#[ignore]`d by default since it needs a broker:

```console
$ devenv up -d                                   # starts mosquitto on 127.0.0.1:1883
$ cargo test --test mqtt_integration -- --ignored
```

`nix flake check` runs `cargo test` (unit tests), `cargo clippy -- -D
warnings`, and `cargo fmt --check` as separate checks.

## Ideas for more builtins (not yet implemented)

Contributions welcome — these all fit the existing `Source`/`Sink` model:

- Wifi SSID / signal strength (`iw dev <iface> link`)
- Active window title / focused app (Hyprland/Niri IPC)
- Idle time / presence (`loginctl show-session -p IdleHint`)
- Keyboard backlight brightness (same shape as screen brightness)
- Lid open/closed, docked/undocked
- Per-process top CPU/memory consumer
- Network throughput rate (bytes/sec) per interface
- Count of failed systemd units, pending NixOS generation/updates
- USB device connect/disconnect as a binary_sensor

Until they're builtin, every one of these is already expressible today as
a `command`/`read_command` custom entity.
