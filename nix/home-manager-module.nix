# home-manager module for ha-stat: `services.ha-stat.enable` + `.settings`,
# mirroring the enable/settings shape of services.immich-public-proxy etc.
#
# ha-stat runs as *you*, in your logged-in session -- it needs your Wayland
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
  cfg = config.services.ha-stat;
  format = pkgs.formats.toml { };
  configFile = format.generate "ha-stat-config.toml" cfg.settings;
in
{
  options.services.ha-stat = {
    enable = lib.mkEnableOption "ha-stat, a Home Assistant MQTT bridge for this session";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.system}.default;
      defaultText = lib.literalExpression "ha-stat.packages.<system>.default";
      description = "The ha-stat package to run.";
    };

    settings = lib.mkOption {
      type = lib.types.submodule { freeformType = format.type; };
      default = { };
      description = ''
        ha-stat's declarative config: `device`, `mqtt`, and any number of
        `sensor` / `binary_sensor` / `number` / `switch` / `lock` / `button`
        entries. See the ha-stat README for the full schema.

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
            username = "ha-stat";
            password_file = config.sops.secrets.ha-stat-mqtt-password.path;
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
    systemd.user.services.ha-stat = {
      Unit.Description = "ha-stat: Home Assistant MQTT bridge";
      Install.WantedBy = [ "default.target" ];
      Service = {
        ExecStart = "${lib.getExe cfg.package} run --config ${configFile}";
        Restart = "on-failure";
        RestartSec = 5;
      };
    };
  };
}
