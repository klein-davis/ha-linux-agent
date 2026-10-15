{
  description = "ha-stat: a Home Assistant MQTT bridge for system/laptop state and control";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    devenv.url = "github:cachix/devenv";
  };

  outputs =
    { self, nixpkgs, flake-utils, devenv, ... }@inputs:
    (flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        ha-stat = pkgs.rustPlatform.buildRustPackage {
          pname = "ha-stat";
          version = "0.1.0";
          src = ./.;
          cargoLock = { lockFile = ./Cargo.lock; };

          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.openssl ];

          # tests/mqtt_integration.rs is #[ignore]d: it needs a live MQTT
          # broker, which isn't available in the build sandbox. `cargo test`
          # skips #[ignore]d tests by default, so no extra flags are needed
          # here; run it explicitly with `cargo test -- --ignored` inside
          # the devShell once `devenv up -d` has started mosquitto.

          meta = with pkgs.lib; {
            description = "Expose laptop/system state and controls to Home Assistant over MQTT";
            homepage = "https://github.com/yourname/ha-stat";
            license = licenses.mit;
            mainProgram = "ha-stat";
          };
        };
      in
      {
        packages.default = ha-stat;
        packages.ha-stat = ha-stat;

        apps.default = flake-utils.lib.mkApp { drv = ha-stat; };

        checks = {
          default = ha-stat; # includes `cargo test` (unit tests) via buildRustPackage's checkPhase
          ha-stat = ha-stat;

          clippy = ha-stat.overrideAttrs (old: {
            pname = "ha-stat-clippy";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
            buildPhase = "true";
            checkPhase = "cargo clippy --all-targets --offline -- -D warnings";
            doCheck = true;
            installPhase = "mkdir -p $out";
          });

          fmt = ha-stat.overrideAttrs (old: {
            pname = "ha-stat-fmt";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.rustfmt ];
            buildPhase = "true";
            checkPhase = "cargo fmt --all -- --check";
            doCheck = true;
            installPhase = "mkdir -p $out";
          });
        };

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

              env.RUST_LOG = "ha_stat=debug,info";

              services.mosquitto = {
                enable = true;
                bind = "127.0.0.1";
                port = 1883;
              };

              enterShell = ''
                echo "ha-stat dev shell: cargo build / cargo test / cargo run -- --help"
                echo "mosquitto test broker: run 'devenv up -d' to start it on 127.0.0.1:1883"
                echo "  then: cargo test -- --include-ignored   (runs the mqtt_integration tests)"
              '';

              scripts.ha-stat-dev.exec = ''
                cargo run -- "$@"
              '';
            })
          ];
        };
      })) // {
      homeManagerModules.default = import ./nix/home-manager-module.nix self;
      homeManagerModules.ha-stat = import ./nix/home-manager-module.nix self;
    };
}
