# BusChain Control — packages, apps, home module, and dev shell.
{
  description = "BusChain Control — tray-resident PipeWire mixer + control surface";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      lib = pkgs.lib;

      eguiLibs = with pkgs; [
        libxkbcommon
        libGL
        wayland
        vulkan-loader
        libx11
        libxcursor
        libxi
        libxrandr
      ];

      rustPlatform = pkgs.rustPlatform;

      commonNative = with pkgs; [
        pkg-config
        rustPlatform.bindgenHook
      ];

      commonBuild = with pkgs; [
        pipewire
        pulseaudio
        libpulseaudio
        dbus
        openssl
      ] ++ eguiLibs;

      buschainControlPkg = rustPlatform.buildRustPackage {
        pname = "buschain-control";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = commonNative ++ [ pkgs.makeWrapper pkgs.copyDesktopItems ];
        buildInputs = commonBuild;
        # Don't use cargo's default install — custom multi-bin layout.
        dontCargoInstall = true;
        buildPhase = ''
          runHook preBuild
          cargo build --release -p buschain-control -p buschain-tools
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          mkdir -p $out/bin $out/share/applications $out/share/icons/hicolor/256x256/apps
          mkdir -p $out/share/buschain-control/waybar
          install -m755 target/release/buschain-control $out/bin/
          install -m755 target/release/buschain-daemon $out/bin/
          install -m755 target/release/buschain-ctl $out/bin/
          install -m755 packaging/waybar/buschain-waybar.sh $out/bin/buschain-waybar
          install -m644 packaging/wayland/buschain-control.desktop $out/share/applications/
          if [ -f assets/icons/buschain-control.png ]; then
            cp assets/icons/buschain-control.png $out/share/icons/hicolor/256x256/apps/buschain-control.png
          fi
          cp packaging/waybar/module.jsonc $out/share/buschain-control/waybar/
          wrapProgram $out/bin/buschain-control \
            --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath commonBuild}
          wrapProgram $out/bin/buschain-daemon \
            --prefix PATH : ${lib.makeBinPath [ pkgs.pipewire pkgs.pulseaudio ]}
          wrapProgram $out/bin/buschain-ctl \
            --prefix PATH : $out/bin
          runHook postInstall
        '';
        doCheck = false;
        meta = with lib; {
          description = "BusChain Control tray mixer and PipeWire control surface";
          license = licenses.mit;
          platforms = platforms.linux;
          mainProgram = "buschain-control";
        };
      };
    in {
      packages.${system} = {
        default = buschainControlPkg;
        buschain-control = buschainControlPkg;
        buschain-daemon = buschainControlPkg;
        buschain-ctl = buschainControlPkg;
      };

      apps.${system} = {
        default = {
          type = "app";
          program = "${buschainControlPkg}/bin/buschain-control";
        };
        daemon = {
          type = "app";
          program = "${buschainControlPkg}/bin/buschain-daemon";
        };
        ctl = {
          type = "app";
          program = "${buschainControlPkg}/bin/buschain-ctl";
        };
      };

      homeModules.buschain-control = { config, lib, pkgs, ... }:
        let cfg = config.services.buschain-control; in {
          options.services.buschain-control = {
            enable = lib.mkEnableOption "BusChain Control tray mixer (Hyprland exec-once)";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.buschain-control;
            };
          };
          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ];
            # Graph ownership is the tray UI — start from the compositor:
            #   exec-once = buschain-control --hidden
            # Do not install a headless systemd unit (steals the IPC socket).
          };
        };

      nixosModules.buschain-control = self.homeModules.buschain-control;

      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          cargo
          rustc
          rustPlatform.bindgenHook
          pkg-config
          gcc
          gnumake
          lv2
          pipewire
          pulseaudio
          libpulseaudio
          dbus
        ] ++ eguiLibs;
        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (eguiLibs ++ [ pkgs.dbus ]);
        shellHook = ''
          export CARGO_TARGET_DIR="$PWD/target"
          export LADSPA_PATH="$PWD/plugins/buschain-denoiser/build:$PWD/plugins/buschain-gate/build:$PWD/plugins/buschain-builtins/build:''${LADSPA_PATH:-}"
          export LV2_PATH="$PWD/plugins/buschain-denoiser/build:$PWD/plugins/buschain-gate/build:$PWD/plugins/buschain-builtins/build:''${LV2_PATH:-}"
          echo "BusChain Control — cargo run -p buschain-control  (tray + in-process + embedded IPC)"
          echo "  ctl:    cargo run -p buschain-tools --bin buschain-ctl"
        '';
      };
    };
}
