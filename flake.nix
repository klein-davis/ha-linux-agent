{
  description = "hastat: a Home Assistant MQTT bridge for system/laptop state and control";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    devenv.url = "github:cachix/devenv";
  };

  outputs = { self, nixpkgs, flake-utils, devenv, ... }@inputs:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        hastat = pkgs.rustPlatform.buildRustPackage {
          pname = "hastat";
          version = "0.1.0";
          src = ./.;
          cargoLock = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.openssl ];

          # Integration tests need a live MQTT broker (provided by the devenv
          # shell / mosquitto) and real backlight/network devices, so they are
          # excluded from the sandboxed package build and run separately via
          # `nix flake check` / `devenv test`.
          checkFlags = [
            "--skip=mqtt_integration"
            "--skip=live_"
          ];

          meta = with pkgs.lib; {
            description = "Expose laptop/system state and controls to Home Assistant over MQTT";
            homepage = "https://github.com/yourname/hastat";
            license = licenses.mit;
            mainProgram = "hastat";
          };
        };
      in
      {
        packages.default = hastat;
        packages.hastat = hastat;

        apps.default = flake-utils.lib.mkApp { drv = hastat; };

        checks.default = hastat;

        devShells.default = devenv.lib.mkShell {
          inherit inputs pkgs;
          modules = [
            ({ pkgs, lib, ... }: {
              packages = with pkgs; [
                cargo
                rustc
                rust-analyzer
                clippy
                rustfmt
                pkg-config
                openssl
                mosquitto
              ];

              env.HASTAT_MQTT_HOST = "127.0.0.1";
              env.HASTAT_MQTT_PORT = "1883";
              env.RUST_LOG = "hastat=debug,info";

              services.mosquitto = {
                enable = true;
                bind = "127.0.0.1";
                port = 1883;
              };

              enterShell = ''
                echo "hastat dev shell: cargo build / cargo test / cargo run -- --help"
                echo "mosquitto test broker: run 'devenv up -d' to start it on 127.0.0.1:1883"
                echo "  then: cargo test -- --include-ignored   (runs the mqtt_integration tests)"
              '';

              scripts.hastat-dev.exec = ''
                cargo run -- "$@"
              '';
            })
          ];
        };
      });
}
