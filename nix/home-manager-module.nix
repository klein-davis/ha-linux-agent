# home-manager module for ha-linux-agent: `services.ha-linux-agent.enable` + `.settings`,
# mirroring the enable/settings shape of services.immich-public-proxy etc.
#
# ha-linux-agent runs as *you*, in your logged-in session -- it needs your Wayland
# compositor, your D-Bus session, and the logind-ACL'd permissions on
# backlight/battery sysfs that only apply to the active session user. That's
# why this is a home-manager module (systemd --user service) rather than a
# NixOS system service: a system-level service would run as a system user
# with none of those session privileges.
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.ha-linux-agent;
  format = pkgs.formats.toml { };
  configFile = format.generate "ha-linux-agent-config.toml" cfg.settings;
in
{
  options.services.ha-linux-agent = {
    enable = lib.mkEnableOption "ha-linux-agent, a Home Assistant MQTT bridge for this session";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "ha-linux-agent.packages.<system>.default";
      description = "The ha-linux-agent package to run.";
    };

    settings = lib.mkOption {
      type = lib.types.submodule { freeformType = format.type; };
      default = { };
      description = ''
        ha-linux-agent's declarative config: `device`, `mqtt`, and any number of
        `sensor` / `binary_sensor` / `number` / `switch` / `lock` / `button`
        entries. See the ha-linux-agent README for the full schema.

        **Do not put `settings.mqtt.password` here.** This gets rendered
        into the Nix store via `pkgs.formats.toml`, which is world-readable
        -- anyone with store access could read your MQTT password. Use
        `settings.mqtt.password_file` instead, pointing at a secret
        deployed outside the store (sops-nix, agenix, or just a file with
        tight permissions that home-manager doesn't manage).
      '';
      example = lib.literalExpression ''
        {
          device.name = "My Laptop";
          mqtt = {
            host = "homeassistant.example.lan";
            username = "ha-linux-agent";
            password_file = config.sops.secrets.ha-linux-agent-mqtt-password.path;
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
        }
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.user.services.ha-linux-agent = {
      Unit.Description = "ha-linux-agent: Home Assistant MQTT bridge";
      Install.WantedBy = [ "default.target" ];
      Service = {
        ExecStart = "${lib.getExe cfg.package} run --config ${configFile}";
        Restart = "on-failure";
        RestartSec = 5;
      };
    };
  };
}
