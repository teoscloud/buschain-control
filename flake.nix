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

      gtkMixer = import ./packaging/nix/gtk-mixer.nix { inherit pkgs; };

      buschainControlPkg = rustPlatform.buildRustPackage {
        pname = "buschain-control";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = commonNative ++ [ pkgs.makeWrapper pkgs.copyDesktopItems ];
        buildInputs = commonBuild;
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
          install -m755 packaging/waybar/buschain-waybar $out/bin/buschain-waybar

          ln -s ${gtkMixer}/bin/buschain-mixer-gtk $out/bin/buschain-mixer-gtk
          ln -s ${gtkMixer}/bin/buschain-mixer $out/bin/buschain-mixer

          # Reference snippets for user rice (not auto-installed into ~/.config).
          install -m644 packaging/waybar/module.jsonc \
            $out/share/buschain-control/waybar/module.jsonc
          install -m644 packaging/waybar/style.css \
            $out/share/buschain-control/waybar/style.css

          install -m644 packaging/wayland/buschain-control.desktop $out/share/applications/
          if [ -f assets/icons/buschain-control.png ]; then
            cp assets/icons/buschain-control.png $out/share/icons/hicolor/256x256/apps/buschain-control.png
          fi

          wrapProgram $out/bin/buschain-control \
            --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath commonBuild} \
            --prefix PATH : $out/bin \
            --set-default BUSCHAIN_CONTROL_USE_GTK_MIXER 1
          wrapProgram $out/bin/buschain-daemon \
            --prefix PATH : ${lib.makeBinPath [ pkgs.pipewire pkgs.pulseaudio ]}
          wrapProgram $out/bin/buschain-ctl \
            --prefix PATH : $out/bin \
            --set-default BUSCHAIN_CONTROL_USE_GTK_MIXER 1
          wrapProgram $out/bin/buschain-waybar \
            --prefix PATH : $out/bin:${lib.makeBinPath [ pkgs.pulseaudio pkgs.procps ]} \
            --set-default BUSCHAIN_CONTROL_CTL $out/bin/buschain-ctl \
            --set-default BUSCHAIN_CONTROL_USE_GTK_MIXER 1

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
        buschain-waybar = buschainControlPkg;
        buschain-mixer-gtk = gtkMixer;
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

      # Optional: put bins on PATH. Waybar module/CSS stay in the user's rice.
      homeModules.buschain-control = { config, lib, pkgs, ... }:
        let cfg = config.services.buschain-control; in {
          options.services.buschain-control = {
            enable = lib.mkEnableOption "BusChain Control package on home.packages";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.buschain-control;
            };
          };
          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ];
            home.sessionVariables.BUSCHAIN_CONTROL_USE_GTK_MIXER = "1";
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
          gtk3
          gtk-layer-shell
          gobject-introspection
          (python3.withPackages (ps: [ ps.pygobject3 ]))
          # Wrapped mixer (gi + layer-shell) — required for waybar/tray popup in develop.
          gtkMixer
        ] ++ eguiLibs;
        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (eguiLibs ++ [ pkgs.dbus ]);
        shellHook = ''
          export CARGO_TARGET_DIR="$PWD/target"
          export LADSPA_PATH="$PWD/plugins/buschain-denoiser/build:$PWD/plugins/buschain-gate/build:$PWD/plugins/buschain-builtins/build:''${LADSPA_PATH:-}"
          export LV2_PATH="$PWD/plugins/buschain-denoiser/build:$PWD/plugins/buschain-gate/build:$PWD/plugins/buschain-builtins/build:''${LV2_PATH:-}"
          export PATH="$PWD/target/debug:$PWD/packaging/waybar:$PATH"
          export BUSCHAIN_CONTROL_CTL="$PWD/target/debug/buschain-ctl"
          # Prefer nix-wrapped mixer from this shell (not the bare packaging/ launcher).
          export BUSCHAIN_CONTROL_MIXER="$(command -v buschain-mixer-gtk)"
          export BUSCHAIN_CONTROL_MIXER_CSS="$PWD/packaging/mixer/legacy/style.css"
          export BUSCHAIN_CONTROL_USE_GTK_MIXER=1
          chmod +x "$PWD/packaging/waybar/buschain-waybar" 2>/dev/null || true
          echo "BusChain Control — cargo run  (plugins + ctl + tray + IPC)"
          echo "  hidden: cargo run -- --hidden"
          echo "  ctl:    cargo ctl -- status"
          echo "  waybar: buschain-waybar popup → GTK ($BUSCHAIN_CONTROL_MIXER)"
        '';
      };
    };
}
